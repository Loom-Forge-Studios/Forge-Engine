//! Workspace presets as data (Ch.31 §31.3): `presets/<preset>/workspace.ron` names the
//! default plugin set, the default panel layout, default project settings, the new-scene
//! template, which extension-point defaults are installed, and an optional keymap layer.
//! Nothing else — **a preset chooses defaults and never gates a panel** (I15): there is no
//! field that could hide one. Users author their own presets by copying one.
//!
//! The built-in presets (2D and 3D) are the `forge.presets` plugin (`plugins/forge-presets`,
//! the repository's `presets/` compiled in), loaded through the ordinary loader onto the
//! `Preset` point ([`builtin_presets`], built once per process); any preset plugin's items
//! build the same way ([`presets_from_registry`]). The running editor lists and creates
//! projects from its **real** `Preset` registry — the one `shell::assemble` loads every plugin
//! into, WASM ones included — through a [`PresetCatalog`] the core and the new-project flow
//! share (WP-21), so a third-party preset plugin appears next to the built-in ones.
//!
//! **A preset's directory.** A project names its preset by directory (`project.preset`:
//! `2d`, `3d`). A plugin may add a preset with a directory of its own by registering it
//! under the key `forge.preset.<dir>` with its files ([`preset_dir_of_key`]): a project made
//! from it names that directory, promotion reaches it through its family's base preset
//! (`crate::project::promote`), and a project may carry its own copy of it like any other.
//! [`Preset::load_dir`] reads a user's copy.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use forge_plugin::points::PresetKind;
use forge_ui::dock::{Layout, PanelId};

use crate::EditorError;
use crate::keymap::KeymapFile;

/// The workspace manifest format version.
pub const WORKSPACE_VERSION: u32 = 1;

/// Which preset family (serialisable mirror of `forge_plugin::points::PresetKind`): what a
/// world of the preset is, 2D or 3D. A plugin's preset belongs to one of them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Family {
    TwoD,
    ThreeD,
}

impl Family {
    pub fn kind(self) -> PresetKind {
        match self {
            Family::TwoD => PresetKind::TwoD,
            Family::ThreeD => PresetKind::ThreeD,
        }
    }
}

/// The preset directory a `Preset` registry key names (`forge.preset.3d` → `3d`), if it has
/// that form.
#[must_use]
pub fn preset_dir_of_key(key: &str) -> Option<&str> {
    key.strip_prefix("forge.preset.")
        .filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

/// `workspace.ron`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    /// `forge.preset.3d`.
    pub id: String,
    /// Shown in the new-project dialog.
    pub label: String,
    pub family: Family,
    /// The default plugin set (plugin ids).
    pub plugins: Vec<String>,
    /// The default layout file, next to this manifest.
    pub layout: String,
    /// The panels the default layout opens (checked against the layout file).
    pub open_panels: Vec<String>,
    /// Default project settings (`key` -> RON value text).
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// The new-scene template.
    #[serde(default)]
    pub scene_template: Option<String>,
    /// Extension-point defaults (`point id` -> item key).
    #[serde(default)]
    pub extension_defaults: BTreeMap<String, String>,
    /// An optional keymap layer file, next to this manifest.
    #[serde(default)]
    pub keymap: Option<String>,
}

/// A loaded preset: its manifest, default layout, keymap layer and new-scene template.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub workspace: Workspace,
    pub layout: Layout,
    pub keymap: Option<KeymapFile>,
    /// The new-scene template's entities (`scene_template`, the `project/scene.ron`
    /// format; empty when the preset names none).
    pub scene: Vec<forge_project::format::EntityDoc>,
}

fn pe(msg: impl Into<String>) -> EditorError {
    EditorError::Preset(msg.into())
}

impl Preset {
    /// Build a preset from its files' text (`read(name)` returns a file next to the
    /// manifest). Checks the manifest, the layout, the keymap layer, and that
    /// `open_panels` is exactly what the layout opens.
    pub fn from_files(
        workspace: &str,
        read: &dyn Fn(&str) -> Result<String, EditorError>,
    ) -> Result<Self, EditorError> {
        let w: Workspace =
            ron::from_str(workspace).map_err(|e| pe(format!("workspace.ron: {e}")))?;
        if w.version == 0 || w.version > WORKSPACE_VERSION {
            return Err(pe(format!(
                "{}: workspace version {} is not readable (1..={WORKSPACE_VERSION})",
                w.id, w.version
            )));
        }
        let layout = Layout::from_ron(&read(&w.layout)?)
            .map_err(|e| pe(format!("{}: {}: {e}", w.id, w.layout)))?;
        let mut opens: Vec<String> = layout.panels().iter().map(PanelId::to_string).collect();
        let mut listed = w.open_panels.clone();
        opens.sort();
        listed.sort();
        if opens != listed {
            return Err(pe(format!(
                "{}: open_panels {listed:?} disagrees with {} which opens {opens:?}",
                w.id, w.layout
            )));
        }
        let keymap = match &w.keymap {
            Some(k) => Some(KeymapFile::from_ron(&read(k)?)?),
            None => None,
        };
        let scene = match &w.scene_template {
            Some(t) => forge_project::format::ProjectDoc::decode_scene(
                &format!("{}: {t}", w.id),
                &read(t)?,
            )
            .map_err(|e| pe(e.to_string()))?,
            None => Vec::new(),
        };
        Ok(Self {
            workspace: w,
            layout,
            keymap,
            scene,
        })
    }

    /// Load a preset directory (a user's copy of one).
    pub fn load_dir(dir: &Path) -> Result<Self, EditorError> {
        let read = |name: &str| -> Result<String, EditorError> {
            let p = dir.join(name);
            std::fs::read_to_string(&p).map_err(|e| pe(format!("{}: {e}", p.display())))
        };
        Self::from_files(&read("workspace.ron")?, &read)
    }
}

/// The built-in preset directories, in the order the new-project dialog lists them.
pub const BUILTIN_PRESETS: &[&str] = forge_presets::DIRS;

/// A preset from a `Preset` registry item. A descriptor that carries its files is read by
/// [`Preset::from_files`] (so a third-party preset plugin's workspace is checked exactly like
/// a built-in one). A **defaults-only** descriptor (no `workspace.ron`: what a small preset
/// plugin, a WASM one say, usually is) is the built-in preset of its family with its label,
/// its id and its default settings on top — so a preset plugin never has to copy a layout to
/// say "3D, with these defaults".
pub fn preset_from_descriptor(
    key: &str,
    d: &forge_plugin::points::PresetDescriptor,
) -> Result<Preset, EditorError> {
    if !d.files.contains_key("workspace.ron") {
        let family = match d.kind {
            PresetKind::TwoD => "2d",
            PresetKind::ThreeD => "3d",
        };
        let mut p = builtin_preset(family)?;
        p.workspace.id = key.to_string();
        p.workspace.label.clone_from(&d.label);
        for (k, v) in &d.defaults {
            p.workspace.settings.insert(k.clone(), v.clone());
        }
        return Ok(p);
    }
    let read = |name: &str| -> Result<String, EditorError> {
        d.files
            .get(name)
            .cloned()
            .ok_or_else(|| pe(format!("preset {key} has no {name}")))
    };
    let mut p = Preset::from_files(&read("workspace.ron")?, &read)?;
    // The descriptor's defaults win over its manifest's (a chained preset adjusts them).
    for (k, v) in &d.defaults {
        p.workspace.settings.insert(k.clone(), v.clone());
    }
    Ok(p)
}

/// Every preset in a `Preset` registry, in registry order (a defaults-only descriptor is
/// built on its family's built-in preset: [`preset_from_descriptor`]).
pub fn presets_from_registry(
    reg: &forge_plugin::Registry<forge_plugin::points::Preset>,
) -> Result<Vec<Preset>, EditorError> {
    reg.iter()
        .map(|(k, d)| preset_from_descriptor(k, d))
        .collect()
}

/// The `Preset` registry holding the built-in presets, loaded from the `forge.presets`
/// plugin through the ordinary loader (I16): the editor has no private copy of them.
pub fn builtin_registry() -> Result<forge_plugin::Extensions, EditorError> {
    let mut x = forge_plugin::Extensions::new();
    x.define::<forge_plugin::points::Preset>()?;
    let plugin = forge_presets::BuiltinPresets::new()?;
    forge_plugin::loader::load(&mut x, &[&plugin], &[], &forge_plugin::Grants::new())?;
    Ok(x)
}

/// The built-in presets, built once per process (the `forge.presets` plugin's files are
/// compiled in, so they never change while it runs): a new project, the promotion rules and
/// every shell ask for them without re-parsing three workspaces each time.
fn builtin_cache() -> Result<&'static [Preset], EditorError> {
    static BUILTIN: std::sync::OnceLock<Result<Vec<Preset>, String>> = std::sync::OnceLock::new();
    let r = BUILTIN.get_or_init(|| {
        let load = || -> Result<Vec<Preset>, EditorError> {
            let x = builtin_registry()?;
            let reg = x
                .registry::<forge_plugin::points::Preset>()
                .ok_or_else(|| pe("the Preset point is not defined".to_string()))?;
            BUILTIN_PRESETS
                .iter()
                .map(|dir| {
                    let key = forge_presets::key_of(dir);
                    let d = reg
                        .get(&key)
                        .ok_or_else(|| pe(format!("presets/{dir} is not built in")))?;
                    // Built-in presets carry their files: never the defaults-only path
                    // (which is built on this cache).
                    let read = |name: &str| -> Result<String, EditorError> {
                        d.files
                            .get(name)
                            .cloned()
                            .ok_or_else(|| pe(format!("preset {key} has no {name}")))
                    };
                    Preset::from_files(&read("workspace.ron")?, &read)
                })
                .collect()
        };
        load().map_err(|e| e.to_string())
    });
    match r {
        Ok(v) => Ok(v.as_slice()),
        Err(e) => Err(pe(e.clone())),
    }
}

/// A built-in preset by directory name (`2d`, `3d`); built once per process.
pub fn builtin_preset(dir: &str) -> Result<Preset, EditorError> {
    let i = BUILTIN_PRESETS
        .iter()
        .position(|d| *d == dir)
        .ok_or_else(|| pe(format!("presets/{dir} is not built in")))?;
    builtin_cache()?
        .get(i)
        .cloned()
        .ok_or_else(|| pe(format!("presets/{dir} is not built in")))
}

/// The preset in `dir`: a built-in one, else the one a plugin of `plugins` provides under
/// `forge.preset.<dir>` (only the plugins whose manifest provides that key are loaded to read
/// it). What the editor starts with (`--preset`) before the rest of its plugins load.
pub fn preset_in(
    dir: &str,
    plugins: &[&dyn forge_plugin::SourcePlugin],
) -> Result<Preset, EditorError> {
    if let Ok(p) = builtin_preset(dir) {
        return Ok(p);
    }
    let key = forge_presets::key_of(dir);
    let owners: Vec<&dyn forge_plugin::SourcePlugin> = plugins
        .iter()
        .copied()
        .filter(|p| {
            p.manifest().provides.iter().any(|i| {
                i.point == <forge_plugin::points::Preset as forge_plugin::ExtensionPoint>::NAME
                    && i.key == key
            })
        })
        .collect();
    if owners.is_empty() {
        let mut known: Vec<String> = BUILTIN_PRESETS.iter().map(|d| (*d).to_string()).collect();
        for p in plugins {
            for i in &p.manifest().provides {
                if i.point == <forge_plugin::points::Preset as forge_plugin::ExtensionPoint>::NAME
                    && let Some(d) = preset_dir_of_key(&i.key)
                {
                    known.push(d.to_string());
                }
            }
        }
        return Err(pe(format!(
            "no preset {dir:?}: this editor has {}",
            known.join(", ")
        )));
    }
    let mut x = crate::editor_extensions()?;
    forge_plugin::loader::load(&mut x, &owners, &[], &forge_plugin::Grants::new())?;
    let reg = x
        .registry::<forge_plugin::points::Preset>()
        .ok_or_else(|| pe("the Preset point is not defined".to_string()))?;
    let d = reg
        .get(&key)
        .ok_or_else(|| pe(format!("no plugin provides the preset {key}")))?;
    preset_from_descriptor(&key, d)
}

/// Every built-in preset (built once per process).
pub fn builtin_presets() -> Result<Vec<Preset>, EditorError> {
    Ok(builtin_cache()?.to_vec())
}

// ---- the presets the running editor offers (WP-21) ---------------------------------------

/// One preset the new-project flow offers: an item of the editor's real `Preset` registry.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogEntry {
    /// Its registry key (`forge.preset.3d`, `com.example.rivers`).
    pub key: String,
    /// The plugin that provides it (after replaces: the replacer).
    pub owner: String,
    /// The built preset.
    pub preset: Preset,
}

impl CatalogEntry {
    /// Whether it is one of the built-in keys (whoever provides it now).
    #[must_use]
    pub fn is_builtin_key(&self) -> bool {
        BUILTIN_PRESETS
            .iter()
            .any(|d| forge_presets::key_of(d) == self.key)
    }

    /// Its family's base preset directory (`2d`, `3d`).
    #[must_use]
    pub fn family_dir(&self) -> &'static str {
        family_base_dir(self.preset.workspace.family)
    }

    /// The directory a project made from it names (`project.preset`): the one its manifest's
    /// id or its key names (`forge.preset.<dir>`, [`preset_dir_of_key`]; a plugin's template
    /// is a preset item under a key of its own whose manifest is its preset's), else its
    /// family's base preset's.
    #[must_use]
    pub fn dir(&self) -> String {
        preset_dir_of_key(&self.preset.workspace.id)
            .or_else(|| preset_dir_of_key(&self.key))
            .map_or_else(|| self.family_dir().to_string(), str::to_string)
    }
}

/// The presets of the editor's `Preset` registry (the built-in `forge.presets` plugin and
/// every other preset plugin, source or WASM, after replaces, removes and chains): what the
/// new-project flow lists and what a new project is created from. A registry item that does
/// not build is listed in [`PresetCatalog::problems`] and left out — it never takes a preset
/// away (I15).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresetCatalog {
    entries: Vec<CatalogEntry>,
    /// Why an item was left out, one line each.
    pub problems: Vec<String>,
    revision: u64,
}

/// The catalog as the core and the shell share it (the core creates projects from it on
/// its own thread; a hot-installed preset plugin updates it in place).
pub type SharedPresets = std::sync::Arc<std::sync::RwLock<PresetCatalog>>;

impl PresetCatalog {
    /// The catalog of `reg`, in registry order.
    #[must_use]
    pub fn from_registry(reg: &forge_plugin::Registry<forge_plugin::points::Preset>) -> Self {
        let mut c = Self::default();
        c.refresh(reg);
        c
    }

    /// The built-in presets only (no registry at hand: a bare core, tests).
    #[must_use]
    pub fn builtin() -> Self {
        let mut c = Self::default();
        match builtin_cache() {
            Ok(all) => {
                c.entries = all
                    .iter()
                    .map(|p| CatalogEntry {
                        key: p.workspace.id.clone(),
                        owner: "forge.presets".into(),
                        preset: p.clone(),
                    })
                    .collect();
            }
            Err(e) => c.problems.push(e.to_string()),
        }
        c
    }

    /// Rebuild from `reg` (a preset plugin was installed); bumps the revision.
    pub fn refresh(&mut self, reg: &forge_plugin::Registry<forge_plugin::points::Preset>) {
        self.entries.clear();
        self.problems.clear();
        for (k, d) in reg.iter() {
            let owner = reg.provenance(k).map_or_else(String::new, |p| {
                p.replaced_by.as_ref().unwrap_or(&p.owner).to_string()
            });
            match preset_from_descriptor(k, d) {
                Ok(preset) => self.entries.push(CatalogEntry {
                    key: k.to_string(),
                    owner,
                    preset,
                }),
                Err(e) => self.problems.push(format!("{k} (from {owner}): {e}")),
            }
        }
        self.revision += 1;
    }

    /// Changes whenever the catalog is rebuilt.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Every preset, in registry order.
    #[must_use]
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    /// The preset registered as `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// The presets beyond the built-in keys (a preset plugin's): the new-project flow lists
    /// them after the built-in templates.
    pub fn added(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.iter().filter(|e| !e.is_builtin_key())
    }

    /// What `dir` (`2d`, `3d`, or a plugin preset's own directory) means in this editor: the
    /// registry's item for its key (a plugin may have replaced or chained it), else the
    /// built-in one.
    pub fn family(&self, dir: &str) -> Result<Preset, EditorError> {
        let key = forge_presets::key_of(dir);
        match self.get(&key) {
            Some(e) => Ok(e.preset.clone()),
            None => builtin_preset(dir),
        }
    }
}

// ---- a project's own presets (WP-16, C-project-presets-from-folder) ----------------------

/// Where a preset in a [`PresetSet`] comes from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PresetSource {
    /// The built-in preset (`plugins/forge-presets`, the repository's `presets/`).
    Builtin,
    /// The project's own copy (`presets/<dir>/` in the project's store).
    Project,
}

/// The presets a project works with: every preset with a directory of its own in the editor's
/// catalog (the built-in `2d` and `3d`, and a plugin's `forge.preset.<dir>`), each replaced
/// by the project's own copy when it carries one (`presets/<dir>/`, Ch.31 §31.3 "users author
/// their own presets by copying one"). A project copy that does not load, or names another
/// family than its directory, is listed in [`PresetSet::problems`] and the editor's one stays
/// — a broken copy can never take anything away (I15).
#[derive(Clone, Debug)]
pub struct PresetSet {
    presets: BTreeMap<String, (Preset, PresetSource)>,
    /// Why a project's own preset was not used, one line each.
    pub problems: Vec<String>,
}

impl PresetSet {
    /// The built-in presets (loaded once per process through the `forge.presets` plugin).
    pub fn builtin() -> Result<Self, EditorError> {
        let all = builtin_presets()?;
        Ok(Self {
            presets: all
                .into_iter()
                .map(|p| {
                    let dir = family_base_dir(p.workspace.family).to_string();
                    (dir, (p, PresetSource::Builtin))
                })
                .collect(),
            problems: Vec::new(),
        })
    }

    /// The presets of `catalog` that have a directory of their own (the built-in ones where
    /// the catalog lacks them).
    pub fn of_catalog(catalog: &PresetCatalog) -> Result<Self, EditorError> {
        let mut set = Self::builtin()?;
        for e in catalog.entries() {
            if let Some(dir) = preset_dir_of_key(&e.key) {
                set.presets
                    .insert(dir.to_string(), (e.preset.clone(), PresetSource::Builtin));
            }
        }
        Ok(set)
    }

    /// The built-in presets with the project's own copies (`files`: its `presets/` folder,
    /// by directory) in their place.
    pub fn with_project(
        files: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> Result<Self, EditorError> {
        Self::with_project_in(&PresetCatalog::builtin(), files)
    }

    /// The presets of `catalog` ([`PresetSet::of_catalog`]) with the project's own copies in
    /// their place.
    pub fn with_project_in(
        catalog: &PresetCatalog,
        files: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> Result<Self, EditorError> {
        let mut set = Self::of_catalog(catalog)?;
        for (dir, fs) in files {
            let Some(family) = set.get(dir).map(|p| p.workspace.family) else {
                let known: Vec<&str> = set.presets.keys().map(String::as_str).collect();
                set.problems.push(format!(
                    "presets/{dir}: a project preset replaces one of this editor's presets \
                     (presets/{}): a preset sets defaults for one of them",
                    known.join(", presets/")
                ));
                continue;
            };
            let read = |name: &str| -> Result<String, EditorError> {
                fs.get(name)
                    .cloned()
                    .ok_or_else(|| pe(format!("presets/{dir} has no {name}")))
            };
            let loaded = read("workspace.ron").and_then(|w| Preset::from_files(&w, &read));
            match loaded {
                Ok(p) if p.workspace.family == family => {
                    set.presets.insert(dir.clone(), (p, PresetSource::Project));
                }
                Ok(p) => set.problems.push(format!(
                    "presets/{dir}: its family is {:?}, not {family:?}",
                    p.workspace.family
                )),
                Err(e) => set.problems.push(format!("presets/{dir}: {e}")),
            }
        }
        Ok(set)
    }

    /// The preset for a directory (`2d`, `3d`, or a plugin preset's).
    #[must_use]
    pub fn get(&self, dir: &str) -> Option<&Preset> {
        self.presets.get(dir).map(|(p, _)| p)
    }

    /// Where the preset for `dir` comes from.
    #[must_use]
    pub fn source(&self, dir: &str) -> Option<PresetSource> {
        self.presets.get(dir).map(|(_, s)| *s)
    }

    /// Every preset, by directory.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Preset, PresetSource)> {
        self.presets.iter().map(|(d, (p, s))| (d.as_str(), p, *s))
    }

    /// Every preset's default project settings, by directory (the promotion rules'
    /// source).
    #[must_use]
    pub fn defaults(&self) -> BTreeMap<String, BTreeMap<String, forge_cmd::Value>> {
        self.presets
            .iter()
            .map(|(d, (p, _))| (d.clone(), crate::project::defaults_of(p)))
            .collect()
    }
}

/// The directory of a family's base (built-in) preset.
#[must_use]
pub fn family_base_dir(f: Family) -> &'static str {
    match f {
        Family::TwoD => "2d",
        Family::ThreeD => "3d",
    }
}
