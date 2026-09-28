//! The plugin manager's model (Ch.21 §21.21 "Plugin manager", Ch.32, Ch.38; DoD M2-52).
//!
//! * **Installed plugins** come from the ordinary loader: the manifests `assemble` loaded
//!   (source plugins, the first-party ones included — I16: they are listed like any other),
//!   the ones it left out as disabled, and WASM plugins hosted by `forge-wasm` on the editor's
//!   one grant table ([`PluginManager::host_wasm`] loads them through
//!   `forge_plugin::loader::load_hosted`, so the same manifest and conflict checks run).
//! * **Capabilities requested vs granted.** Requested is the manifest's list; granted is what
//!   the project grants (`security.grants.plugin.*`, written only by the human-only
//!   `forge.plugin.grant` / `revoke` commands) — the core keeps the shared table equal to it,
//!   so a hosted plugin's next call sees a grant at once.
//! * **`replaces` with both names**: every item a plugin replaces is shown with the plugin
//!   that provides it, and every conflict (two replacers, a replacer and a remover, two
//!   providers) names both plugins (`forge_plugin::loader::conflicts`), before anything is
//!   added.
//! * **The plugin set is project state** (`plugins.<id>.*`): adding (the `forge add`
//!   equivalent), removing, enabling and disabling are commands. Only the downloaded bytes
//!   are a direct write, to the per-machine [`PluginCache`] (user config, §21.18).
//! * **The index** is reached through the [`PluginIndex`] trait. The signed index is M5-14;
//!   until then [`MemoryIndex`] is a labelled in-memory index (D-4) of sample plugins with
//!   real WASM code.
//!
//! Enabling or disabling a plugin changes what the project loads. A WASM plugin added (and
//! cached) while the editor runs installs on its next loop turn ([`crate::hosting`], WP-21);
//! disabling or removing one, and any source plugin change, takes effect at the next start (the
//! row says so). A hosted plugin's grants change at once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forge_plugin::{
    Capability, HostedPlugin, ItemRef, Manifest, PluginError, PluginKind, Principal,
};

use crate::EditorError;
use crate::mirror::ProjectMirror;
use crate::security::{self, PluginSetEntry};

/// How an installed plugin is loaded in this editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallState {
    /// Compiled in and loaded (a source plugin).
    Loaded,
    /// Hosted in the WASM sandbox and installed through the loader.
    Hosted {
        /// How many times its code was loaded (hot reload bumps it).
        generation: u64,
    },
    /// Present but left out of this load (disabled when the editor started).
    Disabled,
    /// Not loaded, with why (a WASM plugin with no host, a failed sandbox check).
    NotLoaded(String),
}

impl InstallState {
    fn label(&self) -> String {
        match self {
            Self::Loaded => forge_ui::tr!("loaded").into(),
            Self::Hosted { generation } => {
                forge_ui::trf!(
                    "hosted in the WASM sandbox (generation {generation})",
                    generation
                )
            }
            Self::Disabled => forge_ui::tr!("disabled (left out of this load)").into(),
            Self::NotLoaded(why) => forge_ui::trf!("not loaded: {why}", why),
        }
    }
}

/// One installed plugin.
#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub state: InstallState,
}

/// One row of the plugin manager.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginRow {
    pub id: String,
    pub version: String,
    /// `source` or `wasm`.
    pub kind: &'static str,
    /// How it is loaded here (`None`: in the project's plugin set but not installed on this
    /// machine).
    pub state: Option<InstallState>,
    /// Its entry in the project's plugin set (`None`: not in the project — a compiled-in
    /// plugin the editor loads for every project).
    pub in_project: Option<PluginSetEntry>,
    /// Whether the project loads it (not in the set: loaded; in it: its `enabled`).
    pub enabled: bool,
    pub requested: Vec<Capability>,
    /// Of `requested`, what the project grants.
    pub granted: Vec<Capability>,
    /// Granted but never requested (a stale grant a human may revoke).
    pub extra_granted: Vec<Capability>,
    /// Every item it replaces, with the plugin that provides it: `EditorPanel("x") from
    /// forge.panels.core`.
    pub replaces: Vec<String>,
    /// Conflicts that name it (each naming both plugins).
    pub conflicts: Vec<String>,
    /// Its extension-point items, as `Point("key")`.
    pub provides: Vec<String>,
}

impl PluginRow {
    /// The row's one-line label.
    #[must_use]
    pub fn label(&self) -> String {
        let granted = if self.requested.is_empty() {
            forge_ui::tr!("requests nothing").to_string()
        } else {
            forge_ui::trf!(
                "{granted}/{requested} granted",
                granted = self.granted.len(),
                requested = self.requested.len()
            )
        };
        let state = match &self.state {
            Some(s) => s.label(),
            None => forge_ui::tr!("in the project, not installed on this machine").into(),
        };
        let enabled = match (&self.in_project, self.enabled) {
            (None, _) => "",
            (Some(_), true) => forge_ui::tr!(" \u{b7} enabled"),
            (Some(_), false) => forge_ui::tr!(" \u{b7} disabled in the project"),
        };
        let kind = match self.kind {
            "source" => forge_ui::tr!("source"),
            "wasm" => forge_ui::tr!("WASM"),
            other => other,
        };
        format!(
            "{} {} ({kind}) \u{b7} {state}{enabled} \u{b7} {granted}",
            self.id, self.version,
        )
    }

    /// Whether what the project says and what this editor loaded disagree (the change takes
    /// effect at the next start).
    #[must_use]
    pub fn pending_restart(&self) -> bool {
        matches!(
            (&self.state, self.enabled),
            (Some(InstallState::Disabled), true)
                | (
                    Some(InstallState::Loaded | InstallState::Hosted { .. }),
                    false
                )
        )
    }
}

/// What the index lists about a plugin.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexEntry {
    /// Its manifest, as published (the conflict check runs on it before anything is added).
    pub manifest: Manifest,
    /// One line about it.
    pub summary: String,
    pub author: String,
}

impl IndexEntry {
    /// `id version — summary`.
    #[must_use]
    pub fn label(&self) -> String {
        let caps: Vec<String> = self
            .manifest
            .capabilities
            .iter()
            .map(ToString::to_string)
            .collect();
        forge_ui::trf!(
            "{id} {version} \u{2014} {summary} (by {author}; requests: {caps})",
            id = self.manifest.id,
            version = self.manifest.version,
            summary = self.summary,
            author = self.author,
            caps = if caps.is_empty() {
                forge_ui::tr!("nothing").into()
            } else {
                caps.join(", ")
            }
        )
    }
}

/// A plugin's files as fetched: the manifest text and the code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub manifest: String,
    pub code: Vec<u8>,
    /// `plugin.wasm` or `plugin.wat`.
    pub code_file: String,
}

/// The plugin index (Ch.38 §38.3; the signed index is M5-14).
pub trait PluginIndex {
    /// What it is, for the panel's footer (a labelled in-memory index says so).
    fn backend(&self) -> String;
    /// Its name, recorded as a plugin's `source` in the project.
    fn name(&self) -> String;
    /// Plugins whose id or summary contains `query` (case-insensitive; empty: all).
    fn search(&self, query: &str) -> Vec<IndexEntry>;
    /// Download a plugin's files.
    fn fetch(&self, id: &str, version: &str) -> Result<Fetched, EditorError>;
}

/// The labelled in-memory index (D-4) until the signed index (M5-14).
#[derive(Clone, Debug, Default)]
pub struct MemoryIndex {
    entries: Vec<(IndexEntry, Fetched)>,
}

impl MemoryIndex {
    /// An empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a plugin (tests; the sample set).
    pub fn publish(
        &mut self,
        manifest_text: &str,
        code: Vec<u8>,
        summary: &str,
        author: &str,
    ) -> Result<(), EditorError> {
        let manifest = Manifest::parse(manifest_text)?;
        let code_file = if code.starts_with(b"\0asm") {
            "plugin.wasm"
        } else {
            "plugin.wat"
        };
        self.entries.push((
            IndexEntry {
                manifest,
                summary: summary.to_string(),
                author: author.to_string(),
            },
            Fetched {
                manifest: manifest_text.to_string(),
                code,
                code_file: code_file.to_string(),
            },
        ));
        Ok(())
    }

    /// The sample index: two small WASM plugins with real code (a command that plans a
    /// setting, and a reader that needs `Fs(ProjectRead)`).
    #[must_use]
    pub fn sample() -> Self {
        let mut ix = Self::new();
        let flag = r#"Plugin(id: "com.example.flag", version: "0.1.0", engine: "^0.1", kind: Wasm,
            provides: [Command("example.flag")], capabilities: [Command(Ordinary)])"#;
        let plan = forge_wasm::wat::constant(
            br#"{"ops":[{"op":"set_setting","key":"example.flag","value":{"Bool":true}}]}"#,
        );
        let _ = ix.publish(
            flag,
            plan.into_bytes(),
            "a command that raises a project flag",
            "Example Co",
        );
        let reader = r#"Plugin(id: "com.example.notes", version: "0.2.0", engine: "^0.1", kind: Wasm,
            provides: [Command("example.notes")], capabilities: [Fs(ProjectRead), Command(Ordinary)])"#;
        let code = forge_wasm::wat::component(
            &["project-read"],
            r#"(func (export "call") (param i32 i32 i32 i32 i32 i32) (result i32)
                 (call $read (local.get 4) (local.get 5) (i32.const 64))
                 (i32.const 64))"#,
        );
        let _ = ix.publish(
            reader,
            code.into_bytes(),
            "reads a project file (needs Fs(ProjectRead))",
            "Example Co",
        );
        ix
    }
}

impl PluginIndex for MemoryIndex {
    fn backend(&self) -> String {
        "in-memory sample index (D-4: the signed plugin index is M5-14, UNBUILT)".into()
    }
    fn name(&self) -> String {
        "memory-index".into()
    }
    fn search(&self, query: &str) -> Vec<IndexEntry> {
        let q = query.trim().to_lowercase();
        self.entries
            .iter()
            .map(|(e, _)| e)
            .filter(|e| {
                q.is_empty()
                    || e.manifest.id.as_str().contains(&q)
                    || e.summary.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }
    fn fetch(&self, id: &str, version: &str) -> Result<Fetched, EditorError> {
        self.entries
            .iter()
            .find(|(e, _)| {
                e.manifest.id.as_str() == id && e.manifest.version.to_string() == version
            })
            .map(|(_, f)| f.clone())
            .ok_or_else(|| EditorError::PluginIndex(format!("the index has no {id} {version}")))
    }
}

/// The downloaded-plugin cache (§21.18: **user config**, the allow-listed direct write):
/// `<config>/plugins/<id>/<version>/plugin.ron` and the code. Fetching bytes changes nothing
/// a project loads until a `forge.plugin.add` command does; the cache is shared by every
/// project on the machine. Without a config directory (tests) it keeps the files in memory.
#[derive(Debug, Default)]
pub struct PluginCache {
    dir: Option<PathBuf>,
    memory: BTreeMap<(String, String), Fetched>,
}

impl PluginCache {
    /// A cache under `config_dir/plugins` (`None`: in memory only).
    #[must_use]
    pub fn new(config_dir: Option<&Path>) -> Self {
        Self {
            dir: config_dir.map(|d| d.join("plugins")),
            memory: BTreeMap::new(),
        }
    }

    /// Where it keeps plugins (`None`: in memory).
    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Keep `f` for `id` `version`; returns where (or `memory:`).
    pub fn store(&mut self, id: &str, version: &str, f: &Fetched) -> Result<String, EditorError> {
        // Ids and versions are validated (PluginId, semver) before they get here: no path
        // separators or `..` can reach the join.
        forge_plugin::PluginId::new(id)?;
        semver::Version::parse(version)
            .map_err(|e| EditorError::PluginIndex(format!("version {version:?}: {e}")))?;
        if f.code_file != "plugin.wasm" && f.code_file != "plugin.wat" {
            return Err(EditorError::PluginIndex(format!(
                "unexpected code file {:?}",
                f.code_file
            )));
        }
        match &self.dir {
            Some(d) => {
                let at = d.join(id).join(version);
                crate::user_config::write_atomic(&at.join("plugin.ron"), f.manifest.as_bytes())?;
                crate::user_config::write_atomic(&at.join(&f.code_file), &f.code)?;
                Ok(at.display().to_string())
            }
            None => {
                self.memory
                    .insert((id.to_string(), version.to_string()), f.clone());
                Ok(format!("memory:{id}/{version}"))
            }
        }
    }

    /// A cached plugin's files.
    pub fn get(&self, id: &str, version: &str) -> Result<Fetched, EditorError> {
        if let Some(f) = self.memory.get(&(id.to_string(), version.to_string())) {
            return Ok(f.clone());
        }
        let Some(d) = &self.dir else {
            return Err(EditorError::PluginIndex(format!(
                "{id} {version} is not cached"
            )));
        };
        let at = d.join(id).join(version);
        let io = |e: std::io::Error| EditorError::PluginIndex(format!("{}: {e}", at.display()));
        let manifest = std::fs::read_to_string(at.join("plugin.ron")).map_err(io)?;
        let (code_file, code) = match std::fs::read(at.join("plugin.wasm")) {
            Ok(c) => ("plugin.wasm", c),
            Err(_) => (
                "plugin.wat",
                std::fs::read(at.join("plugin.wat")).map_err(io)?,
            ),
        };
        Ok(Fetched {
            manifest,
            code,
            code_file: code_file.into(),
        })
    }

    /// Whether `id` `version` is cached.
    #[must_use]
    pub fn contains(&self, id: &str, version: &str) -> bool {
        self.get(id, version).is_ok()
    }
}

/// The plugin manager (see the module docs).
pub struct PluginManager {
    installed: Vec<Installed>,
    wasm: Option<Arc<forge_wasm::WasmHost>>,
    hosted: Vec<forge_wasm::WasmPlugin>,
    revision: u64,
    #[doc(hidden)]
    pub faults: ManagerFaults,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the plugin manager guard. Never set outside it.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct ManagerFaults {
        /// Show every requested capability as granted (a manager that reads the manifest, not
        /// the grants): `test_plugin_manager`'s control.
        pub requested_as_granted: bool,
        /// Name only one plugin of a conflict: the same test's `replaces` control.
        pub conflicts_name_one: bool,
    }
}

impl std::fmt::Debug for PluginManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginManager")
            .field("installed", &self.installed.len())
            .field("hosted", &self.hosted.len())
            .finish_non_exhaustive()
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::from_load(&[], &[])
    }
}

impl PluginManager {
    /// The plugins a load installed (`loaded`) and left out (`disabled`).
    #[must_use]
    pub fn from_load(loaded: &[&Manifest], disabled: &[&Manifest]) -> Self {
        let mut installed: Vec<Installed> = loaded
            .iter()
            .map(|m| Installed {
                manifest: (*m).clone(),
                state: match m.kind {
                    PluginKind::Source => InstallState::Loaded,
                    PluginKind::Wasm => {
                        InstallState::NotLoaded("no WASM host was given this manifest".into())
                    }
                },
            })
            .collect();
        installed.extend(disabled.iter().map(|m| Installed {
            manifest: (*m).clone(),
            state: InstallState::Disabled,
        }));
        installed.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        Self {
            installed,
            wasm: None,
            hosted: Vec::new(),
            revision: 0,
            faults: ManagerFaults::default(),
        }
    }

    /// Changes whenever the installed set changes.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The installed plugins, by id.
    #[must_use]
    pub fn installed(&self) -> &[Installed] {
        &self.installed
    }

    /// The editor's WASM host, if it hosts plugins.
    #[must_use]
    pub fn wasm_host(&self) -> Option<&Arc<forge_wasm::WasmHost>> {
        self.wasm.as_ref()
    }

    /// The hosted WASM plugins.
    #[must_use]
    pub fn hosted(&self) -> &[forge_wasm::WasmPlugin] {
        &self.hosted
    }

    /// Host WASM plugins: install `plugins` through the ordinary loader (`load_hosted`,
    /// next to the already loaded manifests, so a conflict with any of them is refused
    /// naming both) into `ext`, and list them. `host` checks the editor's one grant table.
    pub fn host_wasm(
        &mut self,
        host: Arc<forge_wasm::WasmHost>,
        plugins: Vec<forge_wasm::WasmPlugin>,
        ext: &mut forge_plugin::Extensions,
    ) -> Result<(), EditorError> {
        // The installed source manifests take part in the conflict check (their code is
        // already installed, so only their manifests are passed, as `wasm`-less checks).
        let others: Vec<Manifest> = self
            .installed
            .iter()
            .filter(|i| i.state == InstallState::Loaded)
            .map(|i| i.manifest.clone())
            .collect();
        let mut all: Vec<&Manifest> = others.iter().collect();
        all.extend(plugins.iter().map(HostedPlugin::manifest));
        if let Some(e) = forge_plugin::loader::conflicts(&all).into_iter().next() {
            return Err(e.into());
        }
        let hosted: Vec<&dyn HostedPlugin> =
            plugins.iter().map(|p| p as &dyn HostedPlugin).collect();
        forge_plugin::loader::load_hosted(ext, &[], &hosted, &[], &host.grants().snapshot())?;
        for p in &plugins {
            self.installed.retain(|i| i.manifest.id != *p.id());
            self.installed.push(Installed {
                manifest: p.manifest().clone(),
                state: InstallState::Hosted {
                    generation: p.generation(),
                },
            });
        }
        self.installed
            .sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        self.hosted.extend(plugins);
        self.wasm = Some(host);
        self.revision += 1;
        Ok(())
    }

    /// List WASM plugins the editor's hosting installed through the loader (at start, with
    /// the source plugins, or dropped in while it runs: [`crate::hosting`]).
    pub fn note_hosted(
        &mut self,
        host: &Arc<forge_wasm::WasmHost>,
        plugins: &[forge_wasm::WasmPlugin],
    ) {
        for p in plugins {
            self.installed.retain(|i| i.manifest.id != *p.id());
            self.installed.push(Installed {
                manifest: p.manifest().clone(),
                state: InstallState::Hosted {
                    generation: p.generation(),
                },
            });
            self.hosted.retain(|h| h.id() != p.id());
            self.hosted.push(p.clone());
        }
        self.installed
            .sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        self.wasm = Some(Arc::clone(host));
        self.revision += 1;
    }

    /// A hosted plugin's code was reloaded: its row shows the new generation.
    pub fn note_reloaded(&mut self, id: &forge_plugin::PluginId, generation: u64) {
        for i in &mut self.installed {
            if i.manifest.id == *id && matches!(i.state, InstallState::Hosted { .. }) {
                i.state = InstallState::Hosted { generation };
                self.revision += 1;
            }
        }
    }

    /// A plugin directory that did not load: listed (when its manifest read) with why.
    pub fn note_not_loaded(&mut self, dir: &Path, id: Option<&str>, why: &str) {
        let Some(m) = std::fs::read_to_string(dir.join("plugin.ron"))
            .ok()
            .and_then(|t| Manifest::parse(&t).ok())
            .filter(|m| id.is_none_or(|id| m.id.as_str() == id))
        else {
            return;
        };
        if self.installed.iter().any(|i| {
            i.manifest.id == m.id
                && matches!(i.state, InstallState::Loaded | InstallState::Hosted { .. })
        }) {
            return; // the same id is loaded from elsewhere; the notice names this copy
        }
        self.installed.retain(|i| i.manifest.id != m.id);
        self.installed.push(Installed {
            manifest: m,
            state: InstallState::NotLoaded(format!("{} ({why})", dir.display())),
        });
        self.installed
            .sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        self.revision += 1;
    }

    /// The manifests loaded now (source and hosted): the other side of the conflict check
    /// for a plugin dropped in while the editor runs.
    #[must_use]
    pub fn loaded_manifests(&self) -> Vec<Manifest> {
        self.installed
            .iter()
            .filter(|i| matches!(i.state, InstallState::Loaded | InstallState::Hosted { .. }))
            .map(|i| i.manifest.clone())
            .collect()
    }

    /// Everything the loader would see: installed manifests (and `extra`, a candidate).
    fn manifests<'a>(&'a self, extra: Option<&'a Manifest>) -> Vec<&'a Manifest> {
        let mut v: Vec<&Manifest> = self
            .installed
            .iter()
            .filter(|i| !matches!(i.state, InstallState::Disabled))
            .map(|i| &i.manifest)
            .collect();
        v.extend(extra);
        v
    }

    fn conflict_lines(&self, all: &[&Manifest]) -> Vec<(Vec<String>, String)> {
        forge_plugin::loader::conflicts(all)
            .into_iter()
            .filter_map(|e| {
                let (first, second) = match &e {
                    PluginError::Conflict { first, second, .. }
                    | PluginError::DuplicateItem { first, second, .. } => {
                        (first.clone(), second.clone())
                    }
                    _ => return None,
                };
                let text = if self.faults.conflicts_name_one() {
                    match &e {
                        PluginError::Conflict {
                            item, second_op, ..
                        } => {
                            format!("{second} {second_op} {item}, which another plugin changes")
                        }
                        _ => format!("{second} provides an item another plugin provides"),
                    }
                } else {
                    e.to_string()
                };
                Some((vec![first, second], text))
            })
            .collect()
    }

    /// Who provides each item (point, key) among `all`.
    fn providers<'a>(all: &[&'a Manifest]) -> BTreeMap<&'a ItemRef, String> {
        let mut out = BTreeMap::new();
        for m in all {
            for r in &m.provides {
                out.entry(r).or_insert_with(|| m.id.to_string());
            }
        }
        out
    }

    /// Conflicts adding `candidate` would cause (each naming both plugins), before it is
    /// added: the `forge add` preview.
    #[must_use]
    pub fn conflicts_with(&self, candidate: &Manifest) -> Vec<String> {
        let all = self.manifests(Some(candidate));
        self.conflict_lines(&all)
            .into_iter()
            .filter(|(who, _)| who.contains(&candidate.id.to_string()))
            .map(|(_, t)| t)
            .collect()
    }

    /// What `candidate` replaces, each with who provides it now.
    #[must_use]
    pub fn replaces_of(&self, candidate: &Manifest) -> Vec<String> {
        let all = self.manifests(None);
        let providers = Self::providers(&all);
        candidate
            .replaces
            .iter()
            .map(|r| match providers.get(r) {
                Some(p) => format!("replaces {r} from {p}"),
                None => format!("replaces {r} (no installed plugin provides it)"),
            })
            .collect()
    }

    /// The manager's rows: every installed plugin and every plugin the project's set names,
    /// by id.
    #[must_use]
    pub fn rows(&self, mirror: &ProjectMirror) -> Vec<PluginRow> {
        let set = security::plugin_set(mirror.settings_under("plugins."));
        let all = self.manifests(None);
        let conflicts = self.conflict_lines(&all);
        let providers = Self::providers(&all);
        let granted_in_project = |id: &forge_plugin::PluginId, cap: Capability| {
            security::grant_key(&Principal::Plugin(id.clone()), cap).is_some_and(|k| {
                security::value_grants(
                    forge_plugin::PrincipalKind::Plugin,
                    mirror.setting(&k),
                    security::epoch(),
                )
            })
        };
        let mut rows: Vec<PluginRow> = self
            .installed
            .iter()
            .map(|i| {
                let m = &i.manifest;
                let id = m.id.to_string();
                let in_project = set.get(&id).cloned();
                let granted: Vec<Capability> = m
                    .capabilities
                    .iter()
                    .copied()
                    .filter(|c| self.faults.requested_as_granted() || granted_in_project(&m.id, *c))
                    .collect();
                let extra_granted = Capability::ALL
                    .into_iter()
                    .filter(|c| !m.capabilities.contains(c) && granted_in_project(&m.id, *c))
                    .collect();
                PluginRow {
                    enabled: in_project.as_ref().is_none_or(|e| e.enabled),
                    in_project,
                    version: m.version.to_string(),
                    kind: match m.kind {
                        PluginKind::Source => "source",
                        PluginKind::Wasm => "wasm",
                    },
                    state: Some(i.state.clone()),
                    requested: m.capabilities.clone(),
                    granted,
                    extra_granted,
                    replaces: m
                        .replaces
                        .iter()
                        .map(|r| match providers.get(r) {
                            Some(p) => format!("replaces {r} from {p}"),
                            None => format!("replaces {r} (no installed plugin provides it)"),
                        })
                        .collect(),
                    conflicts: conflicts
                        .iter()
                        .filter(|(who, _)| who.contains(&id))
                        .map(|(_, t)| t.clone())
                        .collect(),
                    provides: m.provides.iter().map(ToString::to_string).collect(),
                    id,
                }
            })
            .collect();
        for (id, e) in &set {
            if rows.iter().any(|r| &r.id == id) {
                continue;
            }
            rows.push(PluginRow {
                id: id.clone(),
                version: e.version.clone(),
                kind: "wasm",
                state: None,
                in_project: Some(e.clone()),
                enabled: e.enabled,
                requested: Vec::new(),
                granted: Vec::new(),
                extra_granted: Vec::new(),
                replaces: Vec::new(),
                conflicts: Vec::new(),
                provides: Vec::new(),
            });
        }
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }

    /// The `forge add` equivalent, up to the command: fetch `id` `version` from `index`,
    /// check it (the manifest is the one the index published, and — with a WASM host — the
    /// sandbox's load checks: imports, requested capabilities, the export), refuse it if it
    /// conflicts with an installed plugin (naming both), keep the bytes in `cache` (user
    /// config), and return the `forge.plugin.add` command for the caller to emit.
    pub fn prepare_add(
        &self,
        index: &dyn PluginIndex,
        cache: &mut PluginCache,
        id: &str,
        version: &str,
    ) -> Result<forge_cmd::EditorCommand, EditorError> {
        let f = index.fetch(id, version)?;
        let m = Manifest::parse(&f.manifest)?;
        if m.id.as_str() != id || m.version.to_string() != version {
            return Err(EditorError::PluginIndex(format!(
                "the index served {} {} for {id} {version}",
                m.id, m.version
            )));
        }
        if let Some(c) = self.conflicts_with(&m).into_iter().next() {
            return Err(EditorError::Plugin(c));
        }
        if let Some(host) = &self.wasm
            && m.kind == PluginKind::Wasm
        {
            host.load(m.clone(), &f.code)
                .map_err(|e| EditorError::PluginIndex(e.to_string()))?;
        }
        cache.store(id, version, &f)?;
        Ok(security::plugin_add_command(id, version, &index.name()))
    }
}
