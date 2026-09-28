//! The virtualised tile grid (§21.16, the §21.21 asset browser's body): tiles with an
//! asynchronously rendered thumbnail, a label and a caption, shown as a grid of cards or as
//! a list of rows.
//!
//! * **Virtualised.** Like the list and tree (§21.12), a tile is a model entry, not a
//!   widget: only tiles meeting the viewport are recorded and only the live range gets
//!   accessibility nodes, so 100,000 assets cost what a screenful costs.
//! * **Async thumbnails, zero idle.** The grid asks its [`ThumbProvider`] for a tile's
//!   thumbnail the first time the tile is painted. The provider renders it on its own
//!   workers and, when it is ready, bumps a signal through the window's
//!   [`Poster`](crate::ui::Poster): the post wakes the loop once, the grid drains what is
//!   ready into the shared image atlas and repaints. Nothing polls; an idle grid schedules
//!   no frame. Resident thumbnails are capped ([`THUMB_CACHE`]); the least recently painted
//!   are evicted from the atlas and re-requested if they scroll back.
//! * **I7.** Selection and scrolling are view state. Renames, moves, deletes and OS file
//!   drops change the model: the grid raises [`RowRenamed`], [`RowsDropped`],
//!   [`RowsDeleteRequested`] and [`FilesDropped`], and the owner (a panel emitting a
//!   command) applies them and then updates the grid through [`VirtualGrid::edit`].
//! * **Keyboard.** Arrows move by tile (Up/Down by a row of tiles), Home/End, Shift
//!   extends, Ctrl+A, Enter activates (opens a folder), F2 renames in place, Delete asks to
//!   delete, Backspace raises [`GridUp`] (go to the parent folder), type-ahead jumps.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

use accesskit::{Action, Role};

use crate::damage::Dirty;
use crate::dnd::{DragPayload, DropVerdict};
use crate::geom::{Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::render::ImageId;
use crate::state::Signal;
use crate::style::ColorRole;
use crate::ui::Ui;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::collections::{
    DropTarget, RowActivated, RowContextMenu, RowRenamed, RowsDeleteRequested, RowsDropped,
    Selection, SelectionChanged,
};
use super::line_edit::{EditOutcome, LineEdit};
use super::{body, small};

/// Resident thumbnails per grid; older ones are evicted from the image atlas.
pub const THUMB_CACHE: usize = 96;
const BAR: f32 = 8.0;
const GAP: f32 = 8.0;
const LABEL_H: f32 = 34.0;
const LIST_ROW_H: f32 = 26.0;
const DRAG_SLOP: f32 = 4.0;
const OVERSCAN_ROWS: usize = 2;

/// Grid of cards, or list of rows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GridMode {
    Grid,
    List,
}

/// One tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridTile {
    pub label: String,
    /// Shown under the label (grid) or right-aligned (list): the asset kind.
    pub caption: String,
    /// Drawn while there is no thumbnail (a folder, a kind icon).
    pub glyph: &'static str,
    /// A folder: dropping tiles on it raises [`RowsDropped`] into it; it never gets a
    /// thumbnail.
    pub folder: bool,
    /// A corner badge (a lock indicator, Ch.33 §33.3).
    pub badge: Option<&'static str>,
    /// What a drag of this tile carries ([`DragPayload::Asset`]); `None`: not draggable
    /// out of the grid.
    pub drag_id: Option<String>,
}

impl GridTile {
    pub fn new(label: impl Into<String>, caption: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            caption: caption.into(),
            glyph: "\u{25a1}",
            folder: false,
            badge: None,
            drag_id: None,
        }
    }
    pub fn folder(label: impl Into<String>) -> Self {
        Self {
            glyph: "\u{1f4c1}",
            folder: true,
            ..Self::new(label, crate::tr!("Folder"))
        }
    }
    pub fn glyph(mut self, g: &'static str) -> Self {
        self.glyph = g;
        self
    }
    pub fn badge(mut self, b: Option<&'static str>) -> Self {
        self.badge = b;
        self
    }
    pub fn drag_id(mut self, id: impl Into<String>) -> Self {
        self.drag_id = Some(id.into());
        self
    }
}

/// A thumbnail that finished rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyThumb {
    pub key: u64,
    pub w: u32,
    pub h: u32,
    /// RGBA8, sRGB, straight alpha, `w * h * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Where a grid's thumbnails come from (the asset browser's adapter over the asset
/// system's thumbnail renderer). See the module docs for the protocol.
pub trait ThumbProvider {
    /// Ask for `key`'s thumbnail. Non-blocking; asking again for a pending key is a no-op.
    fn request(&self, key: u64);
    /// Thumbnails finished since the last call.
    fn take_ready(&self) -> Vec<ReadyThumb>;
    /// Bumped (through the window's poster) when a thumbnail becomes ready.
    fn ready_signal(&self) -> Signal<u64>;
}

/// OS files were dropped on the grid (import them into the folder the grid shows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilesDropped {
    pub view: WidgetId,
    pub files: Vec<PathBuf>,
    /// The folder tile they were dropped on, if any (else the folder the grid shows).
    pub folder: Option<u64>,
}

/// Backspace: go to the parent folder.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GridUp {
    pub view: WidgetId,
}

#[derive(Copy, Clone, Debug)]
struct Press {
    index: usize,
    pos: Point,
    deferred: bool,
    dragging: bool,
}

/// See the module docs.
pub struct VirtualGrid {
    label: String,
    mode: GridMode,
    tiles: Vec<(u64, GridTile)>,
    pos: HashMap<u64, usize>,
    tile: f32,
    scroll: f32,
    view: Size,
    sel: Selection,
    press: Option<Press>,
    last_pos: Point,
    hover: Option<usize>,
    drop_on: Option<usize>,
    dragging: Option<Vec<u64>>,
    rename: Option<(u64, LineEdit)>,
    typeahead: String,
    provider: Option<Rc<dyn ThumbProvider>>,
    thumbs: HashMap<u64, ImageId>,
    /// Thumbnail keys, least recently painted first.
    lru: RefCell<VecDeque<u64>>,
    requested: RefCell<HashSet<u64>>,
    live: Cell<usize>,
    painted: Cell<usize>,
    /// Positive control: request (and keep) a thumbnail for every tile, painted or not.
    fault_request_all: crate::controls::Switch,
}

impl VirtualGrid {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            mode: GridMode::Grid,
            tiles: Vec::new(),
            pos: HashMap::new(),
            tile: 96.0,
            scroll: 0.0,
            view: Size::new(0.0, 0.0),
            sel: Selection {
                multi: true,
                ..Selection::default()
            },
            press: None,
            last_pos: Point::default(),
            hover: None,
            drop_on: None,
            dragging: None,
            rename: None,
            typeahead: String::new(),
            provider: None,
            thumbs: HashMap::new(),
            lru: RefCell::new(VecDeque::new()),
            requested: RefCell::new(HashSet::new()),
            live: Cell::new(0),
            painted: Cell::new(0),
            fault_request_all: crate::controls::Switch::default(),
        }
    }
    pub fn provider(mut self, p: Rc<dyn ThumbProvider>) -> Self {
        self.provider = Some(p);
        self
    }
    pub fn tile_size(mut self, px: f32) -> Self {
        self.tile = px.clamp(32.0, 256.0);
        self
    }
    /// W2 positive control for the virtualised-thumbnail test.
    #[cfg(any(test, feature = "controls"))]
    pub fn with_fault_request_all(mut self) -> Self {
        self.fault_request_all.on = true;
        self
    }

    // ---- model -----------------------------------------------------------------------

    /// Mutate the grid and mark it for repaint.
    pub fn edit<R>(ui: &mut Ui, id: WidgetId, f: impl FnOnce(&mut VirtualGrid) -> R) -> Option<R> {
        let r = ui.widget_mut::<VirtualGrid>(id).map(f);
        ui.invalidate(id, Dirty::PAINT | Dirty::A11Y);
        r
    }

    /// Replace every tile. Selection is kept for keys that remain; thumbnails of keys that
    /// remain stay resident (a relabel costs no re-render).
    pub fn set_tiles(&mut self, tiles: Vec<(u64, GridTile)>) {
        self.pos = tiles
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (*k, i))
            .collect();
        self.tiles = tiles;
        self.sel.keys.retain(|k| self.pos.contains_key(k));
        self.requested
            .borrow_mut()
            .retain(|k| self.pos.contains_key(k));
        if let Some(a) = self.sel.active
            && a >= self.tiles.len()
        {
            self.sel.active = self.tiles.len().checked_sub(1);
        }
        self.clamp_scroll();
    }
    /// Forget `key`'s thumbnail (its content changed): it is re-requested when painted.
    /// Returns the atlas image to free, if one was resident.
    pub fn invalidate_thumb(&mut self, key: u64) -> Option<ImageId> {
        self.requested.borrow_mut().remove(&key);
        self.lru.borrow_mut().retain(|k| *k != key);
        self.thumbs.remove(&key)
    }
    pub fn set_mode(&mut self, mode: GridMode) {
        self.mode = mode;
        self.clamp_scroll();
    }
    pub fn mode(&self) -> GridMode {
        self.mode
    }
    pub fn len(&self) -> usize {
        self.tiles.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
    pub fn key_at(&self, i: usize) -> Option<u64> {
        self.tiles.get(i).map(|(k, _)| *k)
    }
    pub fn index_of(&self, key: u64) -> Option<usize> {
        self.pos.get(&key).copied()
    }
    pub fn tile(&self, key: u64) -> Option<&GridTile> {
        self.index_of(key).map(|i| &self.tiles[i].1)
    }
    /// Selected keys, in tile order.
    pub fn selected(&self) -> Vec<u64> {
        let mut v: Vec<(usize, u64)> = self
            .sel
            .keys
            .iter()
            .filter_map(|k| self.index_of(*k).map(|i| (i, *k)))
            .collect();
        v.sort_unstable();
        v.into_iter().map(|(_, k)| k).collect()
    }
    pub fn select(&mut self, keys: &[u64]) {
        self.sel.keys = keys.iter().copied().collect();
        if let Some(i) = keys.first().and_then(|k| self.index_of(*k)) {
            self.sel.active = Some(i);
            self.sel.anchor = Some(i);
        }
    }
    pub fn active(&self) -> Option<u64> {
        self.sel.active.and_then(|i| self.key_at(i))
    }
    /// The thumbnail image shown for `key`, if resident.
    pub fn thumb_of(&self, key: u64) -> Option<ImageId> {
        self.thumbs.get(&key).copied()
    }
    /// Thumbnails resident.
    pub fn resident_thumbs(&self) -> usize {
        self.thumbs.len()
    }
    /// Thumbnails asked of the provider and not yet resident or evicted.
    pub fn requested_count(&self) -> usize {
        self.requested.borrow().len()
    }
    /// Tiles with live records at the last paint.
    pub fn live_tiles(&self) -> usize {
        self.live.get()
    }
    /// Tiles recorded at the last paint.
    pub fn painted_tiles(&self) -> usize {
        self.painted.get()
    }
    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }
    pub fn scroll_to(&mut self, index: usize) {
        let (row, _) = self.cell_of(index);
        self.scroll = row as f32 * self.row_h();
        self.clamp_scroll();
    }

    // ---- geometry --------------------------------------------------------------------

    fn cols(&self) -> usize {
        match self.mode {
            GridMode::List => 1,
            GridMode::Grid => {
                (((self.view.w - BAR - GAP) / (self.tile + GAP)).floor() as usize).max(1)
            }
        }
    }
    fn row_h(&self) -> f32 {
        match self.mode {
            GridMode::List => LIST_ROW_H,
            GridMode::Grid => self.tile + LABEL_H + GAP,
        }
    }
    fn rows(&self) -> usize {
        self.tiles.len().div_ceil(self.cols())
    }
    fn content_h(&self) -> f32 {
        self.rows() as f32 * self.row_h() + GAP
    }
    fn clamp_scroll(&mut self) {
        let max = (self.content_h() - self.view.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }
    fn cell_of(&self, i: usize) -> (usize, usize) {
        let c = self.cols();
        (i / c, i % c)
    }
    fn tile_rect(&self, r: Rect, i: usize) -> Rect {
        let (row, col) = self.cell_of(i);
        match self.mode {
            GridMode::List => Rect::new(
                r.x + 1.0,
                r.y + row as f32 * LIST_ROW_H - self.scroll,
                r.w - 2.0 - BAR,
                LIST_ROW_H,
            ),
            GridMode::Grid => Rect::new(
                r.x + GAP + col as f32 * (self.tile + GAP),
                r.y + GAP + row as f32 * self.row_h() - self.scroll,
                self.tile,
                self.tile + LABEL_H,
            ),
        }
    }
    fn index_at(&self, r: Rect, p: Point) -> Option<usize> {
        if !r.contains(p) || p.x > r.right() - BAR {
            return None;
        }
        let row = ((p.y - r.y + self.scroll) / self.row_h()).floor();
        if row < 0.0 {
            return None;
        }
        let first = row as usize * self.cols();
        (first..(first + self.cols()).min(self.tiles.len()))
            .find(|i| self.tile_rect(r, *i).contains(p))
    }
    /// Tiles meeting the viewport, plus two rows each side.
    fn live_range(&self, view_h: f32) -> std::ops::Range<usize> {
        let n = self.tiles.len();
        if n == 0 {
            return 0..0;
        }
        let rh = self.row_h();
        let first_row = ((self.scroll / rh).floor() as usize).saturating_sub(OVERSCAN_ROWS);
        let last_row = (((self.scroll + view_h) / rh).ceil() as usize) + OVERSCAN_ROWS;
        let c = self.cols();
        (first_row * c).min(n)..((last_row + 1) * c).min(n)
    }
    fn ensure_visible(&mut self, i: usize) {
        let (row, _) = self.cell_of(i);
        let top = row as f32 * self.row_h();
        if top < self.scroll {
            self.scroll = top;
        } else if top + self.row_h() > self.scroll + self.view.h {
            self.scroll = top + self.row_h() - self.view.h;
        }
        self.clamp_scroll();
    }
    fn thumb_rect(&self, tr: Rect) -> Rect {
        match self.mode {
            GridMode::List => Rect::new(tr.x + 4.0, tr.y + 3.0, tr.h - 6.0, tr.h - 6.0),
            GridMode::Grid => Rect::new(tr.x, tr.y, tr.w, tr.w),
        }
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
        let n = self.tiles.len();
        if n == 0 {
            return;
        }
        let to = to.min(n - 1);
        let tiles = &self.tiles;
        let changed = self
            .sel
            .move_to(to, shift, ctrl, &|i| tiles.get(i).map(|(k, _)| *k));
        self.changed(cx, changed);
    }
    fn acted_on(&self) -> Vec<u64> {
        let s = self.selected();
        if s.is_empty() {
            self.active().into_iter().collect()
        } else {
            s
        }
    }
    fn start_rename(&mut self, cx: &mut EventCx) {
        if let Some(key) = self.active() {
            let mut e = LineEdit::new(self.tile(key).map_or("", |t| t.label.as_str()));
            e.select_all();
            self.rename = Some((key, e));
            cx.set_ime_allowed(true);
            cx.request_paint();
        }
    }
    fn end_rename(&mut self, cx: &mut EventCx, commit: bool) {
        if let Some((key, e)) = self.rename.take() {
            let name = e.text.trim().to_string();
            if commit
                && !name.is_empty()
                && Some(name.as_str()) != self.tile(key).map(|t| t.label.as_str())
            {
                let id = cx.id();
                cx.action(RowRenamed {
                    view: id,
                    key,
                    name,
                });
            }
            cx.set_ime_allowed(false);
            cx.request_paint();
        }
    }
    fn typeahead(&mut self, cx: &mut EventCx, text: &str) {
        let fresh = self.typeahead.is_empty();
        self.typeahead.push_str(&text.to_lowercase());
        cx.cancel_timer(TYPEAHEAD_TIMER);
        cx.set_timer(super::TYPEAHEAD_RESET, TYPEAHEAD_TIMER);
        let n = self.tiles.len();
        let start = self.sel.active.map_or(0, |a| if fresh { a + 1 } else { a });
        for i in 0..n {
            let j = (start + i) % n;
            if self.tiles[j]
                .1
                .label
                .to_lowercase()
                .starts_with(&self.typeahead)
            {
                self.go(cx, j, false, false);
                return;
            }
        }
    }

    /// Drain ready thumbnails into the atlas; evict beyond the cap (never a live tile).
    fn take_thumbs(&mut self, cx: &mut EventCx) {
        let Some(p) = self.provider.clone() else {
            return;
        };
        let ready = p.take_ready();
        if ready.is_empty() {
            return;
        }
        for t in ready {
            if !self.pos.contains_key(&t.key) || !self.requested.borrow().contains(&t.key) {
                continue; // removed, or invalidated since it was asked for
            }
            if let Ok(img) = cx.ui.add_image(t.w, t.h, t.rgba)
                && let Some(old) = self.thumbs.insert(t.key, img)
            {
                cx.ui.remove_image(old);
            }
            self.lru.borrow_mut().push_back(t.key);
        }
        let cap = if self.fault_request_all.on() {
            usize::MAX
        } else {
            THUMB_CACHE
        };
        let live: HashSet<u64> = self
            .live_range(self.view.h)
            .filter_map(|i| self.key_at(i))
            .collect();
        let mut guard = self.lru.borrow().len();
        while self.thumbs.len() > cap && guard > 0 {
            guard -= 1;
            let Some(k) = self.lru.borrow_mut().pop_front() else {
                break;
            };
            if live.contains(&k) {
                self.lru.borrow_mut().push_back(k);
                continue;
            }
            if let Some(img) = self.thumbs.remove(&k) {
                cx.ui.remove_image(img);
            }
            self.requested.borrow_mut().remove(&k);
        }
        cx.request_paint();
    }
}

const TYPEAHEAD_TIMER: u64 = 0x6772_6964;

impl Widget for VirtualGrid {
    fn role(&self) -> Role {
        Role::ListBox
    }
    fn focusable(&self) -> bool {
        true
    }
    fn accepts_text(&self) -> bool {
        self.rename.is_some()
    }
    fn bind(&self, b: &mut Binder) {
        if let Some(p) = &self.provider {
            b.watch(Some(p.ready_signal().any()), Dirty::NONE);
        }
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
        let s = Size::new(known.width.unwrap_or(480.0), known.height.unwrap_or(320.0));
        self.view = s;
        self.clamp_scroll();
        s
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let r = cx.rect();
        self.view = Size::new(r.w, r.h);
        if let Some((_, e)) = self.rename.as_mut() {
            match ev {
                UiEvent::Key(k) if k.pressed => {
                    match e.key(k, cx.clipboard()) {
                        EditOutcome::Submit => self.end_rename(cx, true),
                        EditOutcome::Cancel => self.end_rename(cx, false),
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
        let n = self.tiles.len();
        match ev {
            UiEvent::BindingChanged => {
                self.take_thumbs(cx);
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => {
                let cur = self.sel.active;
                let c = self.cols();
                let (shift, ctrl) = (k.mods.shift, k.mods.ctrl);
                match k.code {
                    KeyCode::Right => self.go(cx, cur.map_or(0, |a| a + 1), shift, ctrl),
                    KeyCode::Left => {
                        self.go(cx, cur.map_or(0, |a| a.saturating_sub(1)), shift, ctrl)
                    }
                    KeyCode::Down => {
                        let to = cur.map_or(0, |a| if a + c < n { a + c } else { a });
                        self.go(cx, to, shift, ctrl);
                    }
                    KeyCode::Up => self.go(cx, cur.map_or(0, |a| a.saturating_sub(c)), shift, ctrl),
                    KeyCode::Home => self.go(cx, 0, shift, ctrl),
                    KeyCode::End => self.go(cx, n.saturating_sub(1), shift, ctrl),
                    KeyCode::Char('a') if ctrl => {
                        self.sel.keys = self.tiles.iter().map(|(k, _)| *k).collect();
                        self.changed(cx, true);
                    }
                    KeyCode::Enter => {
                        if let Some(key) = self.active() {
                            let id = cx.id();
                            cx.action(RowActivated { view: id, key });
                        }
                    }
                    KeyCode::F2 => self.start_rename(cx),
                    KeyCode::Delete => {
                        let keys = self.acted_on();
                        if !keys.is_empty() {
                            let id = cx.id();
                            cx.action(RowsDeleteRequested { view: id, keys });
                        }
                    }
                    KeyCode::Backspace => {
                        let id = cx.id();
                        cx.action(GridUp { view: id });
                    }
                    KeyCode::ContextMenu => {
                        if let Some((key, i)) = self.active().zip(cur) {
                            let tr = self.tile_rect(r, i);
                            let id = cx.id();
                            cx.action(RowContextMenu {
                                view: id,
                                key,
                                at: Point::new(tr.x + 8.0, tr.bottom()),
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
                self.scroll -= dy * self.row_h();
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
                let Some(i) = self.index_at(r, *pos) else {
                    if *button == PointerButton::Primary && !self.sel.keys.is_empty() {
                        self.sel.keys.clear();
                        self.changed(cx, true);
                    }
                    return Handled::Yes;
                };
                let key = self.tiles[i].0;
                if *button == PointerButton::Secondary {
                    if !self.sel.keys.contains(&key) {
                        self.go(cx, i, false, false);
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
                if *clicks >= 2 {
                    self.go(cx, i, false, false);
                    let id = cx.id();
                    cx.action(RowActivated { view: id, key });
                    return Handled::Yes;
                }
                let mods = cx.modifiers();
                let mut deferred = false;
                if mods.ctrl {
                    self.sel.toggle(i, key);
                    self.changed(cx, true);
                } else if mods.shift {
                    self.go(cx, i, true, false);
                } else if self.sel.keys.contains(&key) && self.sel.keys.len() > 1 {
                    self.sel.active = Some(i);
                    deferred = true;
                    self.changed(cx, false);
                } else {
                    self.go(cx, i, false, false);
                }
                self.press = Some(Press {
                    index: i,
                    pos: *pos,
                    deferred,
                    dragging: false,
                });
                cx.capture_pointer();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                self.last_pos = *pos;
                if let Some(p) = self.press.as_mut() {
                    let moved = (pos.x - p.pos.x).abs().max((pos.y - p.pos.y).abs());
                    if !p.dragging && moved > DRAG_SLOP {
                        p.dragging = true;
                        p.deferred = false;
                        let keys = self.selected();
                        let payload = keys
                            .first()
                            .and_then(|k| self.tile(*k))
                            .and_then(|t| t.drag_id.clone())
                            .unwrap_or_default();
                        self.dragging = Some(keys);
                        cx.release_pointer();
                        cx.start_drag(DragPayload::Asset(payload));
                    }
                    return Handled::Yes;
                }
                let h = self.index_at(r, *pos);
                if h != self.hover {
                    self.hover = h;
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::PointerLeave => {
                if self.hover.take().is_some() {
                    cx.request_paint();
                }
                Handled::No
            }
            UiEvent::PointerUp { .. } => {
                if let Some(p) = self.press.take() {
                    if p.deferred && !p.dragging {
                        self.go(cx, p.index, false, false);
                    }
                    cx.release_pointer();
                }
                Handled::Yes
            }
            UiEvent::DragOver(payload) => {
                let over = self.index_at(r, self.last_pos);
                let folder = over.filter(|i| self.tiles[*i].1.folder);
                match payload {
                    DragPayload::Files(_) => cx.set_drop_verdict(DropVerdict::Accepted),
                    DragPayload::Asset(_) if self.dragging.is_some() => match folder {
                        Some(i)
                            if self
                                .dragging
                                .as_ref()
                                .is_some_and(|d| !d.contains(&self.tiles[i].0)) =>
                        {
                            cx.set_drop_verdict(DropVerdict::Accepted);
                        }
                        _ => cx.set_drop_verdict(DropVerdict::Refused(
                            crate::tr!("drop on a folder to move it there").into(),
                        )),
                    },
                    _ => return Handled::No,
                }
                if self.drop_on != folder {
                    self.drop_on = folder;
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::DragLeave | UiEvent::DragCancelled => {
                self.drop_on = None;
                if matches!(ev, UiEvent::DragCancelled) {
                    self.dragging = None;
                }
                cx.request_paint();
                Handled::No
            }
            UiEvent::Drop(payload) => {
                let folder = self.drop_on.take().map(|i| self.tiles[i].0);
                let id = cx.id();
                match payload {
                    DragPayload::Files(files) => cx.action(FilesDropped {
                        view: id,
                        files: files.clone(),
                        folder,
                    }),
                    DragPayload::Asset(_) => {
                        if let (Some(keys), Some(f)) = (self.dragging.take(), folder) {
                            cx.action(RowsDropped {
                                view: id,
                                keys,
                                target: DropTarget {
                                    parent: Some(f),
                                    index: 0,
                                },
                            });
                        }
                    }
                    _ => return Handled::No,
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::A11yChildAction(key, action) => {
                let Some(i) = self.index_of(*key) else {
                    return Handled::No;
                };
                match action {
                    Action::Click | Action::Focus => self.go(cx, i, false, false),
                    _ => return Handled::No,
                }
                Handled::Yes
            }
            UiEvent::FocusGained { .. } => {
                if self.sel.active.is_none() && n > 0 {
                    self.sel.active = Some(0);
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
        let range = if self.fault_request_all.on() {
            0..self.tiles.len()
        } else {
            self.live_range(r.h)
        };
        self.live.set(range.len());
        let style = body(&theme);
        let cap = small(&theme);
        let focused = cx.state().focused;
        cx.primitive(crate::render::Primitive::PushClip {
            rect: r,
            radii: crate::geom::Corners::ZERO,
        });
        if self.tiles.is_empty() {
            cx.text(
                crate::tr!("No items"),
                &cap,
                Point::new(r.x + 8.0, r.y + 6.0),
                ColorRole::FgMuted,
            );
        }
        let mut painted = 0;
        for i in range {
            let (key, t) = &self.tiles[i];
            let tr = self.tile_rect(r, i);
            let visible = tr.bottom() >= r.y && tr.y <= r.bottom();
            if !t.folder
                && let Some(p) = &self.provider
                && !self.thumbs.contains_key(key)
                && (visible || self.fault_request_all.on())
                && self.requested.borrow_mut().insert(*key)
            {
                p.request(*key);
            }
            if !visible {
                continue;
            }
            painted += 1;
            let selected = self.sel.keys.contains(key);
            let bg = if selected {
                cx.fill(tr, ColorRole::Selection, theme.radius.sm);
                ColorRole::Selection
            } else if self.hover == Some(i) || self.drop_on == Some(i) {
                cx.fill(tr, ColorRole::BgHover, theme.radius.sm);
                ColorRole::BgHover
            } else {
                ColorRole::BgSunken
            };
            if self.drop_on == Some(i) {
                cx.panel(tr, None, Some(ColorRole::Accent), theme.radius.sm, None);
            }
            if focused && self.sel.active == Some(i) {
                cx.panel(tr, None, Some(ColorRole::FocusRing), theme.radius.sm, None);
            }
            let th = self.thumb_rect(tr);
            match self.thumbs.get(key) {
                Some(img) => {
                    cx.image_untinted(*img, th);
                    let mut lru = self.lru.borrow_mut();
                    if lru.back() != Some(key) {
                        lru.retain(|k| k != key);
                        lru.push_back(*key);
                    }
                }
                None => {
                    let (_, gs) = cx.shape(t.glyph, &style);
                    cx.text_on(
                        t.glyph,
                        &style,
                        Point::new(th.x + (th.w - gs.w) * 0.5, th.y + (th.h - gs.h) * 0.5),
                        ColorRole::FgMuted,
                        bg,
                    );
                }
            }
            if let Some(b) = t.badge {
                let (_, bs) = cx.shape(b, &cap);
                cx.text_on(
                    b,
                    &cap,
                    Point::new(th.right() - bs.w - 2.0, th.y + 2.0),
                    ColorRole::Warning,
                    bg,
                );
            }
            let (lx, ly, lw) = match self.mode {
                GridMode::List => (th.right() + 6.0, tr.y + 5.0, tr.w - th.w - 120.0),
                GridMode::Grid => (tr.x + 2.0, th.bottom() + 2.0, tr.w - 4.0),
            };
            match &self.rename {
                Some((k, e)) if k == key => {
                    let er = Rect::new(lx - 2.0, ly - 1.0, lw.max(40.0), 20.0);
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
                    let (run, _) = cx.shape(&t.label, &cap);
                    cx.run_on(run, Point::new(lx, ly), ColorRole::FgPrimary, bg);
                    let (crun, cs) = cx.shape(&t.caption, &cap);
                    let cp = match self.mode {
                        GridMode::List => Point::new(tr.right() - cs.w - 8.0, ly),
                        GridMode::Grid => Point::new(lx, ly + 15.0),
                    };
                    cx.run_on(crun, cp, ColorRole::FgMuted, bg);
                }
            }
        }
        let ch = self.content_h();
        if ch > r.h + 0.5 {
            let h = (r.h * r.h / ch).max(20.0).min(r.h);
            let max = (ch - r.h).max(1.0);
            let t = Rect::new(
                r.right() - BAR,
                r.y + (r.h - h) * (self.scroll / max),
                BAR - 2.0,
                h,
            );
            cx.mark(t, ColorRole::Border, ColorRole::BgSunken, (BAR - 2.0) * 0.5);
        }
        cx.primitive(crate::render::Primitive::PopClip);
        self.painted.set(painted);
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_multiselectable();
        node.set_scroll_y(f64::from(self.scroll));
        node.set_scroll_y_min(0.0);
        node.set_scroll_y_max(f64::from((self.content_h() - self.view.h).max(0.0)));
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let n = self.tiles.len();
        for i in self.live_range(cx.rect.h) {
            let (key, t) = &self.tiles[i];
            let mut node = accesskit::Node::new(Role::ListBoxOption);
            let label = match t.badge {
                Some(_) => crate::trf!(
                    "{label}, {caption} (locked)",
                    label = t.label,
                    caption = t.caption
                ),
                None => format!("{}, {}", t.label, t.caption),
            };
            node.set_label(label);
            node.set_selected(self.sel.keys.contains(key));
            node.set_position_in_set(i + 1);
            node.set_size_of_set(n);
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.set_bounds(cx.bounds(self.tile_rect(cx.rect, i)));
            out.push((*key, node));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        self.active()
    }
}
