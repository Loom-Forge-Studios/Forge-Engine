//! [`Vfs`] — the asset system's only view of storage: a [`ProjectStore`] (I17), shared by
//! the importers, the loader workers and hot reload, plus **exact-case reference
//! resolution** (M0-17).
//!
//! Nothing in forge-asset touches `std::fs` (guarded by `tests/test_asset_store_only.rs`):
//! the project may live in a local folder, in memory, in git or in S3, and the asset system
//! behaves identically.
//!
//! ## References resolve exactly (M0-17)
//!
//! A glTF naming `Textures/Rock.PNG` next to `textures/rock.png` loads on Windows (NTFS is
//! case-insensitive) and fails on Linux. [`Vfs::resolve`] compares names against the store's
//! listing **exactly**, never by asking the filesystem, so the reference fails on every
//! platform with `ASSET-0002`, naming the spelling on disk. Backslash separators, absolute
//! paths, unknown schemes and `..` above the root are `ASSET-0001`. It is the same rule
//! `tests/platform/test_asset_ref_case.rs` applies to the engine's own tree.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use bytes::Bytes;
use forge_store::{Blake3, ProjectStore, Stamp, StorePath};

use crate::AssetError;

/// A store shared between the asset server and its workers.
pub type SharedStore = Arc<RwLock<Box<dyn ProjectStore>>>;

/// Listing index: exact paths, and lower-cased path -> exact spellings.
#[derive(Default)]
struct Listing {
    exact: std::collections::HashSet<StorePath>,
    folded: HashMap<String, Vec<StorePath>>,
}

/// The asset system's view of a project store.
pub struct Vfs {
    store: SharedStore,
    listing: Mutex<Option<Arc<Listing>>>,
    /// Paths written or deleted through this VFS since hot reload last took them.
    written: Mutex<std::collections::BTreeSet<StorePath>>,
}

impl Vfs {
    /// Wrap a store.
    pub fn new(store: Box<dyn ProjectStore>) -> Self {
        Self::shared(Arc::new(RwLock::new(store)))
    }

    /// Wrap a store someone else also holds.
    pub fn shared(store: SharedStore) -> Self {
        Self {
            store,
            listing: Mutex::new(None),
            written: Mutex::new(std::collections::BTreeSet::new()),
        }
    }

    /// The underlying store handle.
    #[must_use]
    pub fn store(&self) -> &SharedStore {
        &self.store
    }

    fn read_lock(&self) -> RwLockReadGuard<'_, Box<dyn ProjectStore>> {
        // A poisoned lock means a panic mid-operation elsewhere; the store's own operations
        // are atomic, so its state is still consistent and reading on is safe.
        self.store
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_lock(&self) -> RwLockWriteGuard<'_, Box<dyn ProjectStore>> {
        self.store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Forget the cached listing (after anything changed the file set).
    pub fn invalidate(&self) {
        *self
            .listing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn listing(&self) -> Result<Arc<Listing>, AssetError> {
        let mut slot = self
            .listing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(l) = &*slot {
            return Ok(Arc::clone(l));
        }
        let mut l = Listing::default();
        for p in self.read_lock().list()? {
            l.folded
                .entry(p.as_str().to_lowercase())
                .or_default()
                .push(p.clone());
            l.exact.insert(p);
        }
        let l = Arc::new(l);
        *slot = Some(Arc::clone(&l));
        Ok(l)
    }

    /// A file's bytes.
    pub fn read(&self, p: &StorePath) -> Result<Bytes, AssetError> {
        Ok(self.read_lock().read(p)?)
    }

    /// Whether a file exists (exact spelling).
    pub fn exists(&self, p: &StorePath) -> Result<bool, AssetError> {
        Ok(self.listing()?.exact.contains(p))
    }

    /// Write a file.
    pub fn write(&self, p: &StorePath, b: Bytes) -> Result<(), AssetError> {
        let existed = self.exists(p)?;
        self.write_lock().write(p, b)?;
        self.note_written(p);
        if !existed {
            self.invalidate();
        }
        Ok(())
    }

    /// Delete a file.
    pub fn delete(&self, p: &StorePath) -> Result<(), AssetError> {
        self.write_lock().delete(p)?;
        self.note_written(p);
        self.invalidate();
        Ok(())
    }

    /// Every file, sorted.
    pub fn list(&self) -> Result<Vec<StorePath>, AssetError> {
        let l = self.listing()?;
        let mut v: Vec<StorePath> = l.exact.iter().cloned().collect();
        v.sort();
        Ok(v)
    }

    /// The store's file locks as `(path, holder)` (Ch.33 §33.3; the asset browser's lock
    /// indicators).
    pub fn locks(&self) -> Result<Vec<(StorePath, String)>, AssetError> {
        Ok(self
            .read_lock()
            .locks()?
            .into_iter()
            .map(|l| (l.path, l.owner))
            .collect())
    }

    /// Every file with its change stamp.
    pub fn stamps(&self) -> Result<Vec<(StorePath, Stamp)>, AssetError> {
        Ok(self.read_lock().stamps()?)
    }

    /// A file's size in bytes (`None`: not a file), from metadata where the store has it.
    pub fn size(&self, p: &StorePath) -> Result<Option<u64>, AssetError> {
        Ok(self.read_lock().size(p)?)
    }

    /// The change stamps of just `paths` (absent: not a file).
    pub fn stamps_of(&self, paths: &[StorePath]) -> Result<Vec<(StorePath, Stamp)>, AssetError> {
        Ok(self.read_lock().stamps_of(paths)?)
    }

    fn note_written(&self, p: &StorePath) {
        self.written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(p.clone());
    }

    /// The paths written or deleted through this VFS since the last call (hot reload
    /// re-stamps exactly these after a poll, instead of the whole project).
    pub(crate) fn take_written(&self) -> Vec<StorePath> {
        std::mem::take(
            &mut *self
                .written
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_iter()
        .collect()
    }

    /// A blob (verified against its address by the store).
    pub fn blob_get(&self, h: Blake3) -> Result<Bytes, AssetError> {
        Ok(self.read_lock().blob_get(h)?)
    }

    /// Store a blob.
    pub fn blob_put(&self, b: Bytes) -> Result<Blake3, AssetError> {
        Ok(self.write_lock().blob_put(b)?)
    }

    /// Whether a blob is stored.
    pub fn blob_has(&self, h: Blake3) -> Result<bool, AssetError> {
        Ok(self.read_lock().blob_has(h)?)
    }

    /// Resolve `reference`, written inside the file `from`, to a project path — exactly
    /// (see the module docs). glTF-style percent-encoding is decoded first.
    pub fn resolve(&self, from: &StorePath, reference: &str) -> Result<StorePath, AssetError> {
        let target = join(from, reference)?;
        let l = self.listing()?;
        if l.exact.contains(&target) {
            return Ok(target);
        }
        match l.folded.get(&target.as_str().to_lowercase()) {
            Some(spellings) => Err(AssetError::CaseMismatch {
                reference: reference.to_string(),
                from: from.to_string(),
                on_disk: spellings[0].to_string(),
            }),
            None => Err(AssetError::MissingReference {
                reference: reference.to_string(),
                from: from.to_string(),
            }),
        }
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `reference` relative to `from`'s directory, normalised, as a project path.
pub(crate) fn join(from: &StorePath, reference: &str) -> Result<StorePath, AssetError> {
    let bad = |why: &str| AssetError::BadReference {
        reference: reference.to_string(),
        from: from.to_string(),
        why: why.to_string(),
    };
    let decoded = percent_decode(reference).ok_or_else(|| bad("bad percent-encoding"))?;
    if decoded.contains('\\') {
        return Err(bad(
            "backslash separators (use '/': '\\' is a file-name character on Linux)",
        ));
    }
    if decoded.starts_with('/') || decoded.as_bytes().get(1) == Some(&b':') {
        return Err(bad("absolute paths are not portable; use a relative path"));
    }
    if decoded.contains("://") {
        return Err(bad("the VFS resolves project-relative paths only"));
    }
    let mut parts: Vec<&str> = from.segments().collect();
    parts.pop(); // the file name
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(bad("'..' climbs above the project root"));
                }
            }
            s => parts.push(s),
        }
    }
    StorePath::new(&parts.join("/")).map_err(|e| bad(&e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_store::MemoryStore;

    fn p(s: &str) -> StorePath {
        StorePath::new(s).expect("path")
    }

    #[test]
    fn resolution_is_exact_and_names_the_on_disk_spelling() {
        let v = Vfs::new(Box::new(MemoryStore::new("t")));
        v.write(&p("models/ship.gltf"), Bytes::from_static(b"{}"))
            .expect("w");
        v.write(&p("models/textures/rock.png"), Bytes::from_static(b"x"))
            .expect("w");
        v.write(&p("shared/white space.png"), Bytes::from_static(b"x"))
            .expect("w");
        let from = p("models/ship.gltf");
        assert_eq!(
            v.resolve(&from, "textures/rock.png").expect("exact"),
            p("models/textures/rock.png")
        );
        assert_eq!(
            v.resolve(&from, "./textures/../textures/rock.png")
                .expect("normalised"),
            p("models/textures/rock.png")
        );
        assert_eq!(
            v.resolve(&from, "../shared/white%20space.png")
                .expect("decoded"),
            p("shared/white space.png")
        );
        match v.resolve(&from, "Textures/Rock.PNG") {
            Err(AssetError::CaseMismatch { on_disk, .. }) => {
                assert_eq!(on_disk, "models/textures/rock.png");
            }
            other => panic!("expected a case mismatch, got {other:?}"),
        }
        for bad in [
            "textures\\rock.png",
            "/abs.png",
            "C:/x.png",
            "../../x.png",
            "http://x/y.png",
            "%zz",
        ] {
            assert!(
                matches!(v.resolve(&from, bad), Err(AssetError::BadReference { .. })),
                "{bad} must be a bad reference"
            );
        }
        assert!(matches!(
            v.resolve(&from, "textures/none.png"),
            Err(AssetError::MissingReference { .. })
        ));
    }
}
