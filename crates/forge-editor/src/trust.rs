//! **Project trust** (WP-34, ADR 0045 Amendment 1; keyed by content since WP-35, Amendment
//! 2): a project that carries plugin code or plugin grants runs that code, with those grants,
//! only once a person trusts it — and only the code and the grants the person saw.
//!
//! WP-21 made `<project>/plugins/` a code source, and plugin grants
//! (`security.grants.plugin.*`) ship with a project by design. So one cloned or pulled
//! project could carry both the code and the grants to use it, and a person merely opening it
//! would run the one with the other. Here a project is **trusted** or **untrusted** by the
//! person, and the answer is kept in **user config** ([`FileTrust`],
//! `<user config>/trusted-projects.ron`) — never in the project, which is exactly what a
//! project cannot vouch for.
//!
//! **What an answer covers (WP-35).** The answer is remembered for the project folder
//! ([`trust_key`]) **together with what the project carried when the person answered**: one
//! [`CarriedItem`] per thing it brings, each with a digest ([`TrustRecord::content`]):
//!
//! * each `plugins/<name>/` folder — the BLAKE3 digest of its manifest and its code
//!   ([`plugin_dir_digest`]), so a changed manifest or changed code is a different item;
//! * each plugin its set names from the plugin cache — its id and the version it names;
//! * each plugin grant its files make, and each default automation capability its policy adds to
//!   the standard one.
//!
//! What a trusted project carries **now** is compared with that ([`TrustState::of`]): while it
//! is the same (or less — a removal runs nothing new), the project is trusted and nobody is
//! asked. When a `git pull`, a clone of another repository into the folder, or a teammate's
//! push brings a new plugin, changed plugin code, a new grant or a wider automation policy, the
//! project is [`TrustState::Changed`]: the items the person trusted keep running, and **what
//! changed** waits — a new or changed plugin is not loaded (a loaded one keeps the code the
//! person trusted: its hot reload waits), a new grant is held — and the person is asked again,
//! shown exactly what changed. A person's own change in this editor (a grant, a revoke, the
//! automation policy, a plugin added to the set, accepting held settings) is theirs: the answer
//! follows it at once, so it never asks.
//!
//! What a project that is not trusted (not yet decided, or decided untrusted) gets:
//!
//! * **Its plugin grants are held** (`security::held_plugin_grants`): the core's load writes
//!   none of them and keeps them as a held proposal, like a non-human's load (ADR 0043
//!   Amendment 1) — the files keep them byte-identical on every save (Amendment 2), so a
//!   teammate's grants are never stripped by someone who has not decided yet.
//! * **So is a wider default automation policy** (`security::held_changes` against an empty
//!   project): a default automation capability the standard policy lacks takes effect only once
//!   the person trusts the project (or accepts it) — a cloned project must not widen what
//!   every automation session the person connects may do. A narrower policy takes effect at once.
//! * **Its plugins are not loaded**: the plugins in its `plugins/` folder, and the plugins
//!   its plugin set names from the plugin cache. The Plugin manager lists them *not loaded:
//!   the project is not trusted*. The person's own drop-ins (`<user config>/plugins/<name>/`)
//!   load as always: the person put them there.
//! * **The person is asked, once**: the shell posts a notice that opens the Plugin manager,
//!   whose *Project trust* group says what the project carries and offers *Trust this
//!   project* / *Don't trust*. The answer is remembered for that project folder and what it
//!   carries.
//!
//! Trusting ([`crate::core::EditorCore::decide_trust`]) records it, accepts the grants the
//! gate held (the human-only `forge.security.accept_held`, I7 — so it is audited and in the
//! history with the person's name), and the project's plugins install the next time the
//! editor polls. Not trusting keeps the grants held (the files intact) and the plugins out.
//! A project with nothing to run and nothing to grant needs no decision and is not asked
//! about. A project a person creates in this editor is theirs: it is trusted from the start.
//!
//! Trust is **not a bus command**: it is the person's user config, like their keymap, and no
//! client — an automation session, a script, a remote device — has a path to it. Headless runs
//! have no one to ask: they trust nothing unless their launcher passed `--trust-project`
//! ([`TrustEvery`]), a flag on the command line that no file can set — or, on a `--remote-host`
//! run, the person at its console answers the question with `trust ID` / `distrust ID` (WP-36),
//! which is the Plugin manager's call ([`crate::core::EditorCore::decide_trust_seen`]).
//!
//! **What was vetted is what runs (WP-36).** The hosting compiles an open-project plugin
//! only from the one read of its files whose digest the gate approved
//! ([`forge_wasm::PluginFiles`], [`plugin_files_digest`]), and the hot-reload watcher shows
//! the gate the exact bytes it would swap in; a writer landing between the gate's look and
//! the load cannot run code nobody vetted. The gate's answers are kept against the core's
//! trust version ([`crate::core::EditorCore::trust_version`]), so an idle poll takes no core
//! lock per plugin.
//!
//! The gate is independent of who opened the project: an automation session's or a script's open of
//! a trusted project still holds what it would widen for a person (ADR 0043 Amendment 1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use forge_cmd::Value;
use serde::{Deserialize, Serialize};

use crate::security;

/// The file under the user config directory that remembers the person's answers.
pub const TRUST_FILE: &str = "trusted-projects.ron";

/// The item name of a plugin the project's set names from the plugin cache:
/// `plugin-cache/<id>` (its digest is the version the set names).
pub const PLUGIN_CACHE_ITEM: &str = "plugin-cache/";

/// A person's answer about one project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trust {
    /// Its plugins load and its plugin grants take effect.
    Trusted,
    /// Its plugins stay out and its plugin grants stay held.
    Untrusted,
}

/// What a project carries that needs trust, fingerprinted: each item's name and digest.
pub type Content = BTreeMap<String, String>;

/// One answer as it is remembered: the answer and what the project carried when the person
/// gave it (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRecord {
    pub trust: Trust,
    /// Item → digest ([`CarriedItem`]). A record without it (a WP-34 file) covers nothing,
    /// so the project is asked about once more, showing all it carries.
    #[serde(default)]
    pub content: Content,
}

impl TrustRecord {
    /// A record of `trust` for `content`.
    #[must_use]
    pub fn new(trust: Trust, content: Content) -> Self {
        Self { trust, content }
    }

    /// Whether the person trusted exactly this `item` with this `digest`.
    #[must_use]
    pub fn approves(&self, item: &str, digest: &str) -> bool {
        self.trust == Trust::Trusted && self.content.get(item).is_some_and(|d| d == digest)
    }
}

/// Where the person's answers are remembered (see the module docs). Shared by the core and
/// read on its thread; an implementation never blocks on anything but its own lock.
pub trait TrustBook: Send + Sync {
    /// The answer recorded for the project `key` ([`trust_key`]), if any.
    fn get(&self, key: &str) -> Option<TrustRecord>;
    /// Record an answer. `Err` says why it could not be remembered (it still holds for this
    /// run).
    fn set(&self, key: &str, record: TrustRecord) -> Result<(), String>;
    /// Where the answers live, for the Plugin manager's line.
    fn describe(&self) -> String;
    /// Where the project `key` stands, carrying `content` now.
    fn state(&self, key: &str, content: &Content) -> TrustState {
        TrustState::of(self.get(key).as_ref(), content)
    }
    /// Whether the person trusted `item` of the project `key` with this `digest`.
    fn approves(&self, key: &str, item: &str, digest: &str) -> bool {
        self.get(key).is_some_and(|r| r.approves(item, digest))
    }
}

/// The key a project's trust is remembered under: its folder, made absolute and canonical,
/// for a `file:` location (so `C:\P\..\P` and `C:\P` are one project); the location itself
/// for any other store.
#[must_use]
pub fn trust_key(location: &str) -> String {
    match location.strip_prefix("file:") {
        Some(p) => {
            let path = PathBuf::from(p);
            let abs = std::fs::canonicalize(&path).unwrap_or(path);
            format!("file:{}", abs.display())
        }
        None => location.to_string(),
    }
}

/// Answers kept for this process only: a bare core (tests, library use) and headless runs
/// without `--trust-project`. Nothing is trusted until a person says so in this run.
#[derive(Debug, Default)]
pub struct MemoryTrust {
    map: Mutex<BTreeMap<String, TrustRecord>>,
}

impl TrustBook for MemoryTrust {
    fn get(&self, key: &str) -> Option<TrustRecord> {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }
    fn set(&self, key: &str, record: TrustRecord) -> Result<(), String> {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_string(), record);
        Ok(())
    }
    fn describe(&self) -> String {
        forge_ui::tr!("remembered for this run only (no user config directory)").into()
    }
}

/// Every project is trusted, whatever it carries: `--trust-project`, which the person
/// launching a headless run passes knowingly. Nothing is recorded.
#[derive(Debug, Default)]
pub struct TrustEvery;

impl TrustBook for TrustEvery {
    fn get(&self, _: &str) -> Option<TrustRecord> {
        Some(TrustRecord::new(Trust::Trusted, Content::new()))
    }
    fn set(&self, _: &str, _: TrustRecord) -> Result<(), String> {
        Ok(())
    }
    fn describe(&self) -> String {
        forge_ui::tr!("every project is trusted (--trust-project)").into()
    }
    fn state(&self, _: &str, _: &Content) -> TrustState {
        TrustState::Trusted
    }
    fn approves(&self, _: &str, _: &str, _: &str) -> bool {
        true
    }
}

/// The person's answers in `<user config>/trusted-projects.ron` (written crash-safely, like
/// every user config file). A file that does not parse is kept as it is and the book starts
/// empty (so nothing is trusted by accident); the next answer replaces it. A WP-34 file (the
/// bare answers, no content) reads as answers that cover nothing: each such project is asked
/// about once more.
#[derive(Debug)]
pub struct FileTrust {
    path: PathBuf,
    map: Mutex<BTreeMap<String, TrustRecord>>,
    /// Why the file could not be read, if it could not.
    pub error: Option<String>,
}

impl FileTrust {
    /// The book under `config_dir`.
    #[must_use]
    pub fn open(config_dir: &Path) -> Self {
        let path = config_dir.join(TRUST_FILE);
        let (map, error) = match crate::user_config::read_optional(&path) {
            Ok(None) => (BTreeMap::new(), None),
            Ok(Some(text)) => match ron::from_str::<BTreeMap<String, TrustRecord>>(&text) {
                Ok(m) => (m, None),
                Err(e) => match ron::from_str::<BTreeMap<String, Trust>>(&text) {
                    Ok(old) => (
                        old.into_iter()
                            .map(|(k, t)| (k, TrustRecord::new(t, Content::new())))
                            .collect(),
                        None,
                    ),
                    Err(_) => (BTreeMap::new(), Some(format!("{}: {e}", path.display()))),
                },
            },
            Err(e) => (BTreeMap::new(), Some(e.to_string())),
        };
        Self {
            path,
            map: Mutex::new(map),
            error,
        }
    }

    /// The file it keeps.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TrustBook for FileTrust {
    fn get(&self, key: &str) -> Option<TrustRecord> {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }
    fn set(&self, key: &str, record: TrustRecord) -> Result<(), String> {
        let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        map.insert(key.to_string(), record);
        let text = ron::ser::to_string_pretty(&*map, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())?;
        crate::user_config::write_atomic(&self.path, text.as_bytes()).map_err(|e| e.to_string())
    }
    fn describe(&self) -> String {
        forge_ui::trf!("remembered in {path}", path = self.path.display())
    }
}

/// Where a project stands (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustState {
    /// Nobody decided yet: treated as untrusted, and the person is asked.
    Undecided,
    /// Trusted, and it carries nothing the person did not trust.
    Trusted,
    /// Trusted, but it carries something new or changed since (WP-35): what the person
    /// trusted runs, what changed waits, and the person is asked again.
    Changed,
    Untrusted,
}

impl TrustState {
    /// From a book's `record`, for a project carrying `content` now.
    #[must_use]
    pub fn of(record: Option<&TrustRecord>, content: &Content) -> Self {
        match record {
            None => Self::Undecided,
            Some(r) if r.trust == Trust::Untrusted => Self::Untrusted,
            Some(r) => {
                if content.iter().all(|(item, d)| r.approves(item, d)) {
                    Self::Trusted
                } else {
                    Self::Changed
                }
            }
        }
    }

    /// Whether everything it carries runs and takes effect.
    #[must_use]
    pub fn trusted(self) -> bool {
        self == Self::Trusted
    }

    /// Whether the person is asked (nobody decided, or it changed since they trusted it).
    #[must_use]
    pub fn asks(self) -> bool {
        matches!(self, Self::Undecided | Self::Changed)
    }
}

/// One thing a project carries that needs trust (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarriedItem {
    /// Its name: `plugins/<name>`, `plugin-cache/<id>`, or the setting key of a grant or of a
    /// default automation capability.
    pub item: String,
    /// What it is now: the digest of a plugin folder's manifest and code, the version a
    /// cached plugin is named at, `on` for a grant or a capability.
    pub digest: String,
    /// The line the person reads.
    pub line: String,
    /// Code (a plugin), not a grant.
    pub code: bool,
}

/// The digest of a plugin folder: BLAKE3 over its manifest and its code, each with its length
/// (so no split of the bytes gives another folder the same digest). `None`: no manifest (or
/// the files could not be read).
#[must_use]
pub fn plugin_dir_digest(dir: &Path) -> Option<String> {
    forge_wasm::PluginFiles::read(dir)
        .ok()
        .map(|f| plugin_files_digest(&f))
}

/// [`plugin_dir_digest`] of files already read (WP-36): the digest of exactly the bytes a
/// host compiles from them ([`forge_wasm::WasmHost::load_files`]).
#[must_use]
pub fn plugin_files_digest(files: &forge_wasm::PluginFiles) -> String {
    let manifest = files.manifest_bytes();
    let mut h = blake3::Hasher::new();
    h.update(b"forge.trust.plugin-dir.v1\0");
    h.update(&(manifest.len() as u64).to_le_bytes());
    h.update(manifest);
    match files.code() {
        Some((name, code)) => {
            h.update(name.as_bytes());
            h.update(b"\0");
            h.update(&(code.len() as u64).to_le_bytes());
            h.update(code);
        }
        None => {
            h.update(b"no code\0");
        }
    }
    h.finalize().to_hex().to_string()
}

/// The item of the plugin folder `dir` (`plugins/<name>`) holding `files`.
#[must_use]
pub fn files_item(dir: &Path, files: &forge_wasm::PluginFiles) -> Option<CarriedItem> {
    let name = dir.file_name().and_then(|n| n.to_str())?;
    let item = format!("{}/{name}", crate::hosting::PLUGINS_DIR);
    Some(CarriedItem {
        digest: plugin_files_digest(files),
        line: item.clone(),
        item,
        code: true,
    })
}

/// The item of the plugin folder `dir` (`plugins/<name>`), `None` without a manifest.
#[must_use]
pub fn dir_item(dir: &Path) -> Option<CarriedItem> {
    let name = dir.file_name().and_then(|n| n.to_str())?;
    let item = format!("{}/{name}", crate::hosting::PLUGINS_DIR);
    Some(CarriedItem {
        digest: plugin_dir_digest(dir)?,
        line: item.clone(),
        item,
        code: true,
    })
}

/// The item of plugin `id` named at `version` from the plugin cache.
#[must_use]
pub fn cache_item(id: &str, version: &str) -> CarriedItem {
    CarriedItem {
        item: format!("{PLUGIN_CACHE_ITEM}{id}"),
        digest: version.to_string(),
        line: forge_ui::trf!("{id} {version} (from the plugin cache)", id, version),
        code: true,
    }
}

/// The items `settings` (a project's, or only its security settings) carry: the plugins its
/// set names from the plugin cache, the plugin grants it makes, and the default automation session
/// capabilities its policy adds to the standard one.
#[must_use]
pub fn carried_settings(settings: &BTreeMap<String, Value>) -> Vec<CarriedItem> {
    let mut out: Vec<CarriedItem> = security::plugin_set(
        settings
            .iter()
            .filter(|(k, _)| k.starts_with(security::PLUGINS_PREFIX))
            .map(|(k, v)| (k.as_str(), v)),
    )
    .into_iter()
    .filter(|(_, e)| e.enabled && !e.version.is_empty())
    .map(|(id, e)| cache_item(&id, &e.version))
    .collect();
    out.extend(
        settings
            .iter()
            .filter(|(k, _)| k.starts_with(security::PLUGIN_GRANTS_PREFIX))
            .filter_map(|(k, v)| {
                let (who, cap) = security::parse_grant_key(k)?;
                security::value_grants(who.kind(), Some(v), "").then(|| CarriedItem {
                    item: k.clone(),
                    digest: "on".into(),
                    line: forge_ui::trf!("grant {cap} to {who}", cap, who),
                    code: false,
                })
            }),
    );
    out.extend(
        security::policy_widening(settings)
            .into_iter()
            .map(|c| CarriedItem {
                item: format!("{}.{}", security::AUTOMATION_POLICY_PREFIX, c.key()),
                digest: "on".into(),
                line: forge_ui::trf!(
                    "default automation capability {c} (wider than the standard policy)",
                    c
                ),
                code: false,
            }),
    );
    out
}

/// The security settings of `settings` (what [`carried_settings`] reads).
#[must_use]
pub fn security_settings<'a>(
    settings: impl Iterator<Item = (&'a str, &'a Value)>,
) -> BTreeMap<String, Value> {
    settings
        .filter(|(k, _)| {
            k.starts_with(security::PLUGINS_PREFIX) || security::project_security_setting(k)
        })
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// The item a changed setting `key` belongs to: a grant's key, `plugin-cache/<id>` for a
/// plugin-set field; `None` for the automation policy (every capability's item follows it) and
/// for any other setting.
#[must_use]
pub fn item_of_setting(key: &str) -> Option<String> {
    if key.starts_with(security::PLUGIN_GRANTS_PREFIX) {
        return Some(key.to_string());
    }
    let rest = key
        .strip_prefix(security::PLUGINS_PREFIX)?
        .strip_prefix('.')?;
    let (seg, _) = rest.split_once('.')?;
    security::decode_id(seg).map(|id| format!("{PLUGIN_CACHE_ITEM}{id}"))
}

/// Whether setting `key` is part of the default automation policy.
#[must_use]
pub fn is_policy_setting(key: &str) -> bool {
    key.strip_prefix(security::AUTOMATION_POLICY_PREFIX)
        .is_some_and(|r| r.starts_with('.'))
}

/// The security settings a person trusted of a project (`content`: a [`TrustRecord`]'s): its
/// plugin grants, and the standard automation policy with the capabilities they trusted added —
/// what a load of a project that changed since holds its new grants and capabilities against.
#[must_use]
pub fn approved_settings(content: &Content) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Value> = content
        .keys()
        .filter(|k| k.starts_with(security::PLUGIN_GRANTS_PREFIX))
        .map(|k| (k.clone(), Value::Bool(true)))
        .collect();
    let standard: std::collections::BTreeSet<forge_plugin::Capability> =
        security::standard_automation_policy()
            .capabilities()
            .collect();
    for c in forge_plugin::Capability::ALL
        .into_iter()
        .filter(|c| c.may_be_default())
    {
        let key = format!("{}.{}", security::AUTOMATION_POLICY_PREFIX, c.key());
        let on = standard.contains(&c) || content.contains_key(&key);
        out.insert(key, Value::Bool(on));
    }
    out
}

/// What the project at `location` with `settings` carries (see the module docs): its plugin
/// folders (each read once, for its digest; nothing is run) and what its settings carry.
#[must_use]
pub fn carried(location: &str, settings: &BTreeMap<String, Value>) -> Vec<CarriedItem> {
    let mut out: Vec<CarriedItem> = carried_plugin_dirs(location)
        .into_iter()
        .filter_map(|d| dir_item(&d))
        .collect();
    out.extend(carried_settings(settings));
    out
}

/// Where a project stands with what it carries: the Plugin manager's *Project trust* group
/// and the shell's prompt read it. `None` from the core when the open project carries
/// nothing to run or grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectTrust {
    /// Changes whenever any of this does (a panel follows it without cloning).
    pub id: u64,
    /// The project ([`trust_key`]).
    pub project: String,
    pub state: TrustState,
    /// The plugins it carries: `plugins/<name>` folders, and plugins its set names from the
    /// plugin cache.
    pub plugins: Vec<String>,
    /// The plugin grants its files make, and the default automation capabilities its policy adds
    /// to the standard one, one line each.
    pub grants: Vec<String>,
    /// What changed since the person trusted it ([`TrustState::Changed`]): one line each,
    /// saying whether it is new or changed.
    pub changed: Vec<String>,
    /// Everything it carries, by item: what an answer records.
    pub items: BTreeMap<String, CarriedItem>,
}

impl ProjectTrust {
    /// The question about the project `key` carrying `items`, standing at `state`, given
    /// what the person trusted before (`approved`, for the lines of what changed).
    #[must_use]
    pub fn new(
        id: u64,
        key: &str,
        state: TrustState,
        items: BTreeMap<String, CarriedItem>,
        approved: Option<&Content>,
    ) -> Self {
        let mut plugins = Vec::new();
        let mut grants = Vec::new();
        let mut changed = Vec::new();
        for it in items.values() {
            if it.code {
                plugins.push(it.line.clone());
            } else {
                grants.push(it.line.clone());
            }
            if state == TrustState::Changed {
                match approved.and_then(|a| a.get(&it.item)) {
                    Some(d) if *d == it.digest => {}
                    Some(_) => changed.push(forge_ui::trf!(
                        "{what} changed since you trusted the project",
                        what = it.line
                    )),
                    None => changed.push(forge_ui::trf!(
                        "{what} is new since you trusted the project",
                        what = it.line
                    )),
                }
            }
        }
        Self {
            id,
            project: key.to_string(),
            state,
            plugins,
            grants,
            changed,
            items,
        }
    }

    /// What an answer records: item → digest.
    #[must_use]
    pub fn content(&self) -> Content {
        self.items
            .iter()
            .map(|(k, it)| (k.clone(), it.digest.clone()))
            .collect()
    }

    /// The prompt's and the panel's heading line.
    #[must_use]
    pub fn label(&self) -> String {
        let what = match (self.plugins.len(), self.grants.len()) {
            (0, g) => grant_count(g),
            (p, 0) => plugin_count(p),
            (p, g) => forge_ui::trf!(
                "{plugins} and {grants}",
                plugins = plugin_count(p),
                grants = grant_count(g)
            ),
        };
        match self.state {
            TrustState::Trusted => {
                forge_ui::trf!("This project is trusted: it carries {what}.", what)
            }
            TrustState::Untrusted => forge_ui::trf!(
                "This project is not trusted: it carries {what}, which stay off (its plugins are not loaded and its grants are held).",
                what
            ),
            TrustState::Undecided => forge_ui::trf!(
                "Do you trust this project? It carries {what}. Until you decide, its plugins are not loaded and its grants are held.",
                what
            ),
            TrustState::Changed => forge_ui::trf!(
                "This project changed since you trusted it: {changes}. What you trusted keeps running; until you trust the changes, a new or changed plugin is not loaded (a running one keeps the code you trusted) and a new grant is held.",
                changes = change_count(self.changed.len())
            ),
        }
    }

    /// One line per thing it carries, what changed first.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.changed
            .iter()
            .cloned()
            .chain(self.plugins.iter().map(|p| forge_ui::trf!("plugin {p}", p)))
            .chain(self.grants.iter().cloned())
            .collect()
    }
}

/// "1 grant" or "4 grants".
fn grant_count(n: usize) -> String {
    if n == 1 {
        forge_ui::tr!("1 grant").into()
    } else {
        forge_ui::trf!("{n} grants", n)
    }
}

/// "1 plugin" or "4 plugins".
fn plugin_count(n: usize) -> String {
    if n == 1 {
        forge_ui::tr!("1 plugin").into()
    } else {
        forge_ui::trf!("{n} plugins", n)
    }
}

/// "1 change" or "4 changes".
fn change_count(n: usize) -> String {
    if n == 1 {
        forge_ui::tr!("1 change").into()
    } else {
        forge_ui::trf!("{n} changes", n)
    }
}

/// The plugin folders the project at `location` carries: the `plugins/<name>/` folders
/// holding a manifest (only the directory is listed; nothing is read or run), sorted.
#[must_use]
pub fn carried_plugin_dirs(location: &str) -> Vec<PathBuf> {
    let Some(root) = crate::hosting::PluginDirs::project_dir(location) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("plugin.ron").is_file())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(pairs: &[(&str, &str)]) -> Content {
        pairs
            .iter()
            .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
            .collect()
    }

    #[test]
    fn a_file_book_remembers_across_opens_and_a_bad_file_trusts_nothing() {
        let dir = std::env::temp_dir().join(format!("forge-trust-book-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let b = FileTrust::open(&dir);
        assert_eq!(b.get("file:x"), None);
        let x = TrustRecord::new(Trust::Trusted, content(&[("plugins/a", "d1")]));
        b.set("file:x", x.clone()).unwrap_or_else(|e| panic!("{e}"));
        b.set("file:y", TrustRecord::new(Trust::Untrusted, Content::new()))
            .unwrap_or_else(|e| panic!("{e}"));
        let again = FileTrust::open(&dir);
        assert_eq!(again.get("file:x"), Some(x));
        assert_eq!(again.get("file:y").map(|r| r.trust), Some(Trust::Untrusted));
        std::fs::write(dir.join(TRUST_FILE), "not ron {").unwrap_or_else(|e| panic!("{e}"));
        let bad = FileTrust::open(&dir);
        assert_eq!(bad.get("file:x"), None);
        assert!(bad.error.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A WP-34 file (bare answers) still reads; its answers cover no content, so a trusted
    /// project that carries anything is asked about once more.
    #[test]
    fn a_wp34_file_reads_as_answers_that_cover_nothing() {
        let dir = std::env::temp_dir().join(format!("forge-trust-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(
            dir.join(TRUST_FILE),
            "{\"file:x\": Trusted, \"file:y\": Untrusted}",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let b = FileTrust::open(&dir);
        assert!(b.error.is_none(), "{:?}", b.error);
        let carried = content(&[("plugins/a", "d1")]);
        assert_eq!(b.state("file:x", &carried), TrustState::Changed);
        assert_eq!(b.state("file:x", &Content::new()), TrustState::Trusted);
        assert_eq!(b.state("file:y", &carried), TrustState::Untrusted);
        assert_eq!(b.state("file:z", &carried), TrustState::Undecided);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same content, or less, is trusted; a new item or a changed digest is not.
    #[test]
    fn a_trusted_answer_covers_exactly_what_was_trusted() {
        let r = TrustRecord::new(
            Trust::Trusted,
            content(&[("plugins/a", "d1"), ("security.grants.plugin.x.y", "on")]),
        );
        let s = |c: &[(&str, &str)]| TrustState::of(Some(&r), &content(c));
        assert_eq!(s(&[("plugins/a", "d1")]), TrustState::Trusted);
        assert_eq!(s(&[]), TrustState::Trusted);
        assert_eq!(s(&[("plugins/a", "d2")]), TrustState::Changed);
        assert_eq!(
            s(&[("plugins/a", "d1"), ("plugins/b", "d3")]),
            TrustState::Changed
        );
        let q = ProjectTrust::new(
            1,
            "file:x",
            TrustState::Changed,
            [
                CarriedItem {
                    item: "plugins/a".into(),
                    digest: "d2".into(),
                    line: "plugins/a".into(),
                    code: true,
                },
                CarriedItem {
                    item: "plugins/b".into(),
                    digest: "d3".into(),
                    line: "plugins/b".into(),
                    code: true,
                },
            ]
            .into_iter()
            .map(|i| (i.item.clone(), i))
            .collect(),
            Some(&r.content),
        );
        assert_eq!(q.changed.len(), 2, "{q:?}");
        assert!(q.changed[0].contains("changed since"), "{q:?}");
        assert!(q.changed[1].contains("new since"), "{q:?}");
    }

    #[test]
    fn a_plugin_folders_digest_follows_its_manifest_and_its_code() {
        let dir = std::env::temp_dir().join(format!("forge-trust-digest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(plugin_dir_digest(&dir), None);
        std::fs::write(dir.join("plugin.ron"), "m").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join("plugin.wat"), "(module)").unwrap_or_else(|e| panic!("{e}"));
        let a = plugin_dir_digest(&dir);
        assert!(a.is_some());
        assert_eq!(
            plugin_dir_digest(&dir),
            a,
            "the digest is a pure function of the files"
        );
        std::fs::write(dir.join("plugin.wat"), "(module )").unwrap_or_else(|e| panic!("{e}"));
        let b = plugin_dir_digest(&dir);
        assert_ne!(a, b, "changed code");
        std::fs::write(dir.join("plugin.ron"), "m2").unwrap_or_else(|e| panic!("{e}"));
        assert_ne!(plugin_dir_digest(&dir), b, "changed manifest");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_are_canonical_folders() {
        let dir = std::env::temp_dir().join(format!("forge-trust-key-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("p")).unwrap_or_else(|e| panic!("{e}"));
        let a = trust_key(&format!("file:{}", dir.join("p").display()));
        let b = trust_key(&format!(
            "file:{}",
            dir.join("p").join("..").join("p").display()
        ));
        assert_eq!(a, b);
        assert_eq!(trust_key("memory:demo"), "memory:demo");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
