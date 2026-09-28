//! The **security-class commands** (Ch.21 §21.18, WP-U9): the project's plugin set, plugin
//! and automation capability grants, the default automation capability policy, and a human's
//! approval or rejection of an automation session's preview transaction.
//!
//! **Every one is a command on the bus (I7)**, registered by the core like any other, so it
//! is provenance-tagged, in the bus's audit trail and visible to every client. Their state
//! is ordinary project settings:
//!
//! | Key | Value | Set by |
//! |---|---|---|
//! | `security.grants.plugin.<id>.<cap>` | `Bool(true)` granted, `Bool(false)` revoked | `forge.plugin.grant` / `forge.plugin.revoke` |
//! | `security.grants.automation.<id>.<cap>` | `Text(<epoch>)` granted, `Bool(false)` revoked | `forge.automation.grant` / `forge.automation.revoke` |
//! | `security.automation_policy.<cap>` | `Bool` | `forge.automation.default_policy` |
//! | `plugins.<id>.enabled` / `.version` / `.source` | `Bool` / `Text` / `Text` | `forge.plugin.add` / `remove` / `set_enabled` |
//!
//! `<id>` is the principal's id escaped into one key segment ([`encode_id`]); `<cap>` is
//! [`Capability::key`]. **The grant table is derived from these settings**: the core runs
//! [`GrantSync`] over every change it applies — a command, an undo, a redo, a cancel, a
//! project load — and grants or revokes in the one [`SharedGrants`] table the WASM host and
//! every session host checks (E-27). So a grant a human makes in a panel reaches a running
//! automation session or plugin at its next call, and nothing else can grant: the table
//! follows the project, and only these commands write those keys.
//!
//! **Only these commands write those keys: the bus enforces it.** The core reserves the
//! `security` and `plugins` prefixes on its bus ([`RESERVED_SETTINGS`], through
//! `Bus::reserve_settings`): a command whose diff changes a key under them is refused with
//! `CMD-0014`, nothing applied, unless it is one of that prefix's writers. So a plain
//! `SetSetting security.grants.…` (or a batch holding one, a plugin command, a
//! `#[forge_api]` command) grants nothing, from anyone. The one other writer is the project
//! load (`forge.project.load`), because plugin grants ship with the project: the core
//! performs it when a project is opened, created or pulled, it is never undone or redone,
//! and the core refuses a load sent directly by anything but a human (an automation session could
//! otherwise load a document of its own with a grant in it).
//!
//! **Two rules (§21.18).**
//! * **Only a human issues a security command** (`CommandPolicy::human_only`): an automation
//!   session or a script is refused with `CMD-0014` before anything is planned. An automation
//!   session may *request* a grant; a host may keep a denied call as a request a person answers.
//! * **Undo and redo never grant.** A grant is undoable (its undo revokes) but not
//!   redoable; a revoke and a policy change are not undoable. A capability is only ever
//!   granted by a fresh human command.
//!
//! **Automation grants do not outlive the editor.** An automation session id (`auto-3`) is reused
//! by the next server, so a saved `auto-3` grant must not grant the next `auto-3`: an automation
//! session grant's value is this process's [`epoch`], and [`GrantSync`] honours only its own epoch.
//! And **no load writes one** (WP-20): the core's project load ([`plan_load`]) and the
//! team-baseline sync ([`plan_sync`]) drop every `security.grants.automation.*` key, so an
//! automation session that wrote a project file with its own grant and this run's epoch in it, then
//! opened it, holds nothing; a session host never shows an automation session `security.*` either.
//! Plugin grants are meant to ship with the project (every teammate gets the same sandbox, Ch.32
//! §32.4) and are plain booleans. **No save writes an automation grant** either (WP-33): the core
//! tells its project host to keep [`per_run_setting`] keys out of every file.
//!
//! **A non-human's load never widens project security (WP-33).** Plugin grants and the
//! default automation policy ship with a project, so a load writes them; but when an automation
//! session or a script opened, created, cloned or pulled the project, a plugin grant or a default
//! automation capability the open project does not have is held back ([`held_changes`]):
//! the load writes what is in effect now, and the file's values wait as a [`HeldSecurity`] proposal
//! a person accepts ([`ACCEPT_HELD_CMD`]) or discards ([`DISCARD_HELD_CMD`]) in the Plugin manager,
//! the Plugin manager or the `--remote-host` console (WP-21) — human-only commands the core
//! checks against the proposal it holds. A restriction takes effect, and when a non-human made it
//! the core audits it as `narrowed` ([`narrowed_changes`], WP-21) so a person can see what an
//! automation session or a script took away. A person's load is unchanged. The same holds for a
//! team pull an automation session asked for (`forge.collab.pull`). **Holding never changes the
//! files:** until the person answers, every save, build and team publish writes the held keys as
//! the file held them (`forge_project::host::ProjectHost::set_file_settings`), so no commit reverts
//! a teammate's grants. A headless run accepts only when its launcher passed
//! `--accept-project-security`.
//!
//! The plugin set (add, remove, enable, disable) is ordinary undoable project state: it
//! changes which plugins the project loads, for every teammate and every build, and it never
//! grants anything (a plugin's grants are their own keys).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use forge_cmd::{
    Change, CmdError, CommandPolicy, DiffBuilder, EditorCommand, Project, TxnId, Value,
};
use forge_plugin::{Capability, DefaultPolicy, PluginId, Principal, PrincipalKind, SharedGrants};
use serde_json::{Value as Json, json};

/// Grant a capability to an automation session (human-only; undo revokes, never redone).
pub const AUTOMATION_GRANT_CMD: &str = "forge.automation.grant";
/// Revoke a capability from an automation session (human-only; never undone).
pub const AUTOMATION_REVOKE_CMD: &str = "forge.automation.revoke";
/// Grant a capability to a plugin (human-only; undo revokes, never redone).
pub const PLUGIN_GRANT_CMD: &str = "forge.plugin.grant";
/// Revoke a capability from a plugin (human-only; never undone).
pub const PLUGIN_REVOKE_CMD: &str = "forge.plugin.revoke";
/// Set the project's default automation capability policy (human-only; never undone).
pub const AUTOMATION_POLICY_CMD: &str = "forge.automation.default_policy";
/// Approve an automation session's pending preview transaction: the core commits it (human-only).
pub const AUTOMATION_APPROVE_CMD: &str = "forge.automation.approve";
/// Reject an automation session's pending preview transaction: the core cancels it (human-only).
pub const AUTOMATION_REJECT_CMD: &str = "forge.automation.reject";
/// Accept the security settings a non-human's load held back (human-only; never undone).
pub const ACCEPT_HELD_CMD: &str = "forge.security.accept_held";
/// Discard the security settings a non-human's load held back (human-only).
pub const DISCARD_HELD_CMD: &str = "forge.security.discard_held";
/// Add a plugin to the project's plugin set (the `forge add` equivalent; undoable).
pub const PLUGIN_ADD_CMD: &str = "forge.plugin.add";
/// Remove a plugin from the project's plugin set (undoable).
pub const PLUGIN_REMOVE_CMD: &str = "forge.plugin.remove";
/// Enable or disable a plugin in the project (undoable).
pub const PLUGIN_ENABLE_CMD: &str = "forge.plugin.set_enabled";

/// Where grants live: `security.grants.<plugin|automation>.<id>.<cap>`.
pub const GRANTS_PREFIX: &str = "security.grants";
/// The default automation capability policy: `security.automation_policy.<cap>`.
pub const AUTOMATION_POLICY_PREFIX: &str = "security.automation_policy";
/// The project's plugin set: `plugins.<id>.enabled|version|source`.
pub const PLUGINS_PREFIX: &str = "plugins";

/// Every grant and the default automation policy live under this prefix (reserved).
pub const SECURITY_PREFIX: &str = "security";

/// The setting prefixes the core reserves on its bus, each with the only `Invoke` targets
/// that may change a key under it (see the module docs). `forge.project.load` replaces the
/// whole project, security state included, and is performed by the core.
pub const RESERVED_SETTINGS: &[(&str, &[&str])] = &[
    (
        SECURITY_PREFIX,
        &[
            AUTOMATION_GRANT_CMD,
            AUTOMATION_REVOKE_CMD,
            PLUGIN_GRANT_CMD,
            PLUGIN_REVOKE_CMD,
            AUTOMATION_POLICY_CMD,
            ACCEPT_HELD_CMD,
            forge_project::format::LOAD_CMD,
        ],
    ),
    (
        PLUGINS_PREFIX,
        &[
            PLUGIN_ADD_CMD,
            PLUGIN_REMOVE_CMD,
            PLUGIN_ENABLE_CMD,
            forge_project::format::LOAD_CMD,
        ],
    ),
];

/// The commands a human approves or rejects automation work with: the core performs them.
pub const REVIEW_TARGETS: &[&str] = &[AUTOMATION_APPROVE_CMD, AUTOMATION_REJECT_CMD];

/// The commands a human answers held security settings with (WP-33): the core checks them
/// against the proposal it holds and forgets it once the bus accepted one.
pub const HELD_TARGETS: &[&str] = &[ACCEPT_HELD_CMD, DISCARD_HELD_CMD];

/// Every security-class target (the settings window's and the guards' list).
pub const SECURITY_TARGETS: &[&str] = &[
    AUTOMATION_GRANT_CMD,
    AUTOMATION_REVOKE_CMD,
    PLUGIN_GRANT_CMD,
    PLUGIN_REVOKE_CMD,
    AUTOMATION_POLICY_CMD,
    AUTOMATION_APPROVE_CMD,
    AUTOMATION_REJECT_CMD,
    ACCEPT_HELD_CMD,
    DISCARD_HELD_CMD,
];

/// A grant: human-only, undoable (the undo revokes), never redone.
pub const GRANT_POLICY: CommandPolicy = CommandPolicy {
    human_only: true,
    undoable: true,
    redoable: false,
};
/// A revoke, a policy change, an approval or a rejection: human-only, never undone (an undo
/// of a revoke would grant).
pub const FINAL_POLICY: CommandPolicy = CommandPolicy {
    human_only: true,
    undoable: false,
    redoable: false,
};

/// The default automation policy when the project sets none: read the project and send ordinary
/// (undoable, non-destructive) commands — what a session host pre-grants when the project sets
/// none.
pub fn standard_automation_policy() -> DefaultPolicy {
    DefaultPolicy::new([
        Capability::Fs(forge_plugin::FsScope::ProjectRead),
        Capability::Command(forge_plugin::CommandClass::Ordinary),
    ])
    .unwrap_or_default()
}

/// This process's automation-grant epoch (see the module docs): different in every run.
pub fn epoch() -> &'static str {
    static EPOCH: OnceLock<String> = OnceLock::new();
    EPOCH.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("run-{nanos:x}-{:x}", std::process::id())
    })
}

/// Where automation grants live: `security.grants.automation.<id>.<cap>`.
pub const AUTOMATION_GRANTS_PREFIX: &str = "security.grants.automation.";

/// Is `key` per-run state no file, pull or teammate's baseline may bring in? Automation grants:
/// a session exists only in this run, and a grant's value is this run's epoch — so a project
/// file that carries one (written by an automation session a human let write files, with the epoch
/// it read somewhere) would otherwise grant that automation session whatever it wrote, because the
/// load is an allowed writer of `security.*` (WP-20, the WP-U9 verifier's escalation).
#[must_use]
pub fn per_run_setting(key: &str) -> bool {
    key.starts_with(AUTOMATION_GRANTS_PREFIX)
}

/// Where plugin grants live: `security.grants.plugin.<id>.<cap>`.
pub const PLUGIN_GRANTS_PREFIX: &str = "security.grants.plugin.";

/// Is `key` security state a project carries by design — a plugin grant or the default automation
/// session policy — which a load a non-human started may not widen without a person's decision
/// (WP-33, [`held_changes`])?
#[must_use]
pub fn project_security_setting(key: &str) -> bool {
    key.starts_with(PLUGIN_GRANTS_PREFIX)
        || key
            .strip_prefix(AUTOMATION_POLICY_PREFIX)
            .is_some_and(|r| r.starts_with('.'))
}

/// One security setting a project file carries that did not take effect because a
/// non-human opened, created, cloned or pulled the project (WP-33).
#[derive(Clone, Debug, PartialEq)]
pub struct HeldSetting {
    pub key: String,
    /// What the file holds (`None`: the file has no such key).
    pub file: Option<Value>,
    /// What the loaded project holds instead (`None`: no such key).
    pub in_effect: Option<Value>,
}

impl HeldSetting {
    /// One line for the panel and the audit: what accepting it would change.
    // l10n-block: also the audit record's text, kept stable in the source language
    #[must_use]
    pub fn line(&self) -> String {
        let on = |v: &Option<Value>| matches!(v, Some(Value::Bool(true)));
        if let Some((who, cap)) = parse_grant_key(&self.key) {
            return format!("grant {cap} to {who} (the file grants it; not granted now)");
        }
        let cap = self
            .key
            .strip_prefix(AUTOMATION_POLICY_PREFIX)
            .and_then(|r| r.strip_prefix('.'))
            .and_then(Capability::from_key)
            .map_or_else(|| self.key.clone(), |c| c.to_string());
        match (&self.file, on(&self.file), on(&self.in_effect)) {
            (None, _, _) => format!(
                "default automation capability {cap}: the file sets no policy (the standard one); {} now",
                if on(&self.in_effect) { "on" } else { "off" }
            ),
            (_, f, n) => format!(
                "default automation capability {cap}: {} in the file, {} now",
                if f { "on" } else { "off" },
                if n { "on" } else { "off" }
            ),
        }
    }
}

/// Security settings a non-human's load held back for a person (WP-33): accepted by
/// [`accept_held_command`], discarded by [`discard_held_command`], both human-only.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldSecurity {
    /// Names this proposal (a later load replaces it; the commands name the one they answer).
    pub id: u64,
    /// Who started the load (`automation:auto-3`, `script:<path>`).
    pub by: String,
    /// The project it came from (its location or remote).
    pub source: String,
    pub items: Vec<HeldSetting>,
    /// Held (at least partly) because the project is not trusted (WP-34, [`crate::trust`]):
    /// its plugin grants wait for the person's trust, not only for a person's look at what a
    /// non-human opened.
    pub untrusted: bool,
}

impl HeldSecurity {
    /// The panel's heading line. Trusting the project accepts only what a person's own open
    /// held ([`crate::core::EditorCore::decide_trust`]); what an automation session's or a script's
    /// open held waits for a person's Accept or Discard either way (ADR 0043 Amendment 1).
    #[must_use]
    pub fn label(&self) -> String {
        let (by, source, n) = (&self.by, &self.source, self.items.len());
        if self.untrusted && self.by.starts_with("human:") {
            forge_ui::trf!(
                "{by} opened {source}: {n} security setting(s) held until a person trusts the project, or accepts or discards them",
                by,
                source,
                n
            )
        } else if self.untrusted {
            forge_ui::trf!(
                "{by} opened {source}: {n} security setting(s) held until a person accepts or discards them (trusting the project does not accept them)",
                by,
                source,
                n
            )
        } else {
            forge_ui::trf!(
                "{by} opened {source}: {n} security setting(s) held until a person accepts or discards them",
                by,
                source,
                n
            )
        }
    }

    fn settings_json(&self) -> Json {
        Json::Array(
            self.items
                .iter()
                .map(|i| json!({"key": i.key, "value": i.file}))
                .collect(),
        )
    }
}

/// The capabilities of the default automation policy `setting` describes (the standard one when
/// it sets none).
fn policy_caps<'a>(setting: impl FnMut(&str) -> Option<&'a Value>) -> BTreeSet<Capability> {
    automation_policy(setting)
        .unwrap_or_else(standard_automation_policy)
        .capabilities()
        .collect()
}

/// The default automation capabilities `settings`' policy holds that the standard policy does
/// not: what a project that is not trusted would widen for every automation session (WP-34, ADR
/// 0045 Amendment 1 — held like its plugin grants until a person trusts it).
#[must_use]
pub fn policy_widening(settings: &BTreeMap<String, Value>) -> Vec<Capability> {
    let standard: BTreeSet<Capability> = standard_automation_policy().capabilities().collect();
    policy_caps(|k| settings.get(k))
        .into_iter()
        .filter(|c| !standard.contains(c))
        .collect()
}

/// What a load of a document with `settings`, started by a non-human, may not change without
/// a person (WP-33): every plugin grant the file makes that `current` does not, and — when
/// the file's default automation policy holds a capability `current`'s does not — the policy
/// keys, which then take the intersection of the two policies. Each item's `in_effect` is what the
/// load writes instead; a restriction (a grant the file lacks or revokes, a policy with fewer
/// capabilities) is not held: it takes effect.
#[must_use]
pub fn held_changes(current: &Project, settings: &BTreeMap<String, Value>) -> Vec<HeldSetting> {
    held_changes_by(|k| current.setting(k), settings)
}

/// [`held_changes`] against a baseline read through `current` (WP-35: what a person trusted
/// of a project that changed since, [`crate::trust`]).
#[must_use]
pub fn held_changes_by<'a>(
    current: impl Fn(&str) -> Option<&'a Value>,
    settings: &BTreeMap<String, Value>,
) -> Vec<HeldSetting> {
    let mut out = held_plugin_grants_by(&current, settings);
    let now = policy_caps(&current);
    let file = policy_caps(|k| settings.get(k));
    if !file.is_subset(&now) {
        for c in Capability::ALL.into_iter().filter(|c| c.may_be_default()) {
            let key = format!("{AUTOMATION_POLICY_PREFIX}.{}", c.key());
            let in_file = settings.get(&key).cloned();
            let in_effect = Some(Value::Bool(file.contains(&c) && now.contains(&c)));
            // The load writes the intersection; a key the file already holds that way stays.
            if in_file != in_effect {
                out.push(HeldSetting {
                    key,
                    file: in_file,
                    in_effect,
                });
            }
        }
    }
    out
}

/// Every plugin grant `settings` makes that `baseline` does not, with what `baseline` holds
/// as what takes effect instead: the plugin-grant half of [`held_changes`], and what a
/// project that is not trusted holds (WP-34, [`crate::trust`]) — against an empty project
/// for a load (no grant of an untrusted project takes effect, whatever the project before it
/// granted), against the project in memory for a team pull.
#[must_use]
pub fn held_plugin_grants(
    baseline: &Project,
    settings: &BTreeMap<String, Value>,
) -> Vec<HeldSetting> {
    held_plugin_grants_by(&|k: &str| baseline.setting(k), settings)
}

fn held_plugin_grants_by<'a>(
    baseline: &impl Fn(&str) -> Option<&'a Value>,
    settings: &BTreeMap<String, Value>,
) -> Vec<HeldSetting> {
    let mut out = Vec::new();
    for (k, file) in settings.range::<str, _>((
        std::ops::Bound::Included(PLUGIN_GRANTS_PREFIX),
        std::ops::Bound::Unbounded,
    )) {
        if !k.starts_with(PLUGIN_GRANTS_PREFIX) {
            break;
        }
        let Some((who, _)) = parse_grant_key(k) else {
            continue; // grants nothing
        };
        let now = baseline(k);
        if value_grants(who.kind(), Some(file), "") && !value_grants(who.kind(), now, "") {
            out.push(HeldSetting {
                key: k.clone(),
                file: Some(file.clone()),
                in_effect: now.cloned(),
            });
        }
    }
    out
}

/// What a load of a document with `settings` restricts (WP-21, ADR 0043 Amendment 2's
/// follow-up): every plugin grant `current` makes that the file does not, and every default
/// automation capability `current`'s policy holds that the file's does not. A restriction takes
/// effect at once — it can only make things safer — but when a non-human started the load a
/// person must be able to see it, so the core audits these lines as `narrowed`. One line
/// each, in key order.
#[must_use]
pub fn narrowed_changes(current: &Project, settings: &BTreeMap<String, Value>) -> Vec<String> {
    let mut out = Vec::new();
    for (k, now) in current.settings() {
        if !k.starts_with(PLUGIN_GRANTS_PREFIX) {
            continue;
        }
        let Some((who, cap)) = parse_grant_key(k) else {
            continue;
        };
        if value_grants(who.kind(), Some(now), "") && !value_grants(who.kind(), settings.get(k), "")
        {
            out.push(format!(
                "grant {cap} to {who} dropped (the loaded file does not grant it)"
            ));
        }
    }
    let now = policy_caps(|k| current.setting(k));
    let file = policy_caps(|k| settings.get(k));
    for c in now.difference(&file) {
        out.push(format!(
            "default automation capability {c} turned off (the loaded file's policy lacks it)"
        ));
    }
    out
}

/// `settings` with `held` replaced by what takes effect (see [`held_changes`]).
pub fn apply_held(settings: &mut BTreeMap<String, Value>, held: &[HeldSetting]) {
    for h in held {
        match &h.in_effect {
            Some(v) => settings.insert(h.key.clone(), v.clone()),
            None => settings.remove(&h.key),
        };
    }
}

/// Accept held security settings: a human-only command whose diff writes exactly what the
/// file held (the core checks it names the proposal it holds, item for item).
pub fn accept_held_command(h: &HeldSecurity) -> EditorCommand {
    invoke(
        ACCEPT_HELD_CMD,
        &json!({"id": h.id, "settings": h.settings_json()}),
    )
}

/// Discard held security settings (human-only; changes nothing in the project).
pub fn discard_held_command(h: &HeldSecurity) -> EditorCommand {
    invoke(DISCARD_HELD_CMD, &json!({ "id": h.id }))
}

/// The held proposal an accept / discard names.
pub fn held_id(args: &str) -> Option<u64> {
    serde_json::from_str::<Json>(args).ok()?.get("id")?.as_u64()
}

/// Does an accept's `args` write exactly `h`'s items?
#[must_use]
pub fn accept_matches(args: &str, h: &HeldSecurity) -> bool {
    serde_json::from_str::<Json>(args)
        .ok()
        .and_then(|a| a.get("settings").cloned())
        .is_some_and(|s| s == h.settings_json())
}

fn plan_accept_held(b: &mut DiffBuilder<'_>, a: &Json) -> Result<(), CmdError> {
    a.get("id")
        .and_then(Json::as_u64)
        .ok_or_else(|| bad(ACCEPT_HELD_CMD, "\"id\" must be a number"))?;
    let list = a
        .get("settings")
        .and_then(Json::as_array)
        .ok_or_else(|| bad(ACCEPT_HELD_CMD, "\"settings\" must be a list"))?;
    for item in list {
        let key = text(ACCEPT_HELD_CMD, item, "key")?;
        // Only what a project carries by design: never an automation grant, never anything else.
        if !project_security_setting(key) {
            return Err(bad(
                ACCEPT_HELD_CMD,
                format!("{key} is not a plugin grant or the default automation policy"),
            ));
        }
        let value: Option<Value> =
            serde_json::from_value(item.get("value").cloned().unwrap_or(Json::Null))
                .map_err(|e| bad(ACCEPT_HELD_CMD, format!("{key}: {e}")))?;
        if key.starts_with(AUTOMATION_POLICY_PREFIX)
            && let Some(Value::Bool(true)) = value
            && key
                .rsplit('.')
                .next()
                .and_then(Capability::from_key)
                .is_none_or(|c| !c.may_be_default())
        {
            return Err(bad(
                ACCEPT_HELD_CMD,
                format!("{key}: never in a default policy"),
            ));
        }
        b.set_setting(key, value)?;
    }
    Ok(())
}

fn plan_discard_held(_: &mut DiffBuilder<'_>, a: &Json) -> Result<(), CmdError> {
    // No change of its own: the core forgets the proposal once the bus accepted it.
    a.get("id")
        .and_then(Json::as_u64)
        .map(drop)
        .ok_or_else(|| bad(DISCARD_HELD_CMD, "\"id\" must be a number"))
}

/// The editor's `forge.project.load` planner: the document's settings less every automation session
/// grant ([`per_run_setting`]), and the project's own automation grants cleared (a new project,
/// a new set of grants a human makes in this run).
pub fn plan_load(b: &mut DiffBuilder<'_>, args: &Json) -> Result<(), CmdError> {
    forge_project::format::plan_load_with(b, args, &per_run_setting)
}

/// The core's team-baseline sync (`forge.collab.sync`, WP-U10) the same way: a teammate's
/// baseline never brings an automation grant in (its steps on those keys are dropped).
pub fn plan_sync(b: &mut DiffBuilder<'_>, args: &Json) -> Result<(), CmdError> {
    let mut args = args.clone();
    if let Some(ops) = args.get_mut("ops").and_then(Json::as_array_mut) {
        ops.retain(|op| {
            !op.get("Setting")
                .and_then(|s| s.get("key"))
                .and_then(Json::as_str)
                .is_some_and(per_run_setting)
        });
    }
    forge_project::merge::plan_patch(b, &args)
}

// ---- keys -------------------------------------------------------------------------------

/// Escape an id (`com.example.rivers`, `auto-3`) into one setting-key segment: `id_`, then
/// `a-z 0-9` as they are, `_` as `__`, and every other byte as `_xx` (two lowercase hex
/// digits). Unambiguous, so [`decode_id`] inverts it.
pub fn encode_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len() + 8);
    out.push_str("id_");
    for b in id.bytes() {
        match b {
            b'a'..=b'z' | b'0'..=b'9' => out.push(char::from(b)),
            b'_' => out.push_str("__"),
            _ => out.push_str(&format!("_{b:02x}")),
        }
    }
    out
}

/// The id [`encode_id`] made `seg` from.
pub fn decode_id(seg: &str) -> Option<String> {
    let rest = seg.strip_prefix("id_")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'_' {
            match bytes.get(i + 1) {
                Some(b'_') => {
                    out.push(b'_');
                    i += 2;
                }
                Some(_) => {
                    let hex = rest.get(i + 1..i + 3)?;
                    out.push(u8::from_str_radix(hex, 16).ok()?);
                    i += 3;
                }
                None => return None,
            }
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn kind_segment(p: &Principal) -> Option<(&'static str, &str)> {
    match p {
        Principal::Plugin(id) => Some(("plugin", id.as_str())),
        Principal::Automation(s) => Some(("automation", s.as_str())),
        _ => None,
    }
}

/// The setting key holding `who`'s grant of `cap` (plugins and automation sessions only).
pub fn grant_key(who: &Principal, cap: Capability) -> Option<String> {
    let (kind, id) = kind_segment(who)?;
    Some(format!(
        "{GRANTS_PREFIX}.{kind}.{}.{}",
        encode_id(id),
        cap.key()
    ))
}

/// The principal and capability a grant key names.
pub fn parse_grant_key(key: &str) -> Option<(Principal, Capability)> {
    let rest = key.strip_prefix(GRANTS_PREFIX)?.strip_prefix('.')?;
    let mut it = rest.split('.');
    let (kind, id, cap) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    let id = decode_id(id)?;
    let cap = Capability::from_key(cap)?;
    let who = match kind {
        "plugin" => Principal::Plugin(PluginId::new(&id).ok()?),
        "automation" => Principal::Automation(id),
        _ => return None,
    };
    Some((who, cap))
}

/// Parse `automation:<session>` / `plugin:<id>` (the principal's display form).
pub fn parse_principal(s: &str) -> Option<Principal> {
    let (kind, id) = s.split_once(':')?;
    let id = id.trim();
    match kind.trim() {
        "automation" if !id.is_empty() && id.len() <= 128 => {
            Some(Principal::Automation(id.to_string()))
        }
        "plugin" => PluginId::new(id).ok().map(Principal::Plugin),
        _ => None,
    }
}

/// Whether a setting value grants, for a principal of `kind` (see the module docs).
pub fn value_grants(kind: PrincipalKind, v: Option<&Value>, epoch: &str) -> bool {
    match (kind, v) {
        (PrincipalKind::Plugin, Some(Value::Bool(true))) => true,
        (PrincipalKind::Automation, Some(Value::Text(e))) => e == epoch,
        _ => false,
    }
}

/// The key of a plugin-set field (`enabled`, `version`, `source`).
pub fn plugin_key(id: &str, field: &str) -> String {
    format!("{PLUGINS_PREFIX}.{}.{field}", encode_id(id))
}

// ---- commands ---------------------------------------------------------------------------

fn invoke(target: &str, args: &Json) -> EditorCommand {
    EditorCommand::Invoke {
        target: target.to_string(),
        args: args.to_string(),
    }
}

/// Grant `cap` to `who` (a plugin or an automation session).
pub fn grant_command(who: &Principal, cap: Capability) -> EditorCommand {
    let target = if matches!(who, Principal::Automation(_)) {
        AUTOMATION_GRANT_CMD
    } else {
        PLUGIN_GRANT_CMD
    };
    invoke(
        target,
        &json!({"principal": who.to_string(), "capability": cap.to_string()}),
    )
}

/// Grant `cap` to the plugin with id `id` (checked by the command's planner: a panel names
/// another plugin by the id it shows, it never mints one).
pub fn plugin_grant_command(id: &str, cap: Capability) -> EditorCommand {
    invoke(
        PLUGIN_GRANT_CMD,
        &json!({"principal": format!("plugin:{id}"), "capability": cap.to_string()}),
    )
}

/// Revoke `cap` from the plugin with id `id`.
pub fn plugin_revoke_command(id: &str, cap: Capability) -> EditorCommand {
    invoke(
        PLUGIN_REVOKE_CMD,
        &json!({"principal": format!("plugin:{id}"), "capability": cap.to_string()}),
    )
}

/// Revoke `cap` from `who`.
pub fn revoke_command(who: &Principal, cap: Capability) -> EditorCommand {
    let target = if matches!(who, Principal::Automation(_)) {
        AUTOMATION_REVOKE_CMD
    } else {
        PLUGIN_REVOKE_CMD
    };
    invoke(
        target,
        &json!({"principal": who.to_string(), "capability": cap.to_string()}),
    )
}

/// Set the default automation policy to exactly `caps`.
pub fn policy_command(caps: &[Capability]) -> EditorCommand {
    let caps: Vec<String> = caps.iter().map(ToString::to_string).collect();
    invoke(AUTOMATION_POLICY_CMD, &json!({ "capabilities": caps }))
}

/// Approve (commit) an automation session's preview transaction.
pub fn approve_command(txn: TxnId) -> EditorCommand {
    invoke(AUTOMATION_APPROVE_CMD, &json!({ "txn": txn.0 }))
}

/// Reject (cancel) an automation session's preview transaction.
pub fn reject_command(txn: TxnId) -> EditorCommand {
    invoke(AUTOMATION_REJECT_CMD, &json!({ "txn": txn.0 }))
}

/// Add plugin `id` at `version` to the project (`source`: where it came from, e.g. the
/// index's name).
pub fn plugin_add_command(id: &str, version: &str, source: &str) -> EditorCommand {
    invoke(
        PLUGIN_ADD_CMD,
        &json!({"id": id, "version": version, "source": source}),
    )
}

/// Remove plugin `id` from the project.
pub fn plugin_remove_command(id: &str) -> EditorCommand {
    invoke(PLUGIN_REMOVE_CMD, &json!({ "id": id }))
}

/// Enable or disable plugin `id` in the project.
pub fn plugin_enabled_command(id: &str, enabled: bool) -> EditorCommand {
    invoke(PLUGIN_ENABLE_CMD, &json!({"id": id, "enabled": enabled}))
}

/// The transaction an approve / reject names.
pub fn review_txn(args: &str) -> Option<TxnId> {
    serde_json::from_str::<Json>(args)
        .ok()?
        .get("txn")?
        .as_u64()
        .map(TxnId)
}

// ---- planners ---------------------------------------------------------------------------

fn bad(target: &str, why: impl Into<String>) -> CmdError {
    CmdError::BadArgs {
        target: target.to_string(),
        why: why.into(),
    }
}

fn text<'a>(target: &str, args: &'a Json, k: &str) -> Result<&'a str, CmdError> {
    args.get(k)
        .and_then(Json::as_str)
        .ok_or_else(|| bad(target, format!("\"{k}\" must be text")))
}

fn principal_cap(
    target: &str,
    args: &Json,
    kind: PrincipalKind,
) -> Result<(Principal, Capability, String), CmdError> {
    let who = parse_principal(text(target, args, "principal")?)
        .filter(|p| p.kind() == kind)
        .ok_or_else(|| {
            bad(
                target,
                match kind {
                    PrincipalKind::Automation => "\"principal\" must be automation:<session>",
                    _ => "\"principal\" must be plugin:<id>",
                },
            )
        })?;
    let cap: Capability = text(target, args, "capability")?
        .parse()
        .map_err(|e: forge_plugin::PluginError| bad(target, e.to_string()))?;
    let key = grant_key(&who, cap).ok_or_else(|| bad(target, "no grant key"))?;
    Ok((who, cap, key))
}

/// A planner that may capture (the automation grant writes the epoch).
pub type SecurityPlanner =
    std::sync::Arc<dyn Fn(&mut DiffBuilder<'_>, &Json) -> Result<(), CmdError> + Send + Sync>;

/// The security commands' handlers, for the core to register: `(target, policy, planner)`.
/// Automation grants are written with `epoch` (see the module docs).
pub fn handlers(epoch: &str) -> Vec<(&'static str, CommandPolicy, SecurityPlanner)> {
    let e = epoch.to_string();
    vec![
        (
            AUTOMATION_GRANT_CMD,
            GRANT_POLICY,
            std::sync::Arc::new(move |b: &mut DiffBuilder<'_>, a: &Json| {
                let (_, _, key) =
                    principal_cap(AUTOMATION_GRANT_CMD, a, PrincipalKind::Automation)?;
                b.set_setting(&key, Some(Value::Text(e.clone())))
            }),
        ),
        (
            AUTOMATION_REVOKE_CMD,
            FINAL_POLICY,
            std::sync::Arc::new(|b: &mut DiffBuilder<'_>, a: &Json| {
                let (_, _, key) =
                    principal_cap(AUTOMATION_REVOKE_CMD, a, PrincipalKind::Automation)?;
                b.set_setting(&key, Some(Value::Bool(false)))
            }),
        ),
        (
            PLUGIN_GRANT_CMD,
            GRANT_POLICY,
            std::sync::Arc::new(|b: &mut DiffBuilder<'_>, a: &Json| {
                let (_, _, key) = principal_cap(PLUGIN_GRANT_CMD, a, PrincipalKind::Plugin)?;
                b.set_setting(&key, Some(Value::Bool(true)))
            }),
        ),
        (
            PLUGIN_REVOKE_CMD,
            FINAL_POLICY,
            std::sync::Arc::new(|b: &mut DiffBuilder<'_>, a: &Json| {
                let (_, _, key) = principal_cap(PLUGIN_REVOKE_CMD, a, PrincipalKind::Plugin)?;
                b.set_setting(&key, Some(Value::Bool(false)))
            }),
        ),
        (
            AUTOMATION_POLICY_CMD,
            FINAL_POLICY,
            std::sync::Arc::new(plan_policy),
        ),
        (
            AUTOMATION_APPROVE_CMD,
            FINAL_POLICY,
            std::sync::Arc::new(|_: &mut DiffBuilder<'_>, a: &Json| {
                plan_review(AUTOMATION_APPROVE_CMD, a)
            }),
        ),
        (
            AUTOMATION_REJECT_CMD,
            FINAL_POLICY,
            std::sync::Arc::new(|_: &mut DiffBuilder<'_>, a: &Json| {
                plan_review(AUTOMATION_REJECT_CMD, a)
            }),
        ),
        (
            ACCEPT_HELD_CMD,
            FINAL_POLICY,
            std::sync::Arc::new(plan_accept_held),
        ),
        (
            DISCARD_HELD_CMD,
            FINAL_POLICY,
            std::sync::Arc::new(plan_discard_held),
        ),
        (
            PLUGIN_ADD_CMD,
            CommandPolicy::ORDINARY,
            std::sync::Arc::new(plan_plugin_add),
        ),
        (
            PLUGIN_REMOVE_CMD,
            CommandPolicy::ORDINARY,
            std::sync::Arc::new(plan_plugin_remove),
        ),
        (
            PLUGIN_ENABLE_CMD,
            CommandPolicy::ORDINARY,
            std::sync::Arc::new(plan_plugin_enabled),
        ),
    ]
}

fn plan_policy(b: &mut DiffBuilder<'_>, a: &Json) -> Result<(), CmdError> {
    let list = a
        .get("capabilities")
        .and_then(Json::as_array)
        .ok_or_else(|| bad(AUTOMATION_POLICY_CMD, "\"capabilities\" must be a list"))?;
    let mut caps = BTreeSet::new();
    for c in list {
        let c: Capability = c
            .as_str()
            .ok_or_else(|| bad(AUTOMATION_POLICY_CMD, "a capability is text"))?
            .parse()
            .map_err(|e: forge_plugin::PluginError| bad(AUTOMATION_POLICY_CMD, e.to_string()))?;
        caps.insert(c);
    }
    // Refused when it holds a never-default capability (PLUGIN-0015's rule, Ch.32.4).
    DefaultPolicy::new(caps.iter().copied())
        .map_err(|e| bad(AUTOMATION_POLICY_CMD, e.to_string()))?;
    for c in Capability::ALL.into_iter().filter(|c| c.may_be_default()) {
        b.set_setting(
            &format!("{AUTOMATION_POLICY_PREFIX}.{}", c.key()),
            Some(Value::Bool(caps.contains(&c))),
        )?;
    }
    Ok(())
}

fn plan_review(target: &str, a: &Json) -> Result<(), CmdError> {
    // No change of its own: the core commits or cancels the named transaction once the bus
    // accepted the command (and checked, under the same lock, that it is an automation session's
    // open preview transaction).
    a.get("txn")
        .and_then(Json::as_u64)
        .map(drop)
        .ok_or_else(|| bad(target, "\"txn\" must be a transaction number"))
}

fn plugin_id(target: &str, a: &Json) -> Result<String, CmdError> {
    let id = text(target, a, "id")?;
    PluginId::new(id).map_err(|e| bad(target, e.to_string()))?;
    Ok(id.to_string())
}

fn plan_plugin_add(b: &mut DiffBuilder<'_>, a: &Json) -> Result<(), CmdError> {
    let id = plugin_id(PLUGIN_ADD_CMD, a)?;
    let version = text(PLUGIN_ADD_CMD, a, "version")?;
    semver::Version::parse(version).map_err(|e| bad(PLUGIN_ADD_CMD, format!("version: {e}")))?;
    let source = a.get("source").and_then(Json::as_str).unwrap_or("local");
    b.set_setting(&plugin_key(&id, "enabled"), Some(Value::Bool(true)))?;
    b.set_setting(
        &plugin_key(&id, "version"),
        Some(Value::Text(version.to_string())),
    )?;
    b.set_setting(
        &plugin_key(&id, "source"),
        Some(Value::Text(source.to_string())),
    )
}

fn plan_plugin_remove(b: &mut DiffBuilder<'_>, a: &Json) -> Result<(), CmdError> {
    let id = plugin_id(PLUGIN_REMOVE_CMD, a)?;
    if b.setting(&plugin_key(&id, "enabled")).is_none() {
        return Err(bad(
            PLUGIN_REMOVE_CMD,
            format!("{id} is not in the project's plugin set"),
        ));
    }
    for f in ["enabled", "version", "source"] {
        b.set_setting(&plugin_key(&id, f), None)?;
    }
    Ok(())
}

fn plan_plugin_enabled(b: &mut DiffBuilder<'_>, a: &Json) -> Result<(), CmdError> {
    let id = plugin_id(PLUGIN_ENABLE_CMD, a)?;
    let on = a
        .get("enabled")
        .and_then(Json::as_bool)
        .ok_or_else(|| bad(PLUGIN_ENABLE_CMD, "\"enabled\" must be true or false"))?;
    b.set_setting(&plugin_key(&id, "enabled"), Some(Value::Bool(on)))
}

// ---- reading the project ----------------------------------------------------------------

/// One entry of the project's plugin set.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PluginSetEntry {
    pub enabled: bool,
    pub version: String,
    pub source: String,
}

/// The project's plugin set from its settings (`plugins.*`), by plugin id.
pub fn plugin_set<'a>(
    settings: impl Iterator<Item = (&'a str, &'a Value)>,
) -> BTreeMap<String, PluginSetEntry> {
    let mut out: BTreeMap<String, PluginSetEntry> = BTreeMap::new();
    for (k, v) in settings {
        let Some(rest) = k
            .strip_prefix(PLUGINS_PREFIX)
            .and_then(|r| r.strip_prefix('.'))
        else {
            continue;
        };
        let Some((seg, field)) = rest.split_once('.') else {
            continue;
        };
        let Some(id) = decode_id(seg) else { continue };
        let e = out.entry(id).or_default();
        match (field, v) {
            ("enabled", Value::Bool(b)) => e.enabled = *b,
            ("version", Value::Text(t)) => e.version.clone_from(t),
            ("source", Value::Text(t)) => e.source.clone_from(t),
            _ => {}
        }
    }
    out
}

/// The default automation policy a project's settings describe (`None`: the project sets none).
pub fn automation_policy<'a>(
    mut setting: impl FnMut(&str) -> Option<&'a Value>,
) -> Option<DefaultPolicy> {
    let mut any = false;
    let mut caps = Vec::new();
    for c in Capability::ALL.into_iter().filter(|c| c.may_be_default()) {
        match setting(&format!("{AUTOMATION_POLICY_PREFIX}.{}", c.key())) {
            Some(Value::Bool(true)) => {
                any = true;
                caps.push(c);
            }
            Some(_) => any = true,
            None => {}
        }
    }
    any.then(|| DefaultPolicy::new(caps).unwrap_or_default())
}

// ---- the grant table follows the project ------------------------------------------------

/// What a security change was, for the audit log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecurityEvent {
    /// `who` now holds `cap`.
    Granted { who: Principal, cap: Capability },
    /// `who` no longer holds `cap` (a revoke, or an undone grant).
    Revoked { who: Principal, cap: Capability },
    /// The default automation policy changed.
    Policy { caps: Vec<Capability> },
    /// A plugin-set field changed (`added`, `removed`, `enabled`, `disabled`, `version`).
    PluginSet { plugin: String, what: String },
}

/// Keeps a [`SharedGrants`] table equal to what the project's settings grant (see the module
/// docs). Holds the pairs it granted, so a resync revokes a grant whose setting is gone.
#[derive(Debug)]
pub struct GrantSync {
    epoch: String,
    granted: BTreeSet<(Principal, Capability)>,
    /// W2 positive control: grants are recorded in the project but never reach the table
    /// (`test_security_commands`' control). Never set outside that test.
    pub(crate) disabled: crate::controls::Switch,
}

impl GrantSync {
    /// A sync honouring automation grants of `epoch`.
    pub fn new(epoch: &str) -> Self {
        Self {
            epoch: epoch.to_string(),
            granted: BTreeSet::new(),
            disabled: crate::controls::Switch::default(),
        }
    }

    fn set(&mut self, who: Principal, cap: Capability, on: bool, table: &SharedGrants) {
        if self.disabled.on() {
            return;
        }
        if on {
            table.grant(who.clone(), cap);
            self.granted.insert((who, cap));
        } else {
            table.revoke(&who, cap);
            self.granted.remove(&(who, cap));
        }
    }

    /// Follow one applied diff; returns what it changed, for the audit log.
    pub fn apply(
        &mut self,
        changes: &[Change],
        project: &Project,
        table: &SharedGrants,
    ) -> Vec<SecurityEvent> {
        let mut out = Vec::new();
        let mut policy = false;
        for c in changes {
            let Change::Setting { key, before, after } = c else {
                continue;
            };
            if let Some((who, cap)) = parse_grant_key(key) {
                let now = value_grants(who.kind(), after.as_ref(), &self.epoch);
                let was = value_grants(who.kind(), before.as_ref(), &self.epoch);
                self.set(who.clone(), cap, now, table);
                if now != was || !now {
                    out.push(if now {
                        SecurityEvent::Granted { who, cap }
                    } else {
                        SecurityEvent::Revoked { who, cap }
                    });
                }
            } else if key.starts_with(AUTOMATION_POLICY_PREFIX) {
                policy = true;
            } else if let Some(rest) = key
                .strip_prefix(PLUGINS_PREFIX)
                .and_then(|r| r.strip_prefix('.'))
                && let Some((seg, field)) = rest.split_once('.')
                && let Some(plugin) = decode_id(seg)
            {
                let what = match (field, before, after) {
                    ("enabled", None, Some(_)) => "added".to_string(),
                    ("enabled", Some(_), None) => "removed".to_string(),
                    ("enabled", _, Some(Value::Bool(true))) => "enabled".to_string(),
                    ("enabled", _, Some(Value::Bool(false))) => "disabled".to_string(),
                    ("version", _, Some(Value::Text(v))) => format!("version {v}"),
                    _ => continue,
                };
                out.push(SecurityEvent::PluginSet { plugin, what });
            }
        }
        if policy {
            let p = self.sync_policy(project, table);
            out.push(SecurityEvent::Policy {
                caps: p.capabilities().collect(),
            });
        }
        out
    }

    fn sync_policy(&self, project: &Project, table: &SharedGrants) -> DefaultPolicy {
        let p =
            automation_policy(|k| project.setting(k)).unwrap_or_else(standard_automation_policy);
        if !self.disabled.on() {
            table.set_default_policy(PrincipalKind::Automation, p.clone());
        }
        p
    }

    /// Make the table match the whole project (at start, and after a stream gap).
    pub fn resync(&mut self, project: &Project, table: &SharedGrants) {
        let mut want: BTreeSet<(Principal, Capability)> = BTreeSet::new();
        for (k, v) in project.settings() {
            if let Some((who, cap)) = parse_grant_key(k) {
                if value_grants(who.kind(), Some(v), &self.epoch) {
                    want.insert((who, cap));
                } else {
                    // An explicit revoke also takes a default-policy grant away.
                    self.set(who, cap, false, table);
                }
            }
        }
        let stale: Vec<_> = self.granted.difference(&want).cloned().collect();
        for (who, cap) in stale {
            self.set(who, cap, false, table);
        }
        for (who, cap) in want {
            self.set(who, cap, true, table);
        }
        self.sync_policy(project, table);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trusting the project answers only what a person's own open held: the heading of what
    /// an automation session's or a script's open held never points at the trust buttons.
    #[test]
    fn held_label_offers_trust_only_for_a_persons_open() {
        let held = |by: &str, untrusted: bool| HeldSecurity {
            id: 1,
            by: by.into(),
            source: "file:/p".into(),
            items: Vec::new(),
            untrusted,
        };
        assert!(
            held("human:ada", true)
                .label()
                .contains("trusts the project, or")
        );
        for by in ["automation:auto-3", "script:/s.rhai"] {
            let l = held(by, true).label();
            assert!(!l.contains("trusts the project, or"), "{l}");
            assert!(
                l.contains("trusting the project does not accept them"),
                "{l}"
            );
        }
        assert!(!held("automation:auto-3", false).label().contains("trust"));
    }

    #[test]
    fn narrowed_changes_name_dropped_grants_and_default_capabilities() {
        use forge_cmd::CommandSink;
        let mut bus = forge_cmd::Bus::new();
        let who = forge_cmd::Issuer::Human { user: "ada".into() };
        let read = Capability::Fs(forge_plugin::FsScope::ProjectRead);
        let lakes = Principal::Plugin(
            forge_plugin::PluginId::new("com.example.lakes").unwrap_or_else(|e| panic!("{e}")),
        );
        let key = grant_key(&lakes, read).unwrap_or_else(|| panic!("a grant key"));
        let ordinary = Capability::Command(forge_plugin::CommandClass::Ordinary);
        for (k, v) in [
            (key.clone(), Value::Bool(true)),
            (
                format!("{AUTOMATION_POLICY_PREFIX}.{}", read.key()),
                Value::Bool(true),
            ),
            (
                format!("{AUTOMATION_POLICY_PREFIX}.{}", ordinary.key()),
                Value::Bool(true),
            ),
        ] {
            let e = bus.envelope(
                who.clone(),
                EditorCommand::SetSetting {
                    key: k,
                    value: Some(v),
                },
            );
            bus.apply(e).unwrap_or_else(|r| panic!("{}", r.error));
        }
        // The file drops lakes' grant and turns ordinary commands off.
        let mut file: BTreeMap<String, Value> = BTreeMap::new();
        file.insert(
            format!("{AUTOMATION_POLICY_PREFIX}.{}", read.key()),
            Value::Bool(true),
        );
        file.insert(
            format!("{AUTOMATION_POLICY_PREFIX}.{}", ordinary.key()),
            Value::Bool(false),
        );
        let lines = narrowed_changes(bus.project(), &file);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("com.example.lakes"), "{lines:?}");
        assert!(lines[1].contains(&ordinary.to_string()), "{lines:?}");
        // The same settings narrow nothing.
        file.insert(key, Value::Bool(true));
        file.insert(
            format!("{AUTOMATION_POLICY_PREFIX}.{}", ordinary.key()),
            Value::Bool(true),
        );
        assert!(narrowed_changes(bus.project(), &file).is_empty());
    }

    #[test]
    fn ids_escape_into_one_key_segment_and_back() {
        for id in [
            "com.example.rivers",
            "auto-3",
            "a_b",
            "forge.panels.core",
            "X y",
        ] {
            let e = encode_id(id);
            assert!(forge_cmd::check_path(&e).is_ok(), "{e}");
            assert!(!e.contains('.'), "{e}");
            assert_eq!(decode_id(&e).as_deref(), Some(id));
        }
        assert_ne!(encode_id("a_2e"), encode_id("a.e"));
        assert_eq!(decode_id("nope"), None);
    }

    #[test]
    fn grant_keys_round_trip() {
        let who = Principal::Automation("auto-12".into());
        let cap = Capability::Command(forge_plugin::CommandClass::Destructive);
        let k = grant_key(&who, cap).unwrap_or_default();
        assert!(forge_cmd::check_path(&k).is_ok(), "{k}");
        assert_eq!(parse_grant_key(&k), Some((who, cap)));
        let p = Principal::Plugin(
            PluginId::new("com.example.rivers").unwrap_or_else(|e| panic!("{e}")),
        );
        let k =
            grant_key(&p, Capability::Fs(forge_plugin::FsScope::ProjectRead)).unwrap_or_default();
        assert_eq!(parse_grant_key(&k).map(|x| x.0), Some(p));
        assert_eq!(grant_key(&Principal::Role("admin".into()), cap), None);
        assert_eq!(
            parse_principal("automation:auto-1"),
            Some(Principal::Automation("auto-1".into()))
        );
        assert_eq!(parse_principal("plugin:Bad Id"), None);
        assert_eq!(parse_principal("role:x"), None);
    }

    #[test]
    fn only_this_runs_epoch_grants_an_automation() {
        let a = PrincipalKind::Automation;
        assert!(value_grants(a, Some(&Value::Text("e1".into())), "e1"));
        assert!(
            !value_grants(a, Some(&Value::Text("e0".into())), "e1"),
            "a saved grant from another run"
        );
        assert!(!value_grants(a, Some(&Value::Bool(true)), "e1"));
        assert!(value_grants(
            PrincipalKind::Plugin,
            Some(&Value::Bool(true)),
            "e1"
        ));
        assert!(!value_grants(
            PrincipalKind::Plugin,
            Some(&Value::Bool(false)),
            "e1"
        ));
        assert!(!value_grants(PrincipalKind::Plugin, None, "e1"));
    }
}
