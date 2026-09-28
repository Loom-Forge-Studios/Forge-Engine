//! Virtualised collections (§21.12, §21.16): the list and the tree view. The table is in
//! `table.rs` and shares the selection model here.
//!
//! * **Virtualised.** Rows are painted by the view from its model; only rows that meet the
//!   viewport are recorded, and only rows in the *live range* (visible ± an overscan of
//!   8) get accessibility nodes. Cost per frame is O(visible rows) whatever the row count
//!   (`ui_virtual_list_100k`, `ui_virtual_tree_100k`). A row is not a widget: 100,000
//!   rows are 100,000 model entries and ~40 painted records, not 100,000 taffy nodes
//!   (ADR 0013).
//! * **The tree is never flattened.** Its row index is a [`TreeIndex`] (counted B+tree
//!   per level): row → node and node → row are O(d · log₃₂ b), and expanding a 50,000-row
//!   subtree updates a few dozen index nodes.
//! * **Keyboard** (WAI-ARIA listbox / tree): Up/Down, PageUp/PageDown, Home/End move;
//!   Shift extends the selection, Ctrl moves without selecting and Ctrl+Space toggles;
//!   Ctrl+A selects all; Left/Right collapse/expand or go to the parent/first child;
//!   Enter activates; F2 renames in place; Delete asks to delete; Alt+Up/Down reorders
//!   (the keyboard twin of drag-and-drop); typing jumps to the next row starting with
//!   the typed text (type-ahead); Shift+F10 or the context-menu key asks for a menu.
//! * **I7.** Selection, expansion and scrolling are view state and change here. Renames,
//!   moves and deletes change the *model*: the view raises [`RowRenamed`],
//!   [`RowsDropped`] and [`RowsDeleteRequested`], and the owner (a panel emitting a
//!   command) applies them and then updates the view through [`VirtualTree::edit`].

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};

use accesskit::{Action, Role};

use crate::dnd::{DragPayload, DropVerdict};
use crate::geom::{Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::style::ColorRole;
use crate::ui::Ui;
use crate::virtualized::TreeIndex;
use crate::widget::{A11yCx, EventCx, MeasureCx, PaintCx, Widget};

use super::line_edit::{EditOutcome, LineEdit};
use super::{body, small};

/// Rows beyond each edge of the viewport that stay live (§21.12).
pub const OVERSCAN: usize = 8;
/// The type-ahead buffer clears after this long without a key.
pub const TYPEAHEAD_RESET: std::time::Duration = std::time::Duration::from_millis(900);
const TYPEAHEAD_TIMER: u64 = 0x7479_7065;
const INDENT: f32 = 16.0;
const BAR: f32 = 8.0;
const DRAG_SLOP: f32 = 4.0;

/// A clickable toggle glyph at a row's right edge (the hierarchy's visibility and lock
/// toggles). Clicking it raises [`RowBadgeClicked`]; it never changes the model itself (I7):
/// the owner emits a command and updates the row from the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowBadge {
    /// Glyph shown when `on`.
    pub on_glyph: &'static str,
    /// Glyph shown when off.
    pub off_glyph: &'static str,
    pub on: bool,
    /// What it toggles, for assistive technology ("Visible", "Locked").
    pub label: &'static str,
}

/// What a row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowItem {
    pub label: String,
    /// An icon glyph drawn before the label.
    pub icon: Option<&'static str>,
    /// Toggle glyphs at the right edge, left to right.
    pub badges: Vec<RowBadge>,
    /// Drawn in the muted colour (a hidden entity, an ancestor shown only for context).
    pub muted: bool,
    /// A short note drawn after the label in the muted colour and announced with it (who
    /// else is looking at the row, who claimed it; WP-U10). Not part of the label: renaming
    /// edits the label alone.
    pub note: Option<String>,
    /// The label is text a record or a file carries, shown exactly as written (an audit
    /// record's greppable line): never translated. Marked for assistive technology and for
    /// the pseudo-locale audit by its AccessKit class, as [`Label::verbatim`](super::Label::verbatim).
    pub verbatim: bool,
}

impl RowItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            badges: Vec::new(),
            muted: false,
            note: None,
            verbatim: false,
        }
    }
    /// Shown exactly as written (see [`RowItem::verbatim`]).
    pub fn verbatim(mut self) -> Self {
        self.verbatim = true;
        self
    }
    pub fn icon(mut self, glyph: &'static str) -> Self {
        self.icon = Some(glyph);
        self
    }
    pub fn badge(mut self, b: RowBadge) -> Self {
        self.badges.push(b);
        self
    }
    pub fn muted(mut self, on: bool) -> Self {
        self.muted = on;
        self
    }
    /// A note after the label (see [`RowItem::note`]).
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// Width of one row badge's hit area.
pub const BADGE_W: f32 = 22.0;

/// A row badge was clicked.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RowBadgeClicked {
    pub view: WidgetId,
    pub key: u64,
    /// Which badge, left to right.
    pub badge: usize,
}

/// Where dropped (or keyboard-moved) rows go: under `parent` (`None`: top level), before
/// its child `index`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DropTarget {
    pub parent: Option<u64>,
    pub index: usize,
}

/// Enter or a double click on a row.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RowActivated {
    pub view: WidgetId,
    pub key: u64,
}

/// The selection changed (sorted by row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionChanged {
    pub view: WidgetId,
    pub keys: Vec<u64>,
}

/// A rename in place was committed. The owner renames the model entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowRenamed {
    pub view: WidgetId,
    pub key: u64,
    pub name: String,
}

/// Rows were dropped (drag-and-drop or Alt+Up/Down). The owner moves the model entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowsDropped {
    pub view: WidgetId,
    pub keys: Vec<u64>,
    pub target: DropTarget,
}

/// An asset tile was dropped on the view (a view built with
/// [`VirtualTree::accept_assets`]): `asset` is the tile's drag id, `target` where it
/// landed. The owner decides what an asset there means (the hierarchy instances a scene).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetDropped {
    pub view: WidgetId,
    pub asset: String,
    pub target: DropTarget,
}

/// Delete was pressed on the selection. The owner confirms and deletes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowsDeleteRequested {
    pub view: WidgetId,
    pub keys: Vec<u64>,
}

/// A context menu was asked for over `key` (right click, Shift+F10, the menu key).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RowContextMenu {
    pub view: WidgetId,
    pub key: u64,
    pub at: Point,
}

/// Selection over rows identified by key, with an anchor for Shift ranges and an active
/// (focused) row. Shared by lists, trees and tables.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub keys: BTreeSet<u64>,
    pub anchor: Option<usize>,
    pub active: Option<usize>,
    pub multi: bool,
}

impl Selection {
    /// Apply a move of the active row to `to` with the modifiers held. `key_at` maps a
    /// row to its key. Returns whether the selection changed.
    pub fn move_to(
        &mut self,
        to: usize,
        shift: bool,
        ctrl: bool,
        key_at: &dyn Fn(usize) -> Option<u64>,
    ) -> bool {
        self.active = Some(to);
        if ctrl && self.multi {
            return false;
        }
        let before = self.keys.clone();
        if shift && self.multi {
            let a = self.anchor.unwrap_or(to);
            self.keys.clear();
            let (lo, hi) = (a.min(to), a.max(to));
            for r in lo..=hi {
                if let Some(k) = key_at(r) {
                    self.keys.insert(k);
                }
            }
        } else {
            self.keys.clear();
            if let Some(k) = key_at(to) {
                self.keys.insert(k);
            }
            self.anchor = Some(to);
        }
        before != self.keys
    }

    /// Ctrl+Space / Ctrl+click: toggle row `r`.
    pub fn toggle(&mut self, r: usize, key: u64) {
        self.active = Some(r);
        self.anchor = Some(r);
        if !self.multi {
            self.keys.clear();
            self.keys.insert(key);
            return;
        }
        if !self.keys.remove(&key) {
            self.keys.insert(key);
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum DropSpot {
    Before(u64),
    After(u64),
    Into(u64),
    End,
}

#[derive(Copy, Clone, Debug)]
struct Press {
    row: usize,
    pos: Point,
    /// A plain click on an already-selected row selects only it on release (so a
    /// multi-row selection can be dragged).
    deferred: bool,
    dragging: bool,
}

/// A virtualised tree view (or, built with [`VirtualTree::list`], a flat list).
pub struct VirtualTree {
    label: String,
    flat: bool,
    index: TreeIndex,
    items: HashMap<u64, RowItem>,
    row_h: f32,
    scroll: f32,
    view_h: f32,
    sel: Selection,
    reorderable: bool,
    /// Asset tiles may be dropped on it ([`AssetDropped`]).
    assets: bool,
    renamable: bool,
    rename: Option<(u64, LineEdit)>,
    typeahead: String,
    press: Option<Press>,
    last_pos: Point,
    hover_row: Option<usize>,
    drop: Option<DropSpot>,
    thumb_drag: Option<(f32, f32)>,
    live: Cell<usize>,
    painted: Cell<usize>,
    /// Positive control for `ui_virtual_*_100k`: realise every row.
    fault_no_virtualisation: crate::controls::Switch,
    /// Rows written through the model API (see [`VirtualTree::rows_written`]).
    writes: u64,
}

impl VirtualTree {
    /// A tree view (rows have disclosure triangles and levels).
    pub fn tree(label: &str) -> Self {
        Self {
            label: label.to_string(),
            flat: false,
            index: TreeIndex::new(),
            items: HashMap::new(),
            row_h: 24.0,
            scroll: 0.0,
            view_h: 0.0,
            sel: Selection {
                multi: true,
                ..Selection::default()
            },
            reorderable: true,
            assets: false,
            renamable: true,
            rename: None,
            typeahead: String::new(),
            press: None,
            last_pos: Point::default(),
            hover_row: None,
            drop: None,
            thumb_drag: None,
            live: Cell::new(0),
            painted: Cell::new(0),
            fault_no_virtualisation: crate::controls::Switch::default(),
            writes: 0,
        }
    }
    /// A flat list (a tree with one level).
    pub fn list(label: &str) -> Self {
        Self {
            flat: true,
            ..Self::tree(label)
        }
    }
    pub fn single_select(mut self) -> Self {
        self.sel.multi = false;
        self
    }
    pub fn read_only(mut self) -> Self {
        self.reorderable = false;
        self.renamable = false;
        self
    }
    pub fn row_height(mut self, h: f32) -> Self {
        self.row_h = h.max(8.0);
        self
    }
    /// Accept asset tiles dropped on a row (into it) or below the rows (a root), reported as
    /// [`AssetDropped`].
    pub fn accept_assets(mut self) -> Self {
        self.assets = true;
        self
    }
    /// Positive control for the budget tests: every row is live.
    #[cfg(any(test, feature = "controls"))]
    pub fn with_fault_no_virtualisation(mut self) -> Self {
        self.fault_no_virtualisation.on = true;
        self
    }

    // ---- model (the owner calls these through `edit`) --------------------------------

    /// Mutate the view and mark it for repaint: `VirtualTree::edit(ui, id, |t| ...)`.
    pub fn edit<R>(ui: &mut Ui, id: WidgetId, f: impl FnOnce(&mut VirtualTree) -> R) -> Option<R> {
        let r = ui.widget_mut::<VirtualTree>(id).map(f);
        ui.invalidate(id, crate::damage::Dirty::PAINT | crate::damage::Dirty::A11Y);
        r
    }

    /// Insert `key` under `parent` at child position `pos`.
    pub fn insert(&mut self, parent: Option<u64>, pos: usize, key: u64, item: RowItem) -> bool {
        let parent = if self.flat { None } else { parent };
        if !self.index.insert(parent, pos, key, false) {
            return false;
        }
        self.items.insert(key, item);
        self.writes += 1;
        true
    }
    /// Append under `parent`.
    pub fn push(&mut self, parent: Option<u64>, key: u64, item: RowItem) -> bool {
        let pos = self.index.len_under(if self.flat { None } else { parent });
        self.insert(parent, pos, key, item)
    }
    /// Append many top-level rows (building a 100k list): O(n log n), no per-row repaint.
    pub fn extend(&mut self, rows: impl IntoIterator<Item = (u64, RowItem)>) {
        let mut pos = self.top_len();
        for (k, item) in rows {
            if self.index.insert(None, pos, k, false) {
                self.items.insert(k, item);
                self.writes += 1;
                pos += 1;
            }
        }
    }
    fn top_len(&self) -> usize {
        self.index.len_under(None)
    }
    /// Remove `key` and its subtree: O(subtree + d · log₃₂ b), whatever the row count, so
    /// trimming a long list from the front costs one row. The active row keeps pointing
    /// at the same row below the removed ones.
    pub fn remove(&mut self, key: u64) -> bool {
        let at = self.row_of(key);
        let before = self.row_count();
        let Some(gone) = self.index.remove_subtree(key) else {
            return false;
        };
        for k in &gone {
            self.items.remove(k);
            self.sel.keys.remove(k);
        }
        self.writes += gone.len() as u64;
        if let Some(r) = at {
            let n = before - self.row_count();
            let shift = |x: &mut Option<usize>| {
                if let Some(v) = x
                    && *v > r
                {
                    *v = if *v >= r + n { *v - n } else { r };
                }
            };
            shift(&mut self.sel.active);
            shift(&mut self.sel.anchor);
        }
        self.clamp_rows();
        true
    }
    /// Rows written (inserted, replaced, relabelled or removed) since the view was made:
    /// the owners' guards use it to show a model change cost one row, not a rebuild.
    pub fn rows_written(&self) -> u64 {
        self.writes
    }
    /// Children under `parent` (`None`: top level).
    pub fn child_count(&self, parent: Option<u64>) -> usize {
        self.index.len_under(if self.flat { None } else { parent })
    }
    /// The child at position `pos` under `parent` (`None`: top level).
    pub fn child_at(&self, parent: Option<u64>, pos: usize) -> Option<u64> {
        self.index
            .child_at(if self.flat { None } else { parent }, pos)
    }
    /// The child position a new `key` takes under `parent` when siblings are kept in
    /// key order (O(log b · log₃₂ b)).
    pub fn key_order_position(&self, parent: Option<u64>, key: u64) -> usize {
        let parent = if self.flat { None } else { parent };
        let (mut lo, mut hi) = (0, self.index.len_under(parent));
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.index.child_at(parent, mid) {
                Some(k) if k < key => lo = mid + 1,
                _ => hi = mid,
            }
        }
        lo
    }
    /// Every row's key (at any depth, expanded or not), in no particular order.
    pub fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.index.keys()
    }
    /// A snapshot of every row's key (for work resumed across frames while rows change).
    /// Built here, not in the caller, so it runs at this crate's optimisation level.
    pub fn keys_vec(&self) -> Vec<u64> {
        self.index.keys().collect()
    }
    pub fn move_node(&mut self, key: u64, target: DropTarget) -> bool {
        let parent = if self.flat { None } else { target.parent };
        let ok = self.index.move_node(key, parent, target.index);
        self.writes += u64::from(ok);
        ok
    }
    pub fn set_label(&mut self, key: u64, label: &str) {
        if let Some(i) = self.items.get_mut(&key) {
            i.label = label.to_string();
            self.writes += 1;
        }
    }
    pub fn label_of(&self, key: u64) -> Option<&str> {
        self.items.get(&key).map(|i| i.label.as_str())
    }
    /// A row's item (label, icon, badges).
    pub fn item(&self, key: u64) -> Option<&RowItem> {
        self.items.get(&key)
    }
    /// Replace a row's item (label, badges, muted) in place.
    pub fn set_item(&mut self, key: u64, item: RowItem) {
        if let Some(i) = self.items.get_mut(&key) {
            *i = item;
            self.writes += 1;
        }
    }
    /// True if `key` is a row (at any depth, expanded or not).
    pub fn contains(&self, key: u64) -> bool {
        self.index.contains(key)
    }
    /// The parent of `key` in the view's model.
    pub fn parent_of(&self, key: u64) -> Option<u64> {
        self.index.parent(key)
    }
    /// Remove every row (a model rebuilt from scratch).
    pub fn clear(&mut self) {
        self.index = TreeIndex::new();
        self.writes += self.items.len() as u64;
        self.items.clear();
        self.sel.keys.clear();
        self.clamp_rows();
    }
    /// Expand or collapse; returns the index nodes updated (the §21.12 bound).
    pub fn set_expanded(&mut self, key: u64, on: bool) -> u64 {
        let u = self.index.set_expanded(key, on);
        self.clamp_rows();
        u
    }
    pub fn is_expanded(&self, key: u64) -> bool {
        self.index.is_expanded(key)
    }
    pub fn index(&self) -> &TreeIndex {
        &self.index
    }
    pub fn row_count(&self) -> usize {
        self.index.row_count() as usize
    }
    pub fn key_at(&self, row: usize) -> Option<u64> {
        self.index.node_at_row(row as u64).map(|(k, _)| k)
    }
    pub fn row_of(&self, key: u64) -> Option<usize> {
        self.index.row_of(key).map(|r| r as usize)
    }
    /// Selected keys, in row order.
    pub fn selected(&self) -> Vec<u64> {
        let mut v: Vec<(usize, u64)> = self
            .sel
            .keys
            .iter()
            .map(|k| (self.row_of(*k).unwrap_or(usize::MAX), *k))
            .collect();
        v.sort_unstable();
        v.into_iter().map(|(_, k)| k).collect()
    }
    /// What a keyboard command acts on: the selection, or the focused row when nothing is
    /// selected (F2, Delete and Alt+arrows work on the row the user is looking at).
    fn acted_on(&self) -> Vec<u64> {
        let s = self.selected();
        if s.is_empty() {
            self.active().into_iter().collect()
        } else {
            s
        }
    }

    /// The active (focused) row's key.
    pub fn active(&self) -> Option<u64> {
        self.sel.active.and_then(|r| self.key_at(r))
    }
    pub fn select(&mut self, keys: &[u64]) {
        self.sel.keys = keys.iter().copied().collect();
        if let Some(r) = keys.first().and_then(|k| self.row_of(*k)) {
            self.sel.active = Some(r);
            self.sel.anchor = Some(r);
        }
    }
    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }
    pub fn scroll_offset(&self) -> f32 {
        self.scroll
    }
    /// Rows with live records (painted or with an a11y node) at the last paint.
    pub fn live_rows(&self) -> usize {
        self.live.get()
    }
    /// Rows actually recorded at the last paint.
    pub fn painted_rows(&self) -> usize {
        self.painted.get()
    }
    pub fn scroll_to_row(&mut self, row: usize) {
        self.scroll = row as f32 * self.row_h;
        self.clamp_scroll();
    }

    // ---- geometry --------------------------------------------------------------------

    fn clamp_rows(&mut self) {
        let n = self.row_count();
        if let Some(a) = self.sel.active
            && a >= n
        {
            self.sel.active = n.checked_sub(1);
        }
        self.clamp_scroll();
    }
    fn content_h(&self) -> f32 {
        self.row_count() as f32 * self.row_h
    }
    fn clamp_scroll(&mut self) {
        let max = (self.content_h() - self.view_h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }
    fn visible_rows(&self) -> usize {
        ((self.view_h / self.row_h).ceil() as usize).max(1)
    }
    /// The live range: visible rows ± overscan.
    fn live_range(&self, view_h: f32) -> std::ops::Range<usize> {
        let n = self.row_count();
        if self.fault_no_virtualisation.on() {
            return 0..n;
        }
        live_window(self.scroll, view_h, self.row_h, n)
    }
    fn row_rect(&self, r: Rect, row: usize) -> Rect {
        Rect::new(
            r.x + 1.0,
            r.y + row as f32 * self.row_h - self.scroll,
            r.w - 2.0 - BAR,
            self.row_h,
        )
    }
    fn row_at(&self, r: Rect, p: Point) -> Option<usize> {
        if !r.contains(p) || p.x > r.right() - BAR {
            return None;
        }
        let row = ((p.y - r.y + self.scroll) / self.row_h).floor();
        (row >= 0.0 && (row as usize) < self.row_count()).then_some(row as usize)
    }
    fn ensure_visible(&mut self, row: usize) {
        let top = row as f32 * self.row_h;
        if top < self.scroll {
            self.scroll = top;
        } else if top + self.row_h > self.scroll + self.view_h {
            self.scroll = top + self.row_h - self.view_h;
        }
        self.clamp_scroll();
    }
    /// Badge `i` of `n` at the row's right edge.
    fn badge_rect(rr: Rect, i: usize, n: usize) -> Rect {
        let x = rr.right() - (n - i) as f32 * BADGE_W - 2.0;
        Rect::new(x, rr.y, BADGE_W, rr.h)
    }
    fn badge_at(&self, rr: Rect, key: u64, p: Point) -> Option<usize> {
        let n = self.items.get(&key).map_or(0, |i| i.badges.len());
        (0..n).find(|i| Self::badge_rect(rr, *i, n).contains(p))
    }
    fn disclosure_rect(&self, rr: Rect, depth: u32) -> Rect {
        Rect::new(rr.x + 4.0 + depth as f32 * INDENT, rr.y, INDENT, rr.h)
    }
    fn thumb(&self, r: Rect) -> Option<Rect> {
        let ch = self.content_h();
        if ch <= r.h + 0.5 {
            return None;
        }
        let h = (r.h * r.h / ch).max(20.0).min(r.h);
        let max = (ch - r.h).max(1.0);
        Some(Rect::new(
            r.right() - BAR,
            r.y + (r.h - h) * (self.scroll / max),
            BAR - 2.0,
            h,
        ))
    }

    // ---- behaviour -------------------------------------------------------------------

    fn changed(&mut self, cx: &mut EventCx, sel_changed: bool) {
        if let Some(a) = self.sel.active {
            self.ensure_visible(a);
        }
        if sel_changed {
            let id = cx.id();
            let keys = self.selected();
            cx.action(SelectionChanged { view: id, keys });
        }
        cx.request_paint();
        cx.request_a11y();
    }

    fn go(&mut self, cx: &mut EventCx, to: usize, shift: bool, ctrl: bool) {
        let n = self.row_count();
        if n == 0 {
            return;
        }
        let to = to.min(n - 1);
        let idx = &self.index;
        let changed = self.sel.move_to(to, shift, ctrl, &|r| {
            idx.node_at_row(r as u64).map(|(k, _)| k)
        });
        self.changed(cx, changed);
    }

    fn activate(&mut self, cx: &mut EventCx) {
        if let Some(key) = self.active() {
            let id = cx.id();
            cx.action(RowActivated { view: id, key });
        }
    }

    fn start_rename(&mut self, cx: &mut EventCx) {
        if !self.renamable {
            return;
        }
        if let Some(key) = self.active() {
            let mut e = LineEdit::new(self.label_of(key).unwrap_or(""));
            e.select_all();
            self.rename = Some((key, e));
            cx.set_ime_allowed(true);
            cx.request_paint();
        }
    }

    fn end_rename(&mut self, cx: &mut EventCx, commit: bool) {
        if let Some((key, e)) = self.rename.take() {
            if commit && Some(e.text.as_str()) != self.label_of(key) && !e.text.trim().is_empty() {
                let id = cx.id();
                cx.action(RowRenamed {
                    view: id,
                    key,
                    name: e.text.trim().to_string(),
                });
            }
            cx.set_ime_allowed(false);
            cx.request_paint();
        }
    }

    /// Alt+Up/Down: move the selection one slot among its siblings.
    fn keyboard_move(&mut self, cx: &mut EventCx, down: bool) {
        if !self.reorderable {
            return;
        }
        let keys = self.acted_on();
        let Some(&first) = keys.first() else { return };
        let parent = self.index.parent(first);
        if keys.iter().any(|k| self.index.parent(*k) != parent) {
            return;
        }
        let Some(pos) = self.index.index_in_parent(first) else {
            return;
        };
        let last_pos = keys
            .iter()
            .filter_map(|k| self.index.index_in_parent(*k))
            .max()
            .unwrap_or(pos);
        let siblings = match parent {
            Some(p) => self.index.child_count(p),
            None => self.top_len(),
        };
        let index = if down {
            if last_pos + 1 >= siblings {
                return;
            }
            last_pos + 2
        } else {
            if pos == 0 {
                return;
            }
            pos - 1
        };
        let id = cx.id();
        cx.action(RowsDropped {
            view: id,
            keys,
            target: DropTarget { parent, index },
        });
    }

    fn typeahead(&mut self, cx: &mut EventCx, text: &str) {
        let fresh = self.typeahead.is_empty();
        self.typeahead.push_str(&text.to_lowercase());
        cx.cancel_timer(TYPEAHEAD_TIMER);
        cx.set_timer(TYPEAHEAD_RESET, TYPEAHEAD_TIMER);
        let n = self.row_count();
        if n == 0 {
            return;
        }
        let start = self.sel.active.map_or(0, |a| if fresh { a + 1 } else { a });
        for i in 0..n {
            let r = (start + i) % n;
            if let Some(k) = self.key_at(r)
                && self
                    .items
                    .get(&k)
                    .is_some_and(|it| it.label.to_lowercase().starts_with(&self.typeahead))
            {
                self.go(cx, r, false, false);
                return;
            }
        }
    }

    /// The drop spot for the pointer at `p` while dragging `dragged`.
    fn drop_spot(&self, r: Rect, p: Point, dragged: &[u64]) -> Result<DropSpot, String> {
        let Some(row) = self.row_at(r, p) else {
            return Ok(DropSpot::End);
        };
        let Some((key, _)) = self.index.node_at_row(row as u64) else {
            return Ok(DropSpot::End);
        };
        if dragged
            .iter()
            .any(|d| *d == key || self.index.is_ancestor(*d, key))
        {
            return Err(crate::tr!("cannot move a row into itself").into());
        }
        let rr = self.row_rect(r, row);
        let f = (p.y - rr.y) / rr.h;
        Ok(if self.flat {
            if f < 0.5 {
                DropSpot::Before(key)
            } else {
                DropSpot::After(key)
            }
        } else if f < 0.25 {
            DropSpot::Before(key)
        } else if f > 0.75 {
            DropSpot::After(key)
        } else {
            DropSpot::Into(key)
        })
    }

    /// Where an asset dropped at `p` lands: into the row under it, or below every row (a
    /// root) when there is none.
    fn asset_spot(&self, r: Rect, p: Point) -> DropSpot {
        match self
            .row_at(r, p)
            .and_then(|row| self.index.node_at_row(row as u64))
        {
            Some((key, _)) if !self.flat => DropSpot::Into(key),
            Some((key, _)) => DropSpot::After(key),
            None => DropSpot::End,
        }
    }

    fn target_of(&self, spot: DropSpot) -> DropTarget {
        match spot {
            DropSpot::Before(k) | DropSpot::After(k) => {
                let parent = self.index.parent(k);
                let i = self.index.index_in_parent(k).unwrap_or(0);
                DropTarget {
                    parent,
                    index: if matches!(spot, DropSpot::After(_)) {
                        i + 1
                    } else {
                        i
                    },
                }
            }
            DropSpot::Into(k) => DropTarget {
                parent: Some(k),
                index: self.index.child_count(k),
            },
            DropSpot::End => DropTarget {
                parent: None,
                index: self.top_len(),
            },
        }
    }

    fn dragged_keys(payload: &DragPayload) -> Option<&[u64]> {
        match payload {
            DragPayload::Entities(k) => Some(k),
            _ => None,
        }
    }
}

impl Widget for VirtualTree {
    fn role(&self) -> Role {
        if self.flat { Role::ListBox } else { Role::Tree }
    }
    fn focusable(&self) -> bool {
        true
    }
    fn accepts_text(&self) -> bool {
        self.rename.is_some()
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        _cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let h = known.height.unwrap_or(self.row_h * 10.0);
        self.view_h = h;
        self.clamp_scroll();
        Size::new(known.width.unwrap_or(260.0), h)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        self.view_h = cx.rect().h;
        let n = self.row_count();
        // Rename in place owns the keyboard while it runs.
        if let Some((_, e)) = self.rename.as_mut() {
            match ev {
                UiEvent::Key(k) if k.pressed => {
                    match e.key(k, cx.clipboard()) {
                        EditOutcome::Submit => self.end_rename(cx, true),
                        EditOutcome::Cancel => self.end_rename(cx, false),
                        EditOutcome::Ignored if k.code == KeyCode::Tab => {
                            self.end_rename(cx, true);
                            return Handled::No;
                        }
                        _ => cx.request_paint(),
                    }
                    return Handled::Yes;
                }
                UiEvent::Ime(i) => {
                    e.ime(i);
                    cx.request_paint();
                    return Handled::Yes;
                }
                UiEvent::FocusLost | UiEvent::PointerDown { .. } => {
                    self.end_rename(cx, true);
                    if matches!(ev, UiEvent::FocusLost) {
                        return Handled::Yes;
                    }
                }
                _ => {}
            }
        }
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let cur = self.sel.active;
                let page = self.visible_rows().saturating_sub(1).max(1);
                let (shift, ctrl) = (k.mods.shift, k.mods.ctrl);
                match k.code {
                    KeyCode::Up if k.mods.alt => self.keyboard_move(cx, false),
                    KeyCode::Down if k.mods.alt => self.keyboard_move(cx, true),
                    KeyCode::Down => {
                        let to = cur.map_or(0, |c| (c + 1).min(n.saturating_sub(1)));
                        self.go(cx, to, shift, ctrl);
                    }
                    KeyCode::Up => {
                        let to = cur.map_or(0, |c| c.saturating_sub(1));
                        self.go(cx, to, shift, ctrl);
                    }
                    KeyCode::PageDown => self.go(cx, cur.map_or(0, |c| c + page), shift, ctrl),
                    KeyCode::PageUp => {
                        self.go(cx, cur.map_or(0, |c| c.saturating_sub(page)), shift, ctrl)
                    }
                    KeyCode::Home => self.go(cx, 0, shift, ctrl),
                    KeyCode::End => self.go(cx, n.saturating_sub(1), shift, ctrl),
                    KeyCode::Char('a') if ctrl && self.sel.multi => {
                        for r in 0..n {
                            if let Some(k) = self.key_at(r) {
                                self.sel.keys.insert(k);
                            }
                        }
                        self.changed(cx, true);
                    }
                    KeyCode::Space if ctrl => {
                        if let Some(a) = cur
                            && let Some(key) = self.key_at(a)
                        {
                            self.sel.toggle(a, key);
                            self.changed(cx, true);
                        }
                    }
                    KeyCode::Space if self.typeahead.is_empty() => {
                        if let Some(a) = cur {
                            self.go(cx, a, shift, false);
                        } else {
                            self.go(cx, 0, false, false);
                        }
                    }
                    KeyCode::Right if !self.flat => {
                        if let Some(key) = self.active() {
                            if self.index.has_children(key) && !self.index.is_expanded(key) {
                                self.set_expanded(key, true);
                                self.changed(cx, false);
                            } else if self.index.is_expanded(key)
                                && let Some(a) = cur
                            {
                                self.go(cx, a + 1, false, false);
                            }
                        }
                    }
                    KeyCode::Left if !self.flat => {
                        if let Some(key) = self.active() {
                            if self.index.is_expanded(key) && self.index.has_children(key) {
                                self.set_expanded(key, false);
                                self.changed(cx, false);
                            } else if let Some(p) = self.index.parent(key)
                                && let Some(pr) = self.row_of(p)
                            {
                                self.go(cx, pr, false, false);
                            }
                        }
                    }
                    KeyCode::Enter => self.activate(cx),
                    KeyCode::F2 => self.start_rename(cx),
                    KeyCode::Delete => {
                        let keys = self.acted_on();
                        if !keys.is_empty() {
                            let id = cx.id();
                            cx.action(RowsDeleteRequested { view: id, keys });
                        }
                    }
                    KeyCode::ContextMenu | KeyCode::F10
                        if k.code == KeyCode::ContextMenu || shift =>
                    {
                        if let Some(key) = self.active() {
                            let r = cx.rect();
                            let row = cur.unwrap_or(0);
                            let rr = self.row_rect(r, row);
                            let id = cx.id();
                            cx.action(RowContextMenu {
                                view: id,
                                key,
                                at: Point::new(rr.x + 24.0, rr.bottom()),
                            });
                        }
                    }
                    _ => match &k.text {
                        Some(t) if !ctrl && !k.mods.alt && !t.trim().is_empty() => {
                            let t = t.clone();
                            self.typeahead(cx, &t);
                        }
                        _ => return Handled::No,
                    },
                }
                Handled::Yes
            }
            UiEvent::Timer(TYPEAHEAD_TIMER) => {
                self.typeahead.clear();
                Handled::Yes
            }
            UiEvent::Wheel { dy, .. } => {
                let before = self.scroll;
                self.scroll -= dy * self.row_h * 3.0;
                self.clamp_scroll();
                if self.scroll != before {
                    cx.request_paint();
                    cx.request_a11y();
                }
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button,
                clicks,
            } => {
                let r = cx.rect();
                if *button == PointerButton::Primary
                    && let Some(t) = self.thumb(r)
                    && pos.x >= r.right() - BAR
                {
                    if t.contains(*pos) {
                        self.thumb_drag = Some((pos.y, self.scroll));
                        cx.capture_pointer();
                    } else {
                        let page = self.view_h * if pos.y < t.y { -1.0 } else { 1.0 };
                        self.scroll += page;
                        self.clamp_scroll();
                        cx.request_paint();
                    }
                    return Handled::Yes;
                }
                let Some(row) = self.row_at(r, *pos) else {
                    return Handled::Yes;
                };
                let Some((key, depth)) = self.index.node_at_row(row as u64) else {
                    return Handled::Yes;
                };
                if *button == PointerButton::Secondary {
                    if !self.sel.keys.contains(&key) {
                        self.go(cx, row, false, false);
                    }
                    let id = cx.id();
                    cx.action(RowContextMenu {
                        view: id,
                        key,
                        at: *pos,
                    });
                    return Handled::Yes;
                }
                if *button != PointerButton::Primary {
                    return Handled::Yes;
                }
                let rr = self.row_rect(r, row);
                if let Some(badge) = self.badge_at(rr, key, *pos) {
                    let id = cx.id();
                    cx.action(RowBadgeClicked {
                        view: id,
                        key,
                        badge,
                    });
                    return Handled::Yes;
                }
                if !self.flat
                    && self.index.has_children(key)
                    && self.disclosure_rect(rr, depth).contains(*pos)
                {
                    let on = !self.index.is_expanded(key);
                    self.set_expanded(key, on);
                    self.changed(cx, false);
                    return Handled::Yes;
                }
                if *clicks >= 2 {
                    self.go(cx, row, false, false);
                    self.activate(cx);
                    return Handled::Yes;
                }
                let mods = cx.modifiers();
                let mut deferred = false;
                if mods.ctrl {
                    self.sel.toggle(row, key);
                    self.changed(cx, true);
                } else if mods.shift {
                    self.go(cx, row, true, false);
                } else if self.sel.keys.contains(&key) && self.sel.keys.len() > 1 {
                    self.sel.active = Some(row);
                    deferred = true;
                    self.changed(cx, false);
                } else {
                    self.go(cx, row, false, false);
                }
                self.press = Some(Press {
                    row,
                    pos: *pos,
                    deferred,
                    dragging: false,
                });
                cx.capture_pointer();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                self.last_pos = *pos;
                let r = cx.rect();
                if let Some((y0, s0)) = self.thumb_drag {
                    if let Some(t) = self.thumb(r) {
                        let travel = (r.h - t.h).max(1.0);
                        self.scroll = s0 + (pos.y - y0) / travel * (self.content_h() - r.h);
                        self.clamp_scroll();
                        cx.request_paint();
                    }
                    return Handled::Yes;
                }
                if let Some(p) = self.press.as_mut() {
                    let moved = (pos.x - p.pos.x).abs().max((pos.y - p.pos.y).abs());
                    if !p.dragging && moved > DRAG_SLOP && self.reorderable {
                        p.dragging = true;
                        p.deferred = false;
                        let keys = self.selected();
                        cx.release_pointer();
                        cx.start_drag(DragPayload::Entities(keys));
                    }
                    return Handled::Yes;
                }
                let hr = self.row_at(r, *pos);
                if hr != self.hover_row {
                    self.hover_row = hr;
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::PointerLeave => {
                let had = self.hover_row.take().is_some() | self.drop.take().is_some();
                if had {
                    cx.request_paint();
                }
                Handled::No
            }
            UiEvent::PointerUp { .. } => {
                if self.thumb_drag.take().is_some() {
                    cx.release_pointer();
                    return Handled::Yes;
                }
                if let Some(p) = self.press.take() {
                    if p.deferred && !p.dragging {
                        self.go(cx, p.row, false, false);
                    }
                    cx.release_pointer();
                }
                if self.drop.take().is_some() {
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::DragOver(DragPayload::Asset(_)) if self.assets => {
                let r = cx.rect();
                let spot = self.asset_spot(r, self.last_pos);
                let t = self.target_of(spot);
                cx.set_drop_verdict(DropVerdict::Insert { index: t.index });
                if self.drop != Some(spot) {
                    self.drop = Some(spot);
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::Drop(DragPayload::Asset(asset)) if self.assets => {
                let r = cx.rect();
                let spot = self
                    .drop
                    .take()
                    .unwrap_or_else(|| self.asset_spot(r, self.last_pos));
                let target = self.target_of(spot);
                let id = cx.id();
                cx.action(AssetDropped {
                    view: id,
                    asset: asset.clone(),
                    target,
                });
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::DragOver(payload) => {
                let Some(keys) = Self::dragged_keys(payload) else {
                    return Handled::No;
                };
                if !self.reorderable {
                    cx.set_drop_verdict(DropVerdict::Refused(
                        crate::tr!("this view is read-only").into(),
                    ));
                    return Handled::Yes;
                }
                let r = cx.rect();
                match self.drop_spot(r, self.last_pos, keys) {
                    Ok(spot) => {
                        let t = self.target_of(spot);
                        cx.set_drop_verdict(DropVerdict::Insert { index: t.index });
                        if self.drop != Some(spot) {
                            self.drop = Some(spot);
                            cx.request_paint();
                        }
                    }
                    Err(reason) => {
                        cx.set_drop_verdict(DropVerdict::Refused(reason));
                        if self.drop.take().is_some() {
                            cx.request_paint();
                        }
                    }
                }
                Handled::Yes
            }
            UiEvent::Drop(payload) => {
                let Some(keys) = Self::dragged_keys(payload) else {
                    return Handled::No;
                };
                let r = cx.rect();
                let spot = self
                    .drop
                    .take()
                    .map_or_else(|| self.drop_spot(r, self.last_pos, keys), Ok);
                if let Ok(spot) = spot {
                    let target = self.target_of(spot);
                    let id = cx.id();
                    cx.action(RowsDropped {
                        view: id,
                        keys: keys.to_vec(),
                        target,
                    });
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::A11yChildAction(key, action) => {
                let Some(row) = self.row_of(*key) else {
                    return Handled::No;
                };
                match action {
                    Action::Click | Action::Focus => self.go(cx, row, false, false),
                    Action::Expand => {
                        self.set_expanded(*key, true);
                        self.changed(cx, false);
                    }
                    Action::Collapse => {
                        self.set_expanded(*key, false);
                        self.changed(cx, false);
                    }
                    _ => return Handled::No,
                }
                Handled::Yes
            }
            UiEvent::A11yAction(Action::ScrollDown) => {
                self.scroll += self.view_h;
                self.clamp_scroll();
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::A11yAction(Action::ScrollUp) => {
                self.scroll -= self.view_h;
                self.clamp_scroll();
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::FocusGained { .. } => {
                if self.sel.active.is_none() && n > 0 {
                    self.sel.active = Some((self.scroll / self.row_h) as usize);
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::FocusLost => {
                cx.request_paint();
                Handled::No
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let theme = cx.theme().clone();
        cx.panel(
            r,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            theme.radius.sm,
            None,
        );
        let live = self.live_range(r.h);
        self.live.set(live.len());
        let style = body(&theme);
        let focused = cx.state().focused;
        cx.primitive(crate::render::Primitive::PushClip {
            rect: r,
            radii: crate::geom::Corners::ZERO,
        });
        let mut painted = 0;
        if self.row_count() == 0 {
            cx.text(
                crate::tr!("No items"),
                &small(&theme),
                Point::new(r.x + 8.0, r.y + 6.0),
                ColorRole::FgMuted,
            );
        }
        for row in live {
            let rr = self.row_rect(r, row);
            if !self.fault_no_virtualisation.on() && (rr.bottom() < r.y || rr.y > r.bottom()) {
                continue;
            }
            let Some((key, depth)) = self.index.node_at_row(row as u64) else {
                continue;
            };
            painted += 1;
            let selected = self.sel.keys.contains(&key);
            let bg = if selected {
                cx.fill(rr, ColorRole::Selection, theme.radius.sm);
                ColorRole::Selection
            } else if self.hover_row == Some(row) {
                cx.fill(rr, ColorRole::BgHover, theme.radius.sm);
                ColorRole::BgHover
            } else {
                ColorRole::BgSunken
            };
            if focused && self.sel.active == Some(row) {
                cx.panel(rr, None, Some(ColorRole::FocusRing), theme.radius.sm, None);
            }
            let mut x = rr.x + 4.0;
            if !self.flat {
                let d = self.disclosure_rect(rr, depth);
                if self.index.has_children(key) {
                    let glyph = if self.index.is_expanded(key) {
                        "▾"
                    } else {
                        "▸"
                    };
                    let (_, gs) = cx.shape(glyph, &style);
                    cx.text_on(
                        glyph,
                        &style,
                        Point::new(d.x + (d.w - gs.w) * 0.5, d.y + (d.h - gs.h) * 0.5),
                        ColorRole::FgMuted,
                        bg,
                    );
                }
                x = d.right() + 2.0;
            }
            let Some(item) = self.items.get(&key) else {
                continue;
            };
            if let Some(icon) = item.icon {
                let (_, is) = cx.shape(icon, &style);
                cx.text_on(
                    icon,
                    &style,
                    Point::new(x, rr.y + (rr.h - is.h) * 0.5),
                    ColorRole::FgMuted,
                    bg,
                );
                x += is.w + 6.0;
            }
            match &self.rename {
                Some((k, e)) if *k == key => {
                    let er = Rect::new(x - 2.0, rr.y + 1.0, (rr.right() - x).max(40.0), rr.h - 2.0);
                    cx.panel(
                        er,
                        Some(ColorRole::BgBase),
                        Some(ColorRole::FocusRing),
                        theme.radius.sm,
                        None,
                    );
                    e.paint_line(
                        cx,
                        Rect::new(er.x + 2.0, er.y, er.w - 4.0, er.h),
                        &style,
                        true,
                        true,
                        "",
                        ColorRole::BgBase,
                    );
                }
                _ => {
                    let (run, ts) = cx.shape(&item.label, &style);
                    cx.run_on(
                        run,
                        Point::new(x, rr.y + (rr.h - ts.h) * 0.5),
                        if item.muted {
                            ColorRole::FgMuted
                        } else {
                            ColorRole::FgPrimary
                        },
                        bg,
                    );
                    if let Some(note) = &item.note {
                        let (nrun, ns) = cx.shape(note, &style);
                        cx.run_on(
                            nrun,
                            Point::new(x + ts.w + 8.0, rr.y + (rr.h - ns.h) * 0.5),
                            ColorRole::FgMuted,
                            bg,
                        );
                    }
                }
            }
            let nb = item.badges.len();
            for (i, b) in item.badges.iter().enumerate() {
                let br = Self::badge_rect(rr, i, nb);
                let glyph = if b.on { b.on_glyph } else { b.off_glyph };
                let (_, gs) = cx.shape(glyph, &style);
                cx.text_on(
                    glyph,
                    &style,
                    Point::new(br.x + (br.w - gs.w) * 0.5, br.y + (br.h - gs.h) * 0.5),
                    if b.on {
                        ColorRole::FgPrimary
                    } else {
                        ColorRole::FgMuted
                    },
                    bg,
                );
            }
        }
        // The drop indicator: a line between rows, or an outline around the new parent.
        if let Some(spot) = self.drop {
            let line = |k: u64, after: bool| {
                self.row_of(k).map(|row| {
                    let rr = self.row_rect(r, row);
                    let y = if after { rr.bottom() } else { rr.y };
                    Rect::new(rr.x, y - 1.0, rr.w, 2.0)
                })
            };
            match spot {
                DropSpot::Before(k) => {
                    if let Some(l) = line(k, false) {
                        cx.mark(l, ColorRole::Accent, ColorRole::BgSunken, 1.0);
                    }
                }
                DropSpot::After(k) => {
                    if let Some(l) = line(k, true) {
                        cx.mark(l, ColorRole::Accent, ColorRole::BgSunken, 1.0);
                    }
                }
                DropSpot::Into(k) => {
                    if let Some(row) = self.row_of(k) {
                        cx.panel(
                            self.row_rect(r, row),
                            None,
                            Some(ColorRole::Accent),
                            theme.radius.sm,
                            None,
                        );
                    }
                }
                DropSpot::End => {
                    let y = (r.y + self.content_h() - self.scroll).min(r.bottom() - 2.0);
                    cx.mark(
                        Rect::new(r.x + 2.0, y, r.w - 4.0 - BAR, 2.0),
                        ColorRole::Accent,
                        ColorRole::BgSunken,
                        1.0,
                    );
                }
            }
        }
        if let Some(t) = self.thumb(r) {
            cx.mark(t, ColorRole::Border, ColorRole::BgSunken, (BAR - 2.0) * 0.5);
        }
        cx.primitive(crate::render::Primitive::PopClip);
        self.painted.set(painted);
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        if self.sel.multi {
            node.set_multiselectable();
        }
        node.set_scroll_y(f64::from(self.scroll));
        node.set_scroll_y_min(0.0);
        node.set_scroll_y_max(f64::from((self.content_h() - self.view_h).max(0.0)));
        node.add_action(Action::ScrollDown);
        node.add_action(Action::ScrollUp);
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let role = if self.flat {
            Role::ListBoxOption
        } else {
            Role::TreeItem
        };
        let top = self.top_len();
        for row in self.live_range(cx.rect.h) {
            let Some((key, depth)) = self.index.node_at_row(row as u64) else {
                continue;
            };
            let mut node = accesskit::Node::new(role);
            if let Some(it) = self.items.get(&key) {
                let label = match &it.note {
                    Some(n) => format!("{} \u{2014} {n}", it.label),
                    None => it.label.clone(),
                };
                if it.badges.is_empty() {
                    node.set_label(label);
                } else {
                    let states: Vec<String> = it
                        .badges
                        .iter()
                        .map(|b| {
                            format!(
                                "{} {}",
                                b.label,
                                if b.on {
                                    crate::tr!("on")
                                } else {
                                    crate::tr!("off")
                                }
                            )
                        })
                        .collect();
                    node.set_label(format!("{label} ({})", states.join(", ")));
                }
            }
            if self.items.get(&key).is_some_and(|it| it.verbatim) {
                node.set_class_name(super::VERBATIM_CLASS);
            }
            node.set_selected(self.sel.keys.contains(&key));
            let siblings = match self.index.parent(key) {
                Some(p) => self.index.child_count(p),
                None => top,
            };
            node.set_position_in_set(self.index.index_in_parent(key).unwrap_or(0) + 1);
            node.set_size_of_set(siblings);
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            if !self.flat {
                node.set_level(depth as usize + 1);
                if self.index.has_children(key) {
                    let open = self.index.is_expanded(key);
                    node.set_expanded(open);
                    node.add_action(if open {
                        Action::Collapse
                    } else {
                        Action::Expand
                    });
                }
            }
            let rr = self.row_rect(cx.rect, row);
            node.set_bounds(cx.bounds(rr));
            out.push((key, node));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        self.active()
    }
}

impl VirtualTree {
    /// Positive control for `ui_virtual_tree_100k` (an O(k) expand).
    #[cfg(any(test, feature = "controls"))]
    pub fn set_fault_splice_rows(&mut self, on: bool) {
        self.index.fault_splice_rows.on = on;
    }
}

/// The live rows of a virtualised view: those meeting the viewport, plus [`OVERSCAN`] on
/// each side, never more than `⌈view_h ÷ row_h⌉ + 2·OVERSCAN` (the §21.12 bound, which a
/// partly scrolled bottom row would otherwise exceed by one).
pub fn live_window(scroll: f32, view_h: f32, row_h: f32, n: usize) -> std::ops::Range<usize> {
    let first = (scroll / row_h).floor().max(0.0) as usize;
    let last = (((scroll + view_h) / row_h).ceil().max(0.0) as usize).min(n);
    let cap = ((view_h / row_h).ceil() as usize).max(1) + 2 * OVERSCAN;
    let start = first.saturating_sub(OVERSCAN).min(n);
    let end = (last + OVERSCAN).min(n).min(start + cap);
    start..end.max(start)
}
