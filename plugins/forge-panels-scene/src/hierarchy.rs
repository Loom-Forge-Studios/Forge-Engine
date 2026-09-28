//! The **Scene hierarchy** panel (`forge.hierarchy`, Ch.21 §21.21, DoD M2-35).
//!
//! * A virtualised tree ([`VirtualTree`]) over the mirror: 100,000 entities cost what a
//!   screenful costs (`ui_hierarchy_100k`).
//! * It follows the mirror **incrementally**, filtered or not: each loop turn it applies
//!   the mirror's entity change log (created, removed, renamed, moved, a visibility/lock
//!   flag) to the rows it touches. Under a filter a change re-tests only the entity it
//!   names and walks its ancestors (adding or pruning context rows), so an automation session
//!   streaming edits while the user filters costs O(depth · log) per edit. It rebuilds only when
//!   it fell behind the log. An automation session's edit appears next frame by the same path as
//!   the user's.
//! * The filter is folded once ([`ContainsQuery`]): testing a name allocates nothing. A
//!   keystroke diffs the rows (removes what the new query hides, inserts what it adds);
//!   a query that narrows the previous one re-tests only the rows shown now.
//! * Drag to reparent (and Alt+Up/Down), multi-select, rename in place (F2 or the
//!   `hierarchy.rename` action), Delete, search/filter (matches plus their ancestors,
//!   dimmed), and per-row visibility and lock toggles. **Every edit is a command** through
//!   the emitter; several rows at once are one transaction, one undo entry.
//! * Locked entities refuse rename, move and delete from here (with a notice naming them);
//!   hidden ones are dimmed. The flags are project properties `editor.hidden` and
//!   `editor.locked`, so they are shared, undoable and visible to automation sessions.
//! * Selection is session state, shared with the inspector through the session.
//!
//! Siblings are listed in creation order (entity key order). The project model has no
//! sibling order yet, so a drop between two rows reparents and does not reorder.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::time::{Duration, Instant};

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::feed::{FeedRelay, FeedTicked};
use forge_editor::mirror::{MirrorChange, MirrorEntity, ProjectMirror};
use forge_editor::panels::PanelCx;
use forge_editor::session::ShellRequest;
use forge_ui::dock::PanelId;
use forge_ui::fuzzy::ContainsQuery;
use forge_ui::widgets::{
    AssetDropped, Button, Container, DropTarget, EmptyState, EmptyStateAction, Pressed,
    RowActivated, RowBadge, RowBadgeClicked, RowItem, RowRenamed, RowsDeleteRequested, RowsDropped,
    SearchChanged, SearchField, SelectionChanged, Spinner, VirtualTree,
};
use forge_ui::{InputEvent, KeyCode, KeyEvent, Modifiers, NodeStyle, Role, Signal, Ui, WidgetId};

/// Project property: the entity is hidden in editor views.
pub const HIDDEN: &str = "editor.hidden";
/// Project property: the entity is locked against edits from the editor.
pub const LOCKED: &str = "editor.locked";
/// Badge index of the visibility toggle.
pub const BADGE_VISIBLE: usize = 0;
/// Badge index of the lock toggle.
pub const BADGE_LOCKED: usize = 1;

fn flag(e: &MirrorEntity, p: &str) -> bool {
    e.properties.get(p) == Some(&Value::Bool(true))
}

/// Scene composition's marks (WP-U20): the property paths that decide a row's icon.
const COMPOSITION_MARKS: [&str; 4] = ["scene.library", "scene.id", "scene.instance", "scene.base"];

fn composition_mark(p: &str) -> bool {
    COMPOSITION_MARKS.contains(&p)
}

/// The icon of a row by its part in scene composition: the scene library, a scene or an
/// instance of one, a node an instance inherits; none for a plain entity.
fn composition_icon(e: &MirrorEntity) -> Option<&'static str> {
    let has = |p: &str| e.properties.contains_key(p);
    if flag(e, "scene.library") {
        Some("\u{25a6}")
    } else if has("scene.id") || has("scene.instance") {
        Some("\u{2756}")
    } else if has("scene.base") {
        Some("\u{25c7}")
    } else {
        None
    }
}

/// The row an entity shows.
pub fn row_item(e: &MirrorEntity, context_only: bool) -> RowItem {
    let hidden = flag(e, HIDDEN);
    let mut item = RowItem::new(e.name.clone());
    item.icon = composition_icon(e);
    item.badge(RowBadge {
        on_glyph: "\u{25c9}",
        off_glyph: "\u{25cb}",
        on: !hidden,
        label: forge_ui::tr!("Visible"),
    })
    .badge(RowBadge {
        on_glyph: "\u{25a3}",
        off_glyph: "\u{25a1}",
        on: flag(e, LOCKED),
        label: forge_ui::tr!("Locked"),
    })
    .muted(hidden || context_only)
}

/// What the panel remembers between loop turns.
struct View {
    tree: WidgetId,
    /// The mirror's change sequence already applied.
    seen: u64,
    /// The filter, folded once: testing a name against it allocates nothing.
    query: ContainsQuery,
    /// The filter still being applied (a slice per loop turn).
    job: Option<FilterJob>,
    /// Runs the bar's spinner while a job runs (its frames keep the loop turning).
    filtering: Signal<bool>,
    spinner: WidgetId,
    /// Selection as last pushed to / read from the session.
    selection_rev: u64,
    /// Positive control for `ui_hierarchy_100k`'s filtered case: rebuild every row on each
    /// mirror change while a filter is active.
    fault_filter_rebuilds: bool,
    /// Positive control for its keystroke budget: apply a filter in one go.
    fault_filter_unbounded: bool,
    /// Positive control for `test_hierarchy_filter_bounded`: snapshot rows / roots at a
    /// phase change.
    fault_filter_snapshots: bool,
    /// Positive control for `test_hierarchy_hidden_edits_no_damage`: edit the tree on every
    /// mirror change, shown or not.
    fault_edit_every_change: bool,
    /// Where the job reports its largest step (the guard's probe; `None` in the editor).
    step_probe: Option<Rc<Cell<usize>>>,
}

impl View {
    /// Report a slice's largest step to the guard's probe (if one is installed).
    fn record(&self, meter: &Meter) {
        if let Some(p) = &self.step_probe {
            p.set(p.get().max(meter.max_step));
        }
    }
}

/// How much filter work one loop turn does: a keystroke's turn (this slice plus layout
/// and paint) fits one frame whatever the entity count (D-5); the rest continues on the
/// next turns while the bar's spinner runs.
pub const FILTER_SLICE: Duration = Duration::from_millis(4);
/// Items processed between clock reads (and the least a slice does, so it always
/// progresses).
const SLICE_CHECK: usize = 128;

/// Is `e` a context row under `q` (shown only because something under it matches)?
fn context_only(q: &ContainsQuery, e: &MirrorEntity) -> bool {
    !q.is_empty() && !q.matches(&e.name)
}

/// Does the filter show `k`? A match, or an ancestor of one (`anc`).
fn shows(
    q: &ContainsQuery,
    anc: &HashMap<EntityKey, bool>,
    k: EntityKey,
    e: &MirrorEntity,
) -> bool {
    q.is_empty() || anc.contains_key(&k) || q.matches(&e.name)
}

/// Re-dim a kept row if its match state (or hidden flag) changed. The hidden flag is read
/// only when the row's dimming disagrees with its match state.
fn redim(t: &mut VirtualTree, e: &MirrorEntity, k: u64, context: bool) {
    let muted = t.item(k).map(|i| i.muted);
    if muted != Some(context) && muted != Some(flag(e, HIDDEN) || context) {
        t.set_item(k, row_item(e, context));
    }
}

/// A pre-order walk over the tree's rows (collapsed ones included) that resumes across
/// loop turns **without a snapshot**: a stack of `(parent, last child visited)`, each
/// step finding the next sibling by key (siblings are kept in key order) in
/// O(log b · log₃₂ b). Removing the row just visited (and so its subtree) is safe: its
/// frame finds no children and pops.
/// A walk frame: `(parent, last child visited and the position it was at)`.
type WalkFrame = (Option<u64>, Option<(u64, usize)>);

struct TreeWalk {
    /// `(parent, last child visited and the position it was at)`.
    stack: Vec<WalkFrame>,
}

impl TreeWalk {
    fn new() -> Self {
        Self {
            stack: vec![(None, None)],
        }
    }
    fn next(&mut self, t: &VirtualTree) -> Option<u64> {
        loop {
            let (parent, after) = *self.stack.last()?;
            let pos = match after {
                None => 0,
                // Still where it was (the common case): the next position. Removed or moved
                // (the caller pruned it, or rows changed between slices): search by key.
                Some((a, at)) if t.child_at(parent, at) == Some(a) => at + 1,
                Some((a, _)) => {
                    let p = t.key_order_position(parent, a);
                    if t.child_at(parent, p) == Some(a) {
                        p + 1
                    } else {
                        p
                    }
                }
            };
            match t.child_at(parent, pos) {
                None => {
                    self.stack.pop();
                }
                Some(k) => {
                    if let Some(top) = self.stack.last_mut() {
                        top.1 = Some((k, pos));
                    }
                    if t.index().has_children(k) {
                        self.stack.push((Some(k), None));
                    }
                    return Some(k);
                }
            }
        }
    }
}

/// Where a scan over entities or rows reads from.
enum Scan {
    /// Every entity, resuming after this key.
    Mirror(Option<EntityKey>),
    /// The tree's rows, walked by cursor. For the match scan these are the rows shown when
    /// the job began (the new query narrows the old one, so its matches are among them).
    Rows(TreeWalk),
    /// The positive control's snapshot of every row, resuming at an index.
    Snapshot(Vec<u64>, usize),
}

/// One parent whose children are being merged into the tree, walked by cursor over the
/// mirror's key-ordered child set (no snapshot of the children).
struct Frame {
    parent: Option<EntityKey>,
    /// The last child visited (`None`: none yet).
    after: Option<EntityKey>,
    /// The positive control's snapshot of the children.
    snapshot: Option<(Vec<EntityKey>, usize)>,
    /// Child position in the tree of the next shown child.
    pos: usize,
}

impl Frame {
    fn new(
        m: &ProjectMirror,
        parent: Option<EntityKey>,
        snapshot: bool,
        meter: &mut Meter,
    ) -> Self {
        let snapshot = snapshot.then(|| {
            let kids: Vec<EntityKey> = match parent {
                None => m.roots().collect(),
                Some(p) => m
                    .entity(p)
                    .map(|e| e.children.iter().copied().collect())
                    .unwrap_or_default(),
            };
            meter.spend(kids.len());
            (kids, 0)
        });
        Self {
            parent,
            after: None,
            snapshot,
            pos: 0,
        }
    }
    /// The next child to merge, in key order.
    fn next(&mut self, m: &ProjectMirror) -> Option<EntityKey> {
        if let Some((kids, at)) = &mut self.snapshot {
            let k = kids.get(*at).copied();
            *at += 1;
            return k;
        }
        let k = m.child_after(self.parent, self.after)?;
        self.after = Some(k);
        Some(k)
    }
}

enum Phase {
    /// Find the matches and mark their ancestors.
    Scan(Scan),
    /// Remove rows the query hides, re-dim the rest (skipped when the query broadens: it
    /// hides nothing).
    Prune(Scan),
    /// Insert rows the query adds, re-dim the rest, open parents under a filter (skipped
    /// when the query narrows: it adds nothing).
    Populate(Vec<Frame>),
}

/// The slice clock, and the guard's measure of the work run between two of its checks:
/// every item touched is spent here, and [`Meter::over`] closes a step.
struct Meter {
    deadline: Option<Instant>,
    checks: usize,
    step: usize,
    max_step: usize,
}

impl Meter {
    fn new(deadline: Option<Instant>) -> Self {
        Self {
            deadline,
            checks: 0,
            step: 0,
            max_step: 0,
        }
    }
    fn spend(&mut self, n: usize) {
        self.step += n;
    }
    /// One item done: is the slice over?
    fn over(&mut self) -> bool {
        self.step += 1;
        self.max_step = self.max_step.max(self.step);
        self.step = 0;
        self.checks += 1;
        self.checks.is_multiple_of(SLICE_CHECK)
            && self.deadline.is_some_and(|d| Instant::now() >= d)
    }
}

/// A filter being applied to the rows in bounded slices (see [`FILTER_SLICE`]). The mirror
/// is not followed while a job runs; afterwards the change log since it began is replayed
/// (every step re-reads the current mirror, so the replay is idempotent). Every phase,
/// and every phase change, walks by cursor: no step between two slice checks touches
/// more than one item and its ancestors, whatever the entity count.
struct FilterJob {
    phase: Phase,
    /// Ancestors of matches seen by the scan (`true`: it matches itself).
    anc: HashMap<EntityKey, bool>,
    /// The last parent whose ancestors were marked: siblings are adjacent in key order, so
    /// a thousand matches under one parent cost one look-up.
    last_parent: Option<EntityKey>,
    prune: bool,
    populate: bool,
    /// The positive control's snapshots (`PanelFaults::hierarchy_filter_snapshots`).
    snapshots: bool,
}

impl FilterJob {
    /// A job taking the rows from what `prev` showed (`settled`: no job was cut short, so
    /// the rows are exactly that) to what `q` shows.
    fn new(
        t: &VirtualTree,
        q: &ContainsQuery,
        prev: &ContainsQuery,
        settled: bool,
        snapshots: bool,
        meter: &mut Meter,
    ) -> Self {
        let narrows = settled && !prev.is_empty() && prev.is_narrowed_by(q);
        let broadens = settled && q.is_narrowed_by(prev);
        let scan = if !narrows {
            Scan::Mirror(None)
        } else {
            Self::rows(t, snapshots, meter)
        };
        Self {
            phase: Phase::Scan(scan),
            anc: HashMap::new(),
            last_parent: None,
            prune: !broadens,
            // Narrowing adds no row; from every row (no filter) there is none to add.
            populate: !narrows && !(settled && prev.is_empty()),
            snapshots,
        }
    }

    /// A walk over every row (or the positive control's snapshot of them).
    fn rows(t: &VirtualTree, snapshots: bool, meter: &mut Meter) -> Scan {
        if snapshots {
            let rows = t.keys_vec();
            meter.spend(rows.len());
            Scan::Snapshot(rows, 0)
        } else {
            Scan::Rows(TreeWalk::new())
        }
    }

    /// Work until done (`true`) or the meter's deadline passes.
    fn run(
        &mut self,
        t: &mut VirtualTree,
        m: &ProjectMirror,
        q: &ContainsQuery,
        meter: &mut Meter,
    ) -> bool {
        loop {
            match &mut self.phase {
                Phase::Scan(scan) => {
                    if !q.is_empty()
                        && let Scan::Mirror(after) = scan
                    {
                        // The common case, iterated in place (the range resumes by key).
                        for (k, e) in m.entities_after(*after) {
                            *after = Some(*k);
                            if q.matches(&e.name) {
                                let walked =
                                    mark(&mut self.anc, &mut self.last_parent, m, q, e.parent);
                                meter.spend(walked);
                            }
                            if meter.over() {
                                return false;
                            }
                        }
                    } else if !q.is_empty() {
                        loop {
                            let e = match scan {
                                Scan::Mirror(_) => break,
                                Scan::Rows(walk) => match walk.next(t) {
                                    None => break,
                                    Some(k) => m.entity(EntityKey(k)),
                                },
                                Scan::Snapshot(rows, at) => {
                                    let Some(k) = rows.get(*at).copied() else {
                                        break;
                                    };
                                    *at += 1;
                                    m.entity(EntityKey(k))
                                }
                            };
                            if let Some(e) = e
                                && q.matches(&e.name)
                            {
                                let walked =
                                    mark(&mut self.anc, &mut self.last_parent, m, q, e.parent);
                                meter.spend(walked);
                            }
                            if meter.over() {
                                return false;
                            }
                        }
                    }
                    self.phase = if self.prune {
                        Phase::Prune(Self::rows(t, self.snapshots, meter))
                    } else {
                        Phase::Populate(vec![Frame::new(m, None, self.snapshots, meter)])
                    };
                }
                Phase::Prune(rows) => {
                    loop {
                        let k = match rows {
                            Scan::Rows(walk) => walk.next(t),
                            Scan::Snapshot(rows, at) => {
                                let k = rows.get(*at).copied();
                                *at += 1;
                                k
                            }
                            Scan::Mirror(_) => None,
                        };
                        let Some(k) = k else { break };
                        // Gone already with a removed ancestor.
                        if t.contains(k) {
                            let key = EntityKey(k);
                            match m.entity(key) {
                                Some(e) if shows(q, &self.anc, key, e) => {
                                    redim(t, e, k, context_only(q, e));
                                    // A filter shows every match: open what has rows under it.
                                    if !q.is_empty()
                                        && t.index().has_children(k)
                                        && !t.is_expanded(k)
                                    {
                                        t.set_expanded(k, true);
                                    }
                                }
                                _ => {
                                    t.remove(k);
                                }
                            }
                        }
                        if meter.over() {
                            return false;
                        }
                    }
                    if !self.populate {
                        return true;
                    }
                    self.phase = Phase::Populate(vec![Frame::new(m, None, self.snapshots, meter)]);
                }
                Phase::Populate(stack) => {
                    if meter.over() {
                        return false;
                    }
                    let Some(f) = stack.last_mut() else {
                        return true;
                    };
                    let tp = f.parent.map(|p| p.0);
                    let Some(k) = f.next(m) else {
                        if !q.is_empty()
                            && let Some(p) = tp
                            && t.index().has_children(p)
                            && !t.is_expanded(p)
                        {
                            t.set_expanded(p, true);
                        }
                        stack.pop();
                        continue;
                    };
                    let Some(e) = m.entity(k) else { continue };
                    if !shows(q, &self.anc, k, e) {
                        continue;
                    }
                    let context = context_only(q, e);
                    if t.contains(k.0) {
                        if t.parent_of(k.0) != tp {
                            t.move_node(
                                k.0,
                                DropTarget {
                                    parent: tp,
                                    index: t.key_order_position(tp, k.0),
                                },
                            );
                        }
                        redim(t, e, k.0, context);
                        f.pos += 1;
                    } else {
                        // The merge position, checked against its neighbours (the mirror
                        // may have changed between slices).
                        let fits = (f.pos == 0
                            || t.child_at(tp, f.pos - 1).is_some_and(|c| c < k.0))
                            && t.child_at(tp, f.pos).is_none_or(|c| c > k.0);
                        let pos = if fits {
                            f.pos
                        } else {
                            t.key_order_position(tp, k.0)
                        };
                        t.insert(tp, pos, k.0, row_item(e, context));
                        f.pos = pos + 1;
                    }
                    if !e.children.is_empty() {
                        let child = Frame::new(m, Some(k), self.snapshots, meter);
                        stack.push(child);
                    }
                }
            }
        }
    }
}

/// Mark the ancestors of a match (`last_parent`: siblings are often adjacent, so a
/// thousand matches under one parent cost one look-up). Returns the ancestors walked.
fn mark(
    anc: &mut HashMap<EntityKey, bool>,
    last_parent: &mut Option<EntityKey>,
    m: &ProjectMirror,
    q: &ContainsQuery,
    parent: Option<EntityKey>,
) -> usize {
    if parent.is_none() || parent == *last_parent {
        return 0;
    }
    *last_parent = parent;
    let mut cur = parent;
    let mut walked = 0;
    while let Some(p) = cur {
        walked += 1;
        if anc.contains_key(&p) {
            // A context row has its ancestors marked already; a matching one marks its own
            // when the scan reaches it.
            break;
        }
        let Some(pe) = m.entity(p) else { break };
        let hit = q.matches(&pe.name);
        anc.insert(p, hit);
        if hit {
            break;
        }
        cur = pe.parent;
    }
    walked
}

/// Rebuild every row from the mirror in one go (the change log was lost). Keeps the
/// expansion of rows that remain when unfiltered.
fn rebuild(t: &mut VirtualTree, m: &ProjectMirror, q: &ContainsQuery) {
    let selected = t.selected();
    let expanded: Vec<u64> = if q.is_empty() {
        t.keys().filter(|k| t.is_expanded(*k)).collect()
    } else {
        Vec::new()
    };
    t.clear();
    let mut meter = Meter::new(None);
    let mut job = FilterJob::new(t, q, q, false, false, &mut meter);
    job.prune = false;
    job.run(t, m, q, &mut meter);
    for k in expanded {
        if t.contains(k) {
            t.set_expanded(k, true);
        }
    }
    let keep: Vec<u64> = selected.into_iter().filter(|k| t.contains(*k)).collect();
    t.select(&keep);
}

/// Open the ancestors of `k` (a filter shows every match).
fn open_ancestors(t: &mut VirtualTree, k: u64) {
    let mut cur = t.parent_of(k);
    while let Some(p) = cur {
        if !t.is_expanded(p) {
            t.set_expanded(p, true);
        }
        cur = t.parent_of(p);
    }
}

/// Show `e` with any ancestors the tree lacks (under a filter they are context rows).
/// O(depth · log).
fn ensure_row(t: &mut VirtualTree, m: &ProjectMirror, q: &ContainsQuery, e: EntityKey) -> bool {
    let mut chain = Vec::new();
    let mut cur = Some(e);
    while let Some(k) = cur {
        if t.contains(k.0) {
            break;
        }
        let Some(ke) = m.entity(k) else { return false };
        chain.push(k);
        cur = ke.parent;
    }
    for k in chain.into_iter().rev() {
        let Some(ke) = m.entity(k) else { return false };
        let parent = ke.parent.map(|p| p.0);
        let pos = t.key_order_position(parent, k.0);
        let context = !q.is_empty() && !q.matches(&ke.name);
        if !t.insert(parent, pos, k.0, row_item(ke, context)) {
            return false;
        }
    }
    if !q.is_empty() {
        open_ancestors(t, e.0);
    }
    true
}

/// Under a filter: remove `start` and then each ancestor that is left with no rows under
/// it and does not match itself (context rows exist only for their matches).
fn prune(t: &mut VirtualTree, m: &ProjectMirror, q: &ContainsQuery, start: Option<u64>) {
    let mut cur = start;
    while let Some(k) = cur {
        if t.child_count(Some(k)) > 0 || m.entity(EntityKey(k)).is_some_and(|e| q.matches(&e.name))
        {
            return;
        }
        cur = t.parent_of(k);
        t.remove(k);
    }
}

/// An entity was created, renamed or had a property change: show, update or (under a
/// filter, when it no longer matches and has no matches under it) hide its row.
fn show_or_update(t: &mut VirtualTree, m: &ProjectMirror, q: &ContainsQuery, e: EntityKey) -> bool {
    // Removed again later in the same log: nothing to show.
    let Some(me) = m.entity(e) else { return true };
    let hit = q.matches(&me.name);
    if t.contains(e.0) {
        if !hit && t.child_count(Some(e.0)) == 0 {
            let parent = t.parent_of(e.0);
            t.remove(e.0);
            prune(t, m, q, parent);
        } else {
            t.set_item(e.0, row_item(me, !hit));
        }
        true
    } else if hit {
        ensure_row(t, m, q, e)
    } else {
        true
    }
}

/// Apply the change log to the rows it touches, with or without a filter: each change
/// re-tests only the entity it names and walks its ancestors, so one edit by another
/// issuer costs O(depth · log) whatever the entity count. `false`: rebuild instead.
fn apply_changes(t: &mut VirtualTree, m: &ProjectMirror, q: &ContainsQuery, seen: u64) -> bool {
    let Some(changes) = m.changes_since(seen) else {
        return false;
    };
    for c in changes.filter(|c| shown(c)) {
        match c {
            MirrorChange::Created(e) | MirrorChange::Renamed(e) | MirrorChange::Property(e, _) => {
                if !show_or_update(t, m, q, *e) {
                    return false;
                }
            }
            MirrorChange::Removed { entity, .. } => {
                let parent = t.parent_of(entity.0);
                if t.remove(entity.0) && !q.is_empty() {
                    prune(t, m, q, parent);
                }
            }
            MirrorChange::Reparented { entity, after, .. } => {
                // Not shown: nothing under it matches (a filter), or it is gone again.
                if !t.contains(entity.0) || m.entity(*entity).is_none() {
                    continue;
                }
                if let Some(p) = after
                    && !ensure_row(t, m, q, *p)
                {
                    return false;
                }
                let before = t.parent_of(entity.0);
                let parent = after.map(|p| p.0);
                let index = t.key_order_position(parent, entity.0);
                if !t.move_node(entity.0, DropTarget { parent, index }) {
                    return false;
                }
                if !q.is_empty() {
                    prune(t, m, q, before);
                    open_ancestors(t, entity.0);
                }
            }
        }
    }
    true
}

/// Does a change show in a row? A row shows the entity's name, its place in the tree and
/// the hidden and locked flags; a property change is shown only when it is one of those
/// flags. (A gizmo drag changes a transform every frame: none of it is shown here.)
fn shown(c: &MirrorChange) -> bool {
    match c {
        MirrorChange::Property(_, p) => p == HIDDEN || p == LOCKED || composition_mark(p),
        MirrorChange::Created(_)
        | MirrorChange::Removed { .. }
        | MirrorChange::Renamed(_)
        | MirrorChange::Reparented { .. } => true,
    }
}

/// Bring the rows in line with the mirror (incrementally, filtered or not). Changes no row
/// shows (a transform written every frame of a drag) leave the tree alone: no repaint, no
/// accessibility rebuild.
fn follow(ui: &mut Ui, v: &mut View, m: &ProjectMirror) {
    let seq = m.change_seq();
    if seq == v.seen || v.job.is_some() {
        return;
    }
    let (seen, q) = (v.seen, &v.query);
    if !v.fault_edit_every_change
        && m.changes_since(seen)
            .is_some_and(|mut changes| !changes.any(shown))
    {
        v.seen = seq;
        return;
    }
    let fault = v.fault_filter_rebuilds && !q.is_empty();
    VirtualTree::edit(ui, v.tree, |t| {
        if fault || !apply_changes(t, m, q, seen) {
            rebuild(t, m, q);
        }
    });
    v.seen = seq;
}

/// Start applying a new filter: the first slice runs in this loop turn's sync step.
fn start_filter(ui: &mut Ui, v: &mut View, m: &ProjectMirror, q: ContainsQuery) {
    // Catch up with the mirror under the old filter, so the job starts from rows that
    // agree with it (no-op while a job runs: its rows are mid-way anyway).
    follow(ui, v, m);
    let settled = v.job.is_none();
    let prev = std::mem::replace(&mut v.query, q);
    let mut meter = Meter::new(None);
    if let Some(t) = ui.widget::<VirtualTree>(v.tree) {
        v.job = Some(FilterJob::new(
            t,
            &v.query,
            &prev,
            settled,
            v.fault_filter_snapshots,
            &mut meter,
        ));
    }
    meter.over();
    v.record(&meter);
    v.filtering.set(ui.rt_mut(), true);
    let _ = ui.set_hidden(v.spinner, false);
}

/// One loop turn: a slice of the running filter job (then, once it is done, the change
/// log it deferred), else the change log.
/// Returns whether a job is still running (the caller asks for another turn).
fn step(ui: &mut Ui, v: &mut View, m: &ProjectMirror) -> bool {
    if let Some(mut job) = v.job.take() {
        let deadline = (!v.fault_filter_unbounded).then(|| Instant::now() + FILTER_SLICE);
        let mut meter = Meter::new(deadline);
        let q = &v.query;
        let done = VirtualTree::edit(ui, v.tree, |t| job.run(t, m, q, &mut meter)).unwrap_or(true);
        meter.over();
        v.record(&meter);
        if done {
            v.filtering.set(ui.rt_mut(), false);
            let _ = ui.set_hidden(v.spinner, true);
        } else {
            v.job = Some(job);
        }
    }
    follow(ui, v, m);
    v.job.is_some()
}

/// Keys, keeping only those not locked (and naming the locked ones).
fn unlocked(m: &ProjectMirror, keys: &[u64]) -> (Vec<EntityKey>, Vec<String>) {
    let mut ok = Vec::new();
    let mut locked = Vec::new();
    for k in keys {
        let e = EntityKey(*k);
        match m.entity(e) {
            Some(me) if flag(me, LOCKED) => locked.push(me.name.clone()),
            Some(_) => ok.push(e),
            None => {}
        }
    }
    (ok, locked)
}

fn refuse_locked(session: &mut forge_editor::session::SessionState, what: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    session.problem(forge_editor::notify::Problem::coded(
        forge_editor::notify::Severity::Warning,
        "EDITOR-0014",
        &forge_ui::trf!("{what}: {n} locked", what, n = names.len()),
        &forge_ui::trf!(
            "Unlock {names} first (the lock toggle in the hierarchy).",
            names = names.join(", ")
        ),
    ));
}

/// Keys whose ancestors are not also in the set (deleting a parent takes its subtree).
fn topmost(m: &ProjectMirror, keys: &[EntityKey]) -> Vec<EntityKey> {
    let set: BTreeSet<EntityKey> = keys.iter().copied().collect();
    keys.iter()
        .copied()
        .filter(|k| {
            let mut cur = m.entity(*k).and_then(|e| e.parent);
            while let Some(p) = cur {
                if set.contains(&p) {
                    return false;
                }
                cur = m.entity(p).and_then(|e| e.parent);
            }
            true
        })
        .collect()
}

/// How often the teammates' notes may refresh (Ch.21 §21.11: presence at most 10 Hz).
pub const COLLAB_HZ: u8 = 10;

/// The notes drawn after rows: who else is looking at an entity, who claimed it (WP-U10).
#[derive(Default)]
struct Notes {
    shown: std::collections::BTreeMap<u64, String>,
    seen: Option<u64>,
    feed: bool,
}

impl Notes {
    /// Bring the rows' notes up to date: recompute them when the team's state moved, and
    /// put back any a row rebuilt this turn lost. Costs what the notes cost, not the tree.
    fn apply(
        &mut self,
        ui: &mut Ui,
        tree: WidgetId,
        collab: &forge_editor::collab::CollabServices,
    ) {
        let rev = collab.revision();
        let old = if self.seen == Some(rev) {
            None
        } else {
            self.seen = Some(rev);
            Some(std::mem::replace(&mut self.shown, collab.notes()))
        };
        let shown = &self.shown;
        // Compare before touching the tree: `VirtualTree::edit` repaints it and rebuilds its
        // accessibility node, so it runs only when a row's note really differs (a note
        // changed, went away, or a row rebuilt this turn lost it) — a mirror or session change
        // in a team project costs the hierarchy nothing more than it does alone. The check
        // reads only the rows that have or had a note.
        let differs = |t: &VirtualTree| {
            let lost = old.as_ref().is_some_and(|old| {
                old.keys()
                    .any(|k| !shown.contains_key(k) && t.item(*k).is_some_and(|i| i.note.is_some()))
            });
            lost || shown
                .iter()
                .any(|(k, n)| t.item(*k).is_some_and(|i| i.note.as_ref() != Some(n)))
        };
        if !collab.faults.notes_always_edit()
            && !ui.widget::<VirtualTree>(tree).is_some_and(differs)
        {
            return;
        }
        VirtualTree::edit(ui, tree, |t| {
            let mut put = |k: u64, want: Option<&String>| {
                if let Some(item) = t.item(k)
                    && item.note.as_ref() != want
                {
                    let mut it = item.clone();
                    it.note = want.cloned();
                    t.set_item(k, it);
                }
            };
            if let Some(old) = &old {
                for k in old.keys() {
                    if !shown.contains_key(k) {
                        put(*k, None);
                    }
                }
            }
            for (k, n) in shown {
                put(*k, Some(n));
            }
        });
    }
}

/// `{verb} 1 entity` / `{verb} {n} entities`: the history label of a hierarchy edit. The
/// verb is one of the keys below, so each whole label is a key a translator sees.
// l10n-block: the verb selects the key; every arm's label is a trf! key
fn entities_label(verb: &str, n: usize) -> String {
    match (verb, n == 1) {
        ("Move", true) => forge_ui::trf!("Move {n} entity", n),
        ("Move", false) => forge_ui::trf!("Move {n} entities", n),
        ("Delete", true) => forge_ui::trf!("Delete {n} entity", n),
        ("Delete", false) => forge_ui::trf!("Delete {n} entities", n),
        ("Hide", true) => forge_ui::trf!("Hide {n} entity", n),
        ("Hide", false) => forge_ui::trf!("Hide {n} entities", n),
        ("Show", true) => forge_ui::trf!("Show {n} entity", n),
        ("Show", false) => forge_ui::trf!("Show {n} entities", n),
        ("Lock", true) => forge_ui::trf!("Lock {n} entity", n),
        ("Lock", false) => forge_ui::trf!("Lock {n} entities", n),
        ("Unlock", true) => forge_ui::trf!("Unlock {n} entity", n),
        ("Unlock", false) => forge_ui::trf!("Unlock {n} entities", n),
        (_, true) => forge_ui::trf!("{verb} {n} entity", verb = forge_ui::l10n::tr_str(verb), n),
        (_, false) => {
            forge_ui::trf!(
                "{verb} {n} entities",
                verb = forge_ui::l10n::tr_str(verb),
                n
            )
        }
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let faults = pb.services().faults.clone();
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space[1]).padding(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Hierarchy actions")),
        )?;
        let add = pb.b.add(
            bar,
            "add",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add entity")),
        )?;
        let query = pb.b.signal(String::new());
        let search = pb.b.add(
            bar,
            "search",
            NodeStyle::leaf().grow(1.0),
            SearchField::new(query, forge_ui::tr!("Filter entities")),
        )?;
        let filtering = pb.b.signal(false);
        let spinner = pb.b.add(
            bar,
            "filtering",
            NodeStyle::leaf(),
            Spinner::new(filtering, forge_ui::tr!("Filtering entities")),
        )?;
        pb.b.hide(spinner, true);
        let empty = pb.b.add(
            pb.parent,
            "empty",
            NodeStyle::leaf().padding(space[2]),
            EmptyState::new(forge_ui::tr!(
                "No entities yet. An automation session's or a script's entities appear here as they are made."
            ))
            .action(forge_ui::tr!("Add entity")),
        )?;
        // A scene tile from the asset browser dropped on a row instances it there (WP-U20).
        let mut t = VirtualTree::tree(forge_ui::tr!("Scene hierarchy")).accept_assets();
        #[cfg(any(test, feature = "controls"))]
        if faults.hierarchy_no_virtualisation() {
            t = t.with_fault_no_virtualisation();
        }
        let seq = {
            let m = pb.mirror();
            rebuild(&mut t, &m, &ContainsQuery::default());
            m.change_seq()
        };
        let has_any = t.row_count() > 0;
        let tree =
            pb.b.add(pb.parent, "tree", NodeStyle::leaf().grow(1.0), t)?;
        pb.b.hide(empty, has_any);
        let view = Rc::new(RefCell::new(View {
            tree,
            seen: seq,
            query: ContainsQuery::default(),
            selection_rev: pb.session().selection_revision(),
            job: None,
            filtering,
            spinner,
            fault_filter_rebuilds: faults.hierarchy_filter_rebuilds(),
            fault_filter_unbounded: faults.hierarchy_filter_unbounded(),
            fault_filter_snapshots: faults.hierarchy_filter_snapshots(),
            fault_edit_every_change: faults.hierarchy_edit_every_change(),
            step_probe: faults.hierarchy_step_probe().clone(),
        }));

        let add_entity = |act: &mut forge_editor::panel_rt::PanelAct| {
            let name = match act.mirror.setting("editor.new_entity_name") {
                Some(Value::Text(t)) if !t.trim().is_empty() => t.clone(),
                _ => forge_ui::tr!("Entity").to_string(),
            };
            let parent = act.session.selection.first().copied();
            act.cmd.emit(EditorCommand::Spawn { name, parent });
        };
        pb.on(add, move |act, _: &Pressed| add_entity(act));
        pb.on(empty, move |act, _: &EmptyStateAction| add_entity(act));
        let v = view.clone();
        pb.on(search, move |act, e: &SearchChanged| {
            let mut v = v.borrow_mut();
            let q = ContainsQuery::new(&e.query);
            if q == v.query {
                return;
            }
            start_filter(act.ui, &mut v, act.mirror, q);
            act.want_turn();
        });
        let v = view.clone();
        pb.on(tree, move |act, e: &SelectionChanged| {
            let keys: Vec<EntityKey> = e.keys.iter().map(|k| EntityKey(*k)).collect();
            act.session.set_selection(keys);
            v.borrow_mut().selection_rev = act.session.selection_revision();
        });
        pb.on(tree, |act, e: &RowRenamed| {
            let (ok, locked) = unlocked(act.mirror, &[e.key]);
            refuse_locked(act.session, forge_ui::tr!("Rename"), &locked);
            if let Some(entity) = ok.first() {
                act.cmd.emit(EditorCommand::Rename {
                    entity: *entity,
                    name: e.name.clone(),
                });
            }
        });
        pb.on(tree, |act, e: &RowsDropped| {
            let (ok, locked) = unlocked(act.mirror, &e.keys);
            refuse_locked(act.session, forge_ui::tr!("Move"), &locked);
            let parent = e.target.parent.map(EntityKey);
            let cmds: Vec<EditorCommand> = topmost(act.mirror, &ok)
                .into_iter()
                .filter(|k| act.mirror.entity(*k).is_some_and(|me| me.parent != parent))
                .map(|entity| EditorCommand::Reparent { entity, parent })
                .collect();
            let label = entities_label("Move", cmds.len()); // l10n: selects the key
            act.cmd.emit_all(&label, cmds);
        });
        pb.on(tree, |act, e: &AssetDropped| {
            // Only scenes are placed in the hierarchy; other assets have no entity form here.
            let Some(scene) = forge_editor::composition::payload_scene(&e.asset) else {
                return;
            };
            let parent = e.target.parent.map(EntityKey);
            act.cmd.emit_all(
                forge_ui::tr!("Instance scene"),
                vec![forge_editor::composition::instance_command(scene, parent)],
            );
        });
        pb.on(tree, |act, e: &RowsDeleteRequested| {
            let (ok, locked) = unlocked(act.mirror, &e.keys);
            refuse_locked(act.session, forge_ui::tr!("Delete"), &locked);
            let cmds: Vec<EditorCommand> = topmost(act.mirror, &ok)
                .into_iter()
                .map(|entity| EditorCommand::Despawn { entity })
                .collect();
            let label = entities_label("Delete", cmds.len()); // l10n: selects the key
            act.cmd.emit_all(&label, cmds);
        });
        pb.on(tree, |act, e: &RowBadgeClicked| {
            let key = EntityKey(e.key);
            let targets: Vec<EntityKey> = if act.session.selection.contains(&key) {
                act.session.selection.clone()
            } else {
                vec![key]
            };
            let prop = match e.badge {
                BADGE_VISIBLE => HIDDEN,
                BADGE_LOCKED => LOCKED,
                _ => return,
            };
            let Some(me) = act.mirror.entity(key) else {
                return;
            };
            let set = !flag(me, prop);
            let mut cmds = Vec::new();
            for t in &targets {
                let Some(te) = act.mirror.entity(*t) else {
                    continue;
                };
                if flag(te, prop) == set {
                    continue;
                }
                cmds.push(if set {
                    EditorCommand::SetProperty {
                        entity: *t,
                        path: prop.into(),
                        value: Value::Bool(true),
                    }
                } else {
                    EditorCommand::RemoveProperty {
                        entity: *t,
                        path: prop.into(),
                    }
                });
            }
            // l10n-block: the verbs select entities_label's key (the whole label is looked up there)
            let verb = match (prop == HIDDEN, set) {
                (true, true) => "Hide",
                (true, false) => "Show",
                (false, true) => "Lock",
                (false, false) => "Unlock",
            };
            let label = entities_label(verb, cmds.len());
            act.cmd.emit_all(&label, cmds);
        });
        pb.on(tree, |act, _: &RowActivated| {
            act.session
                .request(ShellRequest::OpenPanel(PanelId::new("forge.inspector")));
        });
        pb.on_op(tree, "hierarchy.rename", move |act| {
            act.ui.set_focus(Some(tree), true);
            act.ui.handle(InputEvent::Key(KeyEvent::press(
                KeyCode::F2,
                Modifiers::NONE,
            )));
        });
        // Teammates (WP-U10): who else is looking at a row, and who claimed it, drawn after
        // its name. A live feed (≤ 10 Hz, only while visible) says when to look again.
        let relay =
            pb.b.add(bar, "collab_feed", NodeStyle::leaf(), FeedRelay::new())?;
        pb.on(relay, |act, _: &FeedTicked| act.want_turn());
        let mut notes = Notes::default();
        let v = view;
        pb.sync(tree, move |s| {
            let mut v = v.borrow_mut();
            if step(s.ui, &mut v, s.mirror) {
                s.want_turn();
            }
            if s.services.collab.attached() {
                if !notes.feed {
                    notes.feed = true;
                    s.ui.add_feed(
                        relay,
                        forge_ui::LiveFeed {
                            source: s.services.collab.feed(),
                            max_hz: COLLAB_HZ,
                            self_ui: false,
                        },
                    );
                }
                notes.apply(s.ui, v.tree, &s.services.collab);
            }
            let rows =
                s.ui.widget::<VirtualTree>(v.tree)
                    .map_or(0, VirtualTree::row_count);
            let _ = s.ui.set_hidden(empty, rows > 0 || !v.query.is_empty());
            let rev = s.session.selection_revision();
            if rev != v.selection_rev {
                v.selection_rev = rev;
                let want: Vec<u64> = s.session.selection.iter().map(|k| k.0).collect();
                let tree = v.tree;
                let differs =
                    s.ui.widget::<VirtualTree>(tree)
                        .is_some_and(|t| t.selected() != want);
                if differs {
                    VirtualTree::edit(s.ui, tree, |t| t.select(&want));
                }
            }
            Ok(())
        });
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_labels_name_the_count() {
        assert_eq!(entities_label("Move", 1), "Move 1 entity");
        assert_eq!(entities_label("Delete", 3), "Delete 3 entities");
        assert_eq!(entities_label("Unlock", 2), "Unlock 2 entities");
    }
}
