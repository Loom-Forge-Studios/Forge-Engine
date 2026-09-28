//! The bus: [`CommandSink`], the in-process [`Bus`] (the headless core), transactions,
//! undo/redo, the audit trail and the [`Applied`] stream.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use forge_core::ErrorCode;
use forge_reflect::CommandDesc;
use serde::{Deserialize, Serialize};

use crate::audit::{AuditAction, AuditEntry, Clock, SystemClock};
use crate::diff::Accumulator;
use crate::stream::{DEFAULT_SUBSCRIPTION_CAPACITY, Publisher, Subscription};
use crate::{
    Change, CmdError, CommandEnvelope, CommandId, CommandPolicy, Diff, DiffBuilder, EditorCommand,
    EntityKey, Issuer, Project, Rejection, TxnId, TxnState,
};

/// The one way to change project state (Ch.7.1). The in-process [`Bus`] implements it; the
/// split editor's `RemoteBus` (M2-16) will too, and panels cannot tell them apart.
pub trait CommandSink {
    /// Compute the effect without applying it. **Total** (a panicking command is a
    /// `Rejection`, never an unwind) and **never mutates** (`&self`). The diff is exactly what
    /// [`CommandSink::apply`] would apply now — the split editor shows it as the optimistic
    /// preview (O-13).
    fn dry_run(&self, e: &CommandEnvelope) -> Result<Diff, Rejection>;
    /// Apply a command: all of its effect or none of it. The event is shared (`Arc`) with
    /// every subscriber rather than copied per consumer.
    fn apply(&mut self, e: CommandEnvelope) -> Result<Arc<Applied>, Rejection>;
    /// Undo a whole transaction (one drag = one undo), on behalf of the transaction's own
    /// issuer. Undoing an *open* transaction cancels it (Esc during a gesture). A client that
    /// acts for someone else uses [`Bus::undo_as`], which refuses to cancel another issuer's
    /// open gesture (`CMD-0012`).
    fn undo(&mut self, txn: TxnId) -> Result<Arc<Applied>, Rejection>;
    /// Redo an undone transaction.
    fn redo(&mut self, txn: TxnId) -> Result<Arc<Applied>, Rejection>;
}

/// What an [`Applied`] event is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum AppliedKind {
    /// A command was applied (inside a gesture, one per frame; otherwise its own undo step).
    Command,
    /// A transaction was committed (its changes were already streamed; the diff is empty).
    Commit,
    /// An open transaction was cancelled; the diff reverts what it had applied.
    Cancel,
    /// A transaction was undone; the diff is the inverse of its net changes.
    Undo,
    /// A transaction was redone; the diff re-applies its net changes.
    Redo,
}

/// A state change that happened, as the UI, the split editor and the collab layer observe
/// it (Ch.21.18: state flows back **only** through this stream, for every issuer).
/// Serializable: the split editor's wire carries it to a remote client (M2-16).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    /// Monotonic order on this bus. A mirror applies events in `seq` order.
    pub seq: u64,
    /// What happened.
    pub kind: AppliedKind,
    /// The transaction.
    pub txn: TxnId,
    /// The command, for [`AppliedKind::Command`].
    pub command: Option<CommandId>,
    /// Who caused it (provenance: the undo-history panel tags `automation:<session>` from this).
    pub issuer: Issuer,
    /// Exactly what changed.
    pub diff: Diff,
}

/// Plans an [`EditorCommand::Invoke`] target: a mutating `#[forge_api]` fn or a plugin
/// command. It gets a [`DiffBuilder`] — never `&mut Project` — so it can only *describe*
/// changes, and every change it describes is undoable and provenance-tagged.
pub trait CommandHandler: Send + Sync {
    /// Describe the effect of calling the target with `args` (a JSON object).
    fn plan(&self, b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError>;
}

impl<F> CommandHandler for F
where
    F: Fn(&mut DiffBuilder<'_>, &serde_json::Value) -> Result<(), CmdError> + Send + Sync,
{
    fn plan(&self, b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
        self(b, args)
    }
}

struct HandlerEntry {
    handler: Box<dyn CommandHandler>,
    policy: CommandPolicy,
    /// The argument names, when registered from a `CommandDesc` (checked before planning).
    fields: Option<Vec<&'static str>>,
}

/// The commands a transaction carried: a span and a count, not a list, so a gesture's
/// memory does not grow per frame. (Ids inside the span may belong to other transactions
/// interleaved with it; `count` is exact.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CommandSpan {
    /// The first command applied in the transaction.
    pub first: Option<CommandId>,
    /// The latest command applied in the transaction.
    pub last: Option<CommandId>,
    /// How many commands were applied in it.
    pub count: u64,
}

/// A reserved setting prefix (see [`Bus::reserve_settings`]).
#[derive(Debug)]
struct ReservedSettings {
    prefix: String,
    writers: BTreeSet<String>,
}

impl ReservedSettings {
    /// `key` is `prefix` itself or a key under it (`prefix.…`); `security` does not reserve
    /// `securityx`.
    fn covers(&self, key: &str) -> bool {
        key.strip_prefix(self.prefix.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    }
}

/// What a [`CommandGuard`] is asked about (see [`Bus::set_guard`]).
#[derive(Clone, Copy, Debug)]
pub struct Guarded<'a> {
    /// Who asks: the envelope's issuer, or who presses undo / redo.
    pub issuer: &'a Issuer,
    /// The command (`None`: an undo or redo replaying `diff`).
    pub cmd: Option<&'a EditorCommand>,
    /// What it would change.
    pub diff: &'a Diff,
    /// The project before `diff` (a batch preview's scratch copy in a batch).
    pub project: &'a Project,
}

/// A check a host adds to every change its bus applies (additive, WP-U10): who may change
/// what, decided **per command** from the issuer and the planned diff. The editor core uses
/// it for Ch.37's team roles and ownership claims: a role reduced mid-session is felt by the
/// very next command, and an edit inside a subtree a teammate claimed is refused whoever
/// sends it. It runs after planning in `apply`, a dry run and a batch preview (so the three
/// agree), and before an undo or redo is applied; never for a cancel, which only reverts an
/// open gesture of the canceller's own. A refusal applies nothing and is a [`Rejection`].
pub trait CommandGuard: Send + Sync {
    /// `Ok` to allow; the error is the refusal (`CMD-0014` by convention).
    fn check(&self, g: &Guarded<'_>) -> Result<(), CmdError>;
}

/// Derived changes a host adds to **every** command's diff (additive, WP-U20): after a
/// command is planned, the deriver sees the builder holding its changes and may append the
/// changes they imply, or refuse the command. Scene composition uses it so that an edit of
/// a base scene's node reaches every instance of it in the same diff — whoever sends the
/// edit, through whichever command — and so one undo step takes the edit and all it
/// propagated back together.
///
/// It runs inside the planning step, so `apply`, a dry run, a batch preview and
/// [`Bus::plan_for`] all see the same derived diff (I8); undo and redo replay the recorded
/// diff and never derive again. Like a handler it gets a [`DiffBuilder`], never
/// `&mut Project`, and a panic in it is `CMD-0009`.
pub trait CommandDeriver: Send + Sync {
    /// Append what `cmd`'s planned changes (`b.changes()`) imply, or refuse `cmd`.
    fn derive(&self, cmd: &EditorCommand, b: &mut DiffBuilder<'_>) -> Result<(), CmdError>;
}

impl CommandSpan {
    fn record(&mut self, id: CommandId) {
        if self.first.is_none() {
            self.first = Some(id);
        }
        self.last = Some(id);
        self.count += 1;
    }
}

/// One transaction, as the undo-history panel lists it.
#[derive(Debug, Clone)]
pub struct TxnRecord {
    txn: TxnId,
    label: String,
    issuer: Issuer,
    state: TxnState,
    acc: Accumulator,
    commands: CommandSpan,
    opened_at_ms: u64,
    undoable: bool,
    redoable: bool,
}

impl TxnRecord {
    fn new(txn: TxnId, label: String, issuer: Issuer, at: u64, state: TxnState) -> Self {
        Self {
            txn,
            label,
            issuer,
            state,
            acc: Accumulator::default(),
            commands: CommandSpan::default(),
            opened_at_ms: at,
            undoable: true,
            redoable: true,
        }
    }

    /// Its id.
    #[must_use]
    pub fn txn(&self) -> TxnId {
        self.txn
    }

    /// Its label (a gesture's label, or the command's).
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Who opened it.
    #[must_use]
    pub fn issuer(&self) -> &Issuer {
        &self.issuer
    }

    /// Its state.
    #[must_use]
    pub fn state(&self) -> TxnState {
        self.state
    }

    /// Its net changes (merged: a drag is one change per property it touched).
    pub fn changes(&self) -> impl Iterator<Item = &Change> {
        self.acc.changes()
    }

    /// The commands applied in it.
    #[must_use]
    pub fn commands(&self) -> CommandSpan {
        self.commands
    }

    /// When it was opened (the bus clock).
    #[must_use]
    pub fn opened_at_ms(&self) -> u64 {
        self.opened_at_ms
    }

    /// Whether its commands allow undo / redo.
    #[must_use]
    pub fn undoable(&self) -> bool {
        self.undoable
    }

    /// See [`TxnRecord::undoable`].
    #[must_use]
    pub fn redoable(&self) -> bool {
        self.redoable
    }

    fn absorb_policy(&mut self, p: CommandPolicy) {
        self.undoable &= p.undoable;
        self.redoable &= p.redoable;
    }
}

/// Default bound on undo history (transactions). Older ones expire (no longer undoable) and
/// their records are dropped.
pub const DEFAULT_MAX_HISTORY: usize = 10_000;

/// How many recently applied command ids the bus remembers exactly for duplicate refusal
/// (`CMD-0013`). Ids below the window are refused as stale: the bus allocates ids in
/// increasing order, so an id that old is a resend (or an envelope held back for thousands
/// of commands), never a new command. Memory is fixed, not one entry per command ever sent.
pub const REPLAY_WINDOW: usize = 4096;

/// How many retired transactions (expired, cancelled, empty commits) keep an exact
/// tombstone. Older retired ids report [`TxnState::Expired`] from a watermark.
pub const RETIRED_WINDOW: usize = 4096;

/// Default bound on the in-memory audit trail (entries) between drains. The persistence
/// layer drains it; if nothing does, the oldest quarter is dropped and counted in
/// [`Bus::audit_dropped`] — never silently.
pub const DEFAULT_MAX_AUDIT: usize = 65_536;

/// What the bus holds in memory, for budgets and the memory guard. Everything here is
/// bounded by the history cap, the touched properties and the fixed windows — none of it
/// grows per gesture frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Footprint {
    /// Live transaction records (open, plus committed/undone ones in the history).
    pub transactions: usize,
    /// Accumulated change slots across those records.
    pub change_slots: usize,
    /// Retired-transaction tombstones (at most [`RETIRED_WINDOW`]).
    pub tombstones: usize,
    /// Remembered command ids (at most [`REPLAY_WINDOW`]).
    pub replay_ids: usize,
    /// Audit entries not yet drained.
    pub audit_entries: usize,
    /// Audit-folding index entries (one per target of the gesture that owns the trail's
    /// tail; at most one gesture folds at a time).
    pub audit_fold_index: usize,
    /// Live stream subscriptions (each bounded by its capacity).
    pub subscribers: usize,
}

/// What a gesture-frame audit entry folds on: the command's target. Matching is a borrowed
/// comparison against the incoming command, so a fold costs no allocation; the owned copy
/// is made once, when a new line starts.
#[derive(Clone, PartialEq, Eq, Debug)]
enum AuditTarget {
    Property(EntityKey, Box<str>),
    Setting(Box<str>),
    Name(EntityKey),
    Invoke(Box<str>),
}

impl AuditTarget {
    fn of(cmd: &EditorCommand) -> Option<Self> {
        match cmd {
            EditorCommand::SetProperty { entity, path, .. } => {
                Some(Self::Property(*entity, path.as_str().into()))
            }
            EditorCommand::SetSetting { key, .. } => Some(Self::Setting(key.as_str().into())),
            EditorCommand::Rename { entity, .. } => Some(Self::Name(*entity)),
            EditorCommand::Invoke { target, .. } => Some(Self::Invoke(target.as_str().into())),
            EditorCommand::Spawn { .. }
            | EditorCommand::Despawn { .. }
            | EditorCommand::Reparent { .. }
            | EditorCommand::RemoveProperty { .. } => None,
        }
    }

    fn foldable(cmd: &EditorCommand) -> bool {
        matches!(
            cmd,
            EditorCommand::SetProperty { .. }
                | EditorCommand::SetSetting { .. }
                | EditorCommand::Rename { .. }
                | EditorCommand::Invoke { .. }
        )
    }

    fn matches(&self, cmd: &EditorCommand) -> bool {
        match (self, cmd) {
            (Self::Property(e, p), EditorCommand::SetProperty { entity, path, .. }) => {
                e == entity && **p == **path
            }
            (Self::Setting(k), EditorCommand::SetSetting { key, .. }) => **k == **key,
            (Self::Name(e), EditorCommand::Rename { entity, .. }) => e == entity,
            (Self::Invoke(t), EditorCommand::Invoke { target, .. }) => **t == **target,
            _ => false,
        }
    }
}

/// One folded audit line of the gesture that owns the trail's tail.
#[derive(Debug)]
struct FoldLine {
    target: AuditTarget,
    outcome: Option<ErrorCode>,
    seq: u64,
}

/// The gesture whose frames fold. Folding never reorders the trail: it is valid only while
/// every entry from its first line onward is one of its own lines. Any other entry (another
/// transaction's command, someone else's refused attempt, a begin/commit) ends it, and the
/// gesture's next frame starts a fresh line after that entry.
#[derive(Debug)]
struct AuditFold {
    txn: TxnId,
    lines: Vec<FoldLine>,
}

/// The headless core: owns the [`Project`] and is the only thing that changes it.
pub struct Bus {
    project: Project,
    /// Live records only: open transactions and those in `history`. Retired ones leave.
    txns: HashMap<TxnId, TxnRecord>,
    /// Non-empty committed transactions (and undone ones, which stay listed), commit order.
    history: VecDeque<TxnId>,
    redo_stack: Vec<TxnId>,
    max_history: usize,
    /// Final states of recently retired transactions (bounded by [`RETIRED_WINDOW`]).
    retired: BTreeMap<TxnId, TxnState>,
    /// Retired ids below this that have no tombstone left report `Expired`.
    txn_floor: u64,
    /// Recently applied command ids (bounded by [`REPLAY_WINDOW`]).
    seen_commands: BTreeSet<CommandId>,
    /// Ids below this are refused as stale resends.
    cmd_floor: u64,
    handlers: BTreeMap<String, HandlerEntry>,
    /// Reserved setting prefixes and the `Invoke` targets that alone may change them (see
    /// [`Bus::reserve_settings`]). A handful at most, so a list.
    reserved: Vec<ReservedSettings>,
    /// The host's per-command check (see [`Bus::set_guard`]).
    guard: Option<Arc<dyn CommandGuard>>,
    /// The host's deriver (see [`Bus::set_deriver`]).
    deriver: Option<Arc<dyn CommandDeriver>>,
    subscribers: Publisher,
    audit: Vec<AuditEntry>,
    max_audit: usize,
    audit_dropped: u64,
    /// The gesture folding into the trail's tail, if any.
    audit_fold: Option<AuditFold>,
    clock: Box<dyn Clock>,
    next_cmd: u64,
    next_txn: u64,
    seq: u64,
    audit_seq: u64,
    poisoned: bool,
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus {
    /// A bus over an empty project, with the system clock.
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(Box::new(SystemClock))
    }

    /// A bus with an injected clock (audit timestamps).
    #[must_use]
    pub fn with_clock(clock: Box<dyn Clock>) -> Self {
        Self {
            project: Project::new(),
            txns: HashMap::new(),
            history: VecDeque::new(),
            redo_stack: Vec::new(),
            max_history: DEFAULT_MAX_HISTORY,
            retired: BTreeMap::new(),
            txn_floor: 0,
            seen_commands: BTreeSet::new(),
            cmd_floor: 0,
            handlers: BTreeMap::new(),
            reserved: Vec::new(),
            guard: None,
            deriver: None,
            subscribers: Publisher::new(),
            audit: Vec::new(),
            max_audit: DEFAULT_MAX_AUDIT,
            audit_dropped: 0,
            audit_fold: None,
            clock,
            next_cmd: 0,
            next_txn: 0,
            seq: 0,
            audit_seq: 0,
            poisoned: false,
        }
    }

    /// Bound the undo history to `n` transactions (at least 1).
    #[must_use]
    pub fn with_max_history(mut self, n: usize) -> Self {
        self.max_history = n.max(1);
        self
    }

    /// Bound the undrained audit trail to `n` entries (at least 4).
    #[must_use]
    pub fn with_max_audit(mut self, n: usize) -> Self {
        self.max_audit = n.max(4);
        self
    }

    /// What the bus holds in memory right now.
    #[must_use]
    pub fn footprint(&self) -> Footprint {
        Footprint {
            transactions: self.txns.len(),
            change_slots: self.txns.values().map(|r| r.acc.slot_count()).sum(),
            tombstones: self.retired.len(),
            replay_ids: self.seen_commands.len(),
            audit_entries: self.audit.len(),
            audit_fold_index: self.audit_fold.as_ref().map_or(0, |f| f.lines.len()),
            subscribers: self.subscribers.len(),
        }
    }

    /// The project, read-only. There is no `project_mut`.
    #[must_use]
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// True once an unrecoverable failure made the bus refuse everything (`CMD-0015`).
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    // ---- ids and envelopes -------------------------------------------------------------

    /// A fresh command id.
    pub fn next_command_id(&mut self) -> CommandId {
        let id = CommandId(self.next_cmd);
        self.next_cmd += 1;
        id
    }

    /// A fresh transaction id (not opened: an envelope carrying it is its own undo step).
    pub fn next_txn_id(&mut self) -> TxnId {
        let id = TxnId(self.next_txn);
        self.next_txn += 1;
        id
    }

    /// An envelope for one command in its own transaction.
    pub fn envelope(&mut self, issuer: Issuer, cmd: EditorCommand) -> CommandEnvelope {
        let txn = self.next_txn_id();
        self.envelope_in(txn, issuer, cmd)
    }

    /// An envelope for a command inside `txn` (an open gesture).
    pub fn envelope_in(
        &mut self,
        txn: TxnId,
        issuer: Issuer,
        cmd: EditorCommand,
    ) -> CommandEnvelope {
        CommandEnvelope {
            id: self.next_command_id(),
            txn,
            issuer,
            cmd,
        }
    }

    // ---- handlers ----------------------------------------------------------------------

    /// Register an `Invoke` target.
    pub fn register_handler(
        &mut self,
        target: &str,
        policy: CommandPolicy,
        handler: impl CommandHandler + 'static,
    ) -> Result<(), CmdError> {
        self.insert_handler(target, policy, None, Box::new(handler))
    }

    /// Register the command output of a mutating `#[forge_api]` fn (Ch.6 output 4). The
    /// arguments are checked against the descriptor's fields before the handler runs, and a
    /// `destructive` fn is human-only until the capability-grant model (Ch.22.3) exists.
    pub fn register_api(
        &mut self,
        desc: &CommandDesc,
        handler: impl CommandHandler + 'static,
    ) -> Result<(), CmdError> {
        let policy = CommandPolicy {
            human_only: desc.destructive,
            ..CommandPolicy::ORDINARY
        };
        let fields = desc.fields.iter().map(|p| p.name).collect();
        self.insert_handler(&desc.target, policy, Some(fields), Box::new(handler))
    }

    fn insert_handler(
        &mut self,
        target: &str,
        policy: CommandPolicy,
        fields: Option<Vec<&'static str>>,
        handler: Box<dyn CommandHandler>,
    ) -> Result<(), CmdError> {
        if self.handlers.contains_key(target) {
            return Err(CmdError::DuplicateHandler(target.to_string()));
        }
        self.handlers.insert(
            target.to_string(),
            HandlerEntry {
                handler,
                policy,
                fields,
            },
        );
        Ok(())
    }

    /// Every registered `Invoke` target, sorted (additive, WP-16: what the command surface
    /// is — `test_no_preset_gating` checks it is the same under every preset, I15).
    pub fn targets(&self) -> impl Iterator<Item = &str> {
        self.handlers.keys().map(String::as_str)
    }

    /// Reserve the settings under `prefix` (the key `prefix` and every `prefix.…` key) for
    /// the `Invoke` targets `writers`. A command that is not one of them (a `SetSetting`, any
    /// other handler, a plugin's or a `#[forge_api]` command) is refused with `CMD-0014`,
    /// nothing applied, when its diff changes such a key: in `apply`, in a dry run and in a
    /// batch preview alike. The editor core reserves its security state this way (Ch.21
    /// §21.18): its grant table follows `security.*`, so only the human-only security
    /// commands may write there. Undo, redo and cancel replay diffs these rules already
    /// admitted. Calling it again for the same prefix adds writers.
    pub fn reserve_settings(&mut self, prefix: &str, writers: &[&str]) {
        let writers = writers.iter().map(|w| (*w).to_string());
        match self.reserved.iter_mut().find(|r| r.prefix == prefix) {
            Some(r) => r.writers.extend(writers),
            None => self.reserved.push(ReservedSettings {
                prefix: prefix.to_string(),
                writers: writers.collect(),
            }),
        }
    }

    /// The targets that alone may change setting `key`, when a reserved prefix covers it.
    #[must_use]
    pub fn setting_writers(&self, key: &str) -> Option<Vec<&str>> {
        self.reserved
            .iter()
            .find(|r| r.covers(key))
            .map(|r| r.writers.iter().map(String::as_str).collect())
    }

    /// Install (or with `None` remove) the host's per-command check (see [`CommandGuard`]).
    /// One guard per bus; a host that needs several composes them.
    pub fn set_guard(&mut self, guard: Option<Arc<dyn CommandGuard>>) {
        self.guard = guard;
    }

    /// Install (or with `None` remove) the host's deriver (see [`CommandDeriver`]). One per
    /// bus; a host that needs several composes them.
    pub fn set_deriver(&mut self, deriver: Option<Arc<dyn CommandDeriver>>) {
        self.deriver = deriver;
    }

    /// Is a deriver installed?
    #[must_use]
    pub fn has_deriver(&self) -> bool {
        self.deriver.is_some()
    }

    /// Ask the host's guard about a change (`Ok` when there is none).
    fn check_guard(
        &self,
        issuer: &Issuer,
        cmd: Option<&EditorCommand>,
        diff: &Diff,
        project: &Project,
    ) -> Result<(), CmdError> {
        match &self.guard {
            Some(g) => g.check(&Guarded {
                issuer,
                cmd,
                diff,
                project,
            }),
            None => Ok(()),
        }
    }

    /// Refuse `diff` when it changes a reserved setting `cmd` may not write.
    fn check_reserved(&self, cmd: &EditorCommand, diff: &Diff) -> Result<(), CmdError> {
        if self.reserved.is_empty() {
            return Ok(());
        }
        let target = match cmd {
            EditorCommand::Invoke { target, .. } => Some(target.as_str()),
            _ => None,
        };
        for c in &diff.changes {
            let Change::Setting { key, .. } = c else {
                continue;
            };
            for r in &self.reserved {
                if r.covers(key) && !target.is_some_and(|t| r.writers.contains(t)) {
                    return Err(CmdError::PolicyRefused {
                        what: format!(
                            "setting {key} is reserved to {}; {} may not change it",
                            r.writers.iter().cloned().collect::<Vec<_>>().join(", "),
                            cmd.label()
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// The policy of `cmd` on this bus (an `Invoke` takes its handler's).
    #[must_use]
    pub fn policy_of(&self, cmd: &EditorCommand) -> CommandPolicy {
        match cmd {
            EditorCommand::Invoke { target, .. } => self
                .handlers
                .get(target)
                .map_or(CommandPolicy::ORDINARY, |h| h.policy),
            other => other.policy(),
        }
    }

    /// Plan `cmd` from `issuer` against **another** project — a [`crate::Replica`]'s — with
    /// this bus's planners, its policies and its reserved settings, changing nothing: the
    /// split editor's optimistic prediction (O-13). Transaction and replay checks are the
    /// authoritative bus's; the prediction is corrected by what it answers.
    pub fn plan_for(
        &self,
        project: &Project,
        issuer: &Issuer,
        cmd: &EditorCommand,
    ) -> Result<Diff, CmdError> {
        if self.poisoned {
            return Err(CmdError::Poisoned);
        }
        if self.policy_of(cmd).human_only && !issuer.is_human() {
            return Err(CmdError::PolicyRefused {
                what: format!("{} accepts only a human issuer, not {issuer}", cmd.label()),
            });
        }
        self.plan_on(project, cmd)
    }

    // ---- the stream and the logs -------------------------------------------------------

    /// Subscribe to every [`Applied`] event from now on, holding at most
    /// [`DEFAULT_SUBSCRIPTION_CAPACITY`] undrained events (then a [`crate::Gap`]).
    pub fn subscribe(&mut self) -> Subscription {
        self.subscribe_with_capacity(DEFAULT_SUBSCRIPTION_CAPACITY)
    }

    /// Subscribe with an explicit bound on undrained events (at least 1).
    pub fn subscribe_with_capacity(&mut self, capacity: usize) -> Subscription {
        self.subscribers.subscribe(capacity)
    }

    /// The `seq` the next [`Applied`] event will carry. A subscriber resyncing after a
    /// [`crate::Gap`] reads [`Bus::project`] and this together, then skips delivered events
    /// whose `seq` is below it (they are already in the snapshot).
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.seq
    }

    /// The audit trail (every operation, every refusal).
    #[must_use]
    pub fn audit(&self) -> &[AuditEntry] {
        &self.audit
    }

    /// Take the audit trail (a persistence layer drains it; the bus keeps nothing twice).
    /// The bus keeps its buffer's capacity, so steady draining does not reallocate it; a
    /// persistence layer that drains every frame uses [`Bus::drain_audit_into`] to reuse its
    /// own buffer as well.
    pub fn drain_audit(&mut self) -> Vec<AuditEntry> {
        let mut out = Vec::with_capacity(self.audit.len());
        self.drain_audit_into(&mut out);
        out
    }

    /// Append the audit trail to `out` and empty it, keeping the capacity of both buffers.
    pub fn drain_audit_into(&mut self, out: &mut Vec<AuditEntry>) {
        out.append(&mut self.audit);
        // The folded lines left with the drain; the gesture's next frame starts a new line.
        self.audit_fold = None;
    }

    /// The audit buffer's capacity (the memory guard checks that draining keeps it).
    #[must_use]
    pub fn audit_capacity(&self) -> usize {
        self.audit.capacity()
    }

    /// Audit entries dropped because nothing drained the trail within
    /// [`Bus::with_max_audit`] entries (0 while a persistence layer keeps up).
    #[must_use]
    pub fn audit_dropped(&self) -> u64 {
        self.audit_dropped
    }

    /// A live transaction: open, or committed/undone and still in the undo history.
    /// Retired transactions (expired, cancelled, commits that changed nothing) have no
    /// record; [`Bus::txn_state`] still reports their state.
    #[must_use]
    pub fn transaction(&self, txn: TxnId) -> Option<&TxnRecord> {
        self.txns.get(&txn)
    }

    /// The state of `txn`, live or retired; `None` if the bus never saw it.
    #[must_use]
    pub fn txn_state(&self, txn: TxnId) -> Option<TxnState> {
        if let Some(r) = self.txns.get(&txn) {
            return Some(r.state);
        }
        if let Some(s) = self.retired.get(&txn) {
            return Some(*s);
        }
        (txn.0 < self.txn_floor).then_some(TxnState::Expired)
    }

    /// The undo history, oldest first: committed and undone transactions that still carry
    /// their changes.
    pub fn history(&self) -> impl Iterator<Item = &TxnRecord> {
        self.history.iter().filter_map(|t| self.txns.get(t))
    }

    /// The open transactions (gestures in progress, automation sessions' preview transactions), in
    /// no particular order.
    pub fn open_transactions(&self) -> impl Iterator<Item = &TxnRecord> {
        self.txns.values().filter(|r| r.state == TxnState::Open)
    }

    /// What Ctrl+Z undoes: the newest committed transaction.
    #[must_use]
    pub fn undo_target(&self) -> Option<TxnId> {
        self.history
            .iter()
            .rev()
            .copied()
            .find(|t| self.txns.get(t).map(|r| r.state) == Some(TxnState::Committed))
    }

    /// What Ctrl+Y redoes: the most recently undone transaction, unless a new commit has
    /// happened since (the usual editor rule; [`Bus::redo_as`] can still redo any undone
    /// transaction whose changes apply cleanly).
    #[must_use]
    pub fn redo_target(&self) -> Option<TxnId> {
        self.redo_stack.last().copied()
    }

    /// Forget all undo history (committed and undone transactions expire). Open
    /// transactions are untouched.
    pub fn clear_history(&mut self) {
        let old: Vec<TxnId> = self.history.drain(..).collect();
        for t in old {
            self.retire(t, TxnState::Expired);
        }
        self.redo_stack.clear();
    }

    /// Drop a transaction's record, keeping a tombstone of its final state.
    fn retire(&mut self, txn: TxnId, state: TxnState) {
        self.txns.remove(&txn);
        self.end_fold(txn);
        self.retired.insert(txn, state);
        while self.retired.len() > RETIRED_WINDOW {
            if let Some((old, _)) = self.retired.pop_first() {
                self.txn_floor = self.txn_floor.max(old.0.saturating_add(1));
            }
        }
    }

    /// The live record of `txn` if it is in state `expected`, else the refusal.
    fn require(&self, txn: TxnId, expected: TxnState) -> Result<&TxnRecord, CmdError> {
        match self.txns.get(&txn) {
            Some(r) if r.state == expected => Ok(r),
            Some(r) => Err(CmdError::TxnState {
                txn,
                expected,
                found: r.state,
            }),
            None => match self.txn_state(txn) {
                Some(found) => Err(CmdError::TxnState {
                    txn,
                    expected,
                    found,
                }),
                None => Err(CmdError::UnknownTxn(txn)),
            },
        }
    }

    // ---- transactions ------------------------------------------------------------------

    /// Open a transaction (a gesture: slider drag, gizmo drag, brush stroke). Commands sent
    /// into it merge into one undo step; [`Bus::commit`] closes it, [`Bus::cancel`] reverts it.
    pub fn begin(&mut self, label: &str, issuer: Issuer) -> TxnId {
        let txn = self.next_txn_id();
        let at = self.clock.now_ms();
        self.txns.insert(
            txn,
            TxnRecord::new(txn, label.to_string(), issuer.clone(), at, TxnState::Open),
        );
        self.log(
            issuer,
            txn,
            None,
            AuditAction::Begin(label.to_string()),
            Ok(0),
        );
        txn
    }

    /// Commit an open transaction: it becomes one undo step (none if it changed nothing).
    pub fn commit(&mut self, txn: TxnId) -> Result<Arc<Applied>, Rejection> {
        let issuer = self.issuer_of(txn);
        let result = self.commit_inner(txn);
        let outcome = result
            .as_ref()
            .map(|_| self.txns.get(&txn).map_or(0, |r| r.changes().count()));
        self.log_result(issuer, txn, None, AuditAction::Commit, outcome);
        result
    }

    fn commit_inner(&mut self, txn: TxnId) -> Result<Arc<Applied>, Rejection> {
        let rej = |e| Rejection::new(e, None, Some(txn));
        let rec = self.require(txn, TxnState::Open).map_err(rej)?;
        let issuer = rec.issuer.clone();
        let empty = rec.acc.is_empty();
        self.end_fold(txn);
        if empty {
            // Nothing to undo: it never enters the history, and its record goes.
            self.retire(txn, TxnState::Expired);
        } else {
            if let Some(rec) = self.txns.get_mut(&txn) {
                rec.state = TxnState::Committed;
            }
            self.push_history(txn);
        }
        Ok(self.emit(AppliedKind::Commit, txn, None, issuer, Diff::default()))
    }

    /// Cancel an open transaction on behalf of its own issuer: revert everything it applied
    /// (Esc during a drag).
    pub fn cancel(&mut self, txn: TxnId) -> Result<Arc<Applied>, Rejection> {
        let issuer = self.issuer_of(txn);
        self.cancel_as(txn, &issuer)
    }

    /// Cancel an open transaction on behalf of `by`. Only the transaction's own issuer may
    /// close it — the rule `apply` enforces for sending into it — so an automation session can
    /// never cancel a human's drag in progress (`CMD-0012`, audited as `by`'s refusal).
    pub fn cancel_as(&mut self, txn: TxnId, by: &Issuer) -> Result<Arc<Applied>, Rejection> {
        let result = self.cancel_inner(txn, by);
        let outcome = result.as_ref().map(|a| a.diff.len());
        self.log_result(by.clone(), txn, None, AuditAction::Cancel, outcome);
        result
    }

    fn cancel_inner(&mut self, txn: TxnId, by: &Issuer) -> Result<Arc<Applied>, Rejection> {
        let rej = |e| Rejection::new(e, None, Some(txn));
        let rec = self.require(txn, TxnState::Open).map_err(rej)?;
        if rec.issuer != *by {
            return Err(rej(CmdError::IssuerMismatch { txn }));
        }
        let inverse = Diff {
            changes: rec.changes().cloned().collect(),
        }
        .inverse();
        let issuer = rec.issuer.clone();
        self.apply_changes(&inverse.changes).map_err(rej)?;
        self.retire(txn, TxnState::Cancelled);
        Ok(self.emit(AppliedKind::Cancel, txn, None, issuer, inverse))
    }

    /// Undo `txn` on behalf of `by` (provenance: who pressed Ctrl+Z, or which automation session
    /// asked). An open transaction is cancelled, and only by its own issuer (`CMD-0012` otherwise:
    /// an automation session cannot cancel a human's drag in progress). Refused with `CMD-0008` if
    /// its changes were since overwritten (nothing is applied), `CMD-0014` if a command in it
    /// forbids undo.
    pub fn undo_as(&mut self, txn: TxnId, by: &Issuer) -> Result<Arc<Applied>, Rejection> {
        if self.txns.get(&txn).map(|r| r.state) == Some(TxnState::Open) {
            return self.cancel_as(txn, by);
        }
        let result = self.undo_inner(txn, by);
        let outcome = result.as_ref().map(|a| a.diff.len());
        self.log_result(by.clone(), txn, None, AuditAction::Undo, outcome);
        result
    }

    fn undo_inner(&mut self, txn: TxnId, by: &Issuer) -> Result<Arc<Applied>, Rejection> {
        let rej = |e| Rejection::new(e, None, Some(txn));
        let rec = self.require(txn, TxnState::Committed).map_err(rej)?;
        if !rec.undoable {
            return Err(rej(CmdError::PolicyRefused {
                what: format!("{txn} contains a command that cannot be undone"),
            }));
        }
        let inverse = Diff {
            changes: rec.changes().cloned().collect(),
        }
        .inverse();
        self.check_guard(by, None, &inverse, &self.project)
            .map_err(rej)?;
        self.apply_changes(&inverse.changes).map_err(rej)?;
        if let Some(rec) = self.txns.get_mut(&txn) {
            rec.state = TxnState::Undone;
        }
        self.redo_stack.push(txn);
        Ok(self.emit(AppliedKind::Undo, txn, None, by.clone(), inverse))
    }

    /// Redo an undone `txn` on behalf of `by`. Refused with `CMD-0008` if the project no
    /// longer matches what it undid, `CMD-0014` if a command in it forbids redo.
    pub fn redo_as(&mut self, txn: TxnId, by: &Issuer) -> Result<Arc<Applied>, Rejection> {
        let result = self.redo_inner(txn, by);
        let outcome = result.as_ref().map(|a| a.diff.len());
        self.log_result(by.clone(), txn, None, AuditAction::Redo, outcome);
        result
    }

    fn redo_inner(&mut self, txn: TxnId, by: &Issuer) -> Result<Arc<Applied>, Rejection> {
        let rej = |e| Rejection::new(e, None, Some(txn));
        let rec = self.require(txn, TxnState::Undone).map_err(rej)?;
        if !rec.redoable {
            return Err(rej(CmdError::PolicyRefused {
                what: format!("{txn} contains a command that cannot be redone"),
            }));
        }
        let forward = Diff {
            changes: rec.changes().cloned().collect(),
        };
        self.check_guard(by, None, &forward, &self.project)
            .map_err(rej)?;
        self.apply_changes(&forward.changes).map_err(rej)?;
        if let Some(rec) = self.txns.get_mut(&txn) {
            rec.state = TxnState::Committed;
        }
        self.redo_stack.retain(|t| *t != txn);
        Ok(self.emit(AppliedKind::Redo, txn, None, by.clone(), forward))
    }

    // ---- batches -----------------------------------------------------------------------

    /// Preview a **batch**: the diff of each envelope in order, each planned against the
    /// project as the ones before it would leave it, exactly as applying them one after the
    /// other now would (a later command may name an entity an earlier one spawns). Nothing
    /// changes: the batch runs on a scratch copy of the project, which is dropped. The first
    /// refusal ends the preview and is returned with its index in the batch.
    ///
    /// A one-envelope batch is [`CommandSink::dry_run`] (no copy). This is the `&self`
    /// preview, for a caller that cannot hold the bus mutably; it pays a copy of the whole
    /// project per multi-command batch. A caller holding the bus mutably previews with
    /// [`Bus::preview_batch`] and applies with [`Bus::apply_batch`], which cost what the batch
    /// touches, not what the project holds (WP-20).
    pub fn dry_run_batch(
        &self,
        batch: &[CommandEnvelope],
    ) -> Result<Vec<Diff>, (usize, Rejection)> {
        if let [one] = batch {
            return self.dry_run(one).map(|d| vec![d]).map_err(|r| (0, r));
        }
        let mut scratch = self.project.clone();
        let mut out = Vec::with_capacity(batch.len());
        for (i, e) in batch.iter().enumerate() {
            let rej = |err| (i, Rejection::new(err, Some(e.id), Some(e.txn)));
            self.check_envelope(e).map_err(rej)?;
            let diff = self.plan_on(&scratch, &e.cmd).map_err(rej)?;
            self.check_guard(&e.issuer, Some(&e.cmd), &diff, &scratch)
                .map_err(rej)?;
            // A panic applying to the scratch copy cannot hurt the bus's own project.
            let applied =
                panic::catch_unwind(AssertUnwindSafe(|| scratch.apply_all(&diff.changes)));
            match applied {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return Err(rej(err)),
                Err(payload) => {
                    return Err(rej(CmdError::Panicked {
                        message: panic_message(payload.as_ref()),
                    }));
                }
            }
            out.push(diff);
        }
        Ok(out)
    }

    /// Apply a **batch**: every envelope in order, each planned against the project as the
    /// ones before it left it — all of them or none. Each envelope is checked, planned,
    /// guarded, passed to `check` (the caller's own test of its diff, e.g. that a spawn got
    /// the key a binding predicted) and applied to the project in place; the first refusal
    /// reverts what the batch had applied (each diff's inverse, newest first, and the key
    /// allocator) and is returned with its index, audited as that command's refusal. Nothing
    /// is streamed until the whole batch applied, so no subscriber ever sees part of a
    /// refused batch.
    ///
    /// Cost: planning and applying the batch's own changes — never a copy of the project
    /// (a `dry_run_batch` then `apply` pair copied the whole project and planned every
    /// command twice: a stall under the editor core's lock on a large project, WP-20).
    pub fn apply_batch(
        &mut self,
        batch: Vec<CommandEnvelope>,
        check: &mut dyn FnMut(usize, &Diff) -> Result<(), CmdError>,
    ) -> Result<Vec<Arc<Applied>>, (usize, Rejection)> {
        let diffs = match self.stage(&batch, check) {
            Ok(d) => d,
            Err((i, r)) => {
                if let Some(e) = batch.into_iter().nth(i) {
                    self.log_outcome(e, Err(r.code()));
                }
                return Err((i, r));
            }
        };
        let mut out = Vec::with_capacity(diffs.len());
        for (e, diff) in batch.into_iter().zip(diffs) {
            let applied = self.record(&e, diff);
            self.log_outcome(e, Ok(applied.diff.len()));
            out.push(applied);
        }
        Ok(out)
    }

    /// Preview a batch like [`Bus::dry_run_batch`], without the copy: the batch is applied
    /// to the project in place and reverted before this returns (the bus is `&mut`, so
    /// nothing observes it in between). Nothing is streamed, audited or recorded, and the key
    /// allocator is restored. The diffs are exactly what [`Bus::apply_batch`] would apply now.
    pub fn preview_batch(
        &mut self,
        batch: &[CommandEnvelope],
    ) -> Result<Vec<Diff>, (usize, Rejection)> {
        if let [one] = batch {
            return self.dry_run(one).map(|d| vec![d]).map_err(|r| (0, r));
        }
        let key = self.project.raw_next_key();
        let diffs = self.stage(batch, &mut |_, _| Ok(()))?;
        self.unstage(&diffs, key);
        Ok(diffs)
    }

    /// Check, plan, guard and apply each envelope in place, recording nothing; on the first
    /// refusal, revert the batch (see [`Bus::apply_batch`]).
    fn stage(
        &mut self,
        batch: &[CommandEnvelope],
        check: &mut dyn FnMut(usize, &Diff) -> Result<(), CmdError>,
    ) -> Result<Vec<Diff>, (usize, Rejection)> {
        let key = self.project.raw_next_key();
        let mut staged: Vec<Diff> = Vec::with_capacity(batch.len());
        for (i, e) in batch.iter().enumerate() {
            let step = if batch[..i].iter().any(|p| p.id == e.id) {
                Err(CmdError::DuplicateCommand(e.id))
            } else {
                self.admit(e)
                    .and_then(|diff| check(i, &diff).map(|()| diff))
                    .and_then(|diff| self.apply_changes(&diff.changes).map(|()| diff))
            };
            match step {
                Ok(diff) => staged.push(diff),
                Err(err) => {
                    self.unstage(&staged, key);
                    return Err((i, Rejection::new(err, Some(e.id), Some(e.txn))));
                }
            }
        }
        Ok(staged)
    }

    /// Revert staged diffs, newest first, and restore the key allocator. A revert that fails
    /// leaves the project in a state nothing planned: the bus is poisoned.
    fn unstage(&mut self, staged: &[Diff], next_key: u64) {
        for d in staged.iter().rev() {
            if self.apply_changes(&d.inverse().changes).is_err() {
                self.poisoned = true;
                return;
            }
        }
        self.project.set_raw_next_key(next_key);
    }

    // ---- internals ---------------------------------------------------------------------

    fn check_envelope(&self, e: &CommandEnvelope) -> Result<(), CmdError> {
        if self.poisoned {
            return Err(CmdError::Poisoned);
        }
        if e.id.0 < self.cmd_floor || self.seen_commands.contains(&e.id) {
            return Err(CmdError::DuplicateCommand(e.id));
        }
        if self.policy_of(&e.cmd).human_only && !e.issuer.is_human() {
            return Err(CmdError::PolicyRefused {
                what: format!(
                    "{} accepts only a human issuer, not {}",
                    e.cmd.label(),
                    e.issuer
                ),
            });
        }
        match self.txns.get(&e.txn) {
            Some(r) if r.state == TxnState::Open => {
                if r.issuer != e.issuer {
                    return Err(CmdError::IssuerMismatch { txn: e.txn });
                }
                Ok(())
            }
            Some(r) => Err(CmdError::TxnState {
                txn: e.txn,
                expected: TxnState::Open,
                found: r.state,
            }),
            // A retired id is never reopened by a late envelope.
            None => match self.txn_state(e.txn) {
                Some(found) => Err(CmdError::TxnState {
                    txn: e.txn,
                    expected: TxnState::Open,
                    found,
                }),
                None => Ok(()),
            },
        }
    }

    /// Plan `cmd` against the project. Panics in a handler are caught here (Ch.1.2).
    fn plan(&self, cmd: &EditorCommand) -> Result<Diff, CmdError> {
        self.plan_on(&self.project, cmd)
    }

    /// Plan `cmd` against `project` (the bus's own, or a batch preview's scratch copy), and
    /// refuse a plan that changes a reserved setting `cmd` may not write.
    fn plan_on(&self, project: &Project, cmd: &EditorCommand) -> Result<Diff, CmdError> {
        let diff = self.plan_unchecked(project, cmd)?;
        self.check_reserved(cmd, &diff)?;
        Ok(diff)
    }

    fn plan_unchecked(&self, project: &Project, cmd: &EditorCommand) -> Result<Diff, CmdError> {
        let handlers = &self.handlers;
        let deriver = self.deriver.as_deref();
        let planned = panic::catch_unwind(AssertUnwindSafe(|| {
            let mut b = DiffBuilder::new(project);
            match cmd {
                EditorCommand::Invoke { target, args } => {
                    let h = handlers
                        .get(target)
                        .ok_or_else(|| CmdError::UnknownHandler(target.clone()))?;
                    let v: serde_json::Value =
                        serde_json::from_str(args).map_err(|e| CmdError::BadArgs {
                            target: target.clone(),
                            why: format!("not JSON: {e}"),
                        })?;
                    if let Some(fields) = &h.fields {
                        check_args(target, fields, &v)?;
                    }
                    h.handler.plan(&mut b, &v)?;
                }
                other => other.plan_builtin(&mut b)?,
            }
            if let Some(d) = deriver {
                d.derive(cmd, &mut b)?;
            }
            Ok(b.finish())
        }));
        planned.unwrap_or_else(|payload| {
            Err(CmdError::Panicked {
                message: panic_message(payload.as_ref()),
            })
        })
    }

    /// Apply changes atomically. A panic here cannot be proven harmless (the project may be
    /// half-changed), so it poisons the bus rather than build on unknown state.
    fn apply_changes(&mut self, changes: &[Change]) -> Result<(), CmdError> {
        if self.poisoned {
            return Err(CmdError::Poisoned);
        }
        let project = &mut self.project;
        match panic::catch_unwind(AssertUnwindSafe(|| project.apply_all(changes))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(CmdError::Poisoned)) => {
                self.poisoned = true;
                Err(CmdError::Poisoned)
            }
            Ok(Err(e)) => Err(e),
            Err(payload) => {
                self.poisoned = true;
                Err(CmdError::Panicked {
                    message: panic_message(payload.as_ref()),
                })
            }
        }
    }

    fn apply_inner(&mut self, e: &CommandEnvelope) -> Result<Arc<Applied>, Rejection> {
        let rej = |err| Rejection::new(err, Some(e.id), Some(e.txn));
        let diff = self.admit(e).map_err(rej)?;
        self.apply_changes(&diff.changes).map_err(rej)?;
        Ok(self.record(e, diff))
    }

    /// Check `e` and plan it against the project as it is now (its guard too): the diff
    /// applying it would apply.
    fn admit(&self, e: &CommandEnvelope) -> Result<Diff, CmdError> {
        self.check_envelope(e)?;
        let diff = self.plan(&e.cmd)?;
        self.check_guard(&e.issuer, Some(&e.cmd), &diff, &self.project)?;
        Ok(diff)
    }

    /// Record an envelope whose diff was applied to the project: the replay window, its
    /// transaction, the history, and the event every subscriber gets.
    fn record(&mut self, e: &CommandEnvelope, diff: Diff) -> Arc<Applied> {
        self.seen_commands.insert(e.id);
        while self.seen_commands.len() > REPLAY_WINDOW {
            if let Some(old) = self.seen_commands.pop_first() {
                self.cmd_floor = self.cmd_floor.max(old.0.saturating_add(1));
            }
        }
        self.next_cmd = self.next_cmd.max(e.id.0.saturating_add(1));
        self.next_txn = self.next_txn.max(e.txn.0.saturating_add(1));
        let policy = self.policy_of(&e.cmd);
        let at = self.clock.now_ms();
        let rec = self.txns.entry(e.txn).or_insert_with(|| {
            // Not opened with `begin`: one command, one transaction, committed now.
            TxnRecord::new(
                e.txn,
                e.cmd.label(),
                e.issuer.clone(),
                at,
                TxnState::Committed,
            )
        });
        let implicit = rec.state == TxnState::Committed;
        for c in &diff.changes {
            rec.acc.push_ref(c);
        }
        rec.commands.record(e.id);
        rec.absorb_policy(policy);
        let empty = rec.acc.is_empty();
        if implicit {
            if empty {
                // A command that changed nothing is not an undo step; keep no record.
                self.retire(e.txn, TxnState::Expired);
            } else {
                self.push_history(e.txn);
            }
        }
        self.emit(
            AppliedKind::Command,
            e.txn,
            Some(e.id),
            e.issuer.clone(),
            diff,
        )
    }

    fn push_history(&mut self, txn: TxnId) {
        self.history.push_back(txn);
        self.redo_stack.clear();
        while self.history.len() > self.max_history {
            if let Some(old) = self.history.pop_front() {
                self.retire(old, TxnState::Expired);
            }
        }
    }

    fn emit(
        &mut self,
        kind: AppliedKind,
        txn: TxnId,
        command: Option<CommandId>,
        issuer: Issuer,
        diff: Diff,
    ) -> Arc<Applied> {
        let applied = Arc::new(Applied {
            seq: self.seq,
            kind,
            txn,
            command,
            issuer,
            diff,
        });
        self.seq += 1;
        if !self.subscribers.is_empty() {
            self.subscribers.publish(&applied);
        }
        applied
    }

    fn issuer_of(&self, txn: TxnId) -> Issuer {
        self.txns
            .get(&txn)
            .map_or(Issuer::Test, |r| r.issuer.clone())
    }

    /// Append an entry that is not a folding gesture frame. It ends any fold: the trail's
    /// tail is no longer the gesture's own.
    fn log(
        &mut self,
        issuer: Issuer,
        txn: TxnId,
        command: Option<CommandId>,
        action: AuditAction,
        outcome: Result<usize, ErrorCode>,
    ) -> u64 {
        self.audit_fold = None;
        self.push_entry(issuer, txn, command, action, outcome)
    }

    fn push_entry(
        &mut self,
        issuer: Issuer,
        txn: TxnId,
        command: Option<CommandId>,
        action: AuditAction,
        outcome: Result<usize, ErrorCode>,
    ) -> u64 {
        let seq = self.audit_seq;
        let at_ms = self.clock.now_ms();
        self.audit.push(AuditEntry {
            seq,
            at_ms,
            last_at_ms: at_ms,
            issuer,
            txn,
            command,
            first_command: command,
            merged: 1,
            action,
            outcome,
        });
        self.audit_seq += 1;
        if self.audit.len() > self.max_audit {
            // Nothing drained the trail: drop the oldest quarter (amortised O(1)), counted.
            let drop = self.audit.len() - self.max_audit + self.max_audit / 4;
            self.audit.drain(..drop);
            self.audit_dropped += drop as u64;
        }
        seq
    }

    fn end_fold(&mut self, txn: TxnId) {
        if self.audit_fold.as_ref().is_some_and(|f| f.txn == txn) {
            self.audit_fold = None;
        }
    }

    /// Audit a command sent by its owner into an open gesture. It folds into the gesture's
    /// line for the same target and outcome **only while the gesture's own lines are the
    /// newest entries of the trail**, so a fold never moves a frame before an entry that
    /// happened earlier. Otherwise it starts a new line, and the fold restarts from there.
    fn log_gesture_frame(&mut self, e: CommandEnvelope, outcome: Result<usize, ErrorCode>) {
        let code = outcome.as_ref().err().copied();
        let now = self.clock.now_ms();
        let hit = self
            .audit_fold
            .as_ref()
            .filter(|f| f.txn == e.txn)
            .and_then(|f| {
                f.lines
                    .iter()
                    .find(|l| l.outcome == code && l.target.matches(&e.cmd))
            })
            .map(|l| l.seq);
        if let Some(seq) = hit {
            let first = self.audit.first().map(|a| a.seq);
            let slot = first
                .and_then(|f| seq.checked_sub(f))
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| self.audit.get_mut(i))
                .filter(|a| a.seq == seq);
            if let Some(entry) = slot {
                entry.command = Some(e.id);
                entry.merged += 1;
                entry.last_at_ms = now;
                entry.action = AuditAction::Command(e.cmd);
                entry.outcome = outcome;
                return;
            }
            // The line was dropped by the audit cap: restart the fold.
            self.audit_fold = None;
        }
        let target = AuditTarget::of(&e.cmd);
        if self.audit_fold.as_ref().is_some_and(|f| f.txn != e.txn) {
            self.audit_fold = None;
        }
        let CommandEnvelope {
            id,
            txn,
            issuer,
            cmd,
        } = e;
        let seq = self.push_entry(issuer, txn, Some(id), AuditAction::Command(cmd), outcome);
        if let Some(target) = target {
            self.audit_fold
                .get_or_insert_with(|| AuditFold {
                    txn,
                    lines: Vec::new(),
                })
                .lines
                .push(FoldLine {
                    target,
                    outcome: code,
                    seq,
                });
        }
    }

    fn log_result(
        &mut self,
        issuer: Issuer,
        txn: TxnId,
        command: Option<CommandId>,
        action: AuditAction,
        outcome: Result<usize, &Rejection>,
    ) {
        let outcome = outcome.map_err(Rejection::code);
        self.log(issuer, txn, command, action, outcome);
    }

    /// Audit an applied or refused command: a gesture frame folds (see
    /// [`Bus::log_gesture_frame`]), anything else gets its own line.
    fn log_outcome(&mut self, e: CommandEnvelope, outcome: Result<usize, ErrorCode>) {
        if self.folds(&e) {
            self.log_gesture_frame(e, outcome);
            return;
        }
        let CommandEnvelope {
            id,
            txn,
            issuer,
            cmd,
        } = e;
        self.log(issuer, txn, Some(id), AuditAction::Command(cmd), outcome);
    }

    /// Whether `e`'s audit entry may fold: a frame of an open gesture, sent by the gesture's
    /// own issuer, on a foldable target (anyone else's attempt always gets its own line).
    fn folds(&self, e: &CommandEnvelope) -> bool {
        AuditTarget::foldable(&e.cmd)
            && self
                .txns
                .get(&e.txn)
                .is_some_and(|r| r.state == TxnState::Open && r.issuer == e.issuer)
    }
}

impl CommandSink for Bus {
    fn dry_run(&self, e: &CommandEnvelope) -> Result<Diff, Rejection> {
        let rej = |err| Rejection::new(err, Some(e.id), Some(e.txn));
        self.check_envelope(e).map_err(rej)?;
        let diff = self.plan(&e.cmd).map_err(rej)?;
        self.check_guard(&e.issuer, Some(&e.cmd), &diff, &self.project)
            .map_err(rej)?;
        Ok(diff)
    }

    fn apply(&mut self, e: CommandEnvelope) -> Result<Arc<Applied>, Rejection> {
        let result = self.apply_inner(&e);
        let outcome = result
            .as_ref()
            .map(|a| a.diff.len())
            .map_err(Rejection::code);
        self.log_outcome(e, outcome);
        result
    }

    /// Attributed to the transaction's own issuer; a client that knows who asked uses
    /// [`Bus::undo_as`].
    fn undo(&mut self, txn: TxnId) -> Result<Arc<Applied>, Rejection> {
        let by = self.issuer_of(txn);
        self.undo_as(txn, &by)
    }

    fn redo(&mut self, txn: TxnId) -> Result<Arc<Applied>, Rejection> {
        let by = self.issuer_of(txn);
        self.redo_as(txn, &by)
    }
}

/// Arguments registered from a `CommandDesc` must be an object with exactly its fields.
fn check_args(target: &str, fields: &[&str], v: &serde_json::Value) -> Result<(), CmdError> {
    let bad = |why: String| CmdError::BadArgs {
        target: target.to_string(),
        why,
    };
    let obj = v
        .as_object()
        .ok_or_else(|| bad("arguments must be a JSON object".into()))?;
    if let Some(missing) = fields.iter().find(|f| !obj.contains_key(**f)) {
        return Err(bad(format!("missing field `{missing}`")));
    }
    if let Some(extra) = obj.keys().find(|k| !fields.contains(&k.as_str())) {
        return Err(bad(format!("unknown field `{extra}`")));
    }
    Ok(())
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_redoable_handler_makes_its_transaction_non_redoable() {
        let mut bus = Bus::new();
        let policy = CommandPolicy {
            human_only: true,
            undoable: true,
            redoable: false,
        };
        bus.register_handler(
            "grant",
            policy,
            |b: &mut DiffBuilder<'_>, _: &serde_json::Value| {
                b.set_setting("grants.automation", Some(crate::Value::Bool(true)))
            },
        )
        .expect("registers");
        let cmd = EditorCommand::Invoke {
            target: "grant".into(),
            args: "{}".into(),
        };
        // An automation session cannot send it.
        let e = bus.envelope(
            Issuer::Automation {
                session: "s".into(),
                tool: "t".into(),
            },
            cmd.clone(),
        );
        let r = bus.apply(e).expect_err("automation session refused");
        assert_eq!(r.code().as_str(), "CMD-0014");
        // A human can; undo revokes; redo is refused (undo never grants).
        let human = Issuer::Human { user: "u".into() };
        let e = bus.envelope(human.clone(), cmd);
        let txn = e.txn;
        bus.apply(e).expect("human ok");
        assert!(!bus.transaction(txn).expect("exists").redoable());
        bus.undo_as(txn, &human).expect("undo revokes");
        assert_eq!(bus.project().setting("grants.automation"), None);
        let r = bus.redo_as(txn, &human).expect_err("redo refused");
        assert_eq!(r.code().as_str(), "CMD-0014");
        assert_eq!(bus.project().setting("grants.automation"), None);
    }

    #[test]
    fn a_batch_preview_sees_earlier_commands_and_changes_nothing() {
        let mut bus = Bus::new();
        let feed = bus.subscribe();
        let me = Issuer::Test;
        let key = bus.project().next_key();
        let spawn = bus.envelope(
            me.clone(),
            EditorCommand::Spawn {
                name: "Cube".into(),
                parent: None,
            },
        );
        let set = bus.envelope(
            me.clone(),
            EditorCommand::SetProperty {
                entity: key,
                path: "scale".into(),
                value: crate::Value::Float(2.0),
            },
        );
        let before = bus.project().state_hash();
        let diffs = bus
            .dry_run_batch(&[spawn.clone(), set.clone()])
            .expect("the second command sees the first one's entity");
        assert_eq!(diffs.len(), 2);
        assert_eq!(bus.project().state_hash(), before, "nothing changed");
        assert!(bus.project().is_empty());
        assert!(feed.drain().events.is_empty(), "a preview emits nothing");
        // Alone, the second command is refused: its entity does not exist yet.
        let (at, r) = bus
            .dry_run_batch(std::slice::from_ref(&set))
            .expect_err("unknown entity");
        assert_eq!((at, r.code().as_str()), (0, "CMD-0001"));
        // A refusal names its index; applying the batch for real gives the previewed diffs.
        let bad = bus.envelope(
            me,
            EditorCommand::Rename {
                entity: EntityKey(999),
                name: "x".into(),
            },
        );
        let (at, _) = bus
            .dry_run_batch(&[spawn.clone(), set.clone(), bad])
            .expect_err("third refused");
        assert_eq!(at, 2);
        let a = bus.apply(spawn).expect("applies");
        let b = bus.apply(set).expect("applies");
        assert_eq!(vec![a.diff.clone(), b.diff.clone()], diffs);
    }

    /// `apply_batch` applies in place and reverts a refused batch exactly: content, key
    /// allocator, stream and history; `preview_batch` is `dry_run_batch` without the copy.
    #[test]
    fn a_batch_applies_in_place_all_or_nothing() {
        let mut bus = Bus::new();
        let me = Issuer::Test;
        let seed = bus.envelope(
            me.clone(),
            EditorCommand::Spawn {
                name: "Base".into(),
                parent: None,
            },
        );
        bus.apply(seed).expect("applies");
        let feed = bus.subscribe();
        let key = bus.project().next_key();
        let make = |bus: &mut Bus, bad: bool| {
            let mut v = vec![
                bus.envelope(
                    me.clone(),
                    EditorCommand::Spawn {
                        name: "Cube".into(),
                        parent: None,
                    },
                ),
                bus.envelope(
                    me.clone(),
                    EditorCommand::SetProperty {
                        entity: key,
                        path: "scale".into(),
                        value: crate::Value::Float(2.0),
                    },
                ),
            ];
            if bad {
                v.push(bus.envelope(
                    me.clone(),
                    EditorCommand::Rename {
                        entity: EntityKey(999),
                        name: "x".into(),
                    },
                ));
            }
            v
        };
        let before = bus.project().state_hash();
        let history = bus.history().count();
        let trail = bus.audit().len();
        // Refused at index 2: nothing of it stays, the allocator is back, nothing streamed.
        let batch = make(&mut bus, true);
        let (at, r) = bus
            .apply_batch(batch, &mut |_, _| Ok(()))
            .expect_err("third refused");
        assert_eq!((at, r.code().as_str()), (2, "CMD-0001"));
        assert_eq!(bus.project().state_hash(), before);
        assert_eq!(bus.project().next_key(), key, "the allocator is restored");
        assert!(
            feed.drain().events.is_empty(),
            "a refused batch streams nothing"
        );
        assert_eq!(bus.history().count(), history);
        assert_eq!(bus.audit().len(), trail + 1, "the refusal is audited");
        // The caller's check refuses a batch the same way.
        let batch = make(&mut bus, false);
        let (at, _) = bus
            .apply_batch(batch, &mut |i, _| {
                if i == 1 {
                    Err(CmdError::Conflict {
                        detail: "no".into(),
                    })
                } else {
                    Ok(())
                }
            })
            .expect_err("check refused");
        assert_eq!(at, 1);
        assert_eq!(bus.project().state_hash(), before);
        assert_eq!(bus.project().next_key(), key);
        // A preview in place changes nothing and equals the copying preview.
        let batch = make(&mut bus, false);
        let copied = bus.dry_run_batch(&batch).expect("previews");
        let in_place = bus.preview_batch(&batch).expect("previews");
        assert_eq!(copied, in_place);
        assert_eq!(bus.project().state_hash(), before);
        assert_eq!(bus.project().next_key(), key);
        assert!(feed.drain().events.is_empty(), "a preview streams nothing");
        // Applied, it gives the previewed diffs, streamed in order.
        let applied = bus.apply_batch(batch, &mut |_, _| Ok(())).expect("applies");
        let diffs: Vec<Diff> = applied.iter().map(|a| a.diff.clone()).collect();
        assert_eq!(diffs, in_place);
        assert_eq!(feed.drain().events.len(), 2);
        assert_eq!(bus.history().count(), history + 2);
    }

    #[test]
    fn args_are_checked_against_the_descriptor() {
        assert!(check_args("t", &["a", "b"], &serde_json::json!({"a": 1, "b": 2})).is_ok());
        assert!(check_args("t", &["a", "b"], &serde_json::json!({"a": 1})).is_err());
        assert!(check_args("t", &["a"], &serde_json::json!({"a": 1, "c": 2})).is_err());
        assert!(check_args("t", &["a"], &serde_json::json!([1])).is_err());
    }
}
