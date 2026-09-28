//! The asset browser's backend (Ch.21 §21.21 "Asset browser", DoD M2-38): the
//! [`AssetCatalog`] trait the panel talks to, and [`ServerCatalog`], its implementation
//! over `forge-asset`'s [`AssetServer`] (Ch.8).
//!
//! * **Reads** come from the asset database: every source's main asset and every
//!   generated asset, with kind, path, id and lock holder (Ch.33 §33.3).
//! * **Writes are commands (I7).** Import, rename and move are `forge.asset.import` /
//!   `forge.asset.rename` on the bus ([`import_command`], [`rename_command`]), so they are
//!   undoable, provenance-tagged and drivable by an automation session. The catalog *follows* the
//!   project: [`AssetCatalog::follow`] reads the import intents from the mirror (the same
//!   project settings the bus changed) and makes the database match — which is also how
//!   undoing a rename moves the file back.
//! * **Staging a dropped file.** Dropping a file from the OS copies its bytes into the
//!   project folder ([`AssetCatalog::stage`]) and then issues the import command. The copy
//!   is the user's own file operation (as if they had copied it in their file manager):
//!   undoing the import removes the registration and keeps the file, exactly as Ch.8.6
//!   specifies for any imported source.
//! * **Thumbnails** render on the asset workers ([`AssetServer::thumbnail`]); the catalog
//!   calls back when one is ready (a `std::task` waker on the handle, no polling), and the
//!   panel's thumbnail provider posts it to the UI thread.
//!
//! **Backend (D-4).** The editor has no project store until the launcher opens a project
//! (WP-U7), so the editor binary runs [`ServerCatalog`] over an in-memory store
//! ([`ServerCatalog::in_memory`], labelled as such in the panel). The asset database
//! itself is the real one.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::task::{Context, Wake, Waker};

use forge_asset::commands::{IMPORT, RENAME, intents_from_settings, invoke};
use forge_asset::{AssetId, AssetServer, AssetSource, LoadState, Thumbnail};
use forge_cmd::EditorCommand;

use crate::EditorError;
use crate::mirror::ProjectMirror;

/// One asset as the browser lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRow {
    /// Stable key for the grid (derived from the asset id).
    pub key: u64,
    /// The asset id (32 hex digits).
    pub id: String,
    /// The source path (`models/ship.gltf`), or `generator@seed` for a generated asset.
    pub path: String,
    /// The folder part of `path` (`models`, `""` for the root).
    pub folder: String,
    pub name: String,
    pub kind: String,
    /// Who holds the store lock on the source, if anyone (Ch.33 §33.3).
    pub locked_by: Option<String>,
    pub generated: bool,
    /// Changes when the asset's content changes (its artefact's address): a thumbnail
    /// rendered for another version is stale.
    pub version: u64,
}

/// A finished thumbnail: `(size, RGBA8 bytes)`, or why it failed.
pub type ThumbResult = Result<(u32, Vec<u8>), String>;
/// Called (on any thread) when a thumbnail is ready.
pub type ThumbDone = Box<dyn FnOnce(ThumbResult) + Send>;

/// What the asset browser talks to (see the module docs).
pub trait AssetCatalog {
    /// Bumped whenever [`AssetCatalog::rows`] would answer differently.
    fn revision(&self) -> u64;
    /// Every listed asset, sorted by path.
    fn rows(&self) -> Vec<AssetRow>;
    /// Render `key`'s thumbnail at `size` px; `done` runs when it is ready (maybe at once).
    fn thumbnail(&self, key: u64, size: u32, done: ThumbDone);
    /// Copy OS files into `folder` of the project (see the module docs). Returns the
    /// project paths written, in order.
    fn stage(&mut self, files: &[PathBuf], folder: &str) -> Result<Vec<String>, EditorError>;
    /// Make the database match the project's import intents (the mirror's settings).
    /// Returns what happened, for the console.
    fn follow(&mut self, mirror: &ProjectMirror) -> Vec<String>;
    /// Can `path` be imported (an importer claims its extension)?
    fn importable(&self, path: &str) -> bool;
    /// What backs it, for the panel's footer.
    fn backend(&self) -> &str;
    /// A plugin that provides importers or exporters was installed while the editor runs
    /// (WP-21): take the ones `ext` holds now.
    fn plugins_changed(&mut self, _ext: &forge_plugin::Extensions) {}
    /// The bytes of the project file `path` (`None`: no such file, or no project store
    /// behind this catalogue). The 2D editors read image headers through it (Ch.35 §35.5).
    fn source(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }
}

/// The command importing the project file `source`.
pub fn import_command(source: &str) -> EditorCommand {
    invoke(IMPORT, &serde_json::json!({ "source": source }))
}

/// The command renaming or moving `from` to `to` (the asset id stays).
pub fn rename_command(from: &str, to: &str) -> EditorCommand {
    invoke(RENAME, &serde_json::json!({ "from": from, "to": to }))
}

/// The command removing `source`'s registration (the file stays; undo restores it).
pub fn remove_command(source: &str) -> EditorCommand {
    invoke(
        forge_asset::commands::REMOVE,
        &serde_json::json!({ "source": source }),
    )
}

/// The grid key of an asset id: its low 64 bits (ids are 128-bit hashes).
pub fn key_of(id: AssetId) -> u64 {
    let hex = id.to_string();
    u64::from_str_radix(&hex[hex.len().saturating_sub(16)..], 16).unwrap_or(0)
}

fn folder_of(path: &str) -> String {
    path.rsplit_once('/')
        .map_or(String::new(), |(d, _)| d.to_string())
}

/// [`AssetCatalog`] over the asset database (see the module docs).
pub struct ServerCatalog {
    server: AssetServer,
    revision: u64,
    keys: BTreeMap<u64, AssetId>,
    backend: String,
    /// The intents last applied (so an unchanged project costs no sync).
    seen_settings_rev: Option<u64>,
}

impl std::fmt::Debug for ServerCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerCatalog")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl ServerCatalog {
    /// Over an opened asset server.
    pub fn new(server: AssetServer, backend: &str) -> Self {
        let mut c = Self {
            server,
            revision: 1,
            keys: BTreeMap::new(),
            backend: backend.to_string(),
            seen_settings_rev: None,
        };
        c.rekey();
        c
    }

    /// Over an empty in-memory project store (D-4: the editor binary until WP-U7 opens real
    /// projects; tests).
    pub fn in_memory() -> Result<Self, EditorError> {
        Self::over_memory_files(&[])
    }

    /// Over an in-memory store holding `files` (`(path, bytes)`), none imported yet.
    pub fn over_memory_files(files: &[(String, Vec<u8>)]) -> Result<Self, EditorError> {
        let ext =
            forge_importers::asset_extensions().map_err(|e| EditorError::Io(e.to_string()))?;
        let (server, _) = AssetServer::open_memory("editor", files, &ext)
            .map_err(|e| EditorError::Io(e.to_string()))?;
        Ok(Self::new(
            server,
            "in-memory project store (D-4) \u{2014} open a project folder to browse its assets",
        ))
    }

    /// Over an in-memory store holding `files`, with the importers, exporters and asset
    /// types of `ext` (the editor's plugin registries: WASM importers included, WP-21).
    pub fn over_extensions(
        files: &[(String, Vec<u8>)],
        ext: &forge_plugin::Extensions,
    ) -> Result<Self, EditorError> {
        let (server, _) = AssetServer::open_memory("editor", files, ext)
            .map_err(|e| EditorError::Io(e.to_string()))?;
        Ok(Self::new(
            server,
            "in-memory project store (D-4) — open a project folder to browse its assets",
        ))
    }

    /// The asset database (tests, tools).
    pub fn server(&self) -> &AssetServer {
        &self.server
    }

    fn rekey(&mut self) {
        self.keys = self
            .server
            .assets()
            .into_iter()
            .map(|a| (key_of(a.id), a.id))
            .collect();
    }
}

/// Calls `done` once the thumbnail handle settles (a waker on its `ready()` future).
struct ThumbWaker {
    handle: forge_asset::Handle<Thumbnail>,
    done: Mutex<Option<ThumbDone>>,
}

impl ThumbWaker {
    fn finish(&self) {
        let r = match self.handle.state() {
            LoadState::Loading => return,
            LoadState::Loaded => self
                .handle
                .get()
                .map(|t| (t.size, t.rgba.to_vec()))
                .ok_or_else(|| "no thumbnail".to_string()),
            LoadState::Failed(e) => Err(e.to_string()),
        };
        let done = self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(d) = done {
            d(r);
        }
    }
}

impl Wake for ThumbWaker {
    fn wake(self: Arc<Self>) {
        self.finish();
    }
}

impl AssetCatalog for ServerCatalog {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn rows(&self) -> Vec<AssetRow> {
        let locks: BTreeMap<String, String> = self
            .server
            .locked_sources()
            .into_iter()
            .map(|(p, o)| (p.to_string(), o))
            .collect();
        self.server
            .assets()
            .into_iter()
            .filter(|a| a.label.is_empty())
            .map(|a| {
                let (path, generated) = match &a.source {
                    AssetSource::File(p) => (p.to_string(), false),
                    AssetSource::Generated { generator, seed } => {
                        (format!("{generator}@{seed}"), true)
                    }
                };
                AssetRow {
                    key: key_of(a.id),
                    id: a.id.to_string(),
                    folder: if generated {
                        "generated".into()
                    } else {
                        folder_of(&path)
                    },
                    locked_by: locks.get(&path).cloned(),
                    path,
                    name: a.name.clone(),
                    kind: a.kind.clone(),
                    generated,
                    version: a.blob.map_or(0, |b| {
                        let h = b.to_hex();
                        u64::from_str_radix(&h[..16], 16).unwrap_or(0)
                    }),
                }
            })
            .collect()
    }

    fn thumbnail(&self, key: u64, size: u32, done: ThumbDone) {
        let Some(id) = self.keys.get(&key).copied() else {
            done(Err(format!("no asset with key {key:016x}")));
            return;
        };
        let handle = self.server.thumbnail(id, size);
        let w = Arc::new(ThumbWaker {
            handle: handle.clone(),
            done: Mutex::new(Some(done)),
        });
        let waker = Waker::from(w.clone());
        let mut fut = Box::pin(handle.ready());
        if fut
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_ready()
        {
            w.finish();
        }
    }

    fn stage(&mut self, files: &[PathBuf], folder: &str) -> Result<Vec<String>, EditorError> {
        let mut out = Vec::new();
        for f in files {
            let name = f
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| EditorError::Io(format!("{}: not a file name", f.display())))?;
            let dest = if folder.is_empty() {
                name.to_string()
            } else {
                format!("{folder}/{name}")
            };
            let sp =
                forge_store::StorePath::new(&dest).map_err(|e| EditorError::Io(e.to_string()))?;
            let bytes =
                std::fs::read(f).map_err(|e| EditorError::Io(format!("{}: {e}", f.display())))?;
            self.server
                .vfs()
                .write(&sp, bytes.into())
                .map_err(|e| EditorError::Io(e.to_string()))?;
            out.push(dest);
        }
        Ok(out)
    }

    fn follow(&mut self, mirror: &ProjectMirror) -> Vec<String> {
        let rev = mirror.settings_revision();
        if self.seen_settings_rev == Some(rev) {
            return Vec::new();
        }
        self.seen_settings_rev = Some(rev);
        let intents = intents_from_settings(mirror.settings());
        let events = self.server.sync_intents(intents);
        if !events.is_empty() {
            self.revision += 1;
            self.rekey();
        }
        events.iter().map(|e| format!("{e:?}")).collect()
    }

    fn importable(&self, path: &str) -> bool {
        forge_store::StorePath::new(path)
            .ok()
            .is_some_and(|p| self.server.importer_for(&p).is_some())
    }

    fn backend(&self) -> &str {
        &self.backend
    }

    fn plugins_changed(&mut self, ext: &forge_plugin::Extensions) {
        self.server.refresh_plugins(ext);
        self.revision += 1;
    }

    fn source(&self, path: &str) -> Option<Vec<u8>> {
        let p = forge_store::StorePath::new(path).ok()?;
        self.server.vfs().read(&p).ok().map(|b| b.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_the_low_bits_of_the_id() {
        let id = AssetId::generated("g", &forge_seed::SeedPath::universe());
        let hex = id.to_string();
        assert_eq!(format!("{:016x}", key_of(id)), hex[16..]);
    }
}
