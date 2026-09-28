//! The core's side of **teams and sandboxes** (Ch.37; WP-U10, ADR 0039): the core is one
//! person's **sandbox** — `project_view = baseline ⊕ sandbox_deltas` (I19) — layered on the
//! team baseline the collaboration server holds.
//!
//! * It checks every collaboration command against the core's own state before the bus
//!   applies it (a refusal is a `Rejection`, nothing applied), and **performs** it on the
//!   backends once the bus accepted it: the identity database for team management, the
//!   server for claims, publishing and the review queue (E-36's shape; the panels never
//!   reach a backend's write side).
//! * **Publishing is `ProjectStore::commit`** on the baseline, a fast-forward from the
//!   sandbox's base, with the envelopes that made the change as its story.
//! * **Pulling is a three-way merge on the reflect tree** (`forge_project::merge`): base =
//!   the baseline at the sandbox's base, theirs = the baseline at the revision pulled, mine
//!   = this project. A clean merge is applied as one key-preserving, precondition-checked
//!   patch (`forge.collab.sync`); conflicts stop it and are kept until each has a side.
//! * **Live and Pull are one mechanism** (I20): Live pulls the head as soon as it moves (on
//!   the next pump of any client), Pull when a person asks, up to the revision they choose.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use forge_cmd::{
    CommandEnvelope, CommandSink, EditorCommand, EntityKey, Issuer, Rejection, TxnId, TxnState,
};
use forge_project::ProjectError;
use forge_project::collab::{
    Claim, CollabBackend, CollabView, IdentityBackend, IdentityView, InviteSpec, InviteTo,
    Observer, Policy, Publish, Role,
};
use forge_project::format::ProjectDoc;
use forge_project::merge::{Merge, Side, changed_paths, glob_match, merge3, patch, sync_args};
use forge_store::RevId;
use serde_json::Value as Json;

use super::EditorCore;
use crate::collab::licence::EntitlementSource;
use crate::collab::{self as c, CollabFaults, CollabStatus, Outcome, PendingPull};
use crate::connect::audit::{AuditOrigin, AuditRecord};

/// Outcomes the status keeps.
const OUTCOMES: usize = 16;

/// The read sides of the attached backends: the identity database, the team server and the
/// licence (see [`EditorCore::collab_views`]).
pub type CollabViews = (
    Arc<dyn IdentityView>,
    Arc<dyn CollabView>,
    Arc<dyn EntitlementSource>,
);

/// What a sandbox holds and has sent ([`EditorCore::collab_counters`]).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollabCounters {
    /// Sandbox deltas held (gesture frames folded).
    pub deltas: usize,
    /// Transactions with deltas.
    pub transactions: usize,
    /// Transactions that count (not undone or cancelled).
    pub counted: usize,
    /// Sandbox rows sent to the server.
    pub rows_sent: u64,
}

/// What [`EditorCore::attach_collab`] connects the core to.
pub struct CollabAttach {
    /// The identity database (write side: only the core holds it).
    pub identity: Arc<dyn IdentityBackend>,
    /// The collaboration server (write side: only the core holds it).
    pub server: Arc<dyn CollabBackend>,
    /// Where the licence comes from (read locally, no network).
    pub licence: Arc<dyn EntitlementSource>,
    /// Whose editor (sandbox) this core is: the person at it.
    pub owner: String,
    /// Their account's email (email invites are accepted by it).
    pub email: Option<String>,
}

/// How far back a gesture frame looks for the earlier frame it replaces (see
/// [`SandboxDeltas::push`]). A gizmo drag writes a few properties per frame; a longer run of
/// distinct writes in one transaction is not a gesture's frames and is kept as sent.
const FOLD_WINDOW: usize = 16;

/// What a pure write command writes: a property of an entity, or a setting.
#[derive(PartialEq, Eq)]
enum WriteKey<'a> {
    Property(EntityKey, &'a str),
    Setting(&'a str),
}

fn write_key(cmd: &EditorCommand) -> Option<WriteKey<'_>> {
    match cmd {
        EditorCommand::SetProperty { entity, path, .. }
        | EditorCommand::RemoveProperty { entity, path } => {
            Some(WriteKey::Property(*entity, path.as_str()))
        }
        EditorCommand::SetSetting { key, .. } => Some(WriteKey::Setting(key.as_str())),
        _ => None,
    }
}

/// A transaction with commands among the sandbox deltas.
struct TxnDeltas {
    /// It counts: not undone or cancelled.
    kept: bool,
}

/// The sandbox deltas (I19): the commands applied here since the base, in application
/// order, with what the status and the server's sandbox row read from them **kept up to date
/// as they arrive** — the count of transactions that count and their semantic summary — so
/// nothing walks the list per frame (owner rule 2). A gesture's frames fold: a write to the
/// property (or setting) an earlier frame of the same transaction wrote, with only other
/// pure writes between them, replaces that frame (the last value is the one that stands, and
/// pure writes to different keys commute), so a 600-frame drag is one delta, not 600.
///
/// **The list is metadata, never a command log to replay** (ADR 0041). It is the story a
/// publish carries (`Publish::envelopes`: the revision's summary, command count and audit
/// trail) and nothing more: folding drops intermediate frames, undone and cancelled
/// transactions are filtered out, pulls (`forge.collab.sync`) and loads are not in it at
/// all, and a pull may re-key an entity the story names by its old key. What a publish
/// makes the baseline is the sandbox's **document** (`Publish::doc`); nothing applies these
/// envelopes to any project (`test_sandbox_story_metadata`).
#[derive(Default)]
pub(crate) struct SandboxDeltas {
    envs: Vec<CommandEnvelope>,
    txns: BTreeMap<TxnId, TxnDeltas>,
    /// Transactions that count.
    kept: usize,
    /// The counted commands' summary.
    summary: forge_project::summary::Summary,
    /// The transactions whose commands moved the summary since the last sandbox row (a
    /// handful: the gestures and automation transactions in flight). A row waits only while every
    /// one of them is still open (`collab_follow_inner`).
    unsent: Vec<TxnId>,
}

impl SandboxDeltas {
    /// A command applied here (its transaction open or just committed: it counts); `fold`:
    /// a gesture frame replaces its earlier frame (see the type docs).
    fn push(&mut self, e: CommandEnvelope, fold: bool) {
        if fold && let Some(k) = write_key(&e.cmd) {
            let mut folded = None;
            for (i, old) in self.envs.iter().enumerate().rev().take(FOLD_WINDOW) {
                if old.txn != e.txn {
                    break;
                }
                match write_key(&old.cmd) {
                    None => break,
                    Some(ok) if ok == k => {
                        folded = Some(i);
                        break;
                    }
                    Some(_) => {}
                }
            }
            if let Some(i) = folded {
                let old = self.envs.remove(i);
                if self.txns.get(&old.txn).is_some_and(|t| t.kept) {
                    self.summary.remove(&old);
                }
            }
        }
        let t = self.txns.entry(e.txn).or_insert(TxnDeltas { kept: false });
        if !t.kept {
            t.kept = true;
            self.kept += 1;
        }
        self.summary.add(&e);
        if !self.unsent.contains(&e.txn) {
            self.unsent.push(e.txn);
        }
        self.envs.push(e);
    }

    /// `txn` was undone, redone or cancelled: whether it counts now is `kept`.
    fn set_kept(&mut self, txn: TxnId, kept: bool) {
        let Some(t) = self.txns.get_mut(&txn) else {
            return;
        };
        if t.kept == kept {
            return;
        }
        t.kept = kept;
        if kept {
            self.kept += 1;
        } else {
            self.kept -= 1;
        }
        // Once per undo or redo (a person's action), never per frame.
        for e in self.envs.iter().filter(|e| e.txn == txn) {
            if kept {
                self.summary.add(e);
            } else {
                self.summary.remove(e);
            }
        }
    }

    fn clear(&mut self) {
        self.envs.clear();
        self.txns.clear();
        self.kept = 0;
        self.summary.clear();
        self.unsent.clear();
    }

    /// Recompute whether each transaction counts from the bus (the core's view of the
    /// `Applied` stream had a gap). A transaction the bus retired as expired keeps what it
    /// had: expiry changes nothing about its changes.
    fn resync_kept(&mut self, state: impl Fn(TxnId) -> Option<TxnState>) {
        let moved: Vec<(TxnId, bool)> = self
            .txns
            .iter()
            .filter_map(|(t, d)| {
                let kept = match state(*t)? {
                    TxnState::Undone | TxnState::Cancelled => false,
                    TxnState::Open | TxnState::Committed => true,
                    TxnState::Expired => return None,
                };
                (kept != d.kept).then_some((*t, kept))
            })
            .collect();
        for (t, kept) in moved {
            self.set_kept(t, kept);
        }
    }

    /// Commands held (after folding).
    fn len(&self) -> usize {
        self.envs.len()
    }

    /// Transactions with commands here (counted or not).
    fn txns(&self) -> usize {
        self.txns.len()
    }

    /// The commands that count, in order: publishing's **story** (metadata, see the type
    /// docs: never replayed).
    fn kept_envelopes(&self) -> Vec<CommandEnvelope> {
        self.envs
            .iter()
            .filter(|e| self.txns.get(&e.txn).is_none_or(|t| t.kept))
            .cloned()
            .collect()
    }
}

/// What a sandbox row sent to the server was made from (see `collab_follow_inner`).
#[derive(Clone, Copy, PartialEq, Eq)]
struct RowKey {
    base: Option<RevId>,
    /// Transactions with deltas, counted or not.
    txns: usize,
    /// Transactions that count.
    kept: usize,
    /// The summary's generation.
    summary: u64,
}

/// The core's collaboration state (see the module docs).
pub(crate) struct CollabHost {
    identity: Arc<dyn IdentityBackend>,
    server: Arc<dyn CollabBackend>,
    licence: Arc<dyn EntitlementSource>,
    owner: String,
    /// The baseline revision this sandbox is layered on.
    base: Option<RevId>,
    /// Commands applied here since the base (the sandbox deltas, I19).
    deltas: SandboxDeltas,
    /// A pull that stopped at conflicts: its target and merge.
    pending: Option<(RevId, usize, Merge)>,
    choices: BTreeMap<String, Side>,
    outcomes: VecDeque<Outcome>,
    precondition: Option<String>,
    /// What the sandbox row last sent to the server was made from.
    recorded: Option<RowKey>,
    /// Sandbox rows sent to the server (`EditorCore::collab_counters`).
    rows_sent: u64,
    pub(crate) faults: CollabFaults,
    status: Option<Arc<CollabStatus>>,
    /// The identity database's and the server's generations `status` was built at.
    status_gens: (u64, u64),
    /// Whether this sandbox follows its project's team, as last worked out (see
    /// [`CollabHost::joined`]).
    joined_memo: std::cell::RefCell<Option<JoinedMemo>>,
}

/// What [`CollabHost::joined`] last worked out, and what it depended on: the identity
/// database's and the server's generations and the project's team setting. Every pump asks,
/// so the answer is kept until one of those moves — asking costs no allocation (the team
/// record is cloned only when something changed).
struct JoinedMemo {
    identity: u64,
    server: u64,
    team: Option<String>,
    joined: bool,
}

/// The project's team id, borrowed (see [`c::team_of`], which copies it).
fn team_id(project: &forge_cmd::Project) -> Option<&str> {
    match project.setting(c::TEAM_ID_SETTING) {
        Some(forge_cmd::Value::Text(t)) if !t.is_empty() => Some(t.as_str()),
        _ => None,
    }
}

fn refuse(e: impl std::fmt::Display) -> Rejection {
    Rejection::new(
        forge_cmd::CmdError::PolicyRefused {
            what: e.to_string(),
        },
        None,
        None,
    )
}

fn sorted(mut d: ProjectDoc) -> ProjectDoc {
    d.entities.sort_by_key(|e| e.id);
    d
}

/// The content path of an entity in `p` (`scene/Levels/One`).
fn entity_label(p: &forge_cmd::Project, k: EntityKey) -> String {
    let mut names = Vec::new();
    let mut cur = Some(k);
    let mut steps = 0usize;
    while let Some(x) = cur {
        let Some(e) = p.entity(x) else { break };
        names.push(e.name().replace('/', "_"));
        cur = e.parent();
        steps += 1;
        if steps > p.len() {
            break;
        }
    }
    names.reverse();
    format!("scene/{}", names.join("/"))
}

impl CollabHost {
    fn team(&self, project: &forge_cmd::Project) -> Option<String> {
        c::team_of(project.setting(c::TEAM_ID_SETTING))
    }

    /// Does this sandbox follow its project's team (the server holds that team's baseline and
    /// the owner is a member)? Asked on every pump: answered from [`JoinedMemo`] while the
    /// identity database, the server and the team setting are where they were.
    fn joined(&self, project: &forge_cmd::Project) -> bool {
        let team = team_id(project);
        // Read the generations first: a change racing the work below leaves a memo keyed to
        // the old generation, which the next ask recomputes.
        let (ig, sg) = (self.identity.generation(), self.server.generation());
        if !self.faults.joined_uncached()
            && let Some(m) = self.joined_memo.borrow().as_ref()
            && m.identity == ig
            && m.server == sg
            && m.team.as_deref() == team
        {
            return m.joined;
        }
        let joined = team.is_some_and(|t| {
            self.server.team().as_deref() == Some(t)
                && self
                    .identity
                    .team(t)
                    .is_some_and(|x| x.member(&self.owner).is_some())
        });
        *self.joined_memo.borrow_mut() = Some(JoinedMemo {
            identity: ig,
            server: sg,
            team: team.map(str::to_string),
            joined,
        });
        joined
    }

    fn outcome(&mut self, what: &str, r: Result<String, ProjectError>) {
        let (ok, message) = match r {
            Ok(m) => (true, m),
            Err(e) => (false, e.to_string()),
        };
        self.outcomes.push_back(Outcome {
            what: what.to_string(),
            ok,
            message,
        });
        while self.outcomes.len() > OUTCOMES {
            self.outcomes.pop_front();
        }
        self.status = None;
    }
}

impl EditorCore {
    /// Connect the core to collaboration backends (the editor binary's in-memory stand-ins,
    /// D-4; a test's): registers the role and claim guard on the bus, and follows the
    /// backends (their changes wake every client, which then pull in Live mode).
    pub fn attach_collab(core: &super::SharedCore, a: CollabAttach) {
        a.identity.ensure_account(&a.owner, a.email.as_deref());
        let wake = Arc::clone(&super::lock(core).wake_list);
        let w2 = Arc::clone(&wake);
        let ping: Observer = Arc::new(move || {
            let list: Vec<super::Waker> = w2
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .cloned()
                .collect();
            for w in list {
                w();
            }
        });
        a.identity.observe(Arc::clone(&ping));
        a.server.observe(ping);
        let mut cg = super::lock(core);
        let faults = CollabFaults::default();
        cg.bus.set_guard(Some(Arc::new(c::guard::CollabGuard {
            identity: a.identity.clone(),
            server: a.server.clone(),
            licence: a.licence.clone(),
            owner: a.owner.clone(),
            lapse_locks: false,
        })));
        cg.collab = Some(Box::new(CollabHost {
            identity: a.identity,
            server: a.server,
            licence: a.licence,
            owner: a.owner,
            base: None,
            deltas: SandboxDeltas::default(),
            pending: None,
            choices: BTreeMap::new(),
            outcomes: VecDeque::new(),
            precondition: None,
            recorded: None,
            rows_sent: 0,
            faults,
            status: None,
            status_gens: (0, 0),
            joined_memo: std::cell::RefCell::new(None),
        }));
        cg.collab_changed();
    }

    /// W2 positive controls only: break a collaboration property of the core.
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn set_collab_faults(core: &super::SharedCore, f: CollabFaults) {
        let mut cg = super::lock(core);
        let guard = cg.collab.as_ref().map(|h| {
            Arc::new(c::guard::CollabGuard {
                identity: h.identity.clone(),
                server: h.server.clone(),
                licence: h.licence.clone(),
                owner: h.owner.clone(),
                lapse_locks: f.lapse_locks,
            }) as Arc<dyn forge_cmd::CommandGuard>
        });
        cg.bus.set_guard(if f.no_guard { None } else { guard });
        if let Some(h) = cg.collab.as_mut() {
            h.faults = f;
        }
    }

    /// The read sides of the attached backends (the panels' services).
    pub fn collab_views(core: &super::SharedCore) -> Option<CollabViews> {
        let cg = super::lock(core);
        let h = cg.collab.as_ref()?;
        let identity: Arc<dyn IdentityView> = h.identity.clone();
        let server: Arc<dyn CollabView> = h.server.clone();
        Some((identity, server, h.licence.clone()))
    }

    /// Call `f` whenever the core's collaboration status changes (the panels' feeds).
    pub fn observe_collab(core: &super::SharedCore, f: Observer) {
        super::lock(core).collab_watchers.push(f);
    }

    /// Moves whenever the collaboration status changes.
    pub fn collab_generation(core: &super::SharedCore) -> u64 {
        super::lock(core).collab_gen
    }

    /// The collaboration status (`None`: no backends attached).
    pub fn collab_status(core: &super::SharedCore) -> Option<Arc<CollabStatus>> {
        let mut cg = super::lock(core);
        cg.follow_security();
        cg.collab_status_now()
    }

    /// The sandbox's counters (the hot-path guards read them); `None` when not attached.
    #[doc(hidden)]
    pub fn collab_counters(core: &super::SharedCore) -> Option<CollabCounters> {
        let mut cg = super::lock(core);
        cg.follow_security();
        cg.collab.as_ref().map(|h| CollabCounters {
            deltas: h.deltas.len(),
            transactions: h.deltas.txns(),
            counted: h.deltas.kept,
            rows_sent: h.rows_sent,
        })
    }

    /// Follow the baseline now (what every client's pump does): record the sandbox row, and
    /// in Live mode pull the head when it moved.
    pub fn collab_follow(core: &super::SharedCore) {
        let mut cg = super::lock(core);
        let before = cg.bus.next_seq();
        cg.collab_follow_inner();
        if cg.bus.next_seq() != before {
            cg.follow_security();
            cg.notify_all();
        }
    }

    // ---- internals (under the core's lock) ------------------------------------------------

    pub(super) fn collab_changed(&mut self) {
        self.collab_gen += 1;
        if let Some(h) = self.collab.as_mut() {
            h.status = None;
        }
        for w in &self.collab_watchers {
            w();
        }
    }

    fn collab_status_now(&mut self) -> Option<Arc<CollabStatus>> {
        let project = self.bus.project();
        let h = self.collab.as_ref()?;
        // The status reads the backends too (the head, membership): it is current while they
        // have not moved since it was built.
        let gens = (h.identity.generation(), h.server.generation());
        if let Some(s) = &h.status
            && h.status_gens == gens
        {
            return Some(Arc::clone(s));
        }
        let team = h.team(project);
        let team_name = match project.setting(c::TEAM_NAME_SETTING) {
            Some(forge_cmd::Value::Text(t)) => Some(t.clone()),
            _ => None,
        };
        let unpublished = h.deltas.kept;
        let summary = h.deltas.summary.lines();
        let status = Arc::new(CollabStatus {
            attached: true,
            owner: h.owner.clone(),
            team,
            team_name,
            joined: h.joined(project),
            base: h.base,
            head: h.server.head(),
            unpublished,
            summary,
            pending: h.pending.as_ref().map(|(t, n, m)| PendingPull {
                target: *t,
                incoming: *n,
                conflicts: m.conflicts.clone(),
                hierarchy_loop: m.hierarchy_loop.clone(),
                rekeyed: m.rekeyed.clone(),
            }),
            outcomes: h.outcomes.iter().cloned().collect(),
            precondition: h.precondition.clone(),
        });
        if let Some(h) = self.collab.as_mut() {
            h.status = Some(Arc::clone(&status));
            h.status_gens = gens;
        }
        Some(status)
    }

    /// The sandbox deltas that still count: not undone or cancelled.
    fn kept_deltas(&self) -> Vec<CommandEnvelope> {
        self.collab
            .as_ref()
            .map(|h| h.deltas.kept_envelopes())
            .unwrap_or_default()
    }

    /// A client's command changed the project: it is a sandbox delta.
    pub(super) fn collab_record(&mut self, e: &CommandEnvelope) {
        if let EditorCommand::Invoke { target, .. } = &e.cmd
            && (c::is_target(target) || target == forge_project::format::LOAD_CMD)
        {
            return;
        }
        if let Some(h) = self.collab.as_mut() {
            let fold = !h.faults.no_fold();
            h.deltas.push(e.clone(), fold);
            h.status = None;
        }
    }

    /// The bus-side hook that keeps the sandbox's transaction counts (ADR 0041): the core's
    /// own view of the `Applied` stream (`follow_security`) hands every event here, and an
    /// undo, redo or cancel moves whether that transaction's deltas count — **whichever path
    /// asked the bus for it** (a client, an approval, an automation session's batch, a path added
    /// later). No call site has to remember. A gap in the stream recomputes every transaction from
    /// the bus instead.
    pub(super) fn collab_follow_applied(&mut self, a: &forge_cmd::Applied) {
        use forge_cmd::AppliedKind;
        if !matches!(
            a.kind,
            AppliedKind::Undo | AppliedKind::Redo | AppliedKind::Cancel
        ) {
            return;
        }
        let kept = !matches!(
            self.bus.txn_state(a.txn),
            Some(TxnState::Undone | TxnState::Cancelled)
        );
        if let Some(h) = self.collab.as_mut()
            && !h.faults.no_txn_hook()
        {
            h.deltas.set_kept(a.txn, kept);
            h.status = None;
        }
    }

    /// The `Applied` stream had a gap: recompute whether each transaction counts.
    pub(super) fn collab_resync_txns(&mut self) {
        let bus = &self.bus;
        if let Some(h) = self.collab.as_mut()
            && !h.faults.no_txn_hook()
        {
            h.deltas.resync_kept(|t| bus.txn_state(t));
            h.status = None;
        }
    }

    pub(super) fn collab_follow_inner(&mut self) {
        if self.collab.is_none() {
            return;
        }
        // Whatever reached the bus by a path that did not drain the core's view of the
        // stream (an undo by core configuration, say) counts before the row is weighed.
        self.follow_security();
        let Some(h) = self.collab.as_ref() else {
            return;
        };
        if !h.joined(self.bus.project()) {
            return;
        }
        // Live: pull the head as soon as it moves (paused while conflicts wait).
        let live = h.server.policy(&h.owner) == Policy::Live;
        let head = h.server.head();
        if live && h.pending.is_none() && head.is_some() && head != h.base {
            let issuer = Issuer::Human {
                user: h.owner.clone(),
            };
            let r = self.collab_pull(head, &issuer);
            if let Some(h) = self.collab.as_mut() {
                h.outcome("Live pull", r);
            }
            self.collab_changed();
        }
        // The sandbox row (I19: a row the server keeps): only when it changed — a new
        // transaction, an undo or redo, a new base at once; a change inside a transaction
        // (a gesture's frames) once **that** transaction is no longer open, not per frame —
        // another transaction left open (an automation session's awaiting review) does not hold it.
        // O(1) when nothing changed: the counts and the summary are kept as the deltas arrive, and
        // the transactions that moved the summary since the last row are a handful.
        let Some(h) = self.collab.as_ref() else {
            return;
        };
        let key = RowKey {
            base: h.base,
            txns: h.deltas.txns(),
            kept: h.deltas.kept,
            summary: h.deltas.summary.generation(),
        };
        let bus = &self.bus;
        let closed = || {
            if h.faults.row_waits_for_every_txn() {
                bus.open_transactions().next().is_none()
            } else {
                h.deltas.unsent.is_empty()
                    || h.deltas
                        .unsent
                        .iter()
                        .any(|t| bus.txn_state(*t) != Some(TxnState::Open))
            }
        };
        let due = match h.recorded {
            None => true,
            Some(r) => {
                (r.base, r.txns, r.kept) != (key.base, key.txns, key.kept)
                    || (r.summary != key.summary && (h.faults.row_every_change() || closed()))
            }
        };
        if due && let Some(h) = self.collab.as_mut() {
            let now = h.licence.now_ms();
            h.server.record_sandbox(
                &h.owner,
                key.base,
                h.deltas.kept,
                h.deltas.summary.lines(),
                now,
            );
            h.recorded = Some(key);
            h.deltas.unsent.clear();
            h.rows_sent += 1;
            h.status = None;
        }
    }

    /// Pull `target` (`None`: the head) into the sandbox (see the module docs). The merge's
    /// conflicts stop it: they are kept, with the choices made so far.
    fn collab_pull(
        &mut self,
        target: Option<RevId>,
        issuer: &Issuer,
    ) -> Result<String, ProjectError> {
        let h = self
            .collab
            .as_ref()
            .ok_or_else(|| ProjectError::Team("no team server is attached".into()))?;
        let server = Arc::clone(&h.server);
        let base = h.base;
        let Some(target) = target.or_else(|| server.head()) else {
            return Ok("The baseline is empty".into());
        };
        if Some(target) == base {
            return Ok("Already up to date".into());
        }
        let incoming = server.revisions(base, usize::MAX);
        let n = incoming
            .iter()
            .position(|r| r.id == target)
            .map(|p| p + 1)
            .ok_or_else(|| {
                ProjectError::Team(format!(
                    "revision {} is not ahead of this sandbox's base",
                    forge_project::collab::short(Some(target))
                ))
            })?;
        let base_doc = server.doc_at(base)?;
        let theirs = server.doc_at(Some(target))?;
        let mine = ProjectDoc::from_project(self.bus.project());
        let faults = h.faults;
        let choices = h.choices.clone();
        let m = merge3(&base_doc, &mine, &theirs, &choices);
        if !m.is_clean() && !faults.swallow_conflicts() {
            let unresolved = m.unresolved().max(usize::from(m.hierarchy_loop.is_some()));
            if let Some(h) = self.collab.as_mut() {
                h.pending = Some((target, n, m));
                h.status = None;
            }
            return Err(ProjectError::Unresolved(unresolved));
        }
        let label = format!(
            "Pull {n} revision(s) from the team ({})",
            incoming[..n]
                .iter()
                .flat_map(|r| r.issuers.iter())
                .cloned()
                .collect::<std::collections::BTreeSet<String>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        );
        // A pull an automation session asked for (it works in its person's sandbox, Ch.37 §37.8)
        // brings in what teammates published — plugin grants and the default automation policy
        // included. As with a non-human's load (WP-33), widenings wait for the person and
        // restrictions take effect, audited (WP-21). A project nobody trusted takes no plugin grant
        // from its team either (WP-34): what the pull would add is held, whoever pulled. A sandbox
        // with no project open is the team baseline itself, which the person chose to join (a
        // human-only command) and which brings no plugin folder: it follows the person's join. A
        // trusted project takes from its team only what the person trusted (WP-35): a plugin grant,
        // a wider automation capability or a cached plugin the person has not seen is held against
        // the project in memory, and the person is asked again.
        let pulled = crate::trust::carried_settings(&m.doc.settings);
        let trusted = pulled.iter().all(|i| self.trust_approves(i));
        let baseline =
            (!trusted).then(|| crate::trust::security_settings(self.bus.project().settings()));
        let (held, untrusted) = self.held_for(issuer, &m.doc.settings, baseline.as_ref());
        let narrowed = self.narrowed_by(issuer, &m.doc.settings);
        let merged = if held.is_empty() {
            None
        } else {
            let mut d = m.doc.clone();
            crate::security::apply_held(&mut d.settings, &held);
            Some(d)
        };
        let cmd = if faults.pull_by_load() {
            merged.as_ref().unwrap_or(&m.doc).load_command()?
        } else {
            let ops = patch(&mine, merged.as_ref().unwrap_or(&m.doc));
            EditorCommand::Invoke {
                target: c::SYNC_CMD.into(),
                args: sync_args(&ops, Some(&target.to_string()), &label).to_string(),
            }
        };
        let env = self.bus.envelope(issuer.clone(), cmd);
        let applied = self.bus.apply(env);
        if let Ok(a) = &applied {
            // What the pull brings is the team's, not the person's own change (WP-35).
            self.core_applied.insert(a.seq);
            if !trusted {
                self.note_carried(&pulled);
            }
        }
        if let Err(r) = applied {
            let why = format!("the pull's patch was refused: {}", r.error);
            if let Some(h) = self.collab.as_mut() {
                h.precondition = Some(why.clone());
                h.status = None;
            }
            return Err(ProjectError::Team(why));
        }
        let source = format!(
            "the team baseline at {}",
            forge_project::collab::short(Some(target))
        );
        self.audit_narrowed(issuer, &source, &narrowed);
        let held = self.hold(issuer, &source, held, untrusted);
        let mine_left = sorted(m.doc.clone()) != sorted((*theirs).clone());
        for r in &incoming[..n] {
            self.audit.record_in(
                AuditOrigin::Teammate,
                &forge_project::collab::short(Some(r.id)),
                &r.issuers.join(" "),
                &issuer_user(issuer),
                "pulled",
                format!("{} ({} command(s))", r.message, r.commands),
            );
        }
        if let Some(h) = self.collab.as_mut() {
            h.base = Some(target);
            h.pending = None;
            h.choices.clear();
            h.precondition = None;
            if !mine_left {
                h.deltas.clear();
            }
            h.status = None;
        }
        Ok(format!(
            "Pulled {n} revision(s){}{}",
            if m.rekeyed.is_empty() {
                String::new()
            } else {
                format!(
                    "; {} new entit{} of yours got new keys (a teammate used theirs)",
                    m.rekeyed.len(),
                    if m.rekeyed.len() == 1 { "y" } else { "ies" }
                )
            },
            super::held_note(held)
        ))
    }

    /// Check a collaboration command against the core's state before the bus applies it.
    pub(super) fn collab_check(
        &self,
        target: &str,
        args: &str,
        issuer: &Issuer,
    ) -> Result<(), Rejection> {
        if !c::PERFORMED.contains(&target) {
            return Ok(());
        }
        let Some(h) = self.collab.as_ref() else {
            return Err(refuse(ProjectError::Team(
                "no team server is attached to this editor".into(),
            )));
        };
        let project = self.bus.project();
        let a: Json = serde_json::from_str(args).unwrap_or(Json::Null);
        let user = c::member_of(issuer, &h.owner);
        match target {
            c::TEAM_CREATE => {
                if h.server.team().is_some() || h.server.head().is_some() {
                    return Err(refuse(ProjectError::Team(
                        "this team server already holds a team's baseline: join it with a code instead".into(),
                    )));
                }
            }
            c::TEAM_ACCEPT => {
                let token = a.get("token").and_then(Json::as_str).unwrap_or("").trim();
                let team = h.server.team().and_then(|t| h.identity.team_record(&t));
                let email = h.identity.account(&user).map(|x| x.email.to_lowercase());
                let ok = team.is_some_and(|t| {
                    t.invites.iter().any(|i| match &i.to {
                        InviteTo::Code(code) => code.eq_ignore_ascii_case(token),
                        InviteTo::Email(e) => {
                            i.id == token && email.as_deref() == Some(e.to_lowercase().as_str())
                        }
                    })
                });
                if !ok {
                    return Err(refuse(ProjectError::Team(
                        "that join code or invite is not valid here (wrong, revoked, used, or for another account)".into(),
                    )));
                }
            }
            c::CLAIM => {
                let k = EntityKey(a.get("entity").and_then(Json::as_u64).unwrap_or(u64::MAX));
                if !project.contains(k) {
                    return Err(refuse(format!("there is no entity {k} to claim")));
                }
                // Overlap: a claim of someone else on an ancestor or a descendant.
                for cl in h.server.claims().iter().filter(|cl| cl.holder != user) {
                    let other = EntityKey(cl.entity);
                    if is_ancestor(project, other, k) || is_ancestor(project, k, other) {
                        return Err(refuse(ProjectError::Claimed(format!(
                            "\u{201c}{}\u{201d} overlaps {}'s claim on \u{201c}{}\u{201d}",
                            entity_label(project, k),
                            cl.holder,
                            cl.label
                        ))));
                    }
                }
            }
            c::PUBLISH => {
                if !h.joined(project) {
                    return Err(refuse(ProjectError::Team(
                        "this sandbox does not follow a team baseline".into(),
                    )));
                }
                if let Some((_, _, m)) = &h.pending {
                    return Err(refuse(ProjectError::Unresolved(m.unresolved().max(1))));
                }
                if self.bus.open_transactions().next().is_some() {
                    return Err(refuse(
                        "an edit is still in progress: finish or cancel it, then publish",
                    ));
                }
                if h.server.head() != h.base {
                    return Err(refuse(ProjectError::BaselineMoved(
                        "the baseline moved since this sandbox's base: pull, then publish".into(),
                    )));
                }
                let base_doc = h.server.doc_at(h.base).map_err(refuse)?;
                let mine = self.project.written_doc(project);
                if sorted(mine.clone()) == sorted((*base_doc).clone()) {
                    return Err(refuse("there is nothing to publish"));
                }
                let paths = changed_paths(&base_doc, &mine);
                let scopes = h
                    .team(project)
                    .and_then(|t| h.identity.team(&t))
                    .and_then(|t| t.member(&user).map(|m| m.paths.clone()))
                    .unwrap_or_default();
                if !scopes.is_empty()
                    && let Some(out) = paths
                        .iter()
                        .find(|p| !scopes.iter().any(|g| glob_match(g, p)))
                {
                    return Err(refuse(ProjectError::NotPermitted(format!(
                        "{out} is outside your publish scope ({}): ask a Maintainer to publish it or widen your scope",
                        scopes.join(", ")
                    ))));
                }
            }
            c::PULL => {
                if !h.joined(project) {
                    return Err(refuse(ProjectError::Team(
                        "this sandbox does not follow a team baseline".into(),
                    )));
                }
            }
            c::RESOLVE if h.pending.is_none() => {
                return Err(refuse("there are no conflicts to resolve"));
            }
            _ => {}
        }
        Ok(())
    }

    /// Perform a collaboration command the bus accepted (see the module docs).
    pub(super) fn collab_perform(&mut self, target: &str, args: &str, issuer: &Issuer) {
        if !c::PERFORMED.contains(&target) || self.collab.is_none() {
            return;
        }
        let a: Json = serde_json::from_str(args).unwrap_or(Json::Null);
        let r = self.collab_do(target, &a, issuer);
        let user = issuer_user(issuer);
        let sandbox = self
            .collab
            .as_ref()
            .map(|h| h.owner.clone())
            .unwrap_or_default();
        let event = target.rsplit('.').next().unwrap_or(target).to_string();
        self.audit.record_in(
            AuditOrigin::Team,
            "",
            &user,
            &sandbox,
            &event,
            match &r {
                Ok(m) => m.clone(),
                Err(e) => format!("refused: {e}"),
            },
        );
        if let Some(h) = self.collab.as_mut() {
            h.outcome(target, r);
        }
        self.collab_changed();
    }

    fn collab_do(
        &mut self,
        target: &str,
        a: &Json,
        issuer: &Issuer,
    ) -> Result<String, ProjectError> {
        let Some(h) = self.collab.as_ref() else {
            return Err(ProjectError::Team("no team server is attached".into()));
        };
        let identity = Arc::clone(&h.identity);
        let server = Arc::clone(&h.server);
        let now = h.licence.now_ms();
        let owner = h.owner.clone();
        let user = c::member_of(issuer, &owner);
        let s = |k: &str| a.get(k).and_then(Json::as_str).unwrap_or("").to_string();
        let project_team = h.team(self.bus.project());
        let team = || project_team.clone().ok_or(ProjectError::NotOpen);
        match target {
            c::TEAM_CREATE => {
                let t = identity.create_team(&s("name"), &user, now)?;
                server.bind(&t.id)?;
                self.collab_bind(&t.id, &t.name, issuer)?;
                // As a save writes it: no automation grant (per run), held security as the files
                // hold it (WP-33).
                let doc = self.project.written_doc(self.bus.project());
                let rev = server.publish(Publish {
                    author: user.clone(),
                    base: None,
                    message: format!("Team \u{201c}{}\u{201d} created by {user}", t.name),
                    doc,
                    envelopes: Vec::new(),
                    paths: Vec::new(),
                    summary: Vec::new(),
                    at_ms: now,
                })?;
                if let Some(h) = self.collab.as_mut() {
                    h.base = Some(rev);
                    h.deltas.clear();
                }
                Ok(format!(
                    "Created team \u{201c}{}\u{201d}; you are its Owner and this project is its baseline",
                    t.name
                ))
            }
            c::TEAM_INVITE => {
                let role = Role::parse(&s("role")).unwrap_or(Role::Developer);
                let email = a.get("email").and_then(Json::as_str).map(str::to_string);
                let paths = c::paths_arg(target, a).unwrap_or_default();
                let inv = identity.invite(
                    &team()?,
                    InviteSpec {
                        email,
                        role,
                        paths: paths.clone(),
                    },
                    &user,
                    now,
                )?;
                // The outcome is audited: a join code is a credential, so its text stays in
                // the pending list (shown only to those who manage members), not in the log.
                let to = if matches!(inv.to, InviteTo::Code(_)) {
                    "a join code".to_string()
                } else {
                    inv.target()
                };
                Ok(format!(
                    "Invited {to} as {}{}",
                    role.label(),
                    if paths.is_empty() {
                        String::new()
                    } else {
                        format!(", publishing to {}", paths.join(", "))
                    }
                ))
            }
            c::TEAM_REVOKE_INVITE => {
                let inv = identity.revoke_invite(&team()?, &s("invite"))?;
                Ok(format!("Revoked the invite for {}", inv.target()))
            }
            c::TEAM_ACCEPT => {
                let (t, m) = identity.accept(&user, &s("token"), now)?;
                self.collab_bind(&t.id, &t.name, issuer)?;
                // Rebuild this sandbox from the baseline, keeping its keys.
                let head = server.head();
                let theirs = server.doc_at(head)?;
                let mine = ProjectDoc::from_project(self.bus.project());
                let ops = patch(&mine, &theirs);
                if !ops.is_empty() {
                    let env = self.bus.envelope(
                        issuer.clone(),
                        EditorCommand::Invoke {
                            target: c::SYNC_CMD.into(),
                            args: sync_args(
                                &ops,
                                head.map(|r| r.to_string()).as_deref(),
                                "Join the team baseline",
                            )
                            .to_string(),
                        },
                    );
                    self.bus
                        .apply(env)
                        .map_err(|r| ProjectError::Team(format!("joining failed: {}", r.error)))?;
                }
                if let Some(h) = self.collab.as_mut() {
                    h.base = head;
                    h.deltas.clear();
                    h.pending = None;
                }
                Ok(format!(
                    "Joined \u{201c}{}\u{201d} as {}; this editor now shows its baseline",
                    t.name,
                    m.role.label()
                ))
            }
            c::TEAM_SET_ROLE => {
                let who = s("user");
                let role = Role::parse(&s("role")).unwrap_or(Role::Viewer);
                let old = identity.set_role(&team()?, &who, role)?;
                Ok(format!("{who}: {} \u{2192} {}", old.label(), role.label()))
            }
            c::TEAM_SET_PATHS => {
                let who = s("user");
                let paths = c::paths_arg(target, a).unwrap_or_default();
                identity.set_paths(&team()?, &who, paths.clone())?;
                Ok(if paths.is_empty() {
                    format!("{who} may publish everywhere")
                } else {
                    format!("{who} may publish to {}", paths.join(", "))
                })
            }
            c::TEAM_REMOVE => {
                let who = s("user");
                let m = identity.remove_member(&team()?, &who)?;
                let gone = server.release_all(&who);
                server.clear_presence(&who);
                Ok(format!(
                    "Removed {who} ({}); {} claim(s) released",
                    m.role.label(),
                    gone.len()
                ))
            }
            c::CLAIM => {
                let k = EntityKey(a.get("entity").and_then(Json::as_u64).unwrap_or(0));
                let label = entity_label(self.bus.project(), k);
                server.claim(Claim {
                    entity: k.0,
                    label: label.clone(),
                    holder: user,
                    at_ms: now,
                })?;
                Ok(format!(
                    "Claimed \u{201c}{label}\u{201d}: it is read-only for everyone else"
                ))
            }
            c::RELEASE => {
                let k = a.get("entity").and_then(Json::as_u64).unwrap_or(0);
                let force = project_team
                    .as_deref()
                    .and_then(|t| identity.team(t))
                    .and_then(|t| t.role_of(&user))
                    .is_some_and(Role::maintains);
                let cl = server.release(k, &user, force)?;
                Ok(format!("Released \u{201c}{}\u{201d}", cl.label))
            }
            c::PUBLISH => self.collab_publish(&user, &s("message"), now),
            c::APPROVE => {
                let id = a.get("request").and_then(Json::as_u64).unwrap_or(0);
                let rev = server.approve(id, &user)?;
                Ok(format!(
                    "Approved request #{id}: published as revision {}",
                    forge_project::collab::short(Some(rev))
                ))
            }
            c::REJECT => {
                let id = a.get("request").and_then(Json::as_u64).unwrap_or(0);
                server.reject(id, &user, &s("reason"))?;
                Ok(format!("Rejected request #{id}"))
            }
            c::PULL => {
                let rev = c::rev_arg(a).ok().flatten();
                self.collab_pull(rev, issuer)
            }
            c::RESOLVE => {
                let choices = c::choices_arg(a).unwrap_or_default();
                let target_rev = {
                    let Some(h) = self.collab.as_mut() else {
                        return Err(ProjectError::NotOpen);
                    };
                    h.choices.extend(choices);
                    h.pending.as_ref().map(|(t, ..)| *t)
                };
                if let Some(h) = self.collab.as_mut() {
                    h.pending = None;
                }
                match target_rev {
                    Some(t) => self.collab_pull(Some(t), issuer),
                    None => Ok("Nothing to resolve".into()),
                }
            }
            _ => Ok(String::new()),
        }
    }

    /// Bind the project to a team: the reserved `team.*` settings, by the core's command.
    fn collab_bind(&mut self, team: &str, name: &str, issuer: &Issuer) -> Result<(), ProjectError> {
        let env = self.bus.envelope(
            issuer.clone(),
            EditorCommand::Invoke {
                target: c::TEAM_BIND.into(),
                args: serde_json::json!({"team": team, "name": name}).to_string(),
            },
        );
        self.bus
            .apply(env)
            .map(drop)
            .map_err(|r| ProjectError::Team(format!("binding the project failed: {}", r.error)))
    }

    fn collab_publish(
        &mut self,
        user: &str,
        message: &str,
        now: u64,
    ) -> Result<String, ProjectError> {
        let kept = self.kept_deltas();
        let Some(h) = self.collab.as_ref() else {
            return Err(ProjectError::NotOpen);
        };
        let server = Arc::clone(&h.server);
        let base = h.base;
        let base_doc = server.doc_at(base)?;
        // The revision is the sandbox's document; the story rides along as metadata.
        let doc = if h.faults.publish_by_replay() {
            replay_story(&base_doc, &kept)?
        } else {
            self.project.written_doc(self.bus.project())
        };
        let paths = changed_paths(&base_doc, &doc);
        let summary = h.deltas.summary.lines();
        let role = h
            .team(self.bus.project())
            .and_then(|t| h.identity.team(&t))
            .and_then(|t| t.role_of(user));
        // The team's rules are the baseline's (published like any project setting; the
        // sandbox is on the head, and nobody without the role can change them).
        let (required, rules) =
            c::review_rules(base_doc.settings.iter().map(|(k, v)| (k.as_str(), v)));
        let gated =
            c::review_needed(required, &rules, &paths) && !role.is_some_and(Role::maintains);
        let message = if message.trim().is_empty() {
            format!(
                "{user}: {}",
                summary.first().cloned().unwrap_or_else(|| "changes".into())
            )
        } else {
            message.trim().to_string()
        };
        let p = Publish {
            author: user.to_string(),
            base,
            message,
            doc,
            envelopes: kept,
            paths,
            summary,
            at_ms: now,
        };
        if gated {
            let id = server.submit(p)?;
            return Ok(format!(
                "Submitted for review as request #{id}: a Maintainer or Reviewer approves it"
            ));
        }
        let rev = server.publish(p)?;
        if let Some(h) = self.collab.as_mut() {
            h.base = Some(rev);
            h.deltas.clear();
            h.status = None;
        }
        Ok(format!(
            "Published revision {} to the team baseline",
            forge_project::collab::short(Some(rev))
        ))
    }
}

/// `test_sandbox_story_metadata`'s positive control only (`CollabFaults::publish_by_replay`):
/// the defect the story's docs forbid — `base` with the story's commands applied on a
/// scratch bus, as if the folded list were a command log.
fn replay_story(base: &ProjectDoc, story: &[CommandEnvelope]) -> Result<ProjectDoc, ProjectError> {
    let mut bus = EditorCore::editor_bus();
    let load = base.load_command()?;
    let who = Issuer::Human {
        user: "replay".into(),
    };
    let e = bus.envelope(who, load);
    bus.apply(e)
        .map_err(|r| ProjectError::Team(format!("replay: {}", r.error)))?;
    for s in story {
        let e = bus.envelope(s.issuer.clone(), s.cmd.clone());
        // A command the base refuses is skipped, as a naive replay would.
        let _ = bus.apply(e);
    }
    Ok(ProjectDoc::from_project(bus.project()))
}

/// Is `a` an ancestor of (or the same as) `b` in `p`?
fn is_ancestor(p: &forge_cmd::Project, a: EntityKey, b: EntityKey) -> bool {
    let mut cur = Some(b);
    let mut steps = 0usize;
    while let Some(x) = cur {
        if x == a {
            return true;
        }
        steps += 1;
        if steps > p.len() {
            return false;
        }
        cur = p.entity(x).and_then(|e| e.parent());
    }
    false
}

fn issuer_user(i: &Issuer) -> String {
    AuditRecord::user_of(i)
}
