//! [`AssetServer`] — the asset database: import, identity, loading, hot reload, export.
//!
//! ## Content addressing
//!
//! An import turns a source into artefacts, each stored in the project's blob store under its
//! BLAKE3 address (`ProjectStore::blob_put`, Ch.33.2). The sidecar records which addresses
//! the import produced. Consequences:
//!
//! * identical artefacts are stored once, across assets and across revisions;
//! * **reimport is free when nothing changed**: the sidecar's record (importer version,
//!   source hash, settings, every dependency's hash, every artefact present) is checked
//!   first, and a fresh record means the importer does not run at all;
//! * a loaded asset reloads only if its artefact **address** changed — touching a file, or
//!   editing a glTF's scene without touching a mesh, reloads nothing that did not change.
//!
//! ## Hot reload
//!
//! [`AssetServer::poll`] compares the store's cheap change stamps
//! ([`forge_store::ProjectStore::stamps`]) with the last poll. A changed source or
//! dependency reimports (content-checked, so a touch is a no-op); changed artefacts reload
//! under their live handles; a renamed file whose sidecar stayed behind is re-attached to
//! its id by content; an edited sidecar (settings changed in a text editor or by a VCS pull)
//! reimports with the new settings. The editor calls `poll` from a timer off the UI thread's
//! idle path; nothing is polled while nothing asks.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use bytes::Bytes;
use forge_plugin::{Extensions, Grants, loader};
use forge_seed::SeedPath;
use forge_store::{Blake3, ProjectStore, Stamp, StorePath};

use crate::handle::{AnyValue, Handle, Slot};
use crate::importer::{
    AssetTypeItem, AssetTypePoint, ExportArtefact, ExportSource, Exporter, ExporterPoint, ImportCx,
    Importer, ImporterPoint,
};
use crate::loader::Pool;
use crate::sidecar::{ArtefactRecord, ImportRecord, SIDECAR_SUFFIX, Settings, Sidecar};
use crate::thumb::{Thumbnail, render_thumbnail};
use crate::{AssetError, AssetId, FirstPartyAssets, Vfs};

/// Where an asset comes from: a project file, or a generator at a seed path (Ch.8: a
/// generated asset has a `SeedPath`, not a file, and is handled the same way).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetSource {
    /// Imported from this project file.
    File(StorePath),
    /// Produced by a generator at a seed path; never stored (Ch.33.2).
    Generated {
        /// The generator.
        generator: String,
        /// Where in the seed tree (its derivation path).
        seed: SeedPath,
    },
}

/// One asset, as the asset browser lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetInfo {
    /// Its stable id.
    pub id: AssetId,
    /// Its kind (`mesh`, `texture`, ...).
    pub kind: String,
    /// Display name.
    pub name: String,
    /// Its label within its source (`""` for the source's main asset).
    pub label: String,
    /// Where it comes from.
    pub source: AssetSource,
    /// Its artefact's content address (`None` for generated assets).
    pub blob: Option<Blake3>,
}

/// Something that happened to the asset database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetEvent {
    /// A source was imported for the first time.
    Imported {
        /// Its main asset.
        id: AssetId,
        /// The source.
        path: StorePath,
    },
    /// A source's importer ran again (its content, settings or a dependency changed).
    Reimported {
        /// Its main asset.
        id: AssetId,
        /// The source.
        path: StorePath,
    },
    /// A live asset's value was replaced (its artefact changed).
    Reloaded {
        /// The asset.
        id: AssetId,
    },
    /// A source moved; its id did not change.
    Renamed {
        /// Its main asset.
        id: AssetId,
        /// Old path.
        from: StorePath,
        /// New path.
        to: StorePath,
    },
    /// A source's asset registration was removed (its sidecar is gone).
    Removed {
        /// Its main asset.
        id: AssetId,
        /// The source.
        path: StorePath,
    },
    /// A file appeared that an importer could handle but that is not imported yet (the
    /// editor may issue `forge.asset.import` for it).
    Discovered {
        /// The file.
        path: StorePath,
    },
    /// An operation on a source failed; the previous artefacts stay loaded.
    Failed {
        /// The source.
        path: StorePath,
        /// Why.
        error: AssetError,
    },
}

/// A generator of assets: seed path -> artefact bytes (deterministic).
pub type GeneratorFn = Arc<dyn Fn(&SeedPath) -> Result<Bytes, String> + Send + Sync>;

#[derive(Clone)]
struct Generator {
    kind: String,
    f: GeneratorFn,
}

type Types = Arc<BTreeMap<String, AssetTypeItem>>;

/// A new file a poll considers as the renamed form of a vanished source; what was learnt
/// about it is kept so no file is looked at twice in one poll.
struct Probe {
    path: StorePath,
    /// `Some(None)`: looked up, not a file any more.
    size: Option<Option<u64>>,
    /// `Some(None)`: tried, it was gone.
    hash: Option<Option<String>>,
    /// Matched to a vanished source already.
    taken: bool,
}

impl Probe {
    fn new(path: StorePath) -> Self {
        Self {
            path,
            size: None,
            hash: None,
            taken: false,
        }
    }
}

/// The asset database.
pub struct AssetServer {
    vfs: Arc<Vfs>,
    importers: Vec<(String, Arc<dyn Importer>)>,
    exporters: Vec<(String, Arc<dyn Exporter>)>,
    types: Types,
    sources: BTreeMap<StorePath, Sidecar>,
    /// Sidecars whose source vanished (kept so a rename can re-attach them).
    orphans: BTreeMap<StorePath, Sidecar>,
    entries: HashMap<AssetId, AssetInfo>,
    dependents: BTreeMap<StorePath, BTreeSet<StorePath>>,
    generators: Arc<RwLock<BTreeMap<String, Generator>>>,
    slots: Mutex<HashMap<AssetId, Weak<Slot>>>,
    thumbs: Mutex<HashMap<(AssetId, u32), Weak<Slot>>>,
    stamps: BTreeMap<StorePath, Stamp>,
    /// What the last `stamps()` call cost (the idle price of one poll).
    stamp_cost: std::time::Duration,
    /// How many paths the last poll found added, changed or removed.
    poll_changes: usize,
    import_runs: AtomicU64,
    pub(crate) intents: BTreeMap<String, crate::commands::Intent>,
    pool: Pool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn extension(p: &StorePath) -> String {
    let name = p.file_name();
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn panic_text(p: &(dyn Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".into())
}

impl AssetServer {
    /// Define the three asset points (`Importer`, `Exporter`, `AssetType`) in `x`, for a host
    /// that loads its own asset plugins.
    pub fn define_points(x: &mut Extensions) -> Result<(), forge_plugin::PluginError> {
        x.define::<ImporterPoint>()?;
        x.define::<ExporterPoint>()?;
        x.define::<AssetTypePoint>()
    }

    /// An [`Extensions`] with the three asset points defined and the kernel's asset types
    /// (`forge.asset`) loaded through the ordinary plugin loader (I16). It has **no importers
    /// or exporters**: those are plugins. The first-party glTF, image and KTX2 ones are
    /// `plugins/forge-importers` (`forge_importers::asset_extensions()` loads both).
    pub fn default_extensions() -> Result<Extensions, AssetError> {
        let plug = |e: forge_plugin::PluginError| AssetError::Import {
            path: String::new(),
            importer: "forge.asset".into(),
            why: e.to_string(),
        };
        let mut x = Extensions::new();
        Self::define_points(&mut x).map_err(plug)?;
        let fp = FirstPartyAssets::new().map_err(plug)?;
        loader::load(&mut x, &[&fp], &[], &Grants::new()).map_err(plug)?;
        Ok(x)
    }

    /// Open the asset database of `store`, with the importers, exporters and types
    /// registered in `ext`. Every sidecar is read and every stale import redone; the events
    /// report what happened (failures do not fail the open: those assets keep their last
    /// artefacts and report `Failed`).
    pub fn open(
        store: Box<dyn ProjectStore>,
        ext: &Extensions,
    ) -> Result<(Self, Vec<AssetEvent>), AssetError> {
        Self::open_vfs(Arc::new(Vfs::new(store)), ext)
    }

    /// An asset database over a fresh in-memory project store holding `files`
    /// (`(path, bytes)`, none imported yet) owned by `owner`, with the plugins loaded in `ext`:
    /// what the editor browses before a project folder is opened, and what tests use.
    pub fn open_memory(
        owner: &str,
        files: &[(String, Vec<u8>)],
        ext: &Extensions,
    ) -> Result<(Self, Vec<AssetEvent>), AssetError> {
        let mut store = forge_store::MemoryStore::new(owner);
        for (p, b) in files {
            store.write(&StorePath::new(p)?, Bytes::from(b.clone()))?;
        }
        Self::open(Box::new(store), ext)
    }

    /// Sources whose file a store lock holds, with the holder (Ch.33 §33.3). A store that
    /// cannot list locks reports none.
    #[must_use]
    pub fn locked_sources(&self) -> Vec<(StorePath, String)> {
        self.vfs.locks().unwrap_or_default()
    }

    /// [`AssetServer::open`] over a VFS someone else also holds.
    pub fn open_vfs(
        vfs: Arc<Vfs>,
        ext: &Extensions,
    ) -> Result<(Self, Vec<AssetEvent>), AssetError> {
        let importers = ext
            .registry::<ImporterPoint>()
            .map(|r| {
                r.iter()
                    .map(|(k, v)| (k.to_string(), Arc::clone(v)))
                    .collect()
            })
            .unwrap_or_default();
        let exporters = ext
            .registry::<ExporterPoint>()
            .map(|r| {
                r.iter()
                    .map(|(k, v)| (k.to_string(), Arc::clone(v)))
                    .collect()
            })
            .unwrap_or_default();
        let types = ext
            .registry::<AssetTypePoint>()
            .map(|r| r.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
            .unwrap_or_default();
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(4))
            .unwrap_or(2);
        let mut s = Self {
            vfs,
            importers,
            exporters,
            types: Arc::new(types),
            sources: BTreeMap::new(),
            orphans: BTreeMap::new(),
            entries: HashMap::new(),
            dependents: BTreeMap::new(),
            generators: Arc::new(RwLock::new(BTreeMap::new())),
            slots: Mutex::new(HashMap::new()),
            thumbs: Mutex::new(HashMap::new()),
            stamps: BTreeMap::new(),
            stamp_cost: std::time::Duration::ZERO,
            poll_changes: 0,
            import_runs: AtomicU64::new(0),
            intents: BTreeMap::new(),
            pool: Pool::new(threads),
        };
        let mut events = Vec::new();
        let files = s.vfs.list()?;
        for sc_path in files
            .iter()
            .filter(|p| p.as_str().ends_with(SIDECAR_SUFFIX))
        {
            let Some(source) = Sidecar::source_of(sc_path) else {
                continue;
            };
            let sc = match s
                .vfs
                .read(sc_path)
                .and_then(|b| Sidecar::decode(sc_path, &b))
            {
                Ok(sc) => sc,
                Err(error) => {
                    events.push(AssetEvent::Failed {
                        path: source,
                        error,
                    });
                    continue;
                }
            };
            if !s.vfs.exists(&source)? {
                s.orphans.insert(source, sc);
                continue;
            }
            // Register what the sidecar records first: if the refresh fails, the last good
            // artefacts still load.
            s.register(&source, sc.clone());
            if let Err(error) = s.refresh(&source, sc, &mut events) {
                events.push(AssetEvent::Failed {
                    path: source,
                    error,
                });
            }
        }
        s.vfs.take_written();
        let t = std::time::Instant::now();
        s.stamps = s.vfs.stamps()?.into_iter().collect();
        s.stamp_cost = t.elapsed();
        Ok((s, events))
    }

    /// Take the importers and exporters `ext` holds now (a plugin that provides one was
    /// installed while the database is open — a WASM plugin dropped into the editor, WP-21).
    /// Asset types stay as opened: a decoder is Rust code a sandboxed plugin cannot add.
    /// Files an added importer claims become importable; nothing already imported changes
    /// until its importer's version does.
    pub fn refresh_plugins(&mut self, ext: &Extensions) {
        if let Some(r) = ext.registry::<ImporterPoint>() {
            self.importers = r
                .iter()
                .map(|(k, v)| (k.to_string(), Arc::clone(v)))
                .collect();
        }
        if let Some(r) = ext.registry::<ExporterPoint>() {
            self.exporters = r
                .iter()
                .map(|(k, v)| (k.to_string(), Arc::clone(v)))
                .collect();
        }
    }

    /// The VFS.
    #[must_use]
    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    /// How many times an importer has actually run (cache misses) since open.
    #[must_use]
    pub fn import_runs(&self) -> u64 {
        self.import_runs.load(Ordering::Relaxed)
    }

    // ---- queries ---------------------------------------------------------------------

    /// An asset by id.
    #[must_use]
    pub fn info(&self, id: AssetId) -> Option<&AssetInfo> {
        self.entries.get(&id)
    }

    /// Every asset, sorted by (source, label).
    #[must_use]
    pub fn assets(&self) -> Vec<AssetInfo> {
        let mut v: Vec<AssetInfo> = self.entries.values().cloned().collect();
        v.sort_by(|a, b| {
            let key = |i: &AssetInfo| match &i.source {
                AssetSource::File(p) => (0, p.to_string(), i.label.clone()),
                AssetSource::Generated { generator, seed } => {
                    (1, format!("{generator}@{seed}"), i.label.clone())
                }
            };
            key(a).cmp(&key(b))
        });
        v
    }

    /// Every asset of `kind`.
    #[must_use]
    pub fn assets_of_kind(&self, kind: &str) -> Vec<AssetInfo> {
        self.assets()
            .into_iter()
            .filter(|a| a.kind == kind)
            .collect()
    }

    /// The main asset imported from `path`.
    #[must_use]
    pub fn id_of(&self, path: &StorePath) -> Option<AssetId> {
        self.sources.get(path).map(|s| s.id)
    }

    /// The source file of an asset (for generated assets, `None`).
    #[must_use]
    pub fn path_of(&self, id: AssetId) -> Option<&StorePath> {
        match &self.entries.get(&id)?.source {
            AssetSource::File(p) => Some(p),
            AssetSource::Generated { .. } => None,
        }
    }

    /// The sidecar of an imported source.
    #[must_use]
    pub fn sidecar(&self, path: &StorePath) -> Option<&Sidecar> {
        self.sources.get(path)
    }

    /// Every imported source, sorted.
    #[must_use]
    pub fn sources(&self) -> Vec<StorePath> {
        self.sources.keys().cloned().collect()
    }

    /// The importer that would handle `path` (registry order: the first claiming its
    /// extension).
    #[must_use]
    pub fn importer_for(&self, path: &StorePath) -> Option<&str> {
        let ext = extension(path);
        if ext.is_empty() || path.as_str().ends_with(SIDECAR_SUFFIX) {
            return None;
        }
        self.importers
            .iter()
            .find(|(_, i)| i.extensions().iter().any(|e| e.eq_ignore_ascii_case(&ext)))
            .map(|(k, _)| k.as_str())
    }

    fn importer(&self, key: &str) -> Result<Arc<dyn Importer>, AssetError> {
        self.importers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, i)| Arc::clone(i))
            .ok_or_else(|| AssetError::NoImporter(format!("(importer {key:?} is not registered)")))
    }

    // ---- import --------------------------------------------------------------------------

    /// Import `path` with `settings` (or change an imported source's settings and reimport).
    /// Returns its main asset id; the id of an already-imported source never changes.
    pub fn import(
        &mut self,
        path: &StorePath,
        settings: Settings,
        events: &mut Vec<AssetEvent>,
    ) -> Result<AssetId, AssetError> {
        if !self.vfs.exists(path)? {
            return Err(AssetError::UnknownAsset(path.to_string()));
        }
        let sc = match self.sources.get(path) {
            Some(sc) => Sidecar {
                settings,
                ..sc.clone()
            },
            // A sidecar whose source was missing: the file is back, and so is its id.
            None if self.orphans.contains_key(path) => match self.orphans.remove(path) {
                Some(o) => Sidecar { settings, ..o },
                None => return Err(AssetError::UnknownAsset(path.to_string())),
            },
            None => {
                let importer = self
                    .importer_for(path)
                    .ok_or_else(|| AssetError::NoImporter(path.to_string()))?
                    .to_string();
                let content = Blake3::of(&self.vfs.read(path)?);
                Sidecar {
                    id: AssetId::mint(path, &content),
                    importer,
                    settings,
                    import: None,
                }
            }
        };
        let id = sc.id;
        self.refresh(path, sc, events)?;
        Ok(id)
    }

    /// Reimport `path` if anything it depends on changed (a no-op otherwise).
    pub fn reimport(
        &mut self,
        path: &StorePath,
        events: &mut Vec<AssetEvent>,
    ) -> Result<AssetId, AssetError> {
        let sc = self
            .sources
            .get(path)
            .cloned()
            .ok_or_else(|| AssetError::UnknownAsset(path.to_string()))?;
        let id = sc.id;
        self.refresh(path, sc, events)?;
        Ok(id)
    }

    fn fresh(&self, sc: &Sidecar, source: &Blake3, version: u32) -> Result<bool, AssetError> {
        let Some(r) = &sc.import else {
            return Ok(false);
        };
        if r.importer != sc.importer
            || r.importer_version != version
            || r.source != source.to_hex()
            || r.settings != crate::sidecar::settings_hash(&sc.settings)
        {
            return Ok(false);
        }
        for (p, h) in &r.deps {
            let Ok(p) = StorePath::new(p) else {
                return Ok(false);
            };
            if !self.vfs.exists(&p)? || Blake3::of(&self.vfs.read(&p)?).to_hex() != *h {
                return Ok(false);
            }
        }
        for a in &r.artefacts {
            match a.blob() {
                Ok(b) if self.vfs.blob_has(b)? => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    /// Bring `path`'s import up to date with `sc`'s settings: run the importer unless the
    /// record is fresh, store artefacts, write the sidecar if it changed, register, reload.
    fn refresh(
        &mut self,
        path: &StorePath,
        mut sc: Sidecar,
        events: &mut Vec<AssetEvent>,
    ) -> Result<(), AssetError> {
        let first = !self.sources.contains_key(path);
        let old = self.sources.get(path).cloned();
        let source = self.vfs.read(path)?;
        let source_hash = Blake3::of(&source);
        let source_size = source.len() as u64;
        let importer = self.importer(&sc.importer)?;
        let version = importer.version();
        let fresh = self.fresh(&sc, &source_hash, version)?;
        if !fresh {
            let mut cx = ImportCx {
                id: sc.id,
                importer: &sc.importer,
                path,
                source,
                settings: &sc.settings,
                vfs: &self.vfs,
                deps: BTreeMap::new(),
                artefacts: Vec::new(),
            };
            self.import_runs.fetch_add(1, Ordering::Relaxed);
            match catch_unwind(AssertUnwindSafe(|| importer.import(&mut cx))) {
                Ok(r) => r?,
                Err(p) => {
                    return Err(AssetError::import(
                        path,
                        &sc.importer,
                        format!("the importer panicked: {}", panic_text(p.as_ref())),
                    ));
                }
            }
            let (deps, artefacts) = (cx.deps, cx.artefacts);
            let mut labels = BTreeSet::new();
            for a in &artefacts {
                if !labels.insert(a.label.as_str()) {
                    return Err(AssetError::import(
                        path,
                        &sc.importer,
                        format!("two artefacts are labelled {:?}", a.label),
                    ));
                }
                if !self.types.contains_key(&a.kind) {
                    return Err(AssetError::UnknownKind(a.kind.clone()));
                }
            }
            if !labels.contains("") {
                return Err(AssetError::import(
                    path,
                    &sc.importer,
                    "no main artefact (label \"\")",
                ));
            }
            let mut records = Vec::with_capacity(artefacts.len());
            for a in artefacts {
                let blob = self.vfs.blob_put(a.bytes)?;
                records.push(ArtefactRecord(
                    a.label.clone(),
                    sc.id.child(&a.label),
                    a.kind,
                    a.name,
                    blob.to_hex(),
                ));
            }
            records.sort_by(|a, b| a.0.cmp(&b.0));
            sc.import = Some(ImportRecord {
                importer: sc.importer.clone(),
                importer_version: version,
                source: source_hash.to_hex(),
                source_size: Some(source_size),
                settings: crate::sidecar::settings_hash(&sc.settings),
                deps: deps
                    .into_iter()
                    .map(|(p, h)| (p.to_string(), h.to_hex()))
                    .collect(),
                artefacts: records,
            });
        }
        // Write the sidecar when its text changed.
        let sc_path = Sidecar::path_for(path)?;
        let text = sc.encode()?;
        let on_disk = if self.vfs.exists(&sc_path)? {
            Some(self.vfs.read(&sc_path)?)
        } else {
            None
        };
        if on_disk.as_deref() != Some(&text[..]) {
            self.vfs.write(&sc_path, text)?;
        }
        let old_blobs: HashMap<AssetId, Blake3> = self.family_blobs(old.as_ref());
        self.register(path, sc.clone());
        let new_blobs = self.family_blobs(Some(&sc));
        if first {
            events.push(AssetEvent::Imported {
                id: sc.id,
                path: path.clone(),
            });
        } else if !fresh {
            events.push(AssetEvent::Reimported {
                id: sc.id,
                path: path.clone(),
            });
        }
        let changed: Vec<AssetId> = new_blobs
            .iter()
            .filter(|(id, b)| old_blobs.get(id) != Some(b))
            .map(|(id, _)| *id)
            .collect();
        let gone: Vec<AssetId> = old_blobs
            .keys()
            .filter(|id| !new_blobs.contains_key(id))
            .copied()
            .collect();
        for id in &gone {
            self.fail_slot(*id, AssetError::UnknownAsset(id.to_string()));
        }
        // Live handles of changed artefacts reload — also on a first import, where a live
        // handle means the asset is coming back (a redo, a returning file).
        if !changed.is_empty() {
            for id in &changed {
                if self.reload_slot(*id) {
                    events.push(AssetEvent::Reloaded { id: *id });
                }
            }
            // Thumbnails of the whole family: a scene's picture depends on its meshes.
            for id in new_blobs.keys() {
                self.reload_thumbs(*id);
            }
        }
        Ok(())
    }

    fn family_blobs(&self, sc: Option<&Sidecar>) -> HashMap<AssetId, Blake3> {
        sc.and_then(|s| s.import.as_ref())
            .map(|r| {
                r.artefacts
                    .iter()
                    .filter_map(|a| a.blob().ok().map(|b| (a.id(), b)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Put `sc`'s artefacts in the database under `path`.
    fn register(&mut self, path: &StorePath, sc: Sidecar) {
        self.unregister(path);
        if let Some(r) = &sc.import {
            for a in &r.artefacts {
                self.entries.insert(
                    a.id(),
                    AssetInfo {
                        id: a.id(),
                        kind: a.kind().to_string(),
                        name: a.name().to_string(),
                        label: a.label().to_string(),
                        source: AssetSource::File(path.clone()),
                        blob: a.blob().ok(),
                    },
                );
            }
            for (d, _) in &r.deps {
                if let Ok(d) = StorePath::new(d) {
                    self.dependents.entry(d).or_default().insert(path.clone());
                }
            }
        }
        self.sources.insert(path.clone(), sc);
    }

    fn unregister(&mut self, path: &StorePath) -> Option<Sidecar> {
        let sc = self.sources.remove(path)?;
        if let Some(r) = &sc.import {
            for a in &r.artefacts {
                self.entries.remove(&a.id());
            }
            for (d, _) in &r.deps {
                if let Ok(d) = StorePath::new(d)
                    && let Some(set) = self.dependents.get_mut(&d)
                {
                    set.remove(path);
                    if set.is_empty() {
                        self.dependents.remove(&d);
                    }
                }
            }
        }
        Some(sc)
    }

    // ---- rename / remove -------------------------------------------------------------

    fn move_file(&self, from: &StorePath, to: &StorePath) -> Result<(), AssetError> {
        let b = self.vfs.read(from)?;
        if from.folded() == to.folded() {
            // A case-only rename: the new name collides with the old until the old is gone.
            self.vfs.delete(from)?;
            if let Err(e) = self.vfs.write(to, b.clone()) {
                let _ = self.vfs.write(from, b);
                return Err(e);
            }
        } else {
            self.vfs.write(to, b)?;
            self.vfs.delete(from)?;
        }
        Ok(())
    }

    /// Move an imported source (and its sidecar) to `to`. Its id and every sub-asset id stay
    /// the same, so references to it survive.
    pub fn rename(
        &mut self,
        from: &StorePath,
        to: &StorePath,
        events: &mut Vec<AssetEvent>,
    ) -> Result<AssetId, AssetError> {
        if !self.sources.contains_key(from) {
            return Err(AssetError::UnknownAsset(from.to_string()));
        }
        let (sc_from, sc_to) = (Sidecar::path_for(from)?, Sidecar::path_for(to)?);
        let case_only = from.folded() == to.folded();
        if !case_only && (self.vfs.exists(to)? || self.vfs.exists(&sc_to)?) {
            return Err(AssetError::AlreadyExists(to.to_string()));
        }
        self.move_file(from, to)?;
        if self.vfs.exists(&sc_from)? {
            self.move_file(&sc_from, &sc_to)?;
        }
        let Some(sc) = self.unregister(from) else {
            return Err(AssetError::UnknownAsset(from.to_string()));
        };
        let id = sc.id;
        self.register(to, sc);
        events.push(AssetEvent::Renamed {
            id,
            from: from.clone(),
            to: to.clone(),
        });
        Ok(id)
    }

    /// Remove `path`'s asset registration (its sidecar). The source file stays.
    pub fn remove(
        &mut self,
        path: &StorePath,
        events: &mut Vec<AssetEvent>,
    ) -> Result<AssetId, AssetError> {
        let sc = self
            .unregister(path)
            .ok_or_else(|| AssetError::UnknownAsset(path.to_string()))?;
        let sc_path = Sidecar::path_for(path)?;
        if self.vfs.exists(&sc_path)? {
            self.vfs.delete(&sc_path)?;
        }
        for id in self.family_blobs(Some(&sc)).keys() {
            self.fail_slot(*id, AssetError::UnknownAsset(id.to_string()));
        }
        events.push(AssetEvent::Removed {
            id: sc.id,
            path: path.clone(),
        });
        Ok(sc.id)
    }

    // ---- generated assets ------------------------------------------------------------

    /// Register a generator of `kind` artefacts.
    pub fn register_generator(&mut self, name: &str, kind: &str, f: GeneratorFn) {
        if let Ok(mut g) = self.generators.write() {
            g.insert(
                name.to_string(),
                Generator {
                    kind: kind.to_string(),
                    f,
                },
            );
        }
    }

    /// The asset `generator` produces at `seed`: registered like any other asset (listed,
    /// loadable through a typed handle, thumbnailed) but never stored — it is regenerated
    /// from its seed path on demand (Ch.33.2).
    pub fn generated(&mut self, generator: &str, seed: &SeedPath) -> Result<AssetId, AssetError> {
        let kind = self
            .generators
            .read()
            .ok()
            .and_then(|g| g.get(generator).map(|g| g.kind.clone()))
            .ok_or_else(|| AssetError::Generator {
                name: generator.to_string(),
                why: "no such generator is registered".into(),
            })?;
        let id = AssetId::generated(generator, seed);
        self.entries.insert(
            id,
            AssetInfo {
                id,
                kind,
                name: format!("{generator} @ {seed}"),
                label: String::new(),
                source: AssetSource::Generated {
                    generator: generator.to_string(),
                    seed: seed.clone(),
                },
                blob: None,
            },
        );
        Ok(id)
    }

    // ---- loading ---------------------------------------------------------------------

    fn fetcher(&self) -> Fetcher {
        Fetcher {
            vfs: Arc::clone(&self.vfs),
            generators: Arc::clone(&self.generators),
        }
    }

    /// Load an asset as `T`. Returns at once; the value arrives on a worker thread.
    pub fn load<T: Any + Send + Sync>(&self, id: AssetId) -> Handle<T> {
        let Some(info) = self.entries.get(&id) else {
            let s = Slot::new(id);
            s.finish(Err(AssetError::UnknownAsset(id.to_string())));
            return Handle::new(s);
        };
        let ty = self.types.get(&info.kind);
        let wanted = TypeId::of::<T>();
        if ty.map(|t| t.type_id) != Some(wanted) {
            let s = Slot::new(id);
            s.finish(Err(match ty {
                None => AssetError::UnknownKind(info.kind.clone()),
                Some(_) => AssetError::WrongType {
                    id: id.to_string(),
                    kind: info.kind.clone(),
                    wanted: std::any::type_name::<T>(),
                },
            }));
            return Handle::new(s);
        }
        let mut slots = lock(&self.slots);
        if let Some(s) = slots.get(&id).and_then(Weak::upgrade) {
            return Handle::new(s);
        }
        let s = Slot::new(id);
        slots.insert(id, Arc::downgrade(&s));
        slots.retain(|_, w| w.strong_count() > 0);
        drop(slots);
        self.schedule(&s);
        Handle::new(s)
    }

    /// Load by source path (the source's main asset).
    pub fn load_path<T: Any + Send + Sync>(&self, path: &StorePath) -> Handle<T> {
        match self.id_of(path) {
            Some(id) => self.load(id),
            None => {
                let s = Slot::new(AssetId(0));
                s.finish(Err(AssetError::UnknownAsset(path.to_string())));
                Handle::new(s)
            }
        }
    }

    fn schedule(&self, slot: &Arc<Slot>) {
        let id = slot.id;
        let Some(info) = self.entries.get(&id).cloned() else {
            slot.finish(Err(AssetError::UnknownAsset(id.to_string())));
            return;
        };
        let Some(ty) = self.types.get(&info.kind).cloned() else {
            slot.finish(Err(AssetError::UnknownKind(info.kind)));
            return;
        };
        slot.begin();
        let fetch = self.fetcher();
        let slot = Arc::clone(slot);
        self.pool.spawn(Box::new(move || {
            let r = fetch.bytes(&info).and_then(|b| {
                match catch_unwind(AssertUnwindSafe(|| (ty.decode)(&b))) {
                    Ok(r) => r.map_err(|e| AssetError::decode(id, e)),
                    Err(p) => Err(AssetError::decode(
                        id,
                        format!("the decoder panicked: {}", panic_text(p.as_ref())),
                    )),
                }
            });
            slot.finish(r);
        }));
    }

    /// Reload a live slot; returns whether one was live.
    fn reload_slot(&self, id: AssetId) -> bool {
        let s = lock(&self.slots).get(&id).and_then(Weak::upgrade);
        match s {
            Some(s) => {
                self.schedule(&s);
                true
            }
            None => false,
        }
    }

    fn fail_slot(&self, id: AssetId, e: AssetError) {
        if let Some(s) = lock(&self.slots).get(&id).and_then(Weak::upgrade) {
            s.finish(Err(e));
        }
    }

    // ---- thumbnails ------------------------------------------------------------------

    /// A square RGBA8 thumbnail of an asset, `size` pixels (clamped to 8..=512), rendered on
    /// a worker thread and refreshed when the asset reloads. The asset browser (WP-U5) shows
    /// these; they are never stored in the project.
    pub fn thumbnail(&self, id: AssetId, size: u32) -> Handle<Thumbnail> {
        let size = size.clamp(8, 512);
        let mut thumbs = lock(&self.thumbs);
        if let Some(s) = thumbs.get(&(id, size)).and_then(Weak::upgrade) {
            return Handle::new(s);
        }
        let s = Slot::new(id);
        thumbs.insert((id, size), Arc::downgrade(&s));
        thumbs.retain(|_, w| w.strong_count() > 0);
        drop(thumbs);
        self.schedule_thumb(&s, size);
        Handle::new(s)
    }

    fn schedule_thumb(&self, slot: &Arc<Slot>, size: u32) {
        let id = slot.id;
        let Some(info) = self.entries.get(&id).cloned() else {
            slot.finish(Err(AssetError::UnknownAsset(id.to_string())));
            return;
        };
        // Referenced assets a thumbnail may need (a scene's meshes, a material's texture).
        let refs: HashMap<AssetId, AssetInfo> = match &info.source {
            AssetSource::File(p) => self
                .sources
                .get(p)
                .and_then(|s| s.import.as_ref())
                .map(|r| {
                    r.artefacts
                        .iter()
                        .filter_map(|a| self.entries.get(&a.id()).map(|e| (a.id(), e.clone())))
                        .collect()
                })
                .unwrap_or_default(),
            AssetSource::Generated { .. } => HashMap::new(),
        };
        slot.begin();
        let fetch = self.fetcher();
        let slot = Arc::clone(slot);
        self.pool.spawn(Box::new(move || {
            let r = fetch.bytes(&info).and_then(|b| {
                let get = |rid: AssetId| refs.get(&rid).and_then(|e| fetch.bytes(e).ok());
                catch_unwind(AssertUnwindSafe(|| {
                    render_thumbnail(&info.kind, &b, &get, size)
                }))
                .unwrap_or_else(|p| Err(format!("thumbnail panicked: {}", panic_text(p.as_ref()))))
                .map(|t| Arc::new(t) as AnyValue)
                .map_err(|e| AssetError::decode(id, e))
            });
            slot.finish(r);
        }));
    }

    fn reload_thumbs(&self, id: AssetId) {
        let live: Vec<(u32, Arc<Slot>)> = lock(&self.thumbs)
            .iter()
            .filter(|((i, _), _)| *i == id)
            .filter_map(|((_, sz), w)| w.upgrade().map(|s| (*sz, s)))
            .collect();
        for (sz, s) in live {
            self.schedule_thumb(&s, sz);
        }
    }

    // ---- export ----------------------------------------------------------------------

    /// Export `id` with exporter `key` (or the first registered one handling its kind).
    pub fn export(
        &self,
        id: AssetId,
        key: Option<&str>,
        name: &str,
    ) -> Result<Vec<(String, Bytes)>, AssetError> {
        let info = self
            .entries
            .get(&id)
            .ok_or_else(|| AssetError::UnknownAsset(id.to_string()))?;
        let ex = self
            .exporters
            .iter()
            .find(|(k, e)| key.map_or(e.kinds().contains(&info.kind.as_str()), |w| w == k))
            .map(|(_, e)| Arc::clone(e))
            .ok_or_else(|| AssetError::Export {
                id: id.to_string(),
                why: format!("no exporter handles {:?}", info.kind),
            })?;
        ex.export(self, id, name)
    }

    // ---- hot reload ------------------------------------------------------------------

    /// How long the editor's hot-reload timer should wait before the next [`poll`]:
    /// twenty times what the last poll's `stamps()` cost — so polling takes at most ~5 % of
    /// one core, off the UI thread — but never less than 250 ms (edits show up at once to a
    /// person) and never more than 2 s (hot reload must stay hot). On `LocalFs` a quiet poll
    /// costs ~3 µs per file (Ch.8 §8.9): a 10 000-file project polls every ~0.6 s, a
    /// 100 000-file one every 2 s at ~19 % of one core — where a store-side watcher takes
    /// over (Ch.8 §8.4, ADR 0017).
    ///
    /// [`poll`]: AssetServer::poll
    #[must_use]
    pub fn poll_interval(&self) -> std::time::Duration {
        const FLOOR: std::time::Duration = std::time::Duration::from_millis(250);
        const CEILING: std::time::Duration = std::time::Duration::from_secs(2);
        self.stamp_cost.saturating_mul(20).clamp(FLOOR, CEILING)
    }

    /// How many files the last [`poll`] found added, changed or removed since the poll
    /// before it — its own writes (sidecars, moved files) excluded, since a poll takes
    /// their stamps as the baseline. Zero means that poll was a quiet one: one `stamps()`
    /// call and nothing else. Diagnostic (hot reload's work, owner rule 2).
    ///
    /// [`poll`]: AssetServer::poll
    #[must_use]
    pub fn last_poll_changes(&self) -> usize {
        self.poll_changes
    }

    /// Look for changes in the store since the last poll and act on them (see the module
    /// docs). Cheap when nothing changed: one `stamps()` call, which on `LocalFs` is one
    /// directory walk and reads no file (measured in `test_import_perf.rs`).
    pub fn poll(&mut self) -> Result<Vec<AssetEvent>, AssetError> {
        let t = std::time::Instant::now();
        let now: BTreeMap<StorePath, Stamp> = self.vfs.stamps()?.into_iter().collect();
        self.stamp_cost = t.elapsed();
        let changed: BTreeSet<StorePath> = now
            .iter()
            .filter(|(p, s)| self.stamps.get(*p) != Some(*s))
            .map(|(p, _)| p.clone())
            .collect();
        let removed: BTreeSet<StorePath> = self
            .stamps
            .keys()
            .filter(|p| !now.contains_key(*p))
            .cloned()
            .collect();
        let added: BTreeSet<StorePath> = changed
            .iter()
            .filter(|p| !self.stamps.contains_key(*p))
            .cloned()
            .collect();
        self.stamps = now;
        self.poll_changes = changed.len() + removed.len();
        let mut events = Vec::new();
        if changed.is_empty() && removed.is_empty() {
            // Anything written before the stamps above is already in them.
            self.vfs.take_written();
            return Ok(events);
        }
        if !added.is_empty() || !removed.is_empty() {
            self.vfs.invalidate();
        }
        let is_sc = |p: &StorePath| p.as_str().ends_with(SIDECAR_SUFFIX);

        // 1. Sources that vanished. Their sidecar moved with them (a pair rename), or stayed
        //    behind (re-attach by content below), or went too (deleted).
        let mut vanished: Vec<(StorePath, Sidecar)> = Vec::new();
        for p in removed.iter().filter(|p| !is_sc(p)) {
            // Live handles keep their last value until we know what happened.
            if let Some(sc) = self.unregister(p) {
                vanished.push((p.clone(), sc));
            }
        }
        // 2. New sidecars (a pair rename's new half, or a VCS pull).
        let mut claimed: BTreeSet<AssetId> = BTreeSet::new();
        for scp in added.iter().filter(|p| is_sc(p)) {
            let Some(src) = Sidecar::source_of(scp) else {
                continue;
            };
            let sc = match self.vfs.read(scp).and_then(|b| Sidecar::decode(scp, &b)) {
                Ok(sc) => sc,
                Err(error) => {
                    events.push(AssetEvent::Failed { path: src, error });
                    continue;
                }
            };
            if self.sources.get(&src) == Some(&sc) {
                // Already registered exactly so: our own write (an import or rename).
                claimed.insert(sc.id);
                continue;
            }
            if !self.vfs.exists(&src)? {
                self.orphans.insert(src, sc);
                continue;
            }
            if let Some((from, _)) = vanished.iter().find(|(_, v)| v.id == sc.id) {
                events.push(AssetEvent::Renamed {
                    id: sc.id,
                    from: from.clone(),
                    to: src.clone(),
                });
            }
            claimed.insert(sc.id);
            self.register(&src, sc.clone());
            if let Err(error) = self.refresh(&src, sc, &mut events) {
                events.push(AssetEvent::Failed { path: src, error });
            }
        }
        // 3. A source renamed without its sidecar: match new files to vanished sources by
        //    content, move the sidecar, keep the id. A branch switch can vanish hundreds of
        //    sources and add thousands of files, so the candidates are listed once, each
        //    one's size is looked up (metadata, no read) at most once and compared first,
        //    and only a candidate whose size matches is read and hashed — at most once per
        //    poll however many vanished sources consider it.
        let mut probes: Option<Vec<Probe>> = None;
        for (from, sc) in vanished {
            if claimed.contains(&sc.id) {
                continue;
            }
            let mut matched = None;
            if let Some(rec) = sc.import.as_ref() {
                let probes = match &mut probes {
                    Some(v) => v,
                    None => {
                        let mut v = Vec::new();
                        for q in added.iter().filter(|q| !is_sc(q)) {
                            // `self.stamps` is this poll's full listing, so a sidecar's
                            // presence is a map lookup, not a store stat per added file.
                            if !self.sources.contains_key(q)
                                && !self.stamps.contains_key(&Sidecar::path_for(q)?)
                            {
                                v.push(Probe::new(q.clone()));
                            }
                        }
                        probes.insert(v)
                    }
                };
                for pr in probes.iter_mut().filter(|pr| !pr.taken) {
                    if let Some(want) = rec.source_size {
                        if pr.size.is_none() {
                            pr.size = Some(self.vfs.size(&pr.path)?);
                        }
                        if pr.size != Some(Some(want)) {
                            continue;
                        }
                    }
                    if pr.hash.is_none() {
                        let hex = match self.vfs.read(&pr.path) {
                            Ok(b) => Some(Blake3::of(&b).to_hex()),
                            // Gone again since the stamps: it cannot be the renamed source.
                            Err(AssetError::Store(forge_store::StoreError::NotFound(_))) => None,
                            Err(e) => return Err(e),
                        };
                        pr.hash = Some(hex);
                    }
                    if pr
                        .hash
                        .as_ref()
                        .is_some_and(|h| h.as_ref() == Some(&rec.source))
                    {
                        pr.taken = true;
                        matched = Some(pr.path.clone());
                        break;
                    }
                }
            }
            let old_sc = Sidecar::path_for(&from)?;
            match matched {
                Some(to) => {
                    let new_sc = Sidecar::path_for(&to)?;
                    if self.vfs.exists(&old_sc)? {
                        self.move_file(&old_sc, &new_sc)?;
                    } else {
                        self.vfs.write(&new_sc, sc.encode()?)?;
                    }
                    events.push(AssetEvent::Renamed {
                        id: sc.id,
                        from,
                        to: to.clone(),
                    });
                    claimed.insert(sc.id);
                    self.register(&to, sc);
                }
                None if self.vfs.exists(&old_sc)? => {
                    // The source is gone, its sidecar is not: keep it for a later return.
                    events.push(AssetEvent::Failed {
                        path: from.clone(),
                        error: AssetError::UnknownAsset(format!(
                            "{from} (the source file is missing)"
                        )),
                    });
                    self.orphans.insert(from, sc);
                }
                None => {
                    for id in self.family_blobs(Some(&sc)).keys() {
                        self.fail_slot(*id, AssetError::UnknownAsset(id.to_string()));
                    }
                    events.push(AssetEvent::Removed {
                        id: sc.id,
                        path: from,
                    });
                }
            }
        }
        // 4. Sidecars deleted while their source stayed: the asset is removed.
        for scp in removed.iter().filter(|p| is_sc(p)) {
            if let Some(src) = Sidecar::source_of(scp)
                && self.vfs.exists(&src)?
                && self.sources.contains_key(&src)
                && let Some(sc) = self.unregister(&src)
            {
                for id in self.family_blobs(Some(&sc)).keys() {
                    self.fail_slot(*id, AssetError::UnknownAsset(id.to_string()));
                }
                events.push(AssetEvent::Removed {
                    id: sc.id,
                    path: src,
                });
            }
        }
        // 5. Returning sources of orphaned sidecars.
        let back: Vec<StorePath> = added
            .iter()
            .filter(|p| self.orphans.contains_key(*p))
            .cloned()
            .collect();
        for p in back {
            if let Some(sc) = self.orphans.remove(&p) {
                self.register(&p, sc.clone());
                if let Err(error) = self.refresh(&p, sc, &mut events) {
                    events.push(AssetEvent::Failed { path: p, error });
                }
            }
        }
        // 6. Changed sources, edited sidecars and changed dependencies: reimport (content
        //    checked, so a touch or our own write is a no-op).
        let mut todo: BTreeSet<StorePath> = BTreeSet::new();
        for p in &changed {
            if is_sc(p) {
                if let Some(src) = Sidecar::source_of(p)
                    && let Some(cur) = self.sources.get(&src)
                    && let Ok(sc) = self.vfs.read(p).and_then(|b| Sidecar::decode(p, &b))
                    && sc != *cur
                {
                    // Edited outside the editor: take its settings (and importer choice).
                    let merged = Sidecar {
                        id: cur.id,
                        importer: sc.importer,
                        settings: sc.settings,
                        import: cur.import.clone(),
                    };
                    self.sources.insert(src.clone(), merged);
                    todo.insert(src);
                }
                continue;
            }
            if self.sources.contains_key(p) {
                todo.insert(p.clone());
            }
            if let Some(deps) = self.dependents.get(p) {
                todo.extend(deps.iter().cloned());
            }
            if added.contains(p)
                && !self.sources.contains_key(p)
                && self.importer_for(p).is_some()
                && !claimed.iter().any(|id| self.path_of(*id) == Some(p))
            {
                events.push(AssetEvent::Discovered { path: p.clone() });
            }
        }
        for p in todo {
            let Some(sc) = self.sources.get(&p).cloned() else {
                continue;
            };
            // The record carries the hash of the settings it was made with, so an edited
            // setting is a stale record and the importer runs.
            if let Err(error) = self.refresh(&p, sc, &mut events) {
                events.push(AssetEvent::Failed { path: p, error });
            }
        }
        // Our own writes (sidecars, moved files) changed stamps: take theirs as the baseline.
        // Only theirs: a second whole-project walk would cost as much as the poll itself and
        // would swallow an edit the user made to some other file while this poll ran.
        let written = self.vfs.take_written();
        if !written.is_empty() {
            let fresh: BTreeMap<StorePath, Stamp> =
                self.vfs.stamps_of(&written)?.into_iter().collect();
            for p in written {
                match fresh.get(&p) {
                    Some(s) => {
                        self.stamps.insert(p, *s);
                    }
                    None => {
                        self.stamps.remove(&p);
                    }
                }
            }
        }
        Ok(events)
    }
}

/// Fetches artefact bytes for workers.
#[derive(Clone)]
struct Fetcher {
    vfs: Arc<Vfs>,
    generators: Arc<RwLock<BTreeMap<String, Generator>>>,
}

impl Fetcher {
    fn bytes(&self, info: &AssetInfo) -> Result<Bytes, AssetError> {
        match &info.source {
            AssetSource::File(_) => {
                let b = info
                    .blob
                    .ok_or_else(|| AssetError::UnknownAsset(info.id.to_string()))?;
                self.vfs.blob_get(b)
            }
            AssetSource::Generated { generator, seed } => {
                let g = self
                    .generators
                    .read()
                    .ok()
                    .and_then(|g| g.get(generator).cloned())
                    .ok_or_else(|| AssetError::Generator {
                        name: generator.clone(),
                        why: "no such generator is registered".into(),
                    })?;
                match catch_unwind(AssertUnwindSafe(|| (g.f)(seed))) {
                    Ok(r) => r.map_err(|why| AssetError::Generator {
                        name: generator.clone(),
                        why,
                    }),
                    Err(p) => Err(AssetError::Generator {
                        name: generator.clone(),
                        why: format!("panicked: {}", panic_text(p.as_ref())),
                    }),
                }
            }
        }
    }
}

impl ExportSource for AssetServer {
    fn artefact(&self, id: AssetId) -> Result<ExportArtefact, AssetError> {
        let info = self
            .entries
            .get(&id)
            .ok_or_else(|| AssetError::UnknownAsset(id.to_string()))?;
        Ok(ExportArtefact {
            kind: info.kind.clone(),
            name: info.name.clone(),
            bytes: self.fetcher().bytes(info)?,
        })
    }
}
