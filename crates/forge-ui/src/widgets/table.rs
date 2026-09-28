//! The virtualised table / grid (§21.12, §21.16): sortable, resizable columns over a
//! [`TableModel`], virtualised like the list (only visible rows are recorded; the live
//! range is visible ± 8), and cells are drawn one clip per *column*, not per cell, so a
//! screen of rows stays a handful of batches (`ui_virtual_table_100k`).
//!
//! * **Sorting happens in the model** (§21.12): a header click or Ctrl+Enter calls
//!   [`TableModel::sort`]; the table only re-binds live rows. The selection is by key, so
//!   it survives a sort.
//! * **Keyboard** (WAI-ARIA grid): Up/Down/PageUp/PageDown/Home/End move the active row
//!   (Shift extends, Ctrl moves without selecting, Ctrl+Space toggles, Ctrl+A all);
//!   Left/Right move the active column; Ctrl+Enter sorts by it (again: reverses);
//!   Alt+Left/Right narrow/widen it (Shift: by 1 px); Enter activates the row.
//! * **Pointer**: click a header to sort, drag a header's right edge to resize, click /
//!   Ctrl-click / Shift-click rows, double-click to activate, wheel to scroll.

use std::cell::Cell;

use accesskit::{Action, Role, SortDirection};

use crate::geom::{Corners, Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::render::Primitive;
use crate::style::ColorRole;
use crate::widget::{A11yCx, EventCx, MeasureCx, PaintCx, Widget};

use super::collections::{RowActivated, Selection, SelectionChanged};
use super::{body, small};

const HEADER_H: f32 = 26.0;
const BAR: f32 = 8.0;
const GRIP: f32 = 5.0;

/// Sort direction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SortDir {
    Ascending,
    Descending,
}

/// The rows a table shows. Sorting is the model's job (it may keep a permutation, ask a
/// database, or sort in place); the table asks and re-binds the rows it shows.
pub trait TableModel {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// A stable identity for row `row` (the selection is kept by key).
    fn key(&self, row: usize) -> u64;
    fn cell(&self, row: usize, col: usize) -> String;
    fn sort(&mut self, col: usize, dir: SortDir);
}

/// A column.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub title: String,
    pub width: f32,
    pub min_width: f32,
    pub sortable: bool,
    /// Numbers read better right-aligned.
    pub align_right: bool,
}

impl Column {
    pub fn new(title: &str, width: f32) -> Self {
        Self {
            title: title.to_string(),
            width,
            min_width: 32.0,
            sortable: true,
            align_right: false,
        }
    }
    pub fn numeric(mut self) -> Self {
        self.align_right = true;
        self
    }
    pub fn unsortable(mut self) -> Self {
        self.sortable = false;
        self
    }
}

/// The sort changed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TableSorted {
    pub table: WidgetId,
    pub col: usize,
    pub dir: SortDir,
}

/// A column was resized (saved with the layout, §21.4).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ColumnResized {
    pub table: WidgetId,
    pub col: usize,
    pub width: f32,
}

/// A virtualised, sortable, resizable table.
pub struct VirtualTable {
    label: String,
    model: Box<dyn TableModel>,
    cols: Vec<Column>,
    sort: Option<(usize, SortDir)>,
    row_h: f32,
    scroll: f32,
    view_h: f32,
    sel: Selection,
    active_col: usize,
    resize: Option<(usize, f32, f32)>,
    live: Cell<usize>,
    /// Cells recorded at the last paint (the "columns realised off-screen" control).
    cells: Cell<usize>,
    fault_realise_all: crate::controls::Switch,
}

impl VirtualTable {
    pub fn new(label: &str, cols: Vec<Column>, model: Box<dyn TableModel>) -> Self {
        Self {
            label: label.to_string(),
            model,
            cols,
            sort: None,
            row_h: 24.0,
            scroll: 0.0,
            view_h: 0.0,
            sel: Selection {
                multi: true,
                ..Selection::default()
            },
            active_col: 0,
            resize: None,
            live: Cell::new(0),
            cells: Cell::new(0),
            fault_realise_all: crate::controls::Switch::default(),
        }
    }
    /// Positive control for `ui_virtual_table_100k`: cells are realised for every row.
    #[cfg(any(test, feature = "controls"))]
    pub fn with_fault_realise_all_rows(mut self) -> Self {
        self.fault_realise_all.on = true;
        self
    }
    pub fn model(&self) -> &dyn TableModel {
        self.model.as_ref()
    }
    pub fn columns(&self) -> &[Column] {
        &self.cols
    }
    pub fn sort_state(&self) -> Option<(usize, SortDir)> {
        self.sort
    }
    pub fn active(&self) -> Option<(usize, usize)> {
        self.sel.active.map(|r| (r, self.active_col))
    }
    pub fn selected(&self) -> Vec<u64> {
        self.sel.keys.iter().copied().collect()
    }
    pub fn live_rows(&self) -> usize {
        self.live.get()
    }
    pub fn cells_recorded(&self) -> usize {
        self.cells.get()
    }
    pub fn scroll_to_row(&mut self, row: usize) {
        self.scroll = row as f32 * self.row_h;
        self.clamp_scroll();
    }

    fn body_rect(&self, r: Rect) -> Rect {
        Rect::new(r.x, r.y + HEADER_H, r.w - BAR, (r.h - HEADER_H).max(0.0))
    }
    fn content_h(&self) -> f32 {
        self.model.len() as f32 * self.row_h
    }
    fn clamp_scroll(&mut self) {
        let max = (self.content_h() - (self.view_h - HEADER_H)).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }
    fn live_range(&self, h: f32) -> std::ops::Range<usize> {
        let n = self.model.len();
        if self.fault_realise_all.on() {
            return 0..n;
        }
        super::collections::live_window(self.scroll, (h - HEADER_H).max(0.0), self.row_h, n)
    }
    fn col_x(&self, r: Rect, c: usize) -> f32 {
        r.x + self.cols.iter().take(c).map(|c| c.width).sum::<f32>()
    }
    fn col_at(&self, r: Rect, x: f32) -> Option<usize> {
        let mut x0 = r.x;
        for (i, c) in self.cols.iter().enumerate() {
            if x >= x0 && x < x0 + c.width {
                return Some(i);
            }
            x0 += c.width;
        }
        None
    }
    fn grip_at(&self, r: Rect, x: f32) -> Option<usize> {
        let mut x0 = r.x;
        for (i, c) in self.cols.iter().enumerate() {
            x0 += c.width;
            if (x - x0).abs() <= GRIP {
                return Some(i);
            }
        }
        None
    }
    fn row_at(&self, r: Rect, p: Point) -> Option<usize> {
        let b = self.body_rect(r);
        if !b.contains(p) {
            return None;
        }
        let row = ((p.y - b.y + self.scroll) / self.row_h).floor() as usize;
        (row < self.model.len()).then_some(row)
    }
    fn ensure_visible(&mut self, row: usize) {
        let top = row as f32 * self.row_h;
        let h = (self.view_h - HEADER_H).max(self.row_h);
        if top < self.scroll {
            self.scroll = top;
        } else if top + self.row_h > self.scroll + h {
            self.scroll = top + self.row_h - h;
        }
        self.clamp_scroll();
    }

    fn go(&mut self, cx: &mut EventCx, to: usize, shift: bool, ctrl: bool) {
        let n = self.model.len();
        if n == 0 {
            return;
        }
        let to = to.min(n - 1);
        let m = &self.model;
        let changed = self
            .sel
            .move_to(to, shift, ctrl, &|r| (r < m.len()).then(|| m.key(r)));
        self.ensure_visible(to);
        if changed {
            let id = cx.id();
            cx.action(SelectionChanged {
                view: id,
                keys: self.selected(),
            });
        }
        cx.request_paint();
        cx.request_a11y();
    }

    /// Sort by `col` (toggling direction when it is already the sort column).
    pub fn sort_by(&mut self, col: usize) -> Option<(usize, SortDir)> {
        if !self.cols.get(col).is_some_and(|c| c.sortable) {
            return None;
        }
        let dir = match self.sort {
            Some((c, SortDir::Ascending)) if c == col => SortDir::Descending,
            _ => SortDir::Ascending,
        };
        let active_key = self.sel.active.map(|r| self.model.key(r));
        self.model.sort(col, dir);
        self.sort = Some((col, dir));
        // Keep the active row on the same record.
        if let Some(k) = active_key {
            self.sel.active = (0..self.model.len()).find(|r| self.model.key(*r) == k);
            self.sel.anchor = self.sel.active;
        }
        Some((col, dir))
    }

    fn do_sort(&mut self, cx: &mut EventCx, col: usize) {
        if let Some((col, dir)) = self.sort_by(col) {
            if let Some(a) = self.sel.active {
                self.ensure_visible(a);
            }
            let id = cx.id();
            cx.action(TableSorted {
                table: id,
                col,
                dir,
            });
            cx.request_paint();
            cx.request_a11y();
        }
    }

    /// Set a column's width (clamped to its minimum).
    pub fn set_width(&mut self, col: usize, w: f32) -> Option<f32> {
        let c = self.cols.get_mut(col)?;
        c.width = w.max(c.min_width).round();
        Some(c.width)
    }

    fn do_resize(&mut self, cx: &mut EventCx, col: usize, w: f32) {
        if let Some(width) = self.set_width(col, w) {
            let id = cx.id();
            cx.action(ColumnResized {
                table: id,
                col,
                width,
            });
            cx.request_paint();
        }
    }
}

impl Widget for VirtualTable {
    fn role(&self) -> Role {
        Role::Grid
    }
    fn focusable(&self) -> bool {
        true
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
        let w: f32 = self.cols.iter().map(|c| c.width).sum::<f32>() + BAR;
        let h = known.height.unwrap_or(HEADER_H + self.row_h * 10.0);
        self.view_h = h;
        self.clamp_scroll();
        Size::new(known.width.unwrap_or(w), h)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        self.view_h = cx.rect().h;
        let n = self.model.len();
        let nc = self.cols.len();
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let cur = self.sel.active;
                let page = (((self.view_h - HEADER_H) / self.row_h) as usize)
                    .saturating_sub(1)
                    .max(1);
                let (shift, ctrl, alt) = (k.mods.shift, k.mods.ctrl, k.mods.alt);
                match k.code {
                    KeyCode::Left if alt => {
                        let d = if shift { 1.0 } else { 8.0 };
                        let w = self.cols.get(self.active_col).map_or(0.0, |c| c.width);
                        self.do_resize(cx, self.active_col, w - d);
                    }
                    KeyCode::Right if alt => {
                        let d = if shift { 1.0 } else { 8.0 };
                        let w = self.cols.get(self.active_col).map_or(0.0, |c| c.width);
                        self.do_resize(cx, self.active_col, w + d);
                    }
                    KeyCode::Left => {
                        self.active_col = self.active_col.saturating_sub(1);
                        cx.request_paint();
                        cx.request_a11y();
                    }
                    KeyCode::Right => {
                        self.active_col = (self.active_col + 1).min(nc.saturating_sub(1));
                        cx.request_paint();
                        cx.request_a11y();
                    }
                    KeyCode::Down => self.go(cx, cur.map_or(0, |c| c + 1), shift, ctrl),
                    KeyCode::Up => self.go(cx, cur.map_or(0, |c| c.saturating_sub(1)), shift, ctrl),
                    KeyCode::PageDown => self.go(cx, cur.map_or(0, |c| c + page), shift, ctrl),
                    KeyCode::PageUp => {
                        self.go(cx, cur.map_or(0, |c| c.saturating_sub(page)), shift, ctrl)
                    }
                    KeyCode::Home => self.go(cx, 0, shift, ctrl),
                    KeyCode::End => self.go(cx, n.saturating_sub(1), shift, ctrl),
                    KeyCode::Enter if ctrl => self.do_sort(cx, self.active_col),
                    KeyCode::Enter => {
                        if let Some(a) = cur.filter(|a| *a < n) {
                            let id = cx.id();
                            cx.action(RowActivated {
                                view: id,
                                key: self.model.key(a),
                            });
                        }
                    }
                    KeyCode::Space => {
                        if let Some(a) = cur.filter(|a| *a < n) {
                            if ctrl {
                                self.sel.toggle(a, self.model.key(a));
                                let id = cx.id();
                                cx.action(SelectionChanged {
                                    view: id,
                                    keys: self.selected(),
                                });
                                cx.request_paint();
                                cx.request_a11y();
                            } else {
                                self.go(cx, a, shift, false);
                            }
                        }
                    }
                    KeyCode::Char('a') if ctrl => {
                        self.sel.keys = (0..n).map(|r| self.model.key(r)).collect();
                        let id = cx.id();
                        cx.action(SelectionChanged {
                            view: id,
                            keys: self.selected(),
                        });
                        cx.request_paint();
                        cx.request_a11y();
                    }
                    _ => return Handled::No,
                }
                Handled::Yes
            }
            UiEvent::Wheel { dy, .. } => {
                let before = self.scroll;
                self.scroll -= dy * self.row_h * 3.0;
                self.clamp_scroll();
                if before != self.scroll {
                    cx.request_paint();
                    cx.request_a11y();
                }
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
            } => {
                let r = cx.rect();
                if pos.y < r.y + HEADER_H {
                    if let Some(c) = self.grip_at(r, pos.x) {
                        self.resize = Some((c, pos.x, self.cols[c].width));
                        cx.capture_pointer();
                    } else if let Some(c) = self.col_at(r, pos.x) {
                        self.active_col = c;
                        self.do_sort(cx, c);
                    }
                    return Handled::Yes;
                }
                if let Some(row) = self.row_at(r, *pos) {
                    if let Some(c) = self.col_at(r, pos.x) {
                        self.active_col = c;
                    }
                    let m = cx.modifiers();
                    if m.ctrl {
                        self.sel.toggle(row, self.model.key(row));
                        let id = cx.id();
                        cx.action(SelectionChanged {
                            view: id,
                            keys: self.selected(),
                        });
                        cx.request_paint();
                    } else {
                        self.go(cx, row, m.shift, false);
                    }
                    if *clicks >= 2 {
                        let id = cx.id();
                        cx.action(RowActivated {
                            view: id,
                            key: self.model.key(row),
                        });
                    }
                }
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                if let Some((c, x0, w0)) = self.resize {
                    let r = cx.rect();
                    let _ = r;
                    self.set_width(c, w0 + pos.x - x0);
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::PointerUp { .. } => {
                if let Some((c, _, _)) = self.resize.take() {
                    cx.release_pointer();
                    let w = self.cols[c].width;
                    let id = cx.id();
                    cx.action(ColumnResized {
                        table: id,
                        col: c,
                        width: w,
                    });
                }
                Handled::Yes
            }
            UiEvent::A11yChildAction(key, Action::Click) => {
                if let Some(c) = key
                    .checked_sub(HEADER_KEY_BASE)
                    .filter(|c| (*c as usize) < nc)
                {
                    self.do_sort(cx, c as usize);
                } else if let Some(row) = self
                    .live_range(self.view_h)
                    .find(|r| self.model.key(*r) == *key)
                {
                    self.go(cx, row, false, false);
                }
                Handled::Yes
            }
            UiEvent::FocusGained { .. } | UiEvent::FocusLost => {
                if self.sel.active.is_none() && n > 0 {
                    self.sel.active = Some((self.scroll / self.row_h) as usize);
                }
                cx.request_paint();
                Handled::No
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let theme = cx.theme().clone();
        let style = body(&theme);
        let hstyle = small(&theme).strong();
        cx.panel(
            r,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            theme.radius.sm,
            None,
        );
        let head = Rect::new(r.x, r.y, r.w, HEADER_H);
        cx.fill(head, ColorRole::BgRaised, theme.radius.sm);
        cx.primitive(Primitive::PushClip {
            rect: head,
            radii: Corners::ZERO,
        });
        for (c, col) in self.cols.iter().enumerate() {
            let x = self.col_x(r, c);
            let mut title = col.title.clone();
            if let Some((sc, dir)) = self.sort
                && sc == c
            {
                title.push_str(if dir == SortDir::Ascending {
                    " ▲"
                } else {
                    " ▼"
                });
            }
            let (_, ts) = cx.shape(&title, &hstyle);
            let tx = if col.align_right {
                x + col.width - ts.w - 8.0
            } else {
                x + 8.0
            };
            cx.text_on(
                &title,
                &hstyle,
                Point::new(tx, r.y + (HEADER_H - ts.h) * 0.5),
                ColorRole::FgPrimary,
                ColorRole::BgRaised,
            );
            cx.mark(
                Rect::new(x + col.width - 1.0, r.y + 4.0, 1.0, HEADER_H - 8.0),
                ColorRole::Border,
                ColorRole::BgRaised,
                0.0,
            );
        }
        cx.primitive(Primitive::PopClip);
        let b = self.body_rect(r);
        let live = self.live_range(r.h);
        self.live.set(live.len());
        let focused = cx.state().focused;
        let visible: Vec<usize> = live
            .clone()
            .filter(|row| {
                let y = b.y + *row as f32 * self.row_h - self.scroll;
                self.fault_realise_all.on() || (y + self.row_h >= b.y && y <= b.bottom())
            })
            .collect();
        // Row backgrounds (selection, focus) under the cells.
        cx.primitive(Primitive::PushClip {
            rect: b,
            radii: Corners::ZERO,
        });
        for &row in &visible {
            let y = b.y + row as f32 * self.row_h - self.scroll;
            let rr = Rect::new(b.x + 1.0, y, b.w - 2.0, self.row_h);
            if self.sel.keys.contains(&self.model.key(row)) {
                cx.fill(rr, ColorRole::Selection, 0.0);
            }
            if focused && self.sel.active == Some(row) {
                cx.panel(rr, None, Some(ColorRole::FocusRing), 0.0, None);
                if let Some(col) = self.cols.get(self.active_col) {
                    let x = self.col_x(r, self.active_col);
                    cx.mark(
                        Rect::new(x + 2.0, rr.bottom() - 2.0, col.width - 4.0, 2.0),
                        ColorRole::Accent,
                        ColorRole::BgSunken,
                        1.0,
                    );
                }
            }
        }
        cx.primitive(Primitive::PopClip);
        // Cells: one clip per column.
        let mut cells = 0;
        for (c, col) in self.cols.iter().enumerate() {
            let x = self.col_x(r, c);
            let clip = b.intersect(&Rect::new(x, b.y, col.width, b.h));
            if clip.is_empty() {
                continue;
            }
            cx.primitive(Primitive::PushClip {
                rect: clip,
                radii: Corners::ZERO,
            });
            for &row in &visible {
                let y = b.y + row as f32 * self.row_h - self.scroll;
                let text = self.model.cell(row, c);
                let bg = if self.sel.keys.contains(&self.model.key(row)) {
                    ColorRole::Selection
                } else {
                    ColorRole::BgSunken
                };
                let (run, ts) = cx.shape(&text, &style);
                let tx = if col.align_right {
                    x + col.width - ts.w - 8.0
                } else {
                    x + 8.0
                };
                cx.run_on(
                    run,
                    Point::new(tx, y + (self.row_h - ts.h) * 0.5),
                    ColorRole::FgPrimary,
                    bg,
                );
                cells += 1;
            }
            cx.primitive(Primitive::PopClip);
        }
        self.cells.set(cells);
        // Scrollbar.
        let ch = self.content_h();
        if ch > b.h + 0.5 {
            let h = (b.h * b.h / ch).max(20.0).min(b.h);
            let max = (ch - b.h).max(1.0);
            let t = Rect::new(
                r.right() - BAR,
                b.y + (b.h - h) * (self.scroll / max),
                BAR - 2.0,
                h,
            );
            cx.mark(t, ColorRole::Border, ColorRole::BgSunken, (BAR - 2.0) * 0.5);
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_row_count(self.model.len());
        node.set_column_count(self.cols.len());
        node.set_multiselectable();
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        for (c, col) in self.cols.iter().enumerate() {
            let mut h = accesskit::Node::new(Role::ColumnHeader);
            h.set_label(col.title.as_str());
            h.set_column_index(c);
            if let Some((sc, dir)) = self.sort
                && sc == c
            {
                h.set_sort_direction(match dir {
                    SortDir::Ascending => SortDirection::Ascending,
                    SortDir::Descending => SortDirection::Descending,
                });
            }
            if col.sortable {
                h.add_action(Action::Click);
            }
            out.push((HEADER_KEY_BASE + c as u64, h));
        }
        let b = self.body_rect(cx.rect);
        for row in self.live_range(cx.rect.h) {
            let key = self.model.key(row);
            let mut n = accesskit::Node::new(Role::Row);
            let label: Vec<String> = (0..self.cols.len())
                .map(|c| self.model.cell(row, c))
                .collect();
            n.set_label(label.join(", "));
            n.set_row_index(row);
            n.set_selected(self.sel.keys.contains(&key));
            n.add_action(Action::Click);
            let y = b.y + row as f32 * self.row_h - self.scroll;
            n.set_bounds(cx.bounds(Rect::new(b.x, y, b.w, self.row_h)));
            out.push((key, n));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        self.sel
            .active
            .filter(|a| *a < self.model.len())
            .map(|a| self.model.key(a))
    }
}

/// Virtual child keys of column headers (row keys are model keys below this).
pub const HEADER_KEY_BASE: u64 = u64::MAX - 1024;
