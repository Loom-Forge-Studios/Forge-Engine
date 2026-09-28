//! **Teams, sandboxes and collaboration** in the editor (Ch.37, Ch.38 §38.2, Ch.21 §21.21;
//! WP-U10, ADR 0039): the commands, the per-command role and claim checks, the licence
//! status, and the read-side services the collaboration panels get.
//!
//! **Every change is a command (I7, §21.18).** Team management, claims, publishing, the review
//! queue, pulling and conflict resolution are `Invoke` commands on the core's bus, so they are
//! provenance-tagged, in the bus's audit trail and the core's audit book, scriptable and
//! drivable by any client. The core checks each against the issuer's team role **per command**
//! ([`guard::CollabGuard`], a `forge_cmd::CommandGuard`), then performs it on the backends
//! (the shape of E-36: the panel never touches the identity database or the baseline store).
//!
//! | Command | Who | What the core does |
//! |---|---|---|
//! | `forge.team.create` | a human, licence Team | creates the team (the issuer is its Owner), binds the project (`forge.team.bind`), publishes the first baseline |
//! | `forge.team.invite` | Owner / Maintainer, never above their own role | a pending invite: an email, or a minted join code |
//! | `forge.team.revoke_invite` | Owner / Maintainer | drops a pending invite |
//! | `forge.team.accept` | the invitee | joins, binds the project, rebuilds the sandbox from the baseline |
//! | `forge.team.set_role` / `set_paths` / `remove` | Owner / Maintainer, below their rank | the identity database; a removed member's claims are released |
//! | `forge.collab.claim` / `release` | Developer and up (release: the holder, or a Maintainer) | a claim on an entity subtree: read-only for everyone else, enforced by the core |
//! | `forge.collab.publish` | Developer and up, within their path scopes | commits the sandbox to the baseline (`ProjectStore::commit`), or submits it for review |
//! | `forge.collab.approve` / `reject` | Owner, Maintainer, Reviewer (not their own) | publishes a request, or rejects it with a reason |
//! | `forge.collab.pull` | any member | three-way merge of the baseline into the sandbox; conflicts stop it |
//! | `forge.collab.resolve` | any member | chooses sides for conflicts, and applies when all are chosen |
//! | `forge.collab.review_policy` | Owner / Maintainer | per-project approval, per-path overrides (O-16) |
//!
//! Team state (`team.*`) and the review rules (`collab.*`) are settings **reserved** on the
//! bus to these commands, like the security settings (ADR 0036): a plain `SetSetting` cannot
//! rebind a project or switch review off. Roles, members and invites live in the identity
//! database (the server's, Ch.37 §37.8), not in the project: cloning a repository never hands
//! out the member list or a join code.
//!
//! Live/Pull, sandbox visibility and presence are **session state** a person sets for
//! themselves (the view traits' setters), never project state.
//!
//! **Not promised (E-37):** two people editing the same property of the same object at the
//! same time. [`E37_STATEMENT`] is shown in the sandbox and conflicts panels.

pub mod guard;
pub mod licence;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use forge_cmd::{CmdError, CommandPolicy, DiffBuilder, EditorCommand, EntityKey, Issuer, Value};
use forge_project::collab::{
    Claim, CollabView, IdentityView, Policy, Presence, Role, Team, Visibility,
};
use forge_project::merge::{Conflict, Side};
use forge_store::RevId;
use forge_ui::LiveCell;
use serde_json::{Value as Json, json};

use crate::core::{EditorCore, SharedCore};
use licence::{EntitlementSource, LicenceState, MemoryEntitlement};

pub use forge_project::merge::SYNC_CMD;

/// The plain statement E-37 requires, in the user-facing UI.
pub const E37_STATEMENT: &str = "Two people editing the same property of the same object at the same time is not supported. Claim what you are working on (Ownership) so nobody else changes it; if two edits still meet, pulling stops and the Conflicts panel shows both sides for you to choose.";

// ---- targets ------------------------------------------------------------------------------

pub const TEAM_CREATE: &str = "forge.team.create";
pub const TEAM_BIND: &str = "forge.team.bind";
pub const TEAM_INVITE: &str = "forge.team.invite";
pub const TEAM_REVOKE_INVITE: &str = "forge.team.revoke_invite";
pub const TEAM_ACCEPT: &str = "forge.team.accept";
pub const TEAM_SET_ROLE: &str = "forge.team.set_role";
pub const TEAM_SET_PATHS: &str = "forge.team.set_paths";
pub const TEAM_REMOVE: &str = "forge.team.remove";
pub const CLAIM: &str = "forge.collab.claim";
pub const RELEASE: &str = "forge.collab.release";
pub const PUBLISH: &str = "forge.collab.publish";
pub const APPROVE: &str = "forge.collab.approve";
pub const REJECT: &str = "forge.collab.reject";
pub const PULL: &str = "forge.collab.pull";
pub const RESOLVE: &str = "forge.collab.resolve";
pub const REVIEW_POLICY: &str = "forge.collab.review_policy";

/// The team the project belongs to (reserved setting).
pub const TEAM_ID_SETTING: &str = "team.id";
/// The team's name, as the project shows it (reserved setting).
pub const TEAM_NAME_SETTING: &str = "team.name";
/// Publishing needs a reviewer's approval (reserved setting, O-16).
pub const REVIEW_REQUIRED_SETTING: &str = "collab.review.required";
/// Per-path review overrides: `collab.review.rule.<escaped glob>` = `Bool(required)`.
pub const REVIEW_RULE_PREFIX: &str = "collab.review.rule";

/// Commands the core performs on the backends once the bus accepted them (their own diff is
/// empty, or — `forge.team.bind`, `forge.collab.review_policy` — the settings they own).
pub const PERFORMED: &[&str] = &[
    TEAM_CREATE,
    TEAM_INVITE,
    TEAM_REVOKE_INVITE,
    TEAM_ACCEPT,
    TEAM_SET_ROLE,
    TEAM_SET_PATHS,
    TEAM_REMOVE,
    CLAIM,
    RELEASE,
    PUBLISH,
    APPROVE,
    REJECT,
    PULL,
    RESOLVE,
];

/// Every collaboration target (the guards' and the audit's list).
pub const TARGETS: &[&str] = &[
    TEAM_CREATE,
    TEAM_BIND,
    TEAM_INVITE,
    TEAM_REVOKE_INVITE,
    TEAM_ACCEPT,
    TEAM_SET_ROLE,
    TEAM_SET_PATHS,
    TEAM_REMOVE,
    CLAIM,
    RELEASE,
    PUBLISH,
    APPROVE,
    REJECT,
    PULL,
    RESOLVE,
    REVIEW_POLICY,
    SYNC_CMD,
];

/// The targets a client may never send: the core performs them (like `forge.project.load`).
pub const CORE_ONLY: &[&str] = &[TEAM_BIND, SYNC_CMD];

/// The setting prefixes reserved on the core's bus (with `Bus::reserve_settings`, as the
/// security settings are, ADR 0036): the team binding and the review rules to their
/// commands and the core's project load; and the core's baseline sync may write whatever a
/// baseline holds, the security settings included (it is never sent by a client).
pub const RESERVED_SETTINGS: &[(&str, &[&str])] = &[
    (
        "team",
        &[TEAM_BIND, forge_project::format::LOAD_CMD, SYNC_CMD],
    ),
    (
        "collab",
        &[REVIEW_POLICY, forge_project::format::LOAD_CMD, SYNC_CMD],
    ),
    (crate::security::SECURITY_PREFIX, &[SYNC_CMD]),
    (crate::security::PLUGINS_PREFIX, &[SYNC_CMD]),
];

/// Team management, publishing and review: human-only (§21.18: roles are grants), and
/// performed operations nothing undoes (their inverse is its own command).
pub const HUMAN_PERFORMED: CommandPolicy = CommandPolicy {
    human_only: true,
    undoable: false,
    redoable: false,
};
/// Claims, releases, pulls and resolutions: an automation session working in its person's sandbox
/// may send them too (it acts with that person's role, Ch.37 §37.8).
pub const PERFORMED_POLICY: CommandPolicy = CommandPolicy {
    human_only: false,
    undoable: false,
    redoable: false,
};

/// Is `target` a collaboration command?
#[must_use]
pub fn is_target(target: &str) -> bool {
    TARGETS.contains(&target)
}

fn invoke(target: &str, args: &Json) -> EditorCommand {
    EditorCommand::Invoke {
        target: target.to_string(),
        args: args.to_string(),
    }
}

/// Create a team named `name`; the project binds to it.
pub fn create_command(name: &str) -> EditorCommand {
    invoke(TEAM_CREATE, &json!({ "name": name }))
}
/// Invite `email` (`None`: mint a join code) as `role`, optionally scoped to `paths`.
pub fn invite_command(email: Option<&str>, role: Role, paths: &[String]) -> EditorCommand {
    invoke(
        TEAM_INVITE,
        &json!({"email": email, "role": role.name(), "paths": paths}),
    )
}
pub fn revoke_invite_command(invite: &str) -> EditorCommand {
    invoke(TEAM_REVOKE_INVITE, &json!({ "invite": invite }))
}
/// Accept with a join code or an email invite's id.
pub fn accept_command(token: &str) -> EditorCommand {
    invoke(TEAM_ACCEPT, &json!({ "token": token }))
}
pub fn set_role_command(user: &str, role: Role) -> EditorCommand {
    invoke(TEAM_SET_ROLE, &json!({"user": user, "role": role.name()}))
}
pub fn set_paths_command(user: &str, paths: &[String]) -> EditorCommand {
    invoke(TEAM_SET_PATHS, &json!({"user": user, "paths": paths}))
}
pub fn remove_command(user: &str) -> EditorCommand {
    invoke(TEAM_REMOVE, &json!({ "user": user }))
}
pub fn claim_command(entity: EntityKey) -> EditorCommand {
    invoke(CLAIM, &json!({ "entity": entity.0 }))
}
pub fn release_command(entity: EntityKey) -> EditorCommand {
    invoke(RELEASE, &json!({ "entity": entity.0 }))
}
pub fn publish_command(message: &str) -> EditorCommand {
    invoke(PUBLISH, &json!({ "message": message }))
}
pub fn approve_command(request: u64) -> EditorCommand {
    invoke(APPROVE, &json!({ "request": request }))
}
pub fn reject_command(request: u64, reason: &str) -> EditorCommand {
    invoke(REJECT, &json!({"request": request, "reason": reason}))
}
/// Pull up to `rev` (`None`: the head).
pub fn pull_command(rev: Option<RevId>) -> EditorCommand {
    invoke(PULL, &json!({ "rev": rev.map(|r| r.to_string()) }))
}
/// Choose sides for conflicts (by subject key); the pull applies once every one is chosen.
pub fn resolve_command(choices: &BTreeMap<String, Side>) -> EditorCommand {
    let c: serde_json::Map<String, Json> = choices
        .iter()
        .map(|(k, s)| (k.clone(), Json::String(s.name().to_string())))
        .collect();
    invoke(RESOLVE, &json!({ "choices": c }))
}
/// Require approval for every publish (`required`), with per-path overrides.
pub fn review_policy_command(required: bool, rules: &[(String, bool)]) -> EditorCommand {
    let rules: Vec<Json> = rules
        .iter()
        .map(|(p, r)| json!({"path": p, "required": r}))
        .collect();
    invoke(
        REVIEW_POLICY,
        &json!({"required": required, "rules": rules}),
    )
}

// ---- arguments ----------------------------------------------------------------------------

fn bad(target: &str, why: impl Into<String>) -> CmdError {
    CmdError::BadArgs {
        target: target.to_string(),
        why: why.into(),
    }
}

pub(crate) fn text<'a>(target: &str, a: &'a Json, k: &str) -> Result<&'a str, CmdError> {
    a.get(k)
        .and_then(Json::as_str)
        .ok_or_else(|| bad(target, format!("\"{k}\" must be text")))
}

pub(crate) fn number(target: &str, a: &Json, k: &str) -> Result<u64, CmdError> {
    a.get(k)
        .and_then(Json::as_u64)
        .ok_or_else(|| bad(target, format!("\"{k}\" must be a number")))
}

pub(crate) fn role_arg(target: &str, a: &Json) -> Result<Role, CmdError> {
    let r = text(target, a, "role")?;
    Role::parse(r).ok_or_else(|| {
        bad(
            target,
            format!("{r:?} is not a role (owner, maintainer, developer, reviewer, viewer)"),
        )
    })
}

/// Path scopes: at most 32 globs of at most 256 characters.
pub(crate) fn paths_arg(target: &str, a: &Json) -> Result<Vec<String>, CmdError> {
    let Some(list) = a.get("paths") else {
        return Ok(Vec::new());
    };
    let list = list
        .as_array()
        .ok_or_else(|| bad(target, "\"paths\" must be a list of path globs"))?;
    if list.len() > 32 {
        return Err(bad(target, "at most 32 path scopes"));
    }
    let mut out = Vec::new();
    for p in list {
        let p = p
            .as_str()
            .ok_or_else(|| bad(target, "a path scope is text"))?
            .trim();
        if p.is_empty() || p.len() > 256 {
            return Err(bad(target, "a path scope has 1 to 256 characters"));
        }
        out.push(p.to_string());
    }
    Ok(out)
}

/// Parse a comma- or newline-separated list of path globs (the panels' field).
#[must_use]
pub fn parse_paths(s: &str) -> Vec<String> {
    s.split([',', '\n'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// The choices of a resolve command.
pub(crate) fn choices_arg(a: &Json) -> Result<BTreeMap<String, Side>, CmdError> {
    let m = a
        .get("choices")
        .and_then(Json::as_object)
        .ok_or_else(|| bad(RESOLVE, "\"choices\" must be an object"))?;
    let mut out = BTreeMap::new();
    for (k, v) in m {
        let s = v
            .as_str()
            .and_then(Side::parse)
            .ok_or_else(|| bad(RESOLVE, format!("{k}: a side is \"mine\" or \"theirs\"")))?;
        out.insert(k.clone(), s);
    }
    Ok(out)
}

/// The revision a pull names (`None`: the head).
pub(crate) fn rev_arg(a: &Json) -> Result<Option<RevId>, CmdError> {
    match a.get("rev") {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) => s
            .parse::<RevId>()
            .map(Some)
            .map_err(|e| bad(PULL, format!("\"rev\": {e}"))),
        Some(_) => Err(bad(PULL, "\"rev\" is a revision id or null")),
    }
}

/// The key of a per-path review rule for `glob`.
#[must_use]
pub fn review_rule_key(glob: &str) -> String {
    format!("{REVIEW_RULE_PREFIX}.{}", crate::security::encode_id(glob))
}

/// The review rules a project's settings hold: `(required everywhere, [(glob, required)])`.
pub fn review_rules<'a>(
    settings: impl Iterator<Item = (&'a str, &'a Value)>,
) -> (bool, Vec<(String, bool)>) {
    let mut required = false;
    let mut rules = Vec::new();
    for (k, v) in settings {
        if k == REVIEW_REQUIRED_SETTING {
            required = *v == Value::Bool(true);
        } else if let Some(seg) = k
            .strip_prefix(REVIEW_RULE_PREFIX)
            .and_then(|r| r.strip_prefix('.'))
            && let Some(glob) = crate::security::decode_id(seg)
        {
            rules.push((glob, *v == Value::Bool(true)));
        }
    }
    (required, rules)
}

/// Does publishing `paths` need a reviewer (O-16: a per-project switch, per-path overrides on
/// top — the most specific matching rule wins, by glob length)?
#[must_use]
pub fn review_needed(required: bool, rules: &[(String, bool)], paths: &[String]) -> bool {
    paths.iter().any(|p| {
        rules
            .iter()
            .filter(|(g, _)| forge_project::merge::glob_match(g, p))
            .max_by_key(|(g, _)| g.len())
            .map_or(required, |(_, r)| *r)
    })
}

// ---- planners -----------------------------------------------------------------------------

/// A planner the core registers.
pub type Planner = Arc<dyn Fn(&mut DiffBuilder<'_>, &Json) -> Result<(), CmdError> + Send + Sync>;

/// The collaboration commands' handlers, for the core: `(target, policy, planner)`. A
/// performed command's planner checks its arguments and changes nothing itself.
pub fn handlers() -> Vec<(&'static str, CommandPolicy, Planner)> {
    fn check(target: &'static str) -> Planner {
        Arc::new(move |_: &mut DiffBuilder<'_>, a: &Json| check_args(target, a))
    }
    let mut v: Vec<(&'static str, CommandPolicy, Planner)> = Vec::new();
    for t in [
        TEAM_CREATE,
        TEAM_INVITE,
        TEAM_REVOKE_INVITE,
        TEAM_ACCEPT,
        TEAM_SET_ROLE,
        TEAM_SET_PATHS,
        TEAM_REMOVE,
        PUBLISH,
        APPROVE,
        REJECT,
    ] {
        v.push((t, HUMAN_PERFORMED, check(t)));
    }
    for t in [CLAIM, RELEASE, PULL, RESOLVE] {
        v.push((t, PERFORMED_POLICY, check(t)));
    }
    v.push((TEAM_BIND, HUMAN_PERFORMED, Arc::new(plan_bind)));
    v.push((REVIEW_POLICY, HUMAN_PERFORMED, Arc::new(plan_review_policy)));
    // The pull's patch runs as whoever asked for the pull — a person, or an automation session
    // working in its person's sandbox (`PULL` is performed by automation sessions too, Ch.37
    // §37.8). It was human-only, so an automation session's pull could never apply (WP-21). No
    // client can send it (`CORE_ONLY`: every client path refuses it); the core issues it after
    // checking the pull, and holds what an automation session's pull would widen for a person
    // (WP-33's rule).
    v.push((
        SYNC_CMD,
        PERFORMED_POLICY,
        Arc::new(crate::security::plan_sync),
    ));
    v
}

/// Check a performed command's arguments (the planner of each: nothing is planned).
pub fn check_args(target: &str, a: &Json) -> Result<(), CmdError> {
    match target {
        TEAM_CREATE => {
            let n = text(target, a, "name")?.trim();
            if n.is_empty() || n.chars().count() > 80 {
                return Err(bad(target, "a team needs a name of 1 to 80 characters"));
            }
        }
        TEAM_INVITE => {
            role_arg(target, a)?;
            paths_arg(target, a)?;
            match a.get("email") {
                None | Some(Json::Null) => {}
                Some(Json::String(e)) if e.contains('@') && e.len() <= 254 => {}
                Some(_) => return Err(bad(target, "\"email\" is an email address or null")),
            }
        }
        TEAM_REVOKE_INVITE => drop(text(target, a, "invite")?),
        TEAM_ACCEPT => {
            if text(target, a, "token")?.trim().is_empty() {
                return Err(bad(target, "a join code or an invite is needed"));
            }
        }
        TEAM_SET_ROLE => {
            text(target, a, "user")?;
            role_arg(target, a)?;
        }
        TEAM_SET_PATHS => {
            text(target, a, "user")?;
            paths_arg(target, a)?;
        }
        TEAM_REMOVE => drop(text(target, a, "user")?),
        CLAIM | RELEASE => drop(number(target, a, "entity")?),
        PUBLISH => {
            if text(target, a, "message")?.chars().count() > 1000 {
                return Err(bad(target, "a message has at most 1000 characters"));
            }
        }
        APPROVE => drop(number(target, a, "request")?),
        REJECT => {
            number(target, a, "request")?;
            text(target, a, "reason")?;
        }
        PULL => drop(rev_arg(a)?),
        RESOLVE => drop(choices_arg(a)?),
        _ => {}
    }
    Ok(())
}

fn plan_bind(b: &mut DiffBuilder<'_>, a: &Json) -> Result<(), CmdError> {
    let team = text(TEAM_BIND, a, "team")?;
    let name = text(TEAM_BIND, a, "name")?;
    b.set_setting(TEAM_ID_SETTING, Some(Value::Text(team.to_string())))?;
    b.set_setting(TEAM_NAME_SETTING, Some(Value::Text(name.to_string())))
}

fn plan_review_policy(b: &mut DiffBuilder<'_>, a: &Json) -> Result<(), CmdError> {
    let required = a
        .get("required")
        .and_then(Json::as_bool)
        .ok_or_else(|| bad(REVIEW_POLICY, "\"required\" must be true or false"))?;
    let rules = a
        .get("rules")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    if rules.len() > 64 {
        return Err(bad(REVIEW_POLICY, "at most 64 per-path rules"));
    }
    let mut want: BTreeMap<String, bool> = BTreeMap::new();
    for r in &rules {
        let p = r
            .get("path")
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty() && p.len() <= 256)
            .ok_or_else(|| {
                bad(
                    REVIEW_POLICY,
                    "a rule's \"path\" is a glob of 1 to 256 characters",
                )
            })?;
        let req = r
            .get("required")
            .and_then(Json::as_bool)
            .ok_or_else(|| bad(REVIEW_POLICY, "a rule's \"required\" is true or false"))?;
        want.insert(review_rule_key(p), req);
    }
    b.set_setting(REVIEW_REQUIRED_SETTING, Some(Value::Bool(required)))?;
    let existing: Vec<String> = b
        .project()
        .settings()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with(REVIEW_RULE_PREFIX))
        .map(str::to_string)
        .collect();
    for k in existing {
        if !want.contains_key(&k) {
            b.set_setting(&k, None)?;
        }
    }
    for (k, v) in want {
        b.set_setting(&k, Some(Value::Bool(v)))?;
    }
    Ok(())
}

/// The team a project belongs to (its `team.id` setting).
#[must_use]
pub fn team_of(setting: Option<&Value>) -> Option<String> {
    match setting {
        Some(Value::Text(t)) if !t.is_empty() => Some(t.clone()),
        _ => None,
    }
}

/// Who an issuer acts as in the team: a person as themselves; an automation session, a script or a
/// test as the person whose editor runs it (Ch.37 §37.8: plugins and scripts run with that user's
/// capabilities, never the server's).
#[must_use]
pub fn member_of(issuer: &Issuer, owner: &str) -> String {
    match issuer {
        Issuer::Human { user } => user.clone(),
        _ => owner.to_string(),
    }
}

// ---- what the core tells the panels ------------------------------------------------------

/// A pull that stopped at conflicts.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingPull {
    /// The baseline revision being pulled.
    pub target: RevId,
    /// Revisions it brings in.
    pub incoming: usize,
    /// Every conflict, with the side chosen so far.
    pub conflicts: Vec<Conflict>,
    /// The chosen sides would loop the hierarchy.
    pub hierarchy_loop: Option<String>,
    /// Entities of mine that will get new keys.
    pub rekeyed: Vec<(u64, u64)>,
}

/// One thing the core did (or could not do) for a collaboration command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub what: String,
    pub ok: bool,
    pub message: String,
}

/// The core's collaboration status (see [`EditorCore::collab_status`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollabStatus {
    /// Backends are attached (otherwise the panels say which are missing).
    pub attached: bool,
    /// Whose sandbox this core is.
    pub owner: String,
    /// The project's team (`team.id`) and its name.
    pub team: Option<String>,
    pub team_name: Option<String>,
    /// The sandbox follows the team baseline (bound, and a member).
    pub joined: bool,
    /// The baseline revision the sandbox is layered on, and the head.
    pub base: Option<RevId>,
    pub head: Option<RevId>,
    /// Unpublished transactions, and what they did.
    pub unpublished: usize,
    pub summary: Vec<String>,
    /// A pull stopped at conflicts.
    pub pending: Option<PendingPull>,
    /// The latest outcomes, newest last (at most 16).
    pub outcomes: Vec<Outcome>,
    /// A failed precondition a pull hit when it applied (never dropped).
    pub precondition: Option<String>,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the collaboration guards. Never set outside them.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct CollabFaults {
        /// The core installs no role / claim guard on its bus (`test_role_enforcement`'s
        /// control: a Viewer's edit and a Maintainer's escalation go through).
        pub no_guard: bool,
        /// A pull takes theirs for every conflicting property without stopping
        /// (`test_rebase_preconditions`' control: a conflict is swallowed).
        pub swallow_conflicts: bool,
        /// The core applies a pulled revision by replacing the project (`forge.project.load`)
        /// instead of the key-preserving patch (`test_live_pull`'s identity control).
        pub pull_by_load: bool,
        /// A lapsed licence refuses every edit, not only the collaboration commands
        /// (`test_lapse_degrades_never_locks`' control: the lock-out E-57 forbids).
        pub lapse_locks: bool,
        /// The sandbox keeps every gesture frame as its own delta instead of folding it into the
        /// transaction's earlier frame (`test_sandbox_deltas`' control: a drag of 600 frames holds
        /// 600 deltas).
        pub no_fold: bool,
        /// The sandbox row is sent to the server whenever the summary moved, even inside an open
        /// transaction (`test_sandbox_deltas`' control: a drag sends a row per frame).
        pub row_every_change: bool,
        /// A summary change waits for **every** open transaction on the bus to close, not for
        /// the one that made it (`test_sandbox_row_follows_its_txn`'s control: a long-open
        /// automation transaction leaves teammates reading a stale summary).
        pub row_waits_for_every_txn: bool,
        /// Whether the sandbox follows a team is recomputed on every pump (the team record
        /// cloned) instead of once per identity / server / setting change
        /// (`test_collab_pump_alloc`'s control).
        pub joined_uncached: bool,
        /// The core does not follow undo, redo and cancel from the bus's `Applied` stream: only
        /// the call sites that remember to say so move a transaction's count
        /// (`test_sandbox_txn_hook`'s control: an undo by a path that does not tell drifts).
        pub no_txn_hook: bool,
        /// Publishing replays the sandbox's folded command story onto the base instead of
        /// publishing the sandbox's document (`test_sandbox_story_metadata`'s control: after a
        /// pull re-keyed an entity, the replay edits a teammate's entity).
        pub publish_by_replay: bool,
    }
}

// ---- the panels' services ----------------------------------------------------------------

/// What the collaboration panels read (Ch.21 §21.18: read-only, plus session state). Not
/// attached: the panels say so and offer nothing.
pub struct CollabServices {
    core: Option<SharedCore>,
    pub identity: Option<Arc<dyn IdentityView>>,
    pub server: Option<Arc<dyn CollabView>>,
    pub licence: Arc<dyn EntitlementSource>,
    cells: Arc<Mutex<Vec<Weak<LiveCell>>>>,
    observers: Vec<(u64, u64)>,
    /// What presence this editor last published (selection revision, panel).
    last_presence: std::cell::Cell<Option<(u64, u64)>>,
    #[doc(hidden)]
    pub faults: PanelFaults,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the collaboration panels' guards. Never set outside
    /// them.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct PanelFaults {
        /// The panels' live feeds refresh at 60 Hz instead of at most 10 (`test_presence`'s
        /// rate control).
        pub feed_uncapped: bool,
        /// The panels ask for a loop turn on every sync instead of following their feed
        /// (`test_collab_panels_idle`'s control: an idle editor that never sleeps).
        pub poll: bool,
        /// The hierarchy rewrites its rows' notes on every sync, changed or not (`test_hierarchy_
        /// notes_damage`' control: each mirror change repaints the tree and rebuilds its a11y).
        pub notes_always_edit: bool,
    }
}

impl std::fmt::Debug for CollabServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollabServices")
            .field("attached", &self.server.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for CollabServices {
    fn default() -> Self {
        Self {
            core: None,
            identity: None,
            server: None,
            licence: Arc::new(MemoryEntitlement::new(None)),
            cells: Arc::new(Mutex::new(Vec::new())),
            observers: Vec::new(),
            last_presence: std::cell::Cell::new(None),
            faults: PanelFaults::default(),
        }
    }
}

fn bump_all(cells: &Mutex<Vec<Weak<LiveCell>>>) {
    let mut v = cells.lock().unwrap_or_else(PoisonError::into_inner);
    v.retain(|w| match w.upgrade() {
        Some(c) => {
            c.bump();
            true
        }
        None => false,
    });
}

impl CollabServices {
    /// Follow `core`'s attached backends (after `EditorCore::attach_collab`).
    pub fn attach(&mut self, core: &SharedCore) {
        self.core = Some(core.clone());
        if let Some((identity, server, licence)) = EditorCore::collab_views(core) {
            let cells = Arc::clone(&self.cells);
            let a = identity.observe(Arc::new(move || bump_all(&cells)));
            let cells = Arc::clone(&self.cells);
            let b = server.observe(Arc::new(move || bump_all(&cells)));
            self.observers.push((a, b));
            let cells = Arc::clone(&self.cells);
            licence.observe(Arc::new(move || bump_all(&cells)));
            self.identity = Some(identity);
            self.server = Some(server);
            self.licence = licence;
        }
        let cells = Arc::clone(&self.cells);
        EditorCore::observe_collab(core, Arc::new(move || bump_all(&cells)));
    }

    /// Attached to backends?
    #[must_use]
    pub fn attached(&self) -> bool {
        self.server.is_some()
    }

    /// A live cell bumped on every collaboration change (a panel's feed, §21.11: the panel's
    /// relay refreshes at most as often as its feed allows, only while visible).
    #[must_use]
    pub fn feed(&self) -> Arc<LiveCell> {
        let c = LiveCell::new();
        c.set_live(true);
        self.cells
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::downgrade(&c));
        c
    }

    /// Everything the panels read has this generation (it moves on every change).
    #[must_use]
    pub fn revision(&self) -> u64 {
        let a = self.identity.as_ref().map_or(0, |i| i.generation());
        let b = self.server.as_ref().map_or(0, |s| s.generation());
        let d = self.licence.generation();
        a.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ b.rotate_left(21)
            ^ d.wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ self.core.as_ref().map_or(0, EditorCore::collab_generation)
    }

    /// The core's collaboration status.
    #[must_use]
    pub fn status(&self) -> Option<Arc<CollabStatus>> {
        self.core.as_ref().and_then(EditorCore::collab_status)
    }

    /// Whose editor this is.
    #[must_use]
    pub fn me(&self) -> String {
        self.status().map(|s| s.owner.clone()).unwrap_or_default()
    }

    /// The project's team, from the identity database.
    #[must_use]
    pub fn team(&self) -> Option<Team> {
        let id = self.status()?.team.clone()?;
        self.identity.as_ref()?.team_as(&id, &self.me())
    }

    /// My role in the project's team.
    #[must_use]
    pub fn my_role(&self) -> Option<Role> {
        self.team()?.role_of(&self.me())
    }

    /// The licence, evaluated locally now (no network call).
    #[must_use]
    pub fn licence_state(&self) -> LicenceState {
        let team = self.status().and_then(|s| s.team.clone());
        licence::evaluate(
            self.licence.entitlement().as_ref(),
            self.licence.now_ms(),
            forge_project::format::ENGINE_VERSION,
            team.as_deref(),
        )
    }

    /// Every claim.
    #[must_use]
    pub fn claims(&self) -> Vec<Claim> {
        self.server.as_ref().map(|s| s.claims()).unwrap_or_default()
    }

    /// The claim covering `key` (on it or an ancestor in `parent_of`'s hierarchy).
    pub fn claim_over(
        &self,
        key: u64,
        mut parent_of: impl FnMut(u64) -> Option<u64>,
    ) -> Option<Claim> {
        let claims: BTreeMap<u64, Claim> =
            self.claims().into_iter().map(|c| (c.entity, c)).collect();
        if claims.is_empty() {
            return None;
        }
        let mut cur = Some(key);
        let mut guard = 0usize;
        while let Some(k) = cur {
            if let Some(c) = claims.get(&k) {
                return Some(c.clone());
            }
            guard += 1;
            if guard > 100_000 {
                break;
            }
            cur = parent_of(k);
        }
        None
    }

    /// Teammates (not me) looking at each entity.
    #[must_use]
    pub fn viewers(&self) -> BTreeMap<u64, BTreeSet<String>> {
        match &self.server {
            Some(s) => forge_project::collab::presence_by_entity(&s.presence(), &self.me()),
            None => BTreeMap::new(),
        }
    }

    /// The short notes the hierarchy draws after a row: who is looking at it, who claimed
    /// it (claims on the subtree root only).
    #[must_use]
    pub fn notes(&self) -> BTreeMap<u64, String> {
        let me = self.me();
        let mut out: BTreeMap<u64, String> = BTreeMap::new();
        for (k, users) in self.viewers() {
            let who: Vec<&str> = users.iter().map(String::as_str).collect();
            out.insert(k, format!("\u{25cf} {}", who.join(", ")));
        }
        for c in self.claims() {
            let s = if c.holder == me {
                "claimed by you".to_string()
            } else {
                format!("claimed by {} (read-only)", c.holder)
            };
            out.entry(c.entity)
                .and_modify(|n| {
                    n.push_str(" \u{b7} ");
                    n.push_str(&s);
                })
                .or_insert(s);
        }
        out
    }

    /// Publish this editor's presence (session state) when the selection or the focused
    /// panel changed since the last call.
    pub fn publish_presence(
        &self,
        selection_rev: u64,
        selection: &[EntityKey],
        panel: Option<&str>,
    ) {
        let (Some(server), Some(status)) = (&self.server, self.status()) else {
            return;
        };
        if !status.joined {
            return;
        }
        let panel_hash = panel.map_or(0, |p| {
            p.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
            })
        });
        if self.last_presence.get() == Some((selection_rev, panel_hash)) {
            return;
        }
        self.last_presence.set(Some((selection_rev, panel_hash)));
        server.set_presence(Presence {
            user: status.owner.clone(),
            selection: selection.iter().map(|k| k.0).collect(),
            panel: panel.map(str::to_string),
            at_ms: self.licence.now_ms(),
        });
    }

    /// Set my Live/Pull policy (session state; one click, reversible, I20).
    pub fn set_policy(&self, p: Policy) {
        if let (Some(s), Some(core)) = (&self.server, &self.core) {
            s.set_policy(&self.me(), p);
            // Live starts following at once.
            EditorCore::collab_follow(core);
        }
    }

    /// Set who may read my sandbox (session state).
    pub fn set_visibility(&self, v: Visibility) {
        if let Some(s) = &self.server {
            s.set_visibility(&self.me(), v);
        }
    }

    /// My policy.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.server
            .as_ref()
            .map_or_else(Policy::default, |s| s.policy(&self.me()))
    }
}

impl Drop for CollabServices {
    fn drop(&mut self) {
        if let (Some(i), Some(s)) = (&self.identity, &self.server) {
            for (a, b) in &self.observers {
                i.unobserve(*a);
                s.unobserve(*b);
            }
        }
    }
}
