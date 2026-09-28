//! `ProjectMirror` — the UI's read-only model of the project (Ch.21 §21.18).
//!
//! * It is fed **only** by the `Applied` stream (and a snapshot at start and after a gap),
//!   so every issuer's change — the user's, an automation session's, a script's — reaches
//!   the UI by the same path, in `seq` order.
//! * Panels read it; they cannot change it. [`ProjectMirror::apply`] is private to the
//!   crate (the shell's stream pump), so a panel cannot fake a state change: it can only
//!   request one through the `CommandEmitter`.
//! * An `Applied` delta updates exactly what it touches and bumps a revision counter only
//!   for what changed. Per-path revisions exist **only for watched paths**
//!   ([`ProjectMirror::watch`]); a panel's sync step compares revisions and does nothing
//!   when its paths did not change (there is no "refresh the inspector" pass).
//! * It also keeps the undo history as the history panel shows it: every transaction with
//!   its issuer (human / automation:session / script), label, time and state.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use forge_cmd::{Applied, AppliedKind, Change, EntityKey, Issuer, Project, TxnId, TxnState, Value};

use crate::client::{Prediction, Ticket, TxnInfo};

/// Bound on the history the mirror keeps (the bus keeps at most as many undoable ones).
pub const MIRROR_HISTORY_CAP: usize = forge_cmd::DEFAULT_MAX_HISTORY;

/// One mirrored entity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MirrorEntity {
    pub name: String,
    pub parent: Option<EntityKey>,
    pub children: BTreeSet<EntityKey>,
    pub properties: BTreeMap<String, Value>,
}

/// One row of the undo history.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub txn: TxnId,
    pub label: String,
    pub issuer: Issuer,
    pub state: TxnState,
    /// When it was opened (ms since the Unix epoch).
    pub at_ms: u64,
}

impl HistoryEntry {
    /// The provenance tag the panel shows: `human:ada`, `automation:<session>`, `script:<path>`.
    pub fn issuer_tag(&self) -> String {
        self.issuer.tag()
    }
}

/// One step of travelling through the history (the undo-history panel's click).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Travel {
    Undo(TxnId),
    Redo(TxnId),
}

/// What a panel can watch for changes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Watch {
    /// A project setting by key.
    Setting(String),
    /// Every project setting whose key starts with this prefix.
    SettingPrefix(String),
    /// A property of an entity.
    Property(EntityKey, String),
    /// An entity's name, parent or existence.
    Entity(EntityKey),
}

/// The read model (see the module docs).
#[derive(Debug, Default)]
pub struct ProjectMirror {
    entities: BTreeMap<EntityKey, MirrorEntity>,
    roots: BTreeSet<EntityKey>,
    settings: BTreeMap<String, Value>,
    /// The undo history; the listed window is `history[history_start..]`. Entries before
    /// the window were evicted by the cap and are dropped in one batch once they make up
    /// half the vector, so a new transaction at the cap costs O(1) amortised instead of a
    /// shift of every entry and a rewrite of every index (`ui_hierarchy_100k`'s renames:
    /// ~0.2 ms each at 10,000 entries, memory-bound, and 4x that beside a build).
    history: Vec<HistoryEntry>,
    history_start: usize,
    /// Transaction -> its position in `history` (the vector, not the window).
    history_index: HashMap<TxnId, usize>,
    /// Events with `seq` below this are already applied.
    next_seq: u64,
    revision: u64,
    structure_rev: u64,
    settings_rev: u64,
    history_rev: u64,
    /// Interior-mutable so a panel holding only `&ProjectMirror` can start watching.
    watched: RefCell<HashMap<Watch, u64>>,
    undo_target: Option<TxnId>,
    redo_target: Option<TxnId>,
    events_applied: u64,
    /// The recent entity changes, oldest first, each with its sequence number (see
    /// [`ProjectMirror::changes_since`]).
    changes: VecDeque<(u64, MirrorChange)>,
    /// The sequence number the next change gets.
    change_seq: u64,
    /// Changes numbered below this were replaced by a snapshot (a resync).
    change_floor: u64,
    /// The recent setting changes (keys), oldest first, each with its sequence number (see
    /// [`ProjectMirror::setting_changes_since`]).
    setting_changes: VecDeque<(u64, String)>,
    /// The sequence number the next setting change gets.
    setting_change_seq: u64,
    /// Setting changes numbered below this were replaced by a snapshot (a resync).
    setting_change_floor: u64,
    /// The project lifecycle's status as the core last reported it (WP-U7).
    project: Option<std::sync::Arc<crate::project::ProjectStatus>>,
    project_rev: u64,
    /// Optimistic overlays (O-13): a remote client's predicted diffs, applied on top of the
    /// authoritative state until their requests settle, oldest first.
    overlays: Vec<(Ticket, forge_cmd::Diff)>,
    /// W2 positive-control switches (`test_optimistic_reconciliation`). Never set outside it.
    #[doc(hidden)]
    pub faults: MirrorFaults,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the mirror's guards. Never set outside those tests.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct MirrorFaults {
        /// A settled request's overlay is never dropped: a wrong prediction stays on screen
        /// (the control of `test_optimistic_reconciliation`).
        pub keep_overlays: bool,
    }
}

/// Bound on the entity change log: a reader further behind rebuilds from the mirror.
pub const CHANGE_LOG_CAP: usize = 8192;

/// One entity change, as the change log records it (what an incremental view — the
/// hierarchy over 100k entities — applies instead of rebuilding).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorChange {
    Created(EntityKey),
    Removed {
        entity: EntityKey,
        parent: Option<EntityKey>,
    },
    Renamed(EntityKey),
    Reparented {
        entity: EntityKey,
        before: Option<EntityKey>,
        after: Option<EntityKey>,
    },
    Property(EntityKey, String),
}

impl ProjectMirror {
    /// An empty mirror (call [`ProjectMirror::resync`] before use).
    pub fn new() -> Self {
        Self::default()
    }

    // ---- reads ---------------------------------------------------------------------------

    pub fn entity(&self, key: EntityKey) -> Option<&MirrorEntity> {
        self.entities.get(&key)
    }
    pub fn entities(&self) -> impl ExactSizeIterator<Item = (&EntityKey, &MirrorEntity)> {
        self.entities.iter()
    }
    /// Entities with keys after `after` (all of them for `None`), in key order: a scan
    /// that resumes across loop turns.
    pub fn entities_after(
        &self,
        after: Option<EntityKey>,
    ) -> impl Iterator<Item = (&EntityKey, &MirrorEntity)> {
        use std::ops::Bound;
        let lo = after.map_or(Bound::Unbounded, Bound::Excluded);
        self.entities.range((lo, Bound::Unbounded))
    }
    pub fn roots(&self) -> impl Iterator<Item = EntityKey> + '_ {
        self.roots.iter().copied()
    }
    /// The first child of `parent` (`None`: the roots) with a key after `after` (`None`:
    /// the first child), in key order: O(log n), so a walk over a parent's children
    /// resumes across loop turns without a snapshot of them.
    pub fn child_after(
        &self,
        parent: Option<EntityKey>,
        after: Option<EntityKey>,
    ) -> Option<EntityKey> {
        use std::ops::Bound;
        let lo = after.map_or(Bound::Unbounded, Bound::Excluded);
        let set = match parent {
            None => &self.roots,
            Some(p) => &self.entities.get(&p)?.children,
        };
        set.range((lo, Bound::Unbounded)).next().copied()
    }
    pub fn len(&self) -> usize {
        self.entities.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
    pub fn setting(&self, key: &str) -> Option<&Value> {
        self.settings.get(key)
    }
    pub fn settings(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.settings.iter().map(|(k, v)| (k.as_str(), v))
    }
    /// The settings whose keys start with `prefix`, in key order: O(log n + matches), so
    /// a domain editor reads its own namespace (`audio.bus.`) without scanning the rest.
    pub fn settings_under<'a, 'p>(
        &'a self,
        prefix: &'p str,
    ) -> impl Iterator<Item = (&'a str, &'a Value)> + use<'a, 'p> {
        self.settings
            .range::<str, _>((
                std::ops::Bound::Included(prefix),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_str(), v))
    }
    pub fn property(&self, e: EntityKey, path: &str) -> Option<&Value> {
        self.entities.get(&e)?.properties.get(path)
    }
    /// The undo history, oldest first (cancelled gestures are not listed).
    pub fn history(&self) -> &[HistoryEntry] {
        &self.history[self.history_start..]
    }
    pub fn history_entry(&self, txn: TxnId) -> Option<&HistoryEntry> {
        self.history_index.get(&txn).map(|i| &self.history[*i])
    }
    /// The bus calls that take the project to the state right after `target` (`None`: the
    /// state before every listed transaction). Newer committed transactions are undone
    /// newest first; if `target` is undone, it and the undone ones before it are redone
    /// oldest first. One bus call per transaction (Ch.21 §21.18).
    pub fn plan_travel(&self, target: Option<TxnId>) -> Vec<Travel> {
        let pos = match target {
            None => None,
            Some(t) => match self.history_index.get(&t) {
                Some(i) => Some(*i),
                None => return Vec::new(),
            },
        };
        let undone_target = pos.is_some_and(|i| self.history[i].state == TxnState::Undone);
        if undone_target {
            let i = pos.unwrap_or(self.history_start);
            return self.history[self.history_start..=i]
                .iter()
                .filter(|h| h.state == TxnState::Undone)
                .map(|h| Travel::Redo(h.txn))
                .collect();
        }
        let from = pos.map_or(self.history_start, |i| i + 1);
        self.history[from..]
            .iter()
            .rev()
            .filter(|h| h.state == TxnState::Committed)
            .map(|h| Travel::Undo(h.txn))
            .collect()
    }

    /// What Ctrl+Z / Ctrl+Y act on, as of the last pump.
    pub fn undo_target(&self) -> Option<TxnId> {
        self.undo_target
    }
    pub fn redo_target(&self) -> Option<TxnId> {
        self.redo_target
    }
    /// The label of what Ctrl+Z would undo (menus, the toolbar tooltip, the status bar).
    pub fn undo_label(&self) -> Option<&str> {
        self.undo_target
            .and_then(|t| self.history_entry(t))
            .map(|h| h.label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo_target
            .and_then(|t| self.history_entry(t))
            .map(|h| h.label.as_str())
    }
    /// The `seq` of the next event the mirror expects.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }
    /// Bumped by every change the mirror sees (a cheap "anything new?" check).
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Bumped when entities appear, disappear, are renamed or move.
    pub fn structure_revision(&self) -> u64 {
        self.structure_rev
    }
    /// Bumped when any project setting changes.
    pub fn settings_revision(&self) -> u64 {
        self.settings_rev
    }
    /// Bumped when the undo history changes (a new transaction, or a state change).
    pub fn history_revision(&self) -> u64 {
        self.history_rev
    }
    /// Events applied since creation (diagnostics, tests).
    pub fn events_applied(&self) -> u64 {
        self.events_applied
    }

    /// The project lifecycle's status (the open project, its revisions, outcomes, the last
    /// build), as the core last reported it. `None` until the first report.
    pub fn project(&self) -> Option<&crate::project::ProjectStatus> {
        self.project.as_deref()
    }
    /// Bumped when the project status changes.
    pub fn project_revision(&self) -> u64 {
        self.project_rev
    }
    pub(crate) fn set_project(&mut self, s: std::sync::Arc<crate::project::ProjectStatus>) {
        if self.project.as_deref() != Some(&*s) {
            self.project = Some(s);
            self.project_rev += 1;
            self.revision += 1;
        }
    }
    /// The core replaced the project (a load cleared its undo history): take the history
    /// as it is now. The load's own events already brought entities and settings across.
    pub(crate) fn reset_history(&mut self, history: Vec<TxnInfo>) {
        self.history.clear();
        self.history_start = 0;
        self.history_index.clear();
        for i in history {
            self.push_history(i);
        }
        self.history_rev += 1;
        self.revision += 1;
    }

    /// The sequence number the next entity change will get: a reader remembers it and
    /// later asks [`ProjectMirror::changes_since`].
    pub fn change_seq(&self) -> u64 {
        self.change_seq
    }
    /// The entity changes numbered `seq` and later, oldest first — or `None` if some were
    /// dropped (the log is bounded, [`CHANGE_LOG_CAP`]) or replaced by a snapshot: the
    /// reader then rebuilds from the mirror. An incremental view applies these instead of
    /// rebuilding, so one rename in a 100k-entity project costs one row.
    pub fn changes_since(&self, seq: u64) -> Option<impl Iterator<Item = &MirrorChange>> {
        if seq < self.change_floor {
            return None;
        }
        let oldest = self.changes.front().map_or(self.change_seq, |(s, _)| *s);
        if seq < oldest {
            return None;
        }
        let skip = (seq - oldest) as usize;
        Some(self.changes.iter().skip(skip).map(|(_, c)| c))
    }

    /// The sequence number the next setting change will get (see
    /// [`ProjectMirror::setting_changes_since`]).
    pub fn setting_change_seq(&self) -> u64 {
        self.setting_change_seq
    }
    /// The keys of the settings changed at sequence number `seq` and later, oldest first (a
    /// key changed twice appears twice) — or `None` if some were dropped (the log is
    /// bounded, [`CHANGE_LOG_CAP`]) or replaced by a snapshot: the reader then re-reads the
    /// settings. A view over a large namespace (a 512 × 512 tile map) applies these instead
    /// of re-parsing it, so one painted chunk costs one chunk.
    pub fn setting_changes_since(&self, seq: u64) -> Option<impl Iterator<Item = &str>> {
        if seq < self.setting_change_floor {
            return None;
        }
        let oldest = self
            .setting_changes
            .front()
            .map_or(self.setting_change_seq, |(s, _)| *s);
        if seq < oldest {
            return None;
        }
        let skip = (seq - oldest) as usize;
        Some(
            self.setting_changes
                .iter()
                .skip(skip)
                .map(|(_, k)| k.as_str()),
        )
    }

    fn log(&mut self, c: MirrorChange) {
        if self.changes.len() >= CHANGE_LOG_CAP {
            self.changes.pop_front();
        }
        self.changes.push_back((self.change_seq, c));
        self.change_seq += 1;
    }

    /// Start tracking `w`; returns its current revision. Only watched paths are tracked.
    pub fn watch(&self, w: Watch) -> u64 {
        *self.watched.borrow_mut().entry(w).or_insert(0)
    }
    /// The revision of a watched path (`None`: not watched).
    pub fn watched_revision(&self, w: &Watch) -> Option<u64> {
        self.watched.borrow().get(w).copied()
    }
    /// How many paths are watched (only those get per-path revisions).
    pub fn watched_len(&self) -> usize {
        self.watched.borrow().len()
    }

    // ---- the stream pump (crate-private: panels cannot call these) -----------------------

    fn bump(&mut self, w: &Watch) {
        if let Some(r) = self.watched.get_mut().get_mut(w) {
            *r += 1;
        }
    }

    fn touch_setting(&mut self, key: &str) {
        self.settings_rev += 1;
        if self.setting_changes.len() >= CHANGE_LOG_CAP {
            self.setting_changes.pop_front();
        }
        self.setting_changes
            .push_back((self.setting_change_seq, key.to_string()));
        self.setting_change_seq += 1;
        let hits: Vec<Watch> = self
            .watched
            .get_mut()
            .keys()
            .filter(|w| match w {
                Watch::Setting(k) => k == key,
                Watch::SettingPrefix(p) => key.starts_with(p.as_str()),
                _ => false,
            })
            .cloned()
            .collect();
        for w in hits {
            self.bump(&w);
        }
    }

    /// Replace everything with a snapshot (first sync, or after a `Gap`). Public for a
    /// read-model a plugin keeps from its own subscription (a protocol server's host).
    /// The mirror is never project state: this changes nothing a command owns.
    pub fn resync(&mut self, project: &Project, next_seq: u64, history: Vec<TxnInfo>) {
        self.entities.clear();
        self.roots.clear();
        for (k, e) in project.entities() {
            self.entities.insert(
                k,
                MirrorEntity {
                    name: e.name().to_string(),
                    parent: e.parent(),
                    children: e.children().collect(),
                    properties: e
                        .properties()
                        .map(|(p, v)| (p.to_string(), v.clone()))
                        .collect(),
                },
            );
        }
        self.roots = project.roots().collect();
        self.settings = project
            .settings()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        self.history.clear();
        self.history_start = 0;
        self.history_index.clear();
        for i in history {
            self.push_history(i);
        }
        self.next_seq = next_seq;
        self.revision += 1;
        self.structure_rev += 1;
        self.settings_rev += 1;
        self.history_rev += 1;
        for r in self.watched.get_mut().values_mut() {
            *r += 1;
        }
        // Step both sequences past the snapshot, so a reader that remembered one from
        // before it is told it missed changes (and rebuilds) instead of seeing none.
        self.changes.clear();
        self.change_seq += 1;
        self.change_floor = self.change_seq;
        self.setting_changes.clear();
        self.setting_change_seq += 1;
        self.setting_change_floor = self.setting_change_seq;
    }

    fn push_history(&mut self, i: TxnInfo) {
        if matches!(i.state, TxnState::Cancelled) {
            return;
        }
        if self.history.len() - self.history_start >= MIRROR_HISTORY_CAP {
            let old = self.history[self.history_start].txn;
            self.history_index.remove(&old);
            self.history_start += 1;
            // Compact once the evicted prefix is as long as the window: one shift and one
            // index rewrite per MIRROR_HISTORY_CAP transactions.
            if self.history_start >= MIRROR_HISTORY_CAP {
                self.compact_history();
            }
        }
        self.history_index.insert(i.txn, self.history.len());
        self.history.push(HistoryEntry {
            txn: i.txn,
            label: i.label,
            issuer: i.issuer,
            state: i.state,
            at_ms: i.opened_at_ms,
        });
    }

    /// Drop the evicted prefix (see `history`) and renumber the index.
    fn compact_history(&mut self) {
        let n = self.history_start;
        self.history.drain(..n);
        self.history_start = 0;
        for v in self.history_index.values_mut() {
            *v -= n;
        }
    }

    fn set_state(&mut self, txn: TxnId, state: TxnState) {
        if let Some(&i) = self.history_index.get(&txn) {
            if state == TxnState::Cancelled {
                self.history.remove(i);
                self.history_index.remove(&txn);
                for v in self.history_index.values_mut() {
                    if *v > i {
                        *v -= 1;
                    }
                }
            } else {
                self.history[i].state = state;
            }
            self.history_rev += 1;
        }
    }

    /// Apply one event. `info` looks up a transaction the mirror has not seen yet (its
    /// label and issuer), once per transaction. Events already covered by a snapshot are
    /// skipped.
    pub fn apply(&mut self, ev: &Applied, info: impl FnOnce(TxnId) -> Option<TxnInfo>) {
        if ev.seq < self.next_seq {
            return;
        }
        self.next_seq = ev.seq + 1;
        self.events_applied += 1;
        self.revision += 1;
        for c in &ev.diff.changes {
            self.apply_change(c);
        }
        match ev.kind {
            AppliedKind::Command => {
                if !self.history_index.contains_key(&ev.txn) {
                    let i = info(ev.txn).unwrap_or(TxnInfo {
                        txn: ev.txn,
                        label: "Edit".into(),
                        issuer: ev.issuer.clone(),
                        state: TxnState::Committed,
                        opened_at_ms: 0,
                        undoable: true,
                    });
                    // A command that changed nothing never enters the bus history.
                    if !ev.diff.is_empty() || i.state == TxnState::Open {
                        self.push_history(i);
                        self.history_rev += 1;
                    }
                }
            }
            AppliedKind::Commit => self.set_state(ev.txn, TxnState::Committed),
            AppliedKind::Cancel => self.set_state(ev.txn, TxnState::Cancelled),
            AppliedKind::Undo => self.set_state(ev.txn, TxnState::Undone),
            AppliedKind::Redo => self.set_state(ev.txn, TxnState::Committed),
        }
    }

    /// Record what Ctrl+Z / Ctrl+Y target now (asked of the client once per pump).
    pub(crate) fn set_targets(&mut self, undo: Option<TxnId>, redo: Option<TxnId>) {
        if (undo, redo) != (self.undo_target, self.redo_target) {
            self.undo_target = undo;
            self.redo_target = redo;
            self.history_rev += 1;
            self.revision += 1;
        }
    }

    fn apply_change(&mut self, c: &Change) {
        match c {
            Change::Created {
                entity,
                name,
                parent,
            } => {
                self.entities.insert(
                    *entity,
                    MirrorEntity {
                        name: name.clone(),
                        parent: *parent,
                        ..MirrorEntity::default()
                    },
                );
                match parent {
                    Some(p) => {
                        if let Some(pe) = self.entities.get_mut(p) {
                            pe.children.insert(*entity);
                        }
                    }
                    None => {
                        self.roots.insert(*entity);
                    }
                }
                self.structure_rev += 1;
                self.bump(&Watch::Entity(*entity));
                self.log(MirrorChange::Created(*entity));
            }
            Change::Removed { entity, parent, .. } => {
                self.log(MirrorChange::Removed {
                    entity: *entity,
                    parent: *parent,
                });
                if let Some(e) = self.entities.remove(entity) {
                    for p in e.properties.keys() {
                        self.bump(&Watch::Property(*entity, p.clone()));
                    }
                }
                match parent {
                    Some(p) => {
                        if let Some(pe) = self.entities.get_mut(p) {
                            pe.children.remove(entity);
                        }
                    }
                    None => {
                        self.roots.remove(entity);
                    }
                }
                self.structure_rev += 1;
                self.bump(&Watch::Entity(*entity));
            }
            Change::Renamed { entity, after, .. } => {
                if let Some(e) = self.entities.get_mut(entity) {
                    e.name = after.clone();
                }
                self.structure_rev += 1;
                self.bump(&Watch::Entity(*entity));
                self.log(MirrorChange::Renamed(*entity));
            }
            Change::Reparented {
                entity,
                before,
                after,
            } => {
                match before {
                    Some(p) => {
                        if let Some(pe) = self.entities.get_mut(p) {
                            pe.children.remove(entity);
                        }
                    }
                    None => {
                        self.roots.remove(entity);
                    }
                }
                match after {
                    Some(p) => {
                        if let Some(pe) = self.entities.get_mut(p) {
                            pe.children.insert(*entity);
                        }
                    }
                    None => {
                        self.roots.insert(*entity);
                    }
                }
                if let Some(e) = self.entities.get_mut(entity) {
                    e.parent = *after;
                }
                self.structure_rev += 1;
                self.bump(&Watch::Entity(*entity));
                self.log(MirrorChange::Reparented {
                    entity: *entity,
                    before: *before,
                    after: *after,
                });
            }
            Change::Property {
                entity,
                path,
                after,
                ..
            } => {
                if let Some(e) = self.entities.get_mut(entity) {
                    match after {
                        Some(v) => {
                            e.properties.insert(path.to_string(), v.clone());
                        }
                        None => {
                            e.properties.remove(&**path);
                        }
                    }
                }
                self.bump(&Watch::Property(*entity, path.to_string()));
                self.log(MirrorChange::Property(*entity, path.to_string()));
            }
            Change::Setting { key, after, .. } => {
                match after {
                    Some(v) => {
                        self.settings.insert(key.clone(), v.clone());
                    }
                    None => {
                        self.settings.remove(key);
                    }
                }
                self.touch_setting(key);
            }
        }
    }

    // ---- optimistic overlays (O-13, the split editor) ---------------------------------------

    /// Pending optimistic overlays: predictions shown before the core answered.
    pub fn overlays(&self) -> usize {
        self.overlays.len()
    }

    /// Whether `c`'s preconditions hold on the mirror (what `Project::apply_all` checks).
    fn applies(&self, c: &Change) -> bool {
        match c {
            Change::Created { entity, parent, .. } => {
                !self.entities.contains_key(entity)
                    && parent.is_none_or(|p| self.entities.contains_key(&p))
            }
            Change::Removed {
                entity,
                name,
                parent,
            } => self.entities.get(entity).is_some_and(|e| {
                e.name == *name
                    && e.parent == *parent
                    && e.children.is_empty()
                    && e.properties.is_empty()
            }),
            Change::Renamed { entity, before, .. } => {
                self.entities.get(entity).is_some_and(|e| e.name == *before)
            }
            Change::Reparented {
                entity,
                before,
                after,
            } => {
                self.entities
                    .get(entity)
                    .is_some_and(|e| e.parent == *before)
                    && after.is_none_or(|p| {
                        self.entities.contains_key(&p) && !self.is_ancestor_or_self(*entity, p)
                    })
            }
            Change::Property {
                entity,
                path,
                before,
                ..
            } => self
                .entities
                .get(entity)
                .is_some_and(|e| e.properties.get(&**path) == before.as_ref()),
            Change::Setting { key, before, .. } => self.settings.get(key) == before.as_ref(),
        }
    }

    fn is_ancestor_or_self(&self, a: EntityKey, b: EntityKey) -> bool {
        let mut at = Some(b);
        let mut steps = 0usize;
        while let Some(k) = at {
            if k == a {
                return true;
            }
            steps += 1;
            if steps > self.entities.len() {
                return true;
            }
            at = self.entities.get(&k).and_then(|e| e.parent);
        }
        false
    }

    /// Apply `diff` as an overlay if every change's precondition holds in turn; otherwise
    /// change nothing.
    fn try_overlay(&mut self, diff: &forge_cmd::Diff) -> bool {
        for (i, c) in diff.changes.iter().enumerate() {
            if !self.applies(c) {
                for d in diff.changes[..i].iter().rev() {
                    self.apply_change(&d.inverse());
                }
                return false;
            }
            self.apply_change(c);
        }
        true
    }

    /// Take the overlays off (newest first), before the core's events are applied on the
    /// authoritative state. Whether there were any.
    pub(crate) fn lift_overlays(&mut self) -> bool {
        if self.overlays.is_empty() {
            return false;
        }
        let overlays = std::mem::take(&mut self.overlays);
        for (_, d) in overlays.iter().rev() {
            for c in d.changes.iter().rev() {
                self.apply_change(&c.inverse());
            }
        }
        self.overlays = overlays;
        self.revision += 1;
        true
    }

    /// After the core's events: forget the overlays of the `settled` requests, put the
    /// others back (`lifted`: they were taken off by [`ProjectMirror::lift_overlays`]) where
    /// they still apply, and lay the new `predicted` ones on top. A wrong prediction is thus
    /// gone the pump its answer arrives in: the one-frame correction of O-13.
    pub(crate) fn settle_overlays(
        &mut self,
        lifted: bool,
        predicted: &[Prediction],
        settled: &[Ticket],
    ) {
        let keep = self.faults.keep_overlays();
        let mut lifted = lifted;
        if !lifted && !keep && self.overlays.iter().any(|(t, _)| settled.contains(t)) {
            // An overlay is dropped only once it is off the state.
            lifted = self.lift_overlays();
        }
        let mut overlays = std::mem::take(&mut self.overlays);
        if !keep {
            overlays.retain(|(t, _)| !settled.contains(t));
        }
        if lifted {
            overlays.retain(|(_, d)| self.try_overlay(d));
        }
        for p in predicted {
            if (keep || !settled.contains(&p.ticket)) && self.try_overlay(&p.diff) {
                overlays.push((p.ticket, p.diff.clone()));
            }
        }
        self.revision += 1;
        self.overlays = overlays;
    }

    /// The mirror's content hash in the bus's terms: equal to `Project::state_hash` of the
    /// project it mirrors (tests compare the two after every pump).
    pub fn matches(&self, project: &Project) -> bool {
        let same_entities = project.len() == self.entities.len()
            && project.entities().all(|(k, e)| {
                self.entities.get(&k).is_some_and(|m| {
                    m.name == e.name()
                        && m.parent == e.parent()
                        && m.children.iter().copied().eq(e.children())
                        && m.properties.len() == e.properties().len()
                        && e.properties()
                            .all(|(p, v)| m.properties.get(p).is_some_and(|mv| mv == v))
                })
            });
        let same_settings = project.settings().len() == self.settings.len()
            && project
                .settings()
                .all(|(k, v)| self.settings.get(k).is_some_and(|mv| mv == v));
        same_entities && same_settings && project.roots().eq(self.roots.iter().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::BusClient;
    use crate::core::EditorCore;
    use forge_cmd::EditorCommand;

    fn pump(m: &mut ProjectMirror, c: &mut crate::core::LocalBus) {
        let p = c.pump();
        if p.gap.is_some() {
            let (project, next) = c.snapshot();
            m.resync(&project, next, c.history());
        }
        for ev in &p.events {
            m.apply(ev, |t| c.txn_info(t));
        }
    }

    #[test]
    fn the_change_log_replays_entity_changes_and_says_when_a_reader_fell_behind() {
        let core = EditorCore::new();
        let mut c = EditorCore::connect(&core, Issuer::Test);
        let mut m = ProjectMirror::new();
        let (project, next) = c.snapshot();
        m.resync(&project, next, c.history());
        let seen = m.change_seq();
        assert_eq!(m.changes_since(seen).map(Iterator::count), Some(0));
        c.apply(
            EditorCommand::Spawn {
                name: "A".into(),
                parent: None,
            },
            None,
        );
        c.apply(
            EditorCommand::Spawn {
                name: "B".into(),
                parent: None,
            },
            None,
        );
        pump(&mut m, &mut c);
        let (a, b) = (EntityKey(0), EntityKey(1));
        c.apply(
            EditorCommand::Rename {
                entity: a,
                name: "A2".into(),
            },
            None,
        );
        c.apply(
            EditorCommand::Reparent {
                entity: b,
                parent: Some(a),
            },
            None,
        );
        c.apply(
            EditorCommand::SetProperty {
                entity: b,
                path: "x".into(),
                value: Value::Int(1),
            },
            None,
        );
        c.apply(EditorCommand::Despawn { entity: a }, None);
        pump(&mut m, &mut c);
        let log: Vec<MirrorChange> = m
            .changes_since(seen)
            .map(|i| i.cloned().collect())
            .unwrap_or_default();
        assert_eq!(log[0], MirrorChange::Created(a));
        assert_eq!(log[1], MirrorChange::Created(b));
        assert_eq!(log[2], MirrorChange::Renamed(a));
        assert_eq!(
            log[3],
            MirrorChange::Reparented {
                entity: b,
                before: None,
                after: Some(a)
            }
        );
        assert_eq!(log[4], MirrorChange::Property(b, "x".into()));
        assert!(
            log[5..].contains(&MirrorChange::Removed {
                entity: a,
                parent: None
            }),
            "{log:?}"
        );
        // A reader from before a snapshot must rebuild.
        let (project, next) = c.snapshot();
        m.resync(&project, next, c.history());
        assert!(m.changes_since(seen).is_none());
        assert_eq!(
            m.changes_since(m.change_seq()).map(Iterator::count),
            Some(0)
        );
        // A reader further behind than the bounded log must rebuild too.
        let before = m.change_seq();
        for i in 0..(CHANGE_LOG_CAP as u64 / 2 + 8) {
            c.apply(
                EditorCommand::Spawn {
                    name: format!("n{i}"),
                    parent: None,
                },
                None,
            );
            c.apply(
                EditorCommand::SetProperty {
                    entity: EntityKey(2 + i),
                    path: "p".into(),
                    value: Value::Int(0),
                },
                None,
            );
            if i % 256 == 0 {
                pump(&mut m, &mut c);
            }
        }
        pump(&mut m, &mut c);
        assert!(m.changes_since(before).is_none(), "fell out of the log");
        assert!(m.matches(&c.snapshot().0));
    }

    #[test]
    fn the_setting_log_replays_changed_keys_and_says_when_a_reader_fell_behind() {
        let core = EditorCore::new();
        let mut c = EditorCore::connect(&core, Issuer::Test);
        let mut m = ProjectMirror::new();
        let (project, next) = c.snapshot();
        m.resync(&project, next, c.history());
        let seen = m.setting_change_seq();
        assert_eq!(m.setting_changes_since(seen).map(Iterator::count), Some(0));
        let set = |c: &mut crate::core::LocalBus, k: &str, v: Option<Value>| {
            c.apply(
                EditorCommand::SetSetting {
                    key: k.into(),
                    value: v,
                },
                None,
            );
        };
        set(&mut c, "a.x", Some(Value::Int(1)));
        set(&mut c, "b.y", Some(Value::Int(2)));
        set(&mut c, "a.x", None);
        pump(&mut m, &mut c);
        let log: Vec<String> = m
            .setting_changes_since(seen)
            .map(|i| i.map(str::to_string).collect())
            .unwrap_or_default();
        assert_eq!(log, ["a.x", "b.y", "a.x"]);
        let mid = m.setting_change_seq();
        set(&mut c, "c.z", Some(Value::Int(3)));
        pump(&mut m, &mut c);
        let tail: Vec<&str> = m
            .setting_changes_since(mid)
            .map(Iterator::collect)
            .unwrap_or_default();
        assert_eq!(tail, ["c.z"]);
        // A snapshot replaces the log: a reader from before it re-reads.
        let (project, next) = c.snapshot();
        m.resync(&project, next, c.history());
        assert!(m.setting_changes_since(seen).is_none());
        // So does falling further behind than the bounded log.
        let before = m.setting_change_seq();
        for i in 0..(CHANGE_LOG_CAP as i64 + 8) {
            set(&mut c, "n", Some(Value::Int(i)));
            if i % 512 == 0 {
                pump(&mut m, &mut c);
            }
        }
        pump(&mut m, &mut c);
        assert!(
            m.setting_changes_since(before).is_none(),
            "fell out of the log"
        );
    }

    /// The history window at the cap: every push evicts the oldest entry, the window is
    /// exactly the newest MIRROR_HISTORY_CAP, lookups and travel plans agree with it across
    /// the batched compaction, and a cancel inside the window still removes its entry.
    #[test]
    fn the_history_window_stays_exact_across_eviction_and_compaction() {
        let info = |t: u64, state: TxnState| TxnInfo {
            txn: TxnId(t),
            label: format!("Edit {t}"),
            issuer: Issuer::Test,
            state,
            opened_at_ms: t,
            undoable: true,
        };
        let mut m = ProjectMirror::new();
        let total = (MIRROR_HISTORY_CAP * 5 / 2) as u64;
        for t in 0..total {
            m.push_history(info(t, TxnState::Committed));
            let len = (t as usize + 1).min(MIRROR_HISTORY_CAP);
            assert_eq!(m.history().len(), len);
            let oldest = t + 1 - len as u64;
            assert_eq!(m.history()[0].txn, TxnId(oldest));
            assert!(m.history_entry(TxnId(t)).is_some_and(|h| h.txn == TxnId(t)));
            if oldest > 0 {
                assert!(
                    m.history_entry(TxnId(oldest - 1)).is_none(),
                    "evicted {}",
                    oldest - 1
                );
            }
        }
        // Stored entries never exceed twice the window (the batch drop ran).
        assert!(m.history.len() < 2 * MIRROR_HISTORY_CAP);
        let first = total - MIRROR_HISTORY_CAP as u64;
        for t in [first, first + 1, first + 2507, total - 1] {
            assert_eq!(
                m.history_entry(TxnId(t)).map(|h| h.label.clone()),
                Some(format!("Edit {t}"))
            );
        }
        // Undo back to (after) the oldest listed transaction: every newer one, newest first.
        let plan = m.plan_travel(Some(TxnId(first)));
        assert_eq!(plan.len(), MIRROR_HISTORY_CAP - 1);
        assert_eq!(plan.first(), Some(&Travel::Undo(TxnId(total - 1))));
        assert_eq!(m.plan_travel(None).len(), MIRROR_HISTORY_CAP);
        // A cancelled gesture inside the window leaves it; the rest still resolve.
        let mid = total - 10;
        m.set_state(TxnId(mid), TxnState::Cancelled);
        assert!(m.history_entry(TxnId(mid)).is_none());
        assert_eq!(m.history().len(), MIRROR_HISTORY_CAP - 1);
        for t in [first, mid - 1, mid + 1, total - 1] {
            assert_eq!(m.history_entry(TxnId(t)).map(|h| h.txn), Some(TxnId(t)));
        }
    }
}
