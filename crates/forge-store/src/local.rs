//! [`LocalFs`] — the default backend: the project is an ordinary folder.
//!
//! Working files are plain files under the root (so a user, a diff tool or git sees the
//! project as it is). The store's own data lives in `<root>/.forge/`:
//!
//! | path | holds |
//! |---|---|
//! | `.forge/blobs/ab/cdef…` | content-addressed blobs (file snapshots, trees, command logs) |
//! | `.forge/revs/<id>.ron` | revision records (RON, diffable) |
//! | `.forge/HEAD` | the newest revision id |
//! | `.forge/locks/<hash>.lock` | one file per held lock (created exclusively: two editors on one folder cannot both take it) |
//! | `.forge/tmp/` | staging for atomic writes (write, then rename into place) |
//!
//! Every write is atomic, every blob read is verified against its address, and commits
//! hash each working file at most once per change (a size + mtime cache).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use forge_cmd::{Clock, CommandEnvelope, SystemClock};

use crate::store::{RevRecord, build_commit, decode_log, walk_history};
use crate::{Blake3, Lock, ProjectStore, Rev, RevId, RevRange, Stamp, StoreError, StorePath, Tree};

const META: &str = ".forge";

/// Distinguishes temp files of stores in one process (the pid distinguishes processes).
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The local-folder project store.
pub struct LocalFs {
    root: PathBuf,
    identity: String,
    clock: Box<dyn Clock>,
    /// path -> (len, mtime, hash, when hashed): commits re-hash only files that changed.
    hashes: HashMap<StorePath, (u64, SystemTime, Blake3, SystemTime)>,
    /// [`ProjectStore::stamps`]' content hashes of recently modified files (see there).
    stamp_cache: Mutex<HashMap<StorePath, Hashed>>,
    /// Bytes `stamps`/`stamps_of` have read and hashed since open (diagnostic, perf guard).
    stamp_hashed: AtomicU64,
}

/// A content hash `stamps` took of a file while its metadata could not yet be trusted.
#[derive(Clone, Copy)]
struct Hashed {
    len: u64,
    mtime: SystemTime,
    hash: Blake3,
    /// When the read that produced `hash` began.
    at: SystemTime,
}

/// How long after a write its `(size, mtime)` can still be repeated by another write.
///
/// Two writes inside one tick of the clock that stamps them carry the same mtime; if they
/// also have the same size, metadata cannot tell them apart ("racy git"). The tick is that
/// of the filesystem's timestamp resolution or of the kernel clock it reads, whichever is
/// coarser. A timestamp with a sub-second part comes from a fine-resolution filesystem
/// (NTFS 100 ns, ext4/APFS/btrfs 1 ns, exFAT 10 ms), whose writes are stamped from a clock
/// that ticks at most every ~16 ms (Windows' default timer, Linux HZ=100 coarse time):
/// 100 ms covers that with margin. A timestamp on a whole second may come from a 1 s (HFS+,
/// ext3) or 2 s (FAT) filesystem: 2 s. (On a fine filesystem one mtime in 10^9 lands on a
/// whole second; it just gets the conservative window.) The window assumes the writer's
/// clock is the local clock; a network share whose server clock is skewed by more than the
/// window can hide a same-size rewrite inside one server tick, as it could before.
fn racy_window(mtime: SystemTime) -> Duration {
    let sub = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    if sub == 0 {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(100)
    }
}

/// Whether `later` is at least `d` after `earlier`.
fn at_least(later: SystemTime, earlier: SystemTime, d: Duration) -> bool {
    later.duration_since(earlier).is_ok_and(|x| x >= d)
}

impl LocalFs {
    /// Open (creating if needed) the project folder `root`, acting as `identity`.
    pub fn open(root: impl Into<PathBuf>, identity: &str) -> Result<Self, StoreError> {
        Self::open_with_clock(root, identity, Box::new(SystemClock))
    }

    /// [`LocalFs::open`] with an injected clock (commit times).
    pub fn open_with_clock(
        root: impl Into<PathBuf>,
        identity: &str,
        clock: Box<dyn Clock>,
    ) -> Result<Self, StoreError> {
        let root = root.into();
        for d in ["blobs", "revs", "locks", "tmp"] {
            let p = root.join(META).join(d);
            fs::create_dir_all(&p).map_err(|e| StoreError::io("create", p.display(), &e))?;
        }
        Ok(Self {
            root,
            identity: identity.to_string(),
            clock,
            hashes: HashMap::new(),
            stamp_cache: Mutex::new(HashMap::new()),
            stamp_hashed: AtomicU64::new(0),
        })
    }

    /// The project folder.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn file(&self, p: &StorePath) -> PathBuf {
        let mut out = self.root.clone();
        out.extend(p.segments());
        out
    }

    fn meta(&self) -> PathBuf {
        self.root.join(META)
    }

    fn blob_path(&self, h: Blake3) -> PathBuf {
        let hex = h.to_hex();
        self.meta().join("blobs").join(&hex[..2]).join(&hex[2..])
    }

    fn lock_path(&self, p: &StorePath) -> PathBuf {
        self.meta().join("locks").join(format!(
            "{}.lock",
            Blake3::of(p.as_str().as_bytes()).to_hex()
        ))
    }

    /// Write `bytes` to `dest` atomically: stage in `.forge/tmp`, then rename over.
    fn atomic_write(&self, dest: &Path, bytes: &[u8]) -> Result<(), StoreError> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| StoreError::io("create", parent.display(), &e))?;
        }
        let tmp = self.meta().join("tmp").join(format!(
            "{}-{}.tmp",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let res = (|| {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, dest)
        })();
        res.map_err(|e| {
            let _ = fs::remove_file(&tmp);
            StoreError::io("write", dest.display(), &e)
        })
    }

    fn lock_owner(&self, p: &StorePath) -> Result<Option<String>, StoreError> {
        let lp = self.lock_path(p);
        match fs::read_to_string(&lp) {
            Ok(text) => Ok(Some(text.lines().next().unwrap_or("").to_string())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::io("read", lp.display(), &e)),
        }
    }

    fn check_lock(&self, p: &StorePath) -> Result<(), StoreError> {
        match self.lock_owner(p)? {
            Some(owner) if owner != self.identity => Err(StoreError::Locked {
                path: p.to_string(),
                owner,
            }),
            _ => Ok(()),
        }
    }

    /// A path must not differ from an existing file or directory only by case: look at each
    /// ancestor directory's entries.
    fn check_case(&self, p: &StorePath) -> Result<(), StoreError> {
        let mut dir = self.root.clone();
        let mut so_far = String::new();
        for seg in p.segments() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(StoreError::io("list", dir.display(), &e)),
            };
            let folded = seg.to_lowercase();
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name != seg && name.to_lowercase() == folded {
                    let existing = if so_far.is_empty() {
                        name.to_string()
                    } else {
                        format!("{so_far}/{name}")
                    };
                    return Err(StoreError::CaseCollision {
                        path: p.to_string(),
                        existing,
                    });
                }
            }
            if !so_far.is_empty() {
                so_far.push('/');
            }
            so_far.push_str(seg);
            dir.push(seg);
        }
        Ok(())
    }

    fn hash_file(&mut self, p: &StorePath) -> Result<(Blake3, Option<Bytes>), StoreError> {
        let path = self.file(p);
        let md = fs::metadata(&path).map_err(|e| StoreError::io("stat", path.display(), &e))?;
        let mtime = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        // Trust the cache only for files last modified well before they were hashed: a
        // same-size rewrite inside one timestamp tick would otherwise go unseen ("racy git").
        if let Some((len, t, h, at)) = self.hashes.get(p)
            && *len == md.len()
            && *t == mtime
            && at.duration_since(mtime).is_ok_and(|d| d.as_secs() >= 2)
            && self.blob_has(*h)?
        {
            return Ok((*h, None));
        }
        let bytes =
            Bytes::from(fs::read(&path).map_err(|e| StoreError::io("read", path.display(), &e))?);
        let h = Blake3::of(&bytes);
        self.hashes
            .insert(p.clone(), (md.len(), mtime, h, SystemTime::now()));
        Ok((h, Some(bytes)))
    }

    fn record(&self, id: &RevId) -> Result<RevRecord, StoreError> {
        let p = self.meta().join("revs").join(format!("{id}.ron"));
        let text = match fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(StoreError::NotFound(format!("revision {id}")));
            }
            Err(e) => return Err(StoreError::io("read", p.display(), &e)),
        };
        RevRecord::decode(&text, id)
    }

    /// Every project file under `dir`, with its directory entry. The store's own `.forge/`
    /// folder, non-UTF-8 names, names that are not portable project paths and anything that
    /// is not a regular file (symlinks included) are skipped.
    fn walk(
        &self,
        dir: &Path,
        prefix: &str,
        f: &mut dyn FnMut(StorePath, &fs::DirEntry) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let entries = fs::read_dir(dir).map_err(|e| StoreError::io("list", dir.display(), &e))?;
        for entry in entries {
            let entry = entry.map_err(|e| StoreError::io("list", dir.display(), &e))?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue; // not UTF-8: not a portable project path
            };
            if prefix.is_empty() && name == META {
                continue;
            }
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let ty = entry
                .file_type()
                .map_err(|e| StoreError::io("stat", entry.path().display(), &e))?;
            if ty.is_dir() {
                self.walk(&entry.path(), &rel, f)?;
            } else if ty.is_file()
                && let Ok(p) = StorePath::new(&rel)
            {
                // Files whose names are not portable project paths are not project files.
                f(p, &entry)?;
            }
        }
        Ok(())
    }

    /// The stamp of one file from its directory entry's size and mtime (see
    /// [`ProjectStore::stamps`]); `cache` holds the recent-file hashes. `None`: the file
    /// vanished meanwhile.
    fn stamp_entry(
        &self,
        p: &StorePath,
        md: &fs::Metadata,
        now: SystemTime,
        cache: &mut HashMap<StorePath, Hashed>,
    ) -> Result<Option<Stamp>, StoreError> {
        let len = md.len();
        let mtime = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let window = racy_window(mtime);
        let cached = cache
            .get(p)
            .copied()
            .filter(|h| h.len == len && h.mtime == mtime);
        match cached {
            // Hashed by a read that began after the window closed: no later write can repeat
            // this metadata, so the hash still describes the file. Keep answering with it
            // (switching to a metadata stamp would report a change that did not happen).
            Some(h) if at_least(h.at, mtime, window) => Ok(Some(Stamp::of_content(&h.hash))),
            // Never seen inside the window: the metadata is exact.
            None if at_least(now, mtime, window) => {
                cache.remove(p);
                let nanos = mtime
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
                Ok(Some(Stamp::of_metadata(&[len, nanos])))
            }
            // Inside the window, or hashed while it was still open: the content decides.
            _ => {
                let at = SystemTime::now();
                let path = self.file(p);
                let file = match fs::File::open(&path) {
                    Ok(f) => f,
                    Err(e) if e.kind() == ErrorKind::NotFound => {
                        cache.remove(p);
                        return Ok(None);
                    }
                    Err(e) => return Err(StoreError::io("read", path.display(), &e)),
                };
                let mut hasher = blake3::Hasher::new();
                hasher
                    .update_reader(file)
                    .map_err(|e| StoreError::io("read", path.display(), &e))?;
                let hash = Blake3(*hasher.finalize().as_bytes());
                self.stamp_hashed.fetch_add(len, Ordering::Relaxed);
                cache.insert(
                    p.clone(),
                    Hashed {
                        len,
                        mtime,
                        hash,
                        at,
                    },
                );
                Ok(Some(Stamp::of_content(&hash)))
            }
        }
    }

    fn stamp_cache(&self) -> MutexGuard<'_, HashMap<StorePath, Hashed>> {
        // The cache only ever holds complete entries: after a panic elsewhere it is still
        // consistent (at worst an entry is missing and that file is hashed again).
        self.stamp_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Bytes [`ProjectStore::stamps`] and [`ProjectStore::stamps_of`] have read and hashed
    /// since this store was opened. Diagnostic: a quiet project's poll adds nothing.
    #[must_use]
    pub fn stamp_hashed_bytes(&self) -> u64 {
        self.stamp_hashed.load(Ordering::Relaxed)
    }
}

impl ProjectStore for LocalFs {
    fn backend(&self) -> &'static str {
        "local-fs"
    }

    fn identity(&self) -> &str {
        &self.identity
    }

    fn read(&self, path: &StorePath) -> Result<Bytes, StoreError> {
        let p = self.file(path);
        match fs::read(&p) {
            Ok(b) => Ok(Bytes::from(b)),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                Err(StoreError::NotFound(path.to_string()))
            }
            Err(e) => Err(StoreError::io("read", p.display(), &e)),
        }
    }

    /// One `stat`; the file is not opened.
    fn size(&self, path: &StorePath) -> Result<Option<u64>, StoreError> {
        let p = self.file(path);
        match fs::metadata(&p) {
            Ok(md) if md.is_file() => Ok(Some(md.len())),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::io("stat", p.display(), &e)),
        }
    }

    fn write(&mut self, path: &StorePath, b: Bytes) -> Result<(), StoreError> {
        self.check_lock(path)?;
        // Always checked, even when the file "exists": on a case-insensitive disk
        // `Rock.png` finds `rock.png`, and writing would silently overwrite it.
        self.check_case(path)?;
        self.atomic_write(&self.file(path), &b)
    }

    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError> {
        self.check_lock(path)?;
        let p = self.file(path);
        match fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(StoreError::NotFound(path.to_string()));
            }
            Err(e) => return Err(StoreError::io("delete", p.display(), &e)),
        }
        self.hashes.remove(path);
        // Remove directories the deletion emptied, up to the root.
        let mut dir = p.parent().map(Path::to_path_buf);
        while let Some(d) = dir {
            if d == self.root || fs::remove_dir(&d).is_err() {
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<StorePath>, StoreError> {
        let mut out = Vec::new();
        self.walk(&self.root, "", &mut |p, _| {
            out.push(p);
            Ok(())
        })?;
        out.sort();
        Ok(out)
    }

    fn blob_get(&self, h: Blake3) -> Result<Bytes, StoreError> {
        let p = self.blob_path(h);
        let b = match fs::read(&p) {
            Ok(b) => b,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(StoreError::NotFound(format!("blob {h}")));
            }
            Err(e) => return Err(StoreError::io("read", p.display(), &e)),
        };
        if Blake3::of(&b) != h {
            return Err(StoreError::Corrupt {
                what: format!("blob {h}"),
                why: format!(
                    "{} hashes differently (disk corruption or tampering)",
                    p.display()
                ),
            });
        }
        Ok(Bytes::from(b))
    }

    fn blob_put(&mut self, b: Bytes) -> Result<Blake3, StoreError> {
        let h = Blake3::of(&b);
        let p = self.blob_path(h);
        if !p.is_file() {
            self.atomic_write(&p, &b)?;
        }
        Ok(h)
    }

    fn blob_has(&self, h: Blake3) -> Result<bool, StoreError> {
        Ok(self.blob_path(h).is_file())
    }

    fn commit(&mut self, msg: &str, envelopes: &[CommandEnvelope]) -> Result<RevId, StoreError> {
        let at = self.clock.now_ms();
        self.commit_at(msg, envelopes, at)
    }

    fn commit_at(
        &mut self,
        msg: &str,
        envelopes: &[CommandEnvelope],
        at: u64,
    ) -> Result<RevId, StoreError> {
        let paths = self.list()?;
        let mut files = Vec::with_capacity(paths.len());
        for p in paths {
            let (h, bytes) = self.hash_file(&p)?;
            if let Some(b) = bytes
                && !self.blob_has(h)?
            {
                self.atomic_write(&self.blob_path(h), &b)?;
            }
            files.push((p, h));
        }
        let head = self.head()?;
        let rec = build_commit(files, head, msg, envelopes, at, &mut |b| self.blob_put(b))?;
        let id = rec.id();
        let rec_path = self.meta().join("revs").join(format!("{id}.ron"));
        self.atomic_write(&rec_path, rec.encode()?.as_bytes())?;
        self.atomic_write(&self.meta().join("HEAD"), format!("{id}\n").as_bytes())?;
        Ok(id)
    }

    fn head(&self) -> Result<Option<RevId>, StoreError> {
        let p = self.meta().join("HEAD");
        match fs::read_to_string(&p) {
            Ok(t) => t.trim().parse().map(Some),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::io("read", p.display(), &e)),
        }
    }

    fn history(&self, range: RevRange) -> Result<Vec<Rev>, StoreError> {
        walk_history(&range, self.head()?, &|id| self.record(id))
    }

    fn tree(&self, rev: &RevId) -> Result<Tree, StoreError> {
        let h = self.record(rev)?.to_rev(*rev)?.tree;
        Tree::decode(&self.blob_get(h)?)
    }

    fn commands(&self, rev: &RevId) -> Result<Vec<CommandEnvelope>, StoreError> {
        let h = self.record(rev)?.to_rev(*rev)?.log;
        decode_log(&self.blob_get(h)?)
    }

    fn lock(&mut self, path: &StorePath) -> Result<Lock, StoreError> {
        let lp = self.lock_path(path);
        let lock = Lock {
            path: path.clone(),
            owner: self.identity.clone(),
        };
        // `create_new` is atomic: of two editors racing for one lock, exactly one wins.
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lp)
        {
            Ok(mut f) => {
                f.write_all(format!("{}\n{}\n", self.identity, path).as_bytes())
                    .map_err(|e| StoreError::io("write", lp.display(), &e))?;
                Ok(lock)
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => match self.lock_owner(path)? {
                Some(owner) if owner == self.identity => Ok(lock),
                Some(owner) => Err(StoreError::Locked {
                    path: path.to_string(),
                    owner,
                }),
                None => Err(StoreError::io("lock", lp.display(), &e)),
            },
            Err(e) => Err(StoreError::io("lock", lp.display(), &e)),
        }
    }

    fn unlock(&mut self, lock: &Lock) -> Result<(), StoreError> {
        match self.lock_owner(&lock.path)? {
            None => Err(StoreError::NotFound(format!("a lock on {}", lock.path))),
            Some(owner) if owner != self.identity => Err(StoreError::NotLockOwner {
                path: lock.path.to_string(),
                owner,
                by: self.identity.clone(),
            }),
            Some(_) => {
                let lp = self.lock_path(&lock.path);
                fs::remove_file(&lp).map_err(|e| StoreError::io("unlock", lp.display(), &e))
            }
        }
    }

    fn locks(&self) -> Result<Vec<Lock>, StoreError> {
        let dir = self.meta().join("locks");
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| StoreError::io("list", dir.display(), &e))? {
            let entry = entry.map_err(|e| StoreError::io("list", dir.display(), &e))?;
            let text = fs::read_to_string(entry.path())
                .map_err(|e| StoreError::io("read", entry.path().display(), &e))?;
            let mut lines = text.lines();
            if let (Some(owner), Some(path)) = (lines.next(), lines.next()) {
                out.push(Lock {
                    path: StorePath::new(path)?,
                    owner: owner.to_string(),
                });
            }
        }
        out.sort();
        Ok(out)
    }

    /// Size and modification time per file, from **one directory walk and nothing else**:
    /// the metadata comes from the directory entries (on Windows `FindNextFileW` returns it
    /// with each name, so there is no per-file open or `stat` at all — the same source git
    /// for Windows' fscache uses; elsewhere one `lstat` per entry). No file is read, except
    /// one modified within its racy window (see `racy_window`: 100 ms on NTFS/ext4/APFS, 2 s
    /// on whole-second filesystems), because two same-size writes inside one clock tick
    /// carry equal metadata. Such a file is hashed, streamed rather than buffered, and the
    /// hash is kept with the metadata it was taken under; once a read that began after the
    /// window has hashed it, that hash is reused, so a just-written file is read once or
    /// twice, not on every poll. The cache holds only such recently written files.
    fn stamps(&self) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        let now = SystemTime::now();
        let mut old = std::mem::take(&mut *self.stamp_cache());
        let mut next = HashMap::new();
        let mut out = Vec::new();
        self.walk(&self.root, "", &mut |p, entry| {
            let md = match entry.metadata() {
                Ok(md) => md,
                // Deleted between the listing and the stat: it is simply not there.
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(StoreError::io("stat", entry.path().display(), &e)),
            };
            let mut one = HashMap::new();
            if !old.is_empty()
                && let Some(h) = old.remove(&p)
            {
                one.insert(p.clone(), h);
            }
            if let Some(stamp) = self.stamp_entry(&p, &md, now, &mut one)? {
                next.extend(one);
                out.push((p, stamp));
            }
            Ok(())
        })?;
        // Entries of files that no longer exist are dropped with `old`.
        *self.stamp_cache() = next;
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Only the named files: one listing per distinct parent directory — the same
    /// directory-entry metadata `stamps` reads, so the two agree — and no tree walk.
    fn stamps_of(&self, paths: &[StorePath]) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        let now = SystemTime::now();
        let mut by_dir: BTreeMap<Vec<&str>, Vec<&StorePath>> = BTreeMap::new();
        for p in paths {
            let mut segs: Vec<&str> = p.segments().collect();
            segs.pop();
            by_dir.entry(segs).or_default().push(p);
        }
        let mut cache = self.stamp_cache();
        let mut out = Vec::new();
        for (segs, want) in by_dir {
            let mut dir = self.root.clone();
            dir.extend(segs.iter());
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    for p in want {
                        cache.remove(p);
                    }
                    continue;
                }
                Err(e) => return Err(StoreError::io("list", dir.display(), &e)),
            };
            let mut found: HashMap<String, fs::DirEntry> = HashMap::new();
            for entry in entries {
                let entry = entry.map_err(|e| StoreError::io("list", dir.display(), &e))?;
                if let Some(name) = entry.file_name().to_str() {
                    found.insert(name.to_string(), entry);
                }
            }
            for p in want {
                let name = p.segments().last().unwrap_or_default();
                let entry = match found.get(name) {
                    Some(e) if e.file_type().is_ok_and(|t| t.is_file()) => e,
                    _ => {
                        cache.remove(p);
                        continue;
                    }
                };
                let md = match entry.metadata() {
                    Ok(md) => md,
                    Err(e) if e.kind() == ErrorKind::NotFound => {
                        cache.remove(p);
                        continue;
                    }
                    Err(e) => return Err(StoreError::io("stat", entry.path().display(), &e)),
                };
                if let Some(stamp) = self.stamp_entry(p, &md, now, &mut cache)? {
                    out.push((p.clone(), stamp));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.dedup_by(|a, b| a.0 == b.0);
        Ok(out)
    }
}
