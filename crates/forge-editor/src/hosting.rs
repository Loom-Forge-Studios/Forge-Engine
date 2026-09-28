//! **WASM plugins in the running editor** (Ch.32 §32.7, Ch.21 §21.19; WP-21, ADR 0045).
//!
//! The editor hosts drop-in plugins on its one grant table (E-27): a directory holding
//! `plugin.ron` (the manifest, `kind: Wasm`) and `plugin.wasm` or `plugin.wat`.
//!
//! **Where it looks** ([`PluginDirs`]):
//!
//! * `<user config>/plugins/<name>/` — the person's drop-ins, for every project; and the
//!   plugin cache's `<user config>/plugins/<id>/<version>/` (what the Plugin manager's *Add*
//!   downloads) for each plugin the open project's set names, enabled, at that version.
//! * `<project folder>/plugins/<name>/` — plugins the open project carries (they travel with
//!   it in version control).
//!
//! A plugin the project's set disables is not loaded from anywhere. The same id in two
//! places loads once (the first by path) and the other is reported.
//!
//! **One loader.** At start ([`crate::shell::assemble_hosted`]) the WASM plugins found go
//! through `forge_plugin::loader::load_hosted` together with the source plugins: one manifest
//! check, one conflict check, one install order — a WASM plugin may add, replace, remove or
//! chain anything a source plugin may, at the points it has adapters for (`Command`,
//! `Preset`, `Importer`, `EditorPanel` via [`crate::viewspec`]). A plugin that fails to load
//! is left out with the reason (in the Plugin manager and a notice); it never stops the
//! editor starting.
//!
//! **Dropped in while the editor runs** ([`PluginHosting::poll`], called by the shell): a new
//! directory is loaded through the same loader into a scratch registry set, conflict-checked
//! against every installed manifest, and its items are added to the live registries — its
//! commands on the core's bus and in the palette, its presets in the new-project flow, its
//! importers in the asset database, its panels in the Window menu. A plugin that replaces,
//! removes or chains existing items changes what other plugins installed; that waits for the
//! next start (the loader's single install order), and the notice says so.
//!
//! **Hot reload** is `forge_wasm::PluginWatcher`: changed code swaps in place under every
//! installed item; a broken save keeps the old code; a changed manifest asks for a restart.
//!
//! **No idle cost (D-5).** Nothing here has a thread or a timer. The shell calls
//! [`PluginHosting::poll`] from the loop turns it already runs — at most once per
//! [`POLL_EVERY`] — and a poll lists each plugin root and stats what it finds and each
//! watched file, reading a file only when something changed. An idle editor makes no
//! wakeups for plugins; a plugin dropped in while the editor sleeps is picked up the next time
//! it wakes (the window gains focus, the pointer moves over it).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use forge_plugin::points::{Command, EditorPanel, PanelDescriptor, Preset};
use forge_plugin::{Extensions, HostedPlugin, Manifest, Order, PluginId, SharedGrants};
use forge_wasm::{PluginWatcher, ReloadEvent, WasmHost, WasmPlugin};

use crate::EditorError;
use crate::core::{EditorCore, SharedCore};
use crate::panels::PanelCx;
use crate::presets::{PresetCatalog, SharedPresets};
use crate::security::PluginSetEntry;

/// The fewest seconds between two polls (the shell's loop may turn at 60 Hz while a person
/// drags; plugin files are checked at most once a second).
pub const POLL_EVERY: Duration = Duration::from_secs(1);

/// The directory under the user config and under a project folder that holds plugins.
pub const PLUGINS_DIR: &str = "plugins";

/// Where the editor looks for WASM plugins (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginDirs {
    /// `<user config>/plugins`.
    pub user: Option<PathBuf>,
    /// `<project folder>/plugins` of the open project (`None`: no project folder is open).
    pub project: Option<PathBuf>,
}

impl PluginDirs {
    /// The user plugin directory of `config_dir`.
    #[must_use]
    pub fn for_user(config_dir: Option<&Path>) -> Self {
        Self {
            user: config_dir.map(|d| d.join(PLUGINS_DIR)),
            project: None,
        }
    }

    /// The plugin directory of the project at `location` (`file:<folder>`; other stores
    /// carry no plugin folder the editor could run code from).
    #[must_use]
    pub fn project_dir(location: &str) -> Option<PathBuf> {
        location
            .strip_prefix("file:")
            .map(|p| PathBuf::from(p).join(PLUGINS_DIR))
    }
}

/// What [`crate::shell::assemble_hosted`] hosts WASM plugins with: the core (its one grant
/// table; it runs the plugin commands and creates projects from the presets) and where to
/// look.
#[derive(Clone)]
pub struct HostingSetup {
    pub core: SharedCore,
    pub dirs: PluginDirs,
}

impl std::fmt::Debug for HostingSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostingSetup")
            .field("dirs", &self.dirs)
            .finish_non_exhaustive()
    }
}

/// The WASM host the editor uses: its one grant table, the built-in `Command` and `Preset`
/// adapters, and the `Importer` and `EditorPanel` (ViewSpec) adapters.
pub fn editor_host(grants: SharedGrants) -> Result<WasmHost, EditorError> {
    let mut h = WasmHost::new(grants).map_err(|e| EditorError::Plugin(e.to_string()))?;
    forge_wasm::importer::adapt_importers(&mut h);
    crate::viewspec::adapt_panels(&mut h);
    // UNBUILT (W9): the open project's store is the core's, whose lock a plugin command's
    // plan already runs under, so `project-read` is not served to editor-hosted plugins yet —
    // the call says so instead of claiming no project is open. (An importer gets the files it
    // needs through `needs`, which is served.)
    h.set_project_reader(Some(Arc::new(|path: &str| {
        Err(format!(
            "project-read of {path}: the editor does not serve project files to WASM plugins yet (an importer names the files it needs instead)"
        ))
    })));
    Ok(h)
}

/// What a scan found (see the module docs for the layout).
#[derive(Debug, Default)]
struct Scan {
    /// The plugin directories to load, sorted per root — each of the open project's own
    /// folders with the digest the trust gate approved (WP-36: the load compiles only bytes
    /// with that digest).
    dirs: Vec<(PathBuf, Option<String>)>,
    /// The plugin directories the open project brings — its `plugins/` folders and the
    /// cached plugins its set names — that the person has not trusted as they are now (WP-34;
    /// by content since WP-35), each with what it is.
    untrusted: Vec<(PathBuf, crate::trust::CarriedItem)>,
}

/// The trust gate a scan asks about each plugin the open project brings (WP-34, WP-35):
/// whether the person trusted it as it is now ([`EditorCore::vet_carried`]).
type Approve<'a> = dyn FnMut(&crate::trust::CarriedItem) -> bool + 'a;

/// The digests of the open project's plugin folders, kept by the files' stamps (a folder is
/// read again only when its manifest or its code changed): what the trust gate compares. A
/// folder that is gone (or a project that is closed) is dropped at the next scan (WP-36).
#[derive(Debug, Default)]
struct Digests(BTreeMap<PathBuf, (FullStamp, Option<crate::trust::CarriedItem>)>);

/// A plugin folder's manifest and code stamps, whole (length and modification time each).
type FullStamp = (Option<(u64, u128)>, Option<(u64, u128)>);

fn full_stamp(dir: &Path) -> FullStamp {
    let one = |p: &Path| {
        std::fs::metadata(p).ok().map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            (m.len(), t)
        })
    };
    (
        one(&dir.join("plugin.ron")),
        forge_wasm::code_file(dir).and_then(|p| one(&p)),
    )
}

impl Digests {
    /// What the plugin folder `dir` is now (`None`: no manifest). Kept, not cloned: an
    /// unchanged folder costs its stamps and nothing else.
    fn item(&mut self, dir: &Path) -> Option<&crate::trust::CarriedItem> {
        let stamp = full_stamp(dir);
        let fresh = self.0.get(dir).is_some_and(|(s, _)| *s == stamp);
        if !fresh {
            let item = crate::trust::dir_item(dir);
            self.0.insert(dir.to_path_buf(), (stamp, item));
        }
        self.0.get(dir).and_then(|(_, item)| item.as_ref())
    }

    /// How many folders it keeps (the prune guard reads it).
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// The gate's answers, kept while nothing they depend on moved (WP-36): the core's trust
/// version ([`EditorCore::trust_version`]) and the open project. An idle poll of a trusted
/// project whose folders did not change asks the core nothing — no lock per plugin per poll.
/// A changed folder has a new digest, so it is asked about afresh; an answer not asked for
/// in a poll is dropped.
#[derive(Debug, Default)]
struct Vets {
    /// The version and the project the answers were given for.
    under: Option<(u64, Option<String>)>,
    /// By item: the digest answered about, the answer, and whether this poll used it. Looked
    /// up by the item's name as borrowed, so a poll that asks nothing new allocates nothing
    /// here.
    answers: BTreeMap<String, VetAnswer>,
    /// How many times the core was asked (each is one lock of the core).
    asked: u64,
}

#[derive(Debug)]
struct VetAnswer {
    digest: String,
    trusted: bool,
    used: bool,
}

impl Vets {
    /// A poll starts at `version` (read before any question) with `project` open.
    fn begin(&mut self, version: Option<u64>, project: Option<&str>) {
        let same = match (&self.under, version) {
            (Some((was, p)), Some(v)) => *was == v && p.as_deref() == project,
            _ => false,
        };
        if !same {
            self.answers.clear();
            self.under = version.map(|v| (v, project.map(str::to_string)));
        }
        for a in self.answers.values_mut() {
            a.used = false;
        }
    }

    /// The gate's answer about `item`: kept, or asked of the core with `ask`.
    fn answer(
        &mut self,
        item: &crate::trust::CarriedItem,
        cache: bool,
        ask: impl FnOnce(&crate::trust::CarriedItem) -> bool,
    ) -> bool {
        if cache
            && self.under.is_some()
            && let Some(a) = self.answers.get_mut(item.item.as_str())
            && a.digest == item.digest
        {
            a.used = true;
            return a.trusted;
        }
        self.asked += 1;
        let trusted = ask(item);
        match self.answers.get_mut(item.item.as_str()) {
            Some(a) => {
                a.digest.clone_from(&item.digest);
                a.trusted = trusted;
                a.used = true;
            }
            None => {
                self.answers.insert(
                    item.item.clone(),
                    VetAnswer {
                        digest: item.digest.clone(),
                        trusted,
                        used: true,
                    },
                );
            }
        }
        trusted
    }

    /// The poll is over: forget what it did not ask about.
    fn end(&mut self) {
        self.answers.retain(|_, a| a.used);
    }
}

/// Every plugin directory under `dirs`. The open project's own (its `plugins/` folder, and
/// the plugin cache's versions its set names) only when `approve` says the person trusted
/// each as it is now ([`crate::trust`]); the person's drop-ins always.
fn scan(
    dirs: &PluginDirs,
    set: &BTreeMap<String, PluginSetEntry>,
    digests: &mut Digests,
    approve: &mut Approve<'_>,
) -> Scan {
    let mut out = Scan::default();
    let mut seen = std::collections::BTreeSet::new();
    for (root, user) in [(&dirs.user, true), (&dirs.project, false)] {
        let Some(root) = root else { continue };
        let Ok(rd) = std::fs::read_dir(root) else {
            continue;
        };
        let mut here: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        here.sort();
        for d in here {
            if d.join("plugin.ron").is_file() {
                if user {
                    out.dirs.push((d, None));
                    continue;
                }
                // The project's own folder: its manifest and code as the person trusted them.
                seen.insert(d.clone());
                let Some(item) = digests.item(&d) else {
                    continue;
                };
                if approve(item) {
                    out.dirs.push((d, Some(item.digest.clone())));
                } else {
                    out.untrusted.push((d, item.clone()));
                }
            } else if user
                && let Some(name) = d.file_name().and_then(|n| n.to_str())
                && let Some(e) = set.get(name)
                && e.enabled
                && !e.version.is_empty()
                && d.join(&e.version).join("plugin.ron").is_file()
            {
                // The plugin cache (the Plugin manager's downloads): the version the open
                // project names — code the project chose, so only when the person trusts it
                // at that version.
                let item = crate::trust::cache_item(name, &e.version);
                let trusted = approve(&item);
                if trusted {
                    out.dirs.push((d.join(&e.version), None));
                } else {
                    out.untrusted.push((d.join(&e.version), item));
                }
            }
        }
    }
    // A folder that is gone, or a project no longer open, keeps no digest (WP-36).
    digests.0.retain(|d, _| seen.contains(d));
    out
}

/// Why a plugin the open project brings did not load (the Plugin manager's row).
pub const UNTRUSTED_WHY: &str = "the project is not trusted, or this plugin is new or changed since you trusted it (decide in the Plugin manager to run it)";

/// A plugin directory that did not load, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub dir: PathBuf,
    /// Its manifest's id, when the manifest read.
    pub id: Option<String>,
    pub why: String,
}

/// What a start-up scan found: the WASM plugins that loaded, and the directories that did not.
pub struct Discovered {
    pub plugins: Vec<(PathBuf, WasmPlugin)>,
    pub refused: Vec<Refused>,
    /// The open project's plugin directories left out because the person has not trusted
    /// them as they are now (WP-34, WP-35).
    pub untrusted: Vec<PathBuf>,
}

/// Load (compile and check) every plugin directory under `dirs` on `host`, leaving out the
/// ones the project's `set` disables, a second directory with an id already found, and the
/// open project's own unless `core` says the person trusted each as it is now
/// ([`crate::trust`]; the ones they did not are added to the question the person is asked).
/// Without a core, the open project's own are left out. A project folder whose files changed
/// between the gate's look and the load is left out too (WP-36): it is vetted again, as it is
/// now, at the first poll.
pub fn discover(
    host: &WasmHost,
    dirs: &PluginDirs,
    set: &BTreeMap<String, PluginSetEntry>,
    core: Option<&SharedCore>,
) -> Discovered {
    let mut approve = |i: &crate::trust::CarriedItem| {
        core.is_some_and(|c| {
            EditorCore::vet_carried(c, std::slice::from_ref(i))
                .first()
                .copied()
                .unwrap_or(false)
        })
    };
    let found = scan(dirs, set, &mut Digests::default(), &mut approve);
    let mut d = Discovered {
        plugins: Vec::new(),
        refused: Vec::new(),
        untrusted: found.untrusted.into_iter().map(|(p, _)| p).collect(),
    };
    for (dir, approved) in found.dirs {
        match load_dir(
            host,
            &dir,
            approved.as_deref(),
            set,
            &d.plugins,
            &HostingFaults::default(),
        ) {
            Ok(Loaded::Plugin(p)) => d.plugins.push((dir, *p)),
            Ok(Loaded::Disabled(_)) => {}
            Ok(Loaded::Changed) => d.untrusted.push(dir),
            Err(r) => d.refused.push(r),
        }
    }
    d
}

fn manifest_of(dir: &Path) -> Option<Manifest> {
    std::fs::read_to_string(dir.join("plugin.ron"))
        .ok()
        .and_then(|t| Manifest::parse(&t).ok())
}

/// What loading one directory came to.
enum Loaded {
    Plugin(Box<WasmPlugin>),
    /// The project's set disables it (its id, when the manifest read).
    Disabled(Option<String>),
    /// The files read are not the ones the trust gate approved (WP-36: a writer changed them
    /// after the gate looked): nothing was compiled.
    Changed,
}

/// Load one directory, its files read once ([`forge_wasm::PluginFiles`]): when `approved`
/// names the digest the trust gate approved for it (the open project's own folder), only
/// bytes with that digest are compiled — the bytes read, not a later read of the files.
/// `faults` are the W2 switches (`HostingFaults::unchecked_reads`, `swap_after_check`,
/// `reread_after_vet`); a production build has none.
fn load_dir(
    host: &WasmHost,
    dir: &Path,
    approved: Option<&str>,
    set: &BTreeMap<String, PluginSetEntry>,
    already: &[(PathBuf, WasmPlugin)],
    faults: &HostingFaults,
) -> Result<Loaded, Refused> {
    let refused = |id: Option<String>, e: forge_wasm::WasmError| Refused {
        dir: dir.to_path_buf(),
        id,
        why: e.to_string(),
    };
    let files = if faults.unchecked_reads() {
        None
    } else {
        Some(forge_wasm::PluginFiles::read(dir).map_err(|e| refused(None, e))?)
    };
    if let (Some(f), Some(digest)) = (&files, approved)
        && crate::trust::plugin_files_digest(f) != digest
    {
        return Ok(Loaded::Changed);
    }
    let m = match &files {
        Some(f) => f.manifest().ok(),
        None => manifest_of(dir),
    };
    let id = m.as_ref().map(|m| m.id.to_string());
    if let Some(id) = &id {
        if set.get(id).is_some_and(|e| !e.enabled) {
            return Ok(Loaded::Disabled(Some(id.clone())));
        }
        if let Some((other, _)) = already.iter().find(|(_, p)| p.id().as_str() == id) {
            return Err(Refused {
                dir: dir.to_path_buf(),
                id: Some(id.clone()),
                why: format!("{id} is already loaded from {}", other.display()),
            });
        }
    }
    if approved.is_some()
        && let Some(code) = faults.swap_after_check()
    {
        swap_code(dir, code); // the seam: after the check, before the compile
    }
    // The checked bytes are the ones compiled: nothing is read between the check and the
    // compile (the control reads again).
    match &files {
        Some(_) if faults.reread_after_vet() => host.load_dir(dir),
        Some(f) => host.load_files(f),
        None => host.load_dir(dir),
    }
    .map(|p| Loaded::Plugin(Box::new(p)))
    .map_err(|e| refused(id, e))
}

/// The seam of `HostingFaults::swap_after_vet` and `swap_after_check`: a writer replaces the
/// code of the plugin folder `dir` with `code` (a `git pull` landing between the trust gate's
/// look and the read, or between the check of the bytes read and their compile).
fn swap_code(dir: &Path, code: &str) {
    if let Some(p) = forge_wasm::code_file(dir) {
        let _ = std::fs::write(p, code);
    }
}

/// A plugin's panels, by key: what the shell adds to its panel host.
pub type NewPanels = Vec<(String, PanelDescriptor<PanelCx>)>;

/// What a poll did, for the shell to show and to wire into the running editor.
#[derive(Default)]
pub struct PollOutcome {
    /// Plugins installed now: `(plugin, where, its panels)`.
    pub installed: Vec<(WasmPlugin, PathBuf, NewPanels)>,
    /// Commands those plugins added (the palette lists them).
    pub commands: Vec<String>,
    /// New code live under a plugin's items: `(id, generation)`.
    pub reloaded: Vec<(PluginId, u64)>,
    /// Problems: one line each (a refused plugin, a failed reload, a restart needed).
    pub problems: Vec<String>,
    /// Whether the importers changed (the asset database refreshes its plugins).
    pub importers_changed: bool,
    /// Plugin directories of the open project newly left out because it is not trusted
    /// (WP-34): the Plugin manager lists them *not loaded*, and the person is asked.
    pub untrusted: Vec<PathBuf>,
    /// Loaded plugin directories of the open project whose files changed into something the
    /// person has not trusted (WP-35: a `git pull` changed their code): the code the person
    /// trusted keeps running, the change waits for their answer (the person is asked).
    pub held_reloads: Vec<PathBuf>,
}

impl PollOutcome {
    /// Whether nothing happened.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.installed.is_empty()
            && self.reloaded.is_empty()
            && self.problems.is_empty()
            && self.untrusted.is_empty()
            && self.held_reloads.is_empty()
    }
}

/// How often the hosting did what (the idle guard reads it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostingStats {
    /// Polls that ran (throttled ones not counted).
    pub polls: u64,
    /// Plugins installed while the editor ran.
    pub installs: u64,
    /// Hot reloads.
    pub reloads: u64,
    /// Questions put to the core's trust gate ([`EditorCore::vet_carried`]), each one lock of
    /// the core (WP-36: an idle poll of a project whose plugins did not change asks none).
    pub vets: u64,
    /// Plugin folders whose digests it keeps (WP-36: a removed folder is dropped).
    pub digests: usize,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the hosting guards. Never set outside them.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct HostingFaults {
        /// The shell never polls (as before WP-21: nothing dropped in installs, nothing
        /// reloads): `test_wasm_plugins_in_editor`'s control.
        pub never_poll: bool,
        /// Every loop turn polls, unthrottled (the idle control: a poll on each turn is fine,
        /// but asking the loop for turns to poll is not).
        pub poll_wakes_the_loop: bool,
        /// The hosting loads the open project's plugins whether or not it is trusted, as before
        /// WP-34 (`test_project_trust`'s code control: a cloned project's plugin then runs).
        pub ignore_trust: bool,
        /// Fault injection for `test_install_live_atomic`: the live install fails at this step.
        pub fail_install_at: Option<InstallStep>,
        /// A live install commits each step as it goes, as before WP-34 (its commands onto the
        /// bus and the live registry before the later steps are staged):
        /// `test_install_live_atomic`'s control — a later failure then leaves them installed.
        pub commit_as_it_goes: bool,
        /// A live install does not check its panels against the shell's panel host, as before
        /// the WP-34 verifier: `test_install_live_atomic`'s panel control — a plugin whose panel
        /// id the editor already shows then goes live without its panel.
        pub skip_panel_check: bool,
        /// Fault **injection** for `test_project_trust`'s check-at-read guard (WP-36): a writer
        /// replaces the code of each open-project plugin folder the trust gate just approved
        /// with this text — after the gate's look, before the load and before the hot-reload
        /// watcher reads (a `git pull` landing in between).
        pub swap_after_vet: Option<&'static str>,
        /// The check-at-read guard's control (WP-36): the load compiles what it reads without
        /// comparing it to the digest the trust gate approved, and the watcher reloads without
        /// showing the gate the bytes, as before WP-36 — a swap between the gate's look and the
        /// read then runs code nobody vetted.
        pub unchecked_reads: bool,
        /// Fault **injection** for `test_project_trust`'s read-once guard (WP-36): a writer
        /// replaces the code of an open-project plugin folder with this text **after** the
        /// bytes read were checked — at the load, after the digest comparison passed; at the
        /// hot reload, after the gate's vet allowed them — and before they are compiled.
        pub swap_after_check: Option<&'static str>,
        /// The read-once guard's control (WP-36): the load and the watcher check the bytes they
        /// read, then read the files **again** and compile that second read — a write between
        /// the check and the compile then runs code nobody vetted.
        pub reread_after_vet: bool,
        /// The vet cache's control (WP-36): every poll asks the core's trust gate about every
        /// plugin the open project brings, as before WP-36 (one core lock each, per poll).
        pub no_vet_cache: bool,
    }
}

/// The steps of a live install ([`PluginHosting::poll`]), for fault injection.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallStep {
    /// Staging its commands (for the live `Command` registry and the core's bus).
    Commands,
    /// Staging its presets.
    Presets,
    /// Staging its importers.
    Importers,
    /// Staging its panels: each id checked absent from the shell's panel host and from the
    /// panels of plugins installed earlier in the same poll.
    Panels,
    /// Committing to the live registries, after its presets went in: they roll back.
    Registries,
    /// The core's bus refusing its commands, the last commit step: the live registries roll
    /// back.
    Bus,
}

/// The running editor's WASM plugins (see the module docs). The shell owns it through
/// [`crate::services::EditorServices::hosting`].
pub struct PluginHosting {
    host: Option<Arc<WasmHost>>,
    /// The registries the shell does not take (`Command`, `Preset`, the asset points, ...):
    /// where plugin items live after assembly.
    ext: Extensions,
    watcher: PluginWatcher,
    dirs: PluginDirs,
    /// Plugin directories loaded (or found disabled), and the id each holds.
    loaded: BTreeMap<PathBuf, PluginId>,
    /// Directories that did not load, with the manifest and code stamp then (retried when
    /// either changes).
    refused: BTreeMap<PathBuf, (Option<(u64, u64)>, String)>,
    /// Directories whose plugin the project's set disables, with its id: skipped without
    /// reading while the set still disables it.
    disabled: BTreeMap<PathBuf, String>,
    /// The open project's plugin directories left out because it is not trusted, already
    /// reported (WP-34).
    untrusted: std::collections::BTreeSet<PathBuf>,
    /// Loaded plugin directories of the open project whose change waits for the person's
    /// trust, already reported (WP-35): the watcher does not reload them meanwhile.
    paused: std::collections::BTreeSet<PathBuf>,
    /// The digests of the open project's plugin folders, by their stamps (WP-35).
    digests: Digests,
    /// The trust gate's answers, kept against the core's trust version (WP-36).
    vets: Vets,
    /// The core's trust version ([`EditorCore::trust_version`]), once attached.
    trust_version: Option<Arc<std::sync::atomic::AtomicU64>>,
    presets: SharedPresets,
    core: Option<SharedCore>,
    last_poll: Option<Duration>,
    stats: HostingStats,
    #[doc(hidden)]
    pub faults: HostingFaults,
}

impl std::fmt::Debug for PluginHosting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHosting")
            .field("hosting", &self.host.is_some())
            .field("dirs", &self.dirs)
            .field("loaded", &self.loaded.len())
            .finish_non_exhaustive()
    }
}

impl Default for PluginHosting {
    fn default() -> Self {
        Self {
            host: None,
            ext: Extensions::new(),
            watcher: PluginWatcher::new(),
            dirs: PluginDirs::default(),
            loaded: BTreeMap::new(),
            refused: BTreeMap::new(),
            disabled: BTreeMap::new(),
            untrusted: std::collections::BTreeSet::new(),
            paused: std::collections::BTreeSet::new(),
            digests: Digests::default(),
            vets: Vets::default(),
            trust_version: None,
            presets: Arc::new(std::sync::RwLock::new(PresetCatalog::builtin())),
            core: None,
            last_poll: None,
            stats: HostingStats::default(),
            faults: HostingFaults::default(),
        }
    }
}

fn stamp_of(dir: &Path) -> Option<(u64, u64)> {
    let one = |p: PathBuf| {
        std::fs::metadata(p).ok().map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as u64);
            m.len() ^ t.rotate_left(17)
        })
    };
    let code = forge_wasm::code_file(dir).and_then(one).unwrap_or(0);
    one(dir.join("plugin.ron")).map(|m| (m, code))
}

/// Attach source plugins to a core with no shell (a headless host): the editor's built-in
/// presets and `plugins` load through the ordinary loader, then the core creates projects
/// from their presets, promotes with their promotion rules and runs their commands — what a
/// GUI editor's core gets from its plugin hosting ([`PluginHosting::attach`]). Returns how many
/// commands were installed.
pub fn attach_source_plugins(
    core: &SharedCore,
    plugins: &[&dyn forge_plugin::SourcePlugin],
) -> Result<usize, EditorError> {
    let mut x = crate::editor_extensions()?;
    forge_asset::AssetServer::define_points(&mut x)?;
    let presets = forge_presets::BuiltinPresets::new()?;
    let mut all: Vec<&dyn forge_plugin::SourcePlugin> = vec![&presets];
    all.extend_from_slice(plugins);
    forge_plugin::loader::load(&mut x, &all, &[], &forge_plugin::Grants::new())?;
    let mut h = PluginHosting::assembled(x, None, PluginDirs::default(), &[], &[], &[]);
    h.attach(core)
}

impl PluginHosting {
    /// The hosting after assembly: `ext` holds the registries the shell did not take, the
    /// WASM `plugins` are installed in it (and in the shell's), watched for changes.
    pub(crate) fn assembled(
        ext: Extensions,
        host: Option<Arc<WasmHost>>,
        dirs: PluginDirs,
        plugins: &[(PathBuf, WasmPlugin)],
        refused: &[Refused],
        untrusted: &[PathBuf],
    ) -> Self {
        let mut h = Self {
            host,
            dirs,
            untrusted: untrusted.iter().cloned().collect(),
            ..Self::default()
        };
        for (dir, p) in plugins {
            h.loaded.insert(dir.clone(), p.id().clone());
            // A watch that cannot read the files now keeps nothing to compare: the plugin
            // still runs, it just does not hot-reload (rare: the files were just read).
            let _ = h.watcher.watch(dir, p);
        }
        for r in refused {
            h.refused
                .insert(r.dir.clone(), (stamp_of(&r.dir), r.why.clone()));
        }
        if let Some(reg) = ext.registry::<Preset>() {
            h.presets = Arc::new(std::sync::RwLock::new(PresetCatalog::from_registry(reg)));
        }
        h.ext = ext;
        h
    }

    /// Run over `core`: create projects from this editor's presets, promote with its
    /// plugins' promotion rules and install the plugin commands on its bus (the core runs
    /// every command, I7).
    pub fn attach(&mut self, core: &SharedCore) -> Result<usize, EditorError> {
        EditorCore::set_presets(core, Arc::clone(&self.presets));
        if let Some(reg) = self
            .ext
            .registry::<crate::project::promote::PromotionRulePoint>()
        {
            EditorCore::set_promotion_rules(
                core,
                crate::project::promote::PromotionRules::from_registry(reg),
            );
        }
        let n = match self.ext.registry::<Command>() {
            Some(reg) => EditorCore::install_new_commands(core, reg)
                .map_err(|e| EditorError::Plugin(e.to_string()))?,
            None => 0,
        };
        self.core = Some(core.clone());
        self.trust_version = Some(EditorCore::trust_version(core));
        Ok(n)
    }

    /// The core the hosting runs over, once attached.
    #[must_use]
    pub fn core(&self) -> Option<&SharedCore> {
        self.core.as_ref()
    }

    /// Whether this editor hosts WASM plugins.
    #[must_use]
    pub fn hosting(&self) -> bool {
        self.host.is_some()
    }

    /// The editor's WASM host.
    #[must_use]
    pub fn host(&self) -> Option<&Arc<WasmHost>> {
        self.host.as_ref()
    }

    /// Where it looks.
    #[must_use]
    pub fn dirs(&self) -> &PluginDirs {
        &self.dirs
    }

    /// The registries plugins filled that the shell does not hold (`Command`, `Preset`,
    /// `Importer`, `Exporter`, `AssetType`).
    #[must_use]
    pub fn extensions(&self) -> &Extensions {
        &self.ext
    }

    /// The presets of this editor's `Preset` registry (the new-project flow lists them; the
    /// core creates from them).
    #[must_use]
    pub fn presets(&self) -> &SharedPresets {
        &self.presets
    }

    /// The plugin that provides `command`, if a plugin does.
    #[must_use]
    pub fn command_owner(&self, command: &str) -> Option<String> {
        self.ext
            .registry::<Command>()?
            .provenance(command)
            .map(|p| {
                p.replaced_by
                    .as_ref()
                    .unwrap_or(&p.owner)
                    .as_str()
                    .to_string()
            })
    }

    /// What it did so far.
    #[must_use]
    pub fn stats(&self) -> HostingStats {
        HostingStats {
            vets: self.vets.asked,
            digests: self.digests.len(),
            ..self.stats
        }
    }

    /// Whether a poll is due at `now` (throttled to [`POLL_EVERY`]).
    #[must_use]
    pub fn due(&self, now: Duration) -> bool {
        if self.host.is_none() || self.faults.never_poll() {
            return false;
        }
        self.faults.poll_wakes_the_loop()
            || self
                .last_poll
                .is_none_or(|t| now.saturating_sub(t) >= POLL_EVERY)
    }

    /// Look for changes (see the module docs): hot-reload changed code, install new plugin
    /// directories, follow the open project's plugin folder. `project` is the open project's
    /// location, `set` its plugin set; `installed` the manifests loaded now (the conflict
    /// check's other side); `panel_taken` says whether the shell's panel host already holds a
    /// panel id — a plugin with a panel that could not join it is not installed at all.
    pub fn poll(
        &mut self,
        now: Duration,
        project: Option<&str>,
        set: &BTreeMap<String, PluginSetEntry>,
        installed: &[Manifest],
        panel_taken: &dyn Fn(&str) -> bool,
    ) -> PollOutcome {
        let mut out = PollOutcome::default();
        if !self.due(now) {
            return out;
        }
        self.last_poll = Some(now);
        self.stats.polls += 1;
        let Some(host) = self.host.clone() else {
            return out;
        };
        // The open project's plugin folder follows the open project.
        let project_dir = project.and_then(PluginDirs::project_dir);
        if project_dir != self.dirs.project {
            self.dirs.project = project_dir;
        }
        // The trust gate first (WP-34; by content since WP-35): the open project's plugins
        // run only as the person trusted them. A loaded one whose files changed into
        // something the person has not trusted (a `git pull`) is not reloaded: the code the
        // person trusted keeps serving until they answer. The gate's answers are kept while
        // the core's trust version and the open project stand still (WP-36): an idle poll
        // asks the core nothing.
        let core = self.core.clone();
        let ignore = self.faults.ignore_trust();
        let cache = !self.faults.no_vet_cache();
        let version = self
            .trust_version
            .as_ref()
            .map(|v| v.load(std::sync::atomic::Ordering::Acquire));
        self.vets.begin(version, project);
        let vets = &mut self.vets;
        let mut approve = |i: &crate::trust::CarriedItem| {
            ignore
                || vets.answer(i, cache, |i| {
                    core.as_ref().is_some_and(|c| {
                        EditorCore::vet_carried(c, std::slice::from_ref(i))
                            .first()
                            .copied()
                            .unwrap_or(false)
                    })
                })
        };
        let found = scan(&self.dirs, set, &mut self.digests, &mut approve);
        let (held, waiting): (Vec<_>, Vec<_>) = found
            .untrusted
            .into_iter()
            .map(|(d, _)| d)
            .partition(|d| self.loaded.contains_key(d));
        self.untrusted.retain(|d| waiting.contains(d));
        for d in waiting {
            if self.untrusted.insert(d.clone()) {
                out.untrusted.push(d);
            }
        }
        self.paused.retain(|d| held.contains(d));
        for d in &held {
            if self.paused.insert(d.clone()) {
                out.held_reloads.push(d.clone());
            }
        }
        if let Some(code) = self.faults.swap_after_vet() {
            // The seam: a writer lands after the gate's look, before the watcher reads.
            for (d, _) in found.dirs.iter().filter(|(_, a)| a.is_some()) {
                if self.loaded.contains_key(d) {
                    swap_code(d, code);
                }
            }
        }
        // Code changes: what is installed keeps serving. An open-project plugin's new code
        // is shown to the gate as the bytes read — the bytes that would run (WP-36).
        let paused = &self.paused;
        let project_root = self.dirs.project.clone();
        let unchecked = self.faults.unchecked_reads();
        let swap_after_check = self.faults.swap_after_check();
        #[cfg(feature = "controls")]
        {
            self.watcher.faults.reread_after_vet = self.faults.reread_after_vet();
        }
        let events = self.watcher.poll_vetted(
            |d| !paused.contains(d),
            |dir, files| {
                if unchecked || !project_root.as_ref().is_some_and(|r| dir.starts_with(r)) {
                    return true;
                }
                let ok = crate::trust::files_item(dir, files).is_some_and(|i| approve(&i));
                if ok && let Some(code) = swap_after_check {
                    swap_code(dir, code); // the seam: after the vet allowed, before the compile
                }
                ok
            },
        );
        self.vets.end();
        for ev in events {
            match ev {
                ReloadEvent::Reloaded { plugin, generation } => {
                    self.stats.reloads += 1;
                    out.reloaded.push((plugin, generation));
                }
                ReloadEvent::Failed { plugin, error } => out.problems.push(format!(
                    "{plugin}: the new code did not load; the loaded code keeps running ({error})"
                )),
                ReloadEvent::NeedsLoad { plugin, error } => out.problems.push(format!(
                    "{plugin}: its manifest changed; restart the editor to load the new declarations ({error})"
                )),
                ReloadEvent::Held { dir, .. } => {
                    // Changed after the gate's look into something the person has not
                    // trusted: held like a change the scan found (the gate added it to the
                    // question).
                    if self.paused.insert(dir.clone()) {
                        out.held_reloads.push(dir);
                    }
                }
            }
        }
        if out
            .reloaded
            .iter()
            .any(|(id, _)| self.provides_importer(id))
        {
            out.importers_changed = true;
        }
        let mut manifests: Vec<Manifest> = installed.to_vec();
        for (dir, approved) in found.dirs {
            if self.loaded.contains_key(&dir) {
                continue;
            }
            if let Some(id) = self.disabled.get(&dir)
                && set.get(id).is_some_and(|e| !e.enabled)
            {
                continue;
            }
            if let Some((stamp, _)) = self.refused.get(&dir)
                && *stamp == stamp_of(&dir)
            {
                continue; // refused before and unchanged since
            }
            if approved.is_some()
                && let Some(code) = self.faults.swap_after_vet()
            {
                swap_code(&dir, code); // the seam: after the gate's look, before the load
            }
            let already: Vec<(PathBuf, WasmPlugin)> = Vec::new();
            let plugin = match load_dir(
                &host,
                &dir,
                approved.as_deref(),
                set,
                &already,
                &self.faults,
            ) {
                Ok(Loaded::Plugin(p)) => *p,
                Ok(Loaded::Disabled(id)) => {
                    if let Some(id) = id {
                        self.disabled.insert(dir.clone(), id);
                    }
                    continue;
                }
                // Changed since the gate's look: the next poll vets it as it is now.
                Ok(Loaded::Changed) => continue,
                Err(r) => {
                    out.problems.push(format!("{}: {}", r.dir.display(), r.why));
                    self.refused.insert(dir.clone(), (stamp_of(&dir), r.why));
                    continue;
                }
            };
            match self.install_live(&plugin, &manifests, panel_taken, &mut out) {
                Ok(panels) => {
                    self.refused.remove(&dir);
                    self.loaded.insert(dir.clone(), plugin.id().clone());
                    let _ = self.watcher.watch(&dir, &plugin);
                    manifests.push(plugin.manifest().clone());
                    self.stats.installs += 1;
                    out.installed.push((plugin, dir, panels));
                }
                Err(why) => {
                    out.problems
                        .push(format!("{} ({}): {why}", plugin.id(), dir.display()));
                    self.refused.insert(dir.clone(), (stamp_of(&dir), why));
                }
            }
        }
        out
    }

    fn provides_importer(&self, id: &PluginId) -> bool {
        self.ext
            .registry::<forge_asset::ImporterPoint>()
            .is_some_and(|r| {
                r.keys()
                    .any(|k| r.provenance(k).is_some_and(|p| &p.owner == id))
            })
    }

    /// Install `plugin` into the running editor: through the loader into a scratch registry
    /// set (its manifest checks and install), then its items into the live registries.
    /// Returns its panels for the shell, each checked absent from the shell's panel host
    /// (`panel_taken`), so the shell adds every one of them.
    fn install_live(
        &mut self,
        plugin: &WasmPlugin,
        installed: &[Manifest],
        panel_taken: &dyn Fn(&str) -> bool,
        out: &mut PollOutcome,
    ) -> Result<NewPanels, String> {
        let m = plugin.manifest();
        if !m.replaces.is_empty() || !m.removes.is_empty() || !m.chains.is_empty() {
            return Err(
                "it replaces, removes or chains installed items: restart the editor to load it (one install order for every plugin)".into(),
            );
        }
        let mut all: Vec<&Manifest> = installed.iter().collect();
        all.push(m);
        if let Some(e) = forge_plugin::loader::conflicts(&all)
            .into_iter()
            .find(|e| e.to_string().contains(m.id.as_str()))
        {
            return Err(e.to_string());
        }
        let host = self.host.as_ref().ok_or("no WASM host")?;
        let mut scratch = Extensions::new();
        let define = |x: &mut Extensions| -> Result<(), forge_plugin::PluginError> {
            x.define::<Command>()?;
            x.define::<Preset>()?;
            x.define::<EditorPanel<PanelCx>>()?;
            forge_asset::AssetServer::define_points(x)
        };
        define(&mut scratch).map_err(|e| e.to_string())?;
        let hosted: [&dyn HostedPlugin; 1] = [plugin];
        forge_plugin::loader::load_hosted(
            &mut scratch,
            &[],
            &hosted,
            &[],
            &host.grants().snapshot(),
        )
        .map_err(|e| e.to_string())?;
        let owner = m.id.clone();
        let faults = self.faults;
        let fail = |s: InstallStep| -> Result<(), String> {
            if faults.fail_install_at() == Some(s) {
                Err(format!(
                    "the install failed at its {s:?} step (fault injected)"
                ))
            } else {
                Ok(())
            }
        };
        // 1. Stage: every item the plugin adds, per point, checked against the live
        //    registries and the core's bus. Nothing live changes here.
        let cmds = staged::<Command>(&scratch, self.ext.registry::<Command>())?;
        if let Some(core) = &self.core {
            let taken = EditorCore::configure(core, |bus| {
                cmds.iter()
                    .find(|(t, _)| bus.targets().any(|x| x == t))
                    .map(|(t, _)| t.clone())
            });
            if let Some(t) = taken {
                return Err(format!("the core already runs {t}"));
            }
        }
        fail(InstallStep::Commands)?;
        if faults.commit_as_it_goes() {
            // W2 control only: the WP-21 order, which put the commands live before the later
            // steps were staged.
            self.commit_commands(&owner, &cmds)?;
        }
        let presets = staged::<Preset>(&scratch, self.ext.registry::<Preset>())?;
        fail(InstallStep::Presets)?;
        let importers = staged::<forge_asset::ImporterPoint>(
            &scratch,
            self.ext.registry::<forge_asset::ImporterPoint>(),
        )?;
        fail(InstallStep::Importers)?;
        let panels: NewPanels = scratch
            .registry::<EditorPanel<PanelCx>>()
            .map(|r| r.iter().map(|(k, d)| (k.to_string(), d.clone())).collect())
            .unwrap_or_default();
        if !faults.skip_panel_check() {
            let point = <EditorPanel<PanelCx> as forge_plugin::ExtensionPoint>::ID;
            for (k, _) in &panels {
                let earlier = out
                    .installed
                    .iter()
                    .find(|(_, _, ps)| ps.iter().any(|(p, _)| p == k));
                if let Some((p, _, _)) = earlier {
                    return Err(format!("{point}/{k} is already installed by {}", p.id()));
                }
                if panel_taken(k) {
                    return Err(format!("{point}/{k} is already a panel in this editor"));
                }
            }
        }
        fail(InstallStep::Panels)?;
        // 2. Commit, all or nothing: the live registries (each add undone if a later one is
        //    refused), and the core's bus, which takes every command or none under its lock.
        //    A refusal anywhere undoes all of it. Only then is anything announced.
        let mut undo = Undo::default();
        let r = (|| -> Result<(), String> {
            undo.presets = add_all::<Preset>(&mut self.ext, &owner, &presets)?;
            fail(InstallStep::Registries)?;
            undo.importers =
                add_all::<forge_asset::ImporterPoint>(&mut self.ext, &owner, &importers)?;
            if !faults.commit_as_it_goes() {
                self.commit_commands(&owner, &cmds)?;
            }
            Ok(())
        })();
        if let Err(e) = r {
            undo.roll_back(&mut self.ext, &owner);
            return Err(e);
        }
        out.commands.extend(cmds.iter().map(|(k, _)| k.clone()));
        if !presets.is_empty()
            && let Some(reg) = self.ext.registry::<Preset>()
        {
            self.presets
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .refresh(reg);
        }
        if !importers.is_empty() {
            out.importers_changed = true;
        }
        Ok(panels)
    }

    /// Put `cmds` live, both or neither: the live `Command` registry, then the core's bus —
    /// which takes every command or none (`EditorCore::install_commands_all`); a refusal
    /// there undoes the registry's adds. The last step of a live install's commit.
    fn commit_commands(
        &mut self,
        owner: &PluginId,
        cmds: &[(String, CommandItem)],
    ) -> Result<(), String> {
        if cmds.is_empty() {
            return Ok(());
        }
        let ids = add_all::<Command>(&mut self.ext, owner, cmds)?;
        let r = (|| -> Result<(), String> {
            let mut added = forge_plugin::Registry::<Command>::new();
            for (k, item) in cmds {
                added
                    .add(owner.clone(), k, item.clone(), Order::Last)
                    .map_err(|e| e.to_string())?;
            }
            if self.faults.fail_install_at() == Some(InstallStep::Bus) {
                return Err("the core refused the commands (fault injected)".into());
            }
            match &self.core {
                Some(core) => EditorCore::install_commands_all(core, &added)
                    .map(drop)
                    .map_err(|e| e.to_string()),
                None => Ok(()),
            }
        })();
        if let Err(e) = r {
            remove_all::<Command>(&mut self.ext, owner, &ids);
            return Err(e);
        }
        Ok(())
    }
}

type CommandItem = <Command as forge_plugin::ExtensionPoint>::Item;

/// A live install's items, staged: `P`'s items in `scratch`, each checked absent from the
/// live registry `live` (refused naming the plugin that holds it).
fn staged<P: forge_plugin::ExtensionPoint>(
    scratch: &Extensions,
    live: Option<&forge_plugin::Registry<P>>,
) -> Result<Vec<(String, P::Item)>, String>
where
    P::Item: Clone,
{
    let Some(from) = scratch.registry::<P>() else {
        return Ok(Vec::new());
    };
    if from.is_empty() {
        return Ok(Vec::new());
    }
    let live = live.ok_or_else(|| format!("the {} point is not defined in this editor", P::ID))?;
    let mut v = Vec::with_capacity(from.len());
    for (k, item) in from.iter() {
        if let Some(p) = live.provenance(k) {
            return Err(format!("{}/{k} is already installed by {}", P::ID, p.owner));
        }
        v.push((k.to_string(), item.clone()));
    }
    Ok(v)
}

/// Add `items` to `ext`'s `P` registry for `owner`, all or none (the adds made are removed
/// again when one is refused). The ids added.
fn add_all<P: forge_plugin::ExtensionPoint>(
    ext: &mut Extensions,
    owner: &PluginId,
    items: &[(String, P::Item)],
) -> Result<Vec<forge_plugin::ItemId>, String>
where
    P::Item: Clone,
{
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::with_capacity(items.len());
    let r = (|| -> Result<(), String> {
        let reg = ext
            .registry_mut::<P>()
            .ok_or_else(|| format!("the {} point is not defined in this editor", P::ID))?;
        for (k, item) in items {
            ids.push(
                reg.add(owner.clone(), k, item.clone(), Order::Last)
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(())
    })();
    match r {
        Ok(()) => Ok(ids),
        Err(e) => {
            remove_all::<P>(ext, owner, &ids);
            Err(e)
        }
    }
}

/// Remove the items `ids` from `ext`'s `P` registry (a roll-back: they were just added).
fn remove_all<P: forge_plugin::ExtensionPoint>(
    ext: &mut Extensions,
    owner: &PluginId,
    ids: &[forge_plugin::ItemId],
) {
    if let Some(reg) = ext.registry_mut::<P>() {
        for id in ids.iter().rev() {
            let _ = reg.remove(owner, id);
        }
    }
}

/// What a live install added to the live registries, taken back if a later step fails.
#[derive(Default)]
struct Undo {
    presets: Vec<forge_plugin::ItemId>,
    importers: Vec<forge_plugin::ItemId>,
}

impl Undo {
    fn roll_back(&self, ext: &mut Extensions, owner: &PluginId) {
        remove_all::<forge_asset::ImporterPoint>(ext, owner, &self.importers);
        remove_all::<Preset>(ext, owner, &self.presets);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_dir(root: &Path, name: &str) {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(d.join("plugin.ron"), name).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(d.join("plugin.wat"), "(component)").unwrap_or_else(|e| panic!("{e}"));
    }

    /// WP-36: the digest cache follows the project's folders — a removed folder, and every
    /// folder of a project no longer open, is dropped at the next scan (control: without
    /// the scan's `retain` the removed folders stay and the counts below fail).
    #[test]
    fn a_removed_plugin_folder_keeps_no_digest() {
        let root = std::env::temp_dir().join(format!("forge-wp36-digests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        plugin_dir(&root, "a");
        plugin_dir(&root, "b");
        let mut dirs = PluginDirs {
            user: None,
            project: Some(root.clone()),
        };
        let set = BTreeMap::new();
        let mut digests = Digests::default();
        let mut approve = |_: &crate::trust::CarriedItem| true;
        let found = scan(&dirs, &set, &mut digests, &mut approve);
        assert_eq!(found.dirs.len(), 2);
        assert!(found.dirs.iter().all(|(_, approved)| approved.is_some()));
        assert_eq!(digests.len(), 2);
        std::fs::remove_dir_all(root.join("b")).unwrap_or_else(|e| panic!("{e}"));
        scan(&dirs, &set, &mut digests, &mut approve);
        assert_eq!(digests.len(), 1, "the removed folder's digest was kept");
        dirs.project = None;
        scan(&dirs, &set, &mut digests, &mut approve);
        assert_eq!(digests.len(), 0, "a closed project's digests were kept");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// WP-36: the gate's answers are kept while the version and the project stand still, and
    /// only those; each miss is one question.
    #[test]
    fn vet_answers_are_kept_against_the_version_and_the_project() {
        let item = |d: &str| crate::trust::CarriedItem {
            item: "plugins/a".into(),
            digest: d.into(),
            line: String::new(),
            code: true,
        };
        let mut v = Vets::default();
        let ask = |v: &mut Vets, i: &crate::trust::CarriedItem| v.answer(i, true, |_| true);
        v.begin(Some(1), Some("file:p"));
        ask(&mut v, &item("d1"));
        v.end();
        v.begin(Some(1), Some("file:p"));
        ask(&mut v, &item("d1"));
        v.end();
        assert_eq!(v.asked, 1, "an unchanged item was asked again");
        v.begin(Some(1), Some("file:p"));
        ask(&mut v, &item("d2"));
        v.end();
        assert_eq!(v.asked, 2, "a new digest is a new question");
        v.begin(Some(2), Some("file:p"));
        ask(&mut v, &item("d2"));
        v.end();
        assert_eq!(v.asked, 3, "a moved version asks again");
        v.begin(Some(2), Some("file:q"));
        ask(&mut v, &item("d2"));
        v.end();
        assert_eq!(v.asked, 4, "another project asks again");
        v.begin(None, Some("file:q"));
        ask(&mut v, &item("d2"));
        v.end();
        assert_eq!(v.asked, 5, "no version: nothing is kept");
        assert!(v.answers.len() <= 1, "unused answers were kept");
    }
}
