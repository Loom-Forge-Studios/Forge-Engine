//! The domain editors' custom widgets. Each paints from a small shared view model the
//! panel fills from the project (`Rc<RefCell<…>>`), raises **actions** for what the user
//! did (the panel turns them into commands — a widget never edits the project), is
//! keyboard-operable where it edits, and gives assistive technology a role and a name.
//!
//! * [`TileCanvas`] — a tile-map layer, **virtualised**: it visits only the cells in view
//!   (`painted_cells`), whatever the map's size. Drag paints, right-drag erases; arrows
//!   move a cursor, Space paints and Delete erases at it.
//! * [`TilePalette`] — a tile set's tiles, to pick a brush or assign an autotile rule.
//! * [`SheetPreview`] — a sprite sheet's slicing grid with frame numbers; pick a frame.
//! * [`RigView`] — a `Skeleton2D` rig's pose; drag a bone's tip to rotate it.
//! * [`MeterBridge`] — every bus's meters, from the audio backend's live feed (§21.11).
//! * [`SpatialPad`] — the spatial preview: drag the source around the listener.
//! * [`BindCapture`] — "press to bind": the next key, mouse button or gamepad control.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use accesskit::{Action, Node};
use forge_editor::domain::audio::{AudioBuses, Meter, PreviewOffset, Spatial, SpatialOut};
use forge_editor::domain::input::{Control, InputActions};
use forge_editor::domain::scene2d::{
    Cell as MapCell, FrameRect, Layer, Pose, Scene2d, SheetSlice, Stroke, Tileset,
};
use forge_ui::input::PointerButton;
use forge_ui::render::mesh::PathBuilder;
use forge_ui::text::TextStyle;
use forge_ui::widget::{A11yCx, EventCx, PaintCx};
use forge_ui::{
    ColorRole, Handled, KeyCode, Modifiers, Point, Rect, Role, Signal, UiEvent, Widget,
};

fn small(cx: &PaintCx) -> TextStyle {
    TextStyle::body(cx.theme().type_scale.small)
}

/// An outline of `r` (four thin marks).
fn outline(cx: &mut PaintCx, r: Rect, role: ColorRole, w: f32) {
    let on = cx.parent_bg();
    cx.mark(Rect::new(r.x, r.y, r.w, w), role, on, 0.0);
    cx.mark(Rect::new(r.x, r.bottom() - w, r.w, w), role, on, 0.0);
    cx.mark(Rect::new(r.x, r.y, w, r.h), role, on, 0.0);
    cx.mark(Rect::new(r.right() - w, r.y, w, r.h), role, on, 0.0);
}

// ---- tile map canvas --------------------------------------------------------------------

/// Where a stroke is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrokePhase {
    Begin,
    Move,
    End,
    /// A one-cell stroke from the keyboard (Space / Delete at the cursor).
    Single,
}

/// Cells to paint (or erase) as part of a stroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaintCells {
    pub cells: Vec<(i64, i64)>,
    pub erase: bool,
    pub phase: StrokePhase,
}

/// What the canvas shows.
#[derive(Default)]
pub struct CanvasView {
    /// The selected layer, shared with the editor's model (never a copy of its cells).
    pub layer: Option<Arc<Layer>>,
    pub tileset: Option<Tileset>,
    /// The stroke being painted (shown over the layer until the mirror has it).
    pub stroke: Stroke,
    /// "Tile map Level 1, layer Ground".
    pub label: String,
}

/// The tile a cell shows (`Err`: a terrain cell no rule resolves).
pub fn shown_tile(
    backend: &dyn Scene2d,
    view: &CanvasView,
    layer: &Layer,
    x: i64,
    y: i64,
) -> Option<Result<u32, ()>> {
    match view.stroke.cell(layer, x, y) {
        MapCell::Empty => None,
        MapCell::Tile(t) => Some(Ok(*t)),
        MapCell::Terrain(t) => {
            let terrain = view.tileset.as_ref().and_then(|ts| ts.terrains.get(t));
            Some(
                terrain
                    .and_then(|tr| backend.autotile(tr, layer.mask(&view.stroke, x, y, t)))
                    .ok_or(()),
            )
        }
    }
}

pub struct TileCanvas {
    view: Rc<RefCell<CanvasView>>,
    backend: Rc<dyn Scene2d>,
    cell_px: f32,
    /// The top-left cell in view.
    origin: (i64, i64),
    cursor: (i64, i64),
    painting: Option<bool>,
    last: Option<(i64, i64)>,
    painted: Cell<usize>,
    fault_paint_all: bool,
}

impl TileCanvas {
    pub fn new(view: Rc<RefCell<CanvasView>>, backend: Rc<dyn Scene2d>) -> Self {
        Self {
            view,
            backend,
            cell_px: 20.0,
            origin: (0, 0),
            cursor: (0, 0),
            painting: None,
            last: None,
            painted: Cell::new(0),
            fault_paint_all: false,
        }
    }
    /// Positive control for `test_tilemap_canvas_virtual`: visit every painted cell.
    pub fn with_fault_paint_all(mut self) -> Self {
        self.fault_paint_all = true;
        self
    }
    /// Cells visited by the last paint (the virtualisation guard's counter).
    pub fn painted_cells(&self) -> usize {
        self.painted.get()
    }
    /// The cell under a point.
    pub fn cell_at(&self, r: Rect, p: Point) -> (i64, i64) {
        (
            self.origin.0 + ((p.x - r.x) / self.cell_px).floor() as i64,
            self.origin.1 + ((p.y - r.y) / self.cell_px).floor() as i64,
        )
    }
    /// The screen rect of a cell.
    pub fn cell_rect(&self, r: Rect, (x, y): (i64, i64)) -> Rect {
        Rect::new(
            r.x + (x - self.origin.0) as f32 * self.cell_px,
            r.y + (y - self.origin.1) as f32 * self.cell_px,
            self.cell_px,
            self.cell_px,
        )
    }
    /// The cells in view `(cols, rows)`.
    fn span(&self, r: Rect) -> (i64, i64) {
        (
            (r.w / self.cell_px).ceil() as i64,
            (r.h / self.cell_px).ceil() as i64,
        )
    }
    /// Scroll so the top-left cell is `o`.
    pub fn scroll_to(&mut self, o: (i64, i64)) {
        self.origin = o;
    }

    fn keep_cursor_in_view(&mut self, r: Rect) {
        let (cols, rows) = self.span(r);
        let (cx, cy) = self.cursor;
        if cx < self.origin.0 {
            self.origin.0 = cx;
        } else if cx >= self.origin.0 + cols.max(1) {
            self.origin.0 = cx - cols.max(1) + 1;
        }
        if cy < self.origin.1 {
            self.origin.1 = cy;
        } else if cy >= self.origin.1 + rows.max(1) {
            self.origin.1 = cy - rows.max(1) + 1;
        }
    }

    fn describe_cursor(&self) -> String {
        let v = self.view.borrow();
        let (x, y) = self.cursor;
        let what = match &v.layer {
            None => forge_ui::tr!("no layer").to_string(),
            Some(l) => match shown_tile(&*self.backend, &v, l, x, y) {
                None => forge_ui::tr!("empty").into(),
                Some(Ok(t)) => match v.stroke.cell(l, x, y) {
                    MapCell::Terrain(tr) => forge_ui::trf!("terrain {tr}, tile {t}", tr, t),
                    _ => forge_ui::trf!("tile {t}", t),
                },
                Some(Err(())) => forge_ui::tr!("terrain with no matching rule").into(),
            },
        };
        forge_ui::trf!("Cell {x}, {y}: {what}", x, y, what)
    }
}

/// The cells on a line from `a` to `b`, without `a` (a fast drag leaves no gaps).
fn line(a: (i64, i64), b: (i64, i64)) -> Vec<(i64, i64)> {
    let (dx, dy) = ((b.0 - a.0).abs(), -(b.1 - a.1).abs());
    let (sx, sy) = (
        if a.0 < b.0 { 1 } else { -1 },
        if a.1 < b.1 { 1 } else { -1 },
    );
    let (mut x, mut y, mut err) = (a.0, a.1, dx + dy);
    let mut out = Vec::new();
    while (x, y) != b && out.len() < 4096 {
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        out.push((x, y));
    }
    out
}

impl Widget for TileCanvas {
    fn role(&self) -> Role {
        Role::Grid
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let r = cx.rect();
        match ev {
            UiEvent::PointerDown { pos, button, .. }
                if matches!(button, PointerButton::Primary | PointerButton::Secondary) =>
            {
                let c = self.cell_at(r, *pos);
                let erase = *button == PointerButton::Secondary;
                self.painting = Some(erase);
                self.last = Some(c);
                self.cursor = c;
                cx.request_focus();
                cx.capture_pointer();
                cx.action(PaintCells {
                    cells: vec![c],
                    erase,
                    phase: StrokePhase::Begin,
                });
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                let (Some(erase), Some(last)) = (self.painting, self.last) else {
                    return Handled::No;
                };
                let c = self.cell_at(r, *pos);
                if c != last {
                    self.last = Some(c);
                    cx.action(PaintCells {
                        cells: line(last, c),
                        erase,
                        phase: StrokePhase::Move,
                    });
                }
                Handled::Yes
            }
            UiEvent::PointerUp { .. } => {
                if let Some(erase) = self.painting.take() {
                    self.last = None;
                    cx.release_pointer();
                    cx.action(PaintCells {
                        cells: Vec::new(),
                        erase,
                        phase: StrokePhase::End,
                    });
                }
                Handled::Yes
            }
            UiEvent::Wheel { dx, dy } => {
                let step = |v: f32| (v / 8.0).round() as i64;
                let (sx, sy) = if cx.modifiers().shift {
                    (step(*dy), step(*dx))
                } else {
                    (step(*dx), step(*dy))
                };
                if sx != 0 || sy != 0 {
                    self.origin.0 -= sx;
                    self.origin.1 -= sy;
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed && k.mods == Modifiers::NONE => {
                let mv = match k.code {
                    KeyCode::Left => Some((-1, 0)),
                    KeyCode::Right => Some((1, 0)),
                    KeyCode::Up => Some((0, -1)),
                    KeyCode::Down => Some((0, 1)),
                    _ => None,
                };
                if let Some((dx, dy)) = mv {
                    self.cursor = (self.cursor.0 + dx, self.cursor.1 + dy);
                    self.keep_cursor_in_view(r);
                    cx.request_paint();
                    cx.request_a11y();
                    return Handled::Yes;
                }
                let erase = match k.code {
                    KeyCode::Space | KeyCode::Enter => false,
                    KeyCode::Delete | KeyCode::Backspace => true,
                    _ => return Handled::No,
                };
                cx.action(PaintCells {
                    cells: vec![self.cursor],
                    erase,
                    phase: StrokePhase::Single,
                });
                cx.request_a11y();
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgSunken, 0.0);
        let v = self.view.borrow();
        let Some(layer) = v.layer.as_ref() else {
            self.painted.set(0);
            cx.text(
                forge_ui::tr!("Pick a tile map and a layer to paint."),
                &small(cx),
                Point::new(r.x + 8.0, r.y + 8.0),
                ColorRole::FgMuted,
            );
            return;
        };
        let style = small(cx);
        let mut visited = 0usize;
        let mut draw = |cx: &mut PaintCx, x: i64, y: i64| {
            visited += 1;
            let cr = self.cell_rect(r, (x, y));
            if !r.intersects(&cr) {
                return;
            }
            match shown_tile(&*self.backend, &v, layer, x, y) {
                None => {}
                Some(Ok(t)) => {
                    let inner = Rect::new(cr.x + 1.0, cr.y + 1.0, cr.w - 2.0, cr.h - 2.0);
                    cx.mark(inner, ColorRole::Accent, ColorRole::BgSunken, 2.0);
                    cx.text_on(
                        &t.to_string(),
                        &style,
                        Point::new(cr.x + 3.0, cr.y + 3.0),
                        ColorRole::FgOnAccent,
                        ColorRole::Accent,
                    );
                }
                Some(Err(())) => {
                    let inner = Rect::new(cr.x + 1.0, cr.y + 1.0, cr.w - 2.0, cr.h - 2.0);
                    cx.mark(inner, ColorRole::Danger, ColorRole::BgSunken, 2.0);
                    cx.text_on(
                        "?",
                        &style,
                        Point::new(cr.x + 5.0, cr.y + 3.0),
                        ColorRole::FgOnDanger,
                        ColorRole::Danger,
                    );
                }
            }
        };
        if self.fault_paint_all {
            // The positive control: every painted cell of the layer, in view or not.
            let cells: Vec<(i64, i64)> = layer
                .chunks
                .keys()
                .flat_map(|(cx0, cy0)| {
                    (0..forge_editor::domain::scene2d::CHUNK * forge_editor::domain::scene2d::CHUNK)
                        .map(move |i| {
                            (
                                cx0 * forge_editor::domain::scene2d::CHUNK
                                    + i % forge_editor::domain::scene2d::CHUNK,
                                cy0 * forge_editor::domain::scene2d::CHUNK
                                    + i / forge_editor::domain::scene2d::CHUNK,
                            )
                        })
                })
                .collect();
            for (x, y) in cells {
                draw(cx, x, y);
            }
        } else {
            let (cols, rows) = self.span(r);
            for y in self.origin.1..self.origin.1 + rows {
                for x in self.origin.0..self.origin.0 + cols {
                    draw(cx, x, y);
                }
            }
        }
        self.painted.set(visited);
        if cx.state().focused {
            let cr = self.cell_rect(r, self.cursor);
            if r.intersects(&cr) {
                outline(cx, cr, ColorRole::FocusRing, 2.0);
            }
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut Node) {
        let label = self.view.borrow().label.clone();
        node.set_label(if label.is_empty() {
            forge_ui::tr!("Tile map").to_string()
        } else {
            label
        });
        node.set_value(self.describe_cursor());
        node.set_description(forge_ui::tr!("Arrows move the cursor; Space paints the brush; Delete erases; drag paints, right-drag erases"));
    }
}

// ---- tile palette -----------------------------------------------------------------------

/// A tile was picked in the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilePicked {
    pub tile: u32,
}

#[derive(Default)]
pub struct PaletteView {
    pub count: u32,
    pub columns: u32,
    /// The highlighted tile (the brush, or the selected rule's tile).
    pub selected: Option<u32>,
    pub label: String,
}

pub struct TilePalette {
    view: Rc<RefCell<PaletteView>>,
    cursor: u32,
    tile_px: f32,
}

impl TilePalette {
    pub fn new(view: Rc<RefCell<PaletteView>>) -> Self {
        Self {
            view,
            cursor: 0,
            tile_px: 28.0,
        }
    }
    fn cols(&self, r: Rect) -> u32 {
        let v = self.view.borrow();
        let fit = ((r.w / self.tile_px).floor() as u32).max(1);
        if v.columns == 0 {
            fit
        } else {
            v.columns.min(fit)
        }
    }
    fn tile_rect(&self, r: Rect, t: u32) -> Rect {
        let c = self.cols(r);
        Rect::new(
            r.x + (t % c) as f32 * self.tile_px,
            r.y + (t / c) as f32 * self.tile_px,
            self.tile_px - 2.0,
            self.tile_px - 2.0,
        )
    }
}

impl Widget for TilePalette {
    fn role(&self) -> Role {
        Role::Grid
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let r = cx.rect();
        let count = self.view.borrow().count;
        match ev {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let c = self.cols(r);
                let col = ((pos.x - r.x) / self.tile_px).floor();
                let row = ((pos.y - r.y) / self.tile_px).floor();
                if col >= 0.0 && row >= 0.0 && (col as u32) < c {
                    let t = row as u32 * c + col as u32;
                    if t < count {
                        self.cursor = t;
                        cx.request_focus();
                        cx.action(TilePicked { tile: t });
                    }
                }
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed && k.mods == Modifiers::NONE && count > 0 => {
                let c = self.cols(r);
                let cur = i64::from(self.cursor);
                let next = match k.code {
                    KeyCode::Left => cur - 1,
                    KeyCode::Right => cur + 1,
                    KeyCode::Up => cur - i64::from(c),
                    KeyCode::Down => cur + i64::from(c),
                    KeyCode::Enter | KeyCode::Space => {
                        cx.action(TilePicked { tile: self.cursor });
                        return Handled::Yes;
                    }
                    _ => return Handled::No,
                };
                self.cursor = next.clamp(0, i64::from(count) - 1) as u32;
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let v = self.view.borrow();
        let style = small(cx);
        let visible_rows = (r.h / self.tile_px).ceil() as u32 + 1;
        let last = (self.cols(r) * visible_rows).min(v.count);
        for t in 0..last {
            let tr = self.tile_rect(r, t);
            let sel = v.selected == Some(t);
            let (bg, fg) = if sel {
                (ColorRole::Accent, ColorRole::FgOnAccent)
            } else {
                (ColorRole::BgRaised, ColorRole::FgPrimary)
            };
            cx.mark(tr, bg, cx.parent_bg(), 3.0);
            cx.text_on(
                &t.to_string(),
                &style,
                Point::new(tr.x + 4.0, tr.y + 5.0),
                fg,
                bg,
            );
            if cx.state().focused && t == self.cursor {
                outline(cx, tr, ColorRole::FocusRing, 2.0);
            }
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut Node) {
        let v = self.view.borrow();
        node.set_label(if v.label.is_empty() {
            forge_ui::tr!("Tile palette").to_string()
        } else {
            v.label.clone()
        });
        node.set_value(forge_ui::trf!(
            "Tile {tile} of {count}{selected}",
            tile = self.cursor,
            count = v.count,
            selected = if v.selected == Some(self.cursor) {
                forge_ui::tr!(", selected")
            } else {
                ""
            }
        ));
    }
}

// ---- sprite sheet preview ---------------------------------------------------------------

/// A frame was picked in the sheet preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramePicked {
    pub frame: u32,
}

#[derive(Default)]
pub struct SheetView {
    pub slice: Option<SheetSlice>,
    pub frames: Vec<FrameRect>,
    /// The selected animation's frames (highlighted, in order).
    pub highlight: Vec<u32>,
    pub label: String,
}

pub struct SheetPreview {
    view: Rc<RefCell<SheetView>>,
    cursor: u32,
}

impl SheetPreview {
    pub fn new(view: Rc<RefCell<SheetView>>) -> Self {
        Self { view, cursor: 0 }
    }
    /// Image pixels → screen: `(scale, origin)`.
    fn fit(r: Rect, s: &SheetSlice) -> (f32, Point) {
        let (w, h) = (s.image_w.max(1) as f32, s.image_h.max(1) as f32);
        let k = ((r.w - 8.0) / w).min((r.h - 8.0) / h).max(0.01);
        (k, Point::new(r.x + 4.0, r.y + 4.0))
    }
}

impl Widget for SheetPreview {
    fn role(&self) -> Role {
        Role::Grid
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let r = cx.rect();
        match ev {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let hit = {
                    let v = self.view.borrow();
                    v.slice.and_then(|s| {
                        let (k, o) = Self::fit(r, &s);
                        let (ix, iy) = (((pos.x - o.x) / k) as i64, ((pos.y - o.y) / k) as i64);
                        v.frames
                            .iter()
                            .find(|f| ix >= f.x && ix < f.x + f.w && iy >= f.y && iy < f.y + f.h)
                            .map(|f| f.index)
                    })
                };
                if let Some(f) = hit {
                    self.cursor = f;
                    cx.request_focus();
                    cx.action(FramePicked { frame: f });
                }
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed && k.mods == Modifiers::NONE => {
                let n = self.view.borrow().frames.len() as i64;
                if n == 0 {
                    return Handled::No;
                }
                let cur = i64::from(self.cursor);
                let next = match k.code {
                    KeyCode::Left | KeyCode::Up => cur - 1,
                    KeyCode::Right | KeyCode::Down => cur + 1,
                    KeyCode::Enter | KeyCode::Space => {
                        cx.action(FramePicked { frame: self.cursor });
                        return Handled::Yes;
                    }
                    _ => return Handled::No,
                };
                self.cursor = next.clamp(0, n - 1) as u32;
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgSunken, 0.0);
        let v = self.view.borrow();
        let Some(s) = v.slice else {
            cx.text(
                forge_ui::tr!("Pick a sprite sheet."),
                &small(cx),
                Point::new(r.x + 8.0, r.y + 8.0),
                ColorRole::FgMuted,
            );
            return;
        };
        let (k, o) = Self::fit(r, &s);
        let img = Rect::new(o.x, o.y, s.image_w as f32 * k, s.image_h as f32 * k);
        // The sheet's backdrop is decorative; the frames drawn on it are the boundaries.
        cx.tint(img, ColorRole::BgRaised, 0.0, 1.0);
        let style = small(cx);
        for f in &v.frames {
            let fr = Rect::new(
                o.x + f.x as f32 * k,
                o.y + f.y as f32 * k,
                f.w as f32 * k,
                f.h as f32 * k,
            );
            if let Some(pos) = v.highlight.iter().position(|h| *h == f.index) {
                cx.mark(fr, ColorRole::Accent, ColorRole::BgRaised, 0.0);
                cx.text_on(
                    &format!("{} #{}", f.index, pos + 1),
                    &style,
                    Point::new(fr.x + 2.0, fr.y + 2.0),
                    ColorRole::FgOnAccent,
                    ColorRole::Accent,
                );
            } else {
                outline(cx, fr, ColorRole::Border, 1.0);
                cx.text_on(
                    &f.index.to_string(),
                    &style,
                    Point::new(fr.x + 2.0, fr.y + 2.0),
                    ColorRole::FgPrimary,
                    ColorRole::BgRaised,
                );
            }
            if cx.state().focused && f.index == self.cursor {
                outline(cx, fr, ColorRole::FocusRing, 2.0);
            }
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut Node) {
        let v = self.view.borrow();
        node.set_label(if v.label.is_empty() {
            forge_ui::tr!("Sprite sheet frames").to_string()
        } else {
            v.label.clone()
        });
        node.set_value(forge_ui::trf!(
            "Frame {n} of {count}",
            n = self.cursor,
            count = v.frames.len()
        ));
        node.set_description(forge_ui::tr!(
            "Arrows choose a frame; Enter adds it to the selected animation"
        ));
    }
}

// ---- rig view ---------------------------------------------------------------------------

/// A bone was clicked, or its tip dragged to a new world angle.
#[derive(Clone, Debug, PartialEq)]
pub struct BoneDrag {
    pub bone: String,
    /// World angle from the bone's origin to the pointer (degrees, counter-clockwise).
    pub angle: f64,
    pub phase: StrokePhase,
}

#[derive(Default)]
pub struct RigViewData {
    pub pose: Pose,
    pub selected: Option<String>,
    pub name: String,
}

pub struct RigView {
    data: Rc<RefCell<RigViewData>>,
    dragging: Option<String>,
}

impl RigView {
    pub fn new(data: Rc<RefCell<RigViewData>>) -> Self {
        Self {
            data,
            dragging: None,
        }
    }
    /// Rig space (y up) → screen, the rig's origin at the lower middle.
    pub fn to_screen(r: Rect, p: (f64, f64)) -> Point {
        Point::new(r.x + r.w * 0.5 + p.0 as f32, r.y + r.h * 0.75 - p.1 as f32)
    }
    fn angle_to(r: Rect, origin: (f64, f64), pos: Point) -> f64 {
        let o = Self::to_screen(r, origin);
        let (dx, dy) = (f64::from(pos.x - o.x), f64::from(o.y - pos.y));
        dy.atan2(dx).to_degrees()
    }
}

impl Widget for RigView {
    fn role(&self) -> Role {
        Role::Image
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let r = cx.rect();
        match ev {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let hit = {
                    let d = self.data.borrow();
                    d.pose
                        .bones
                        .iter()
                        .map(|b| {
                            let t = Self::to_screen(r, b.tip);
                            ((t.x - pos.x).hypot(t.y - pos.y), b.id.clone(), b.origin)
                        })
                        .filter(|(dist, ..)| *dist <= 10.0)
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                };
                if let Some((_, id, origin)) = hit {
                    self.dragging = Some(id.clone());
                    cx.capture_pointer();
                    cx.action(BoneDrag {
                        bone: id,
                        angle: Self::angle_to(r, origin, *pos),
                        phase: StrokePhase::Begin,
                    });
                }
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                let Some(id) = self.dragging.clone() else {
                    return Handled::No;
                };
                let origin = self
                    .data
                    .borrow()
                    .pose
                    .bones
                    .iter()
                    .find(|b| b.id == id)
                    .map(|b| b.origin);
                if let Some(o) = origin {
                    cx.action(BoneDrag {
                        bone: id,
                        angle: Self::angle_to(r, o, *pos),
                        phase: StrokePhase::Move,
                    });
                }
                Handled::Yes
            }
            UiEvent::PointerUp { .. } => {
                if let Some(id) = self.dragging.take() {
                    cx.release_pointer();
                    cx.action(BoneDrag {
                        bone: id,
                        angle: 0.0,
                        phase: StrokePhase::End,
                    });
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgSunken, 0.0);
        let d = self.data.borrow();
        if d.pose.bones.is_empty() {
            cx.text(
                forge_ui::tr!("Pick a rig; its bones are drawn here."),
                &small(cx),
                Point::new(r.x + 8.0, r.y + 8.0),
                ColorRole::FgMuted,
            );
            return;
        }
        for b in &d.pose.bones {
            let sel = d.selected.as_deref() == Some(b.id.as_str());
            let (a, t) = (Self::to_screen(r, b.origin), Self::to_screen(r, b.tip));
            let mut p = PathBuilder::new();
            p.move_to(a).line_to(t);
            let role = if sel {
                ColorRole::Accent
            } else {
                ColorRole::FgMuted
            };
            cx.stroke_path(
                &p.build(),
                if sel { 4.0 } else { 2.5 },
                role,
                ColorRole::BgSunken,
            );
            cx.mark(
                Rect::new(a.x - 3.0, a.y - 3.0, 6.0, 6.0),
                ColorRole::FgPrimary,
                ColorRole::BgSunken,
                3.0,
            );
            cx.mark(
                Rect::new(t.x - 4.0, t.y - 4.0, 8.0, 8.0),
                role,
                ColorRole::BgSunken,
                4.0,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut Node) {
        let d = self.data.borrow();
        let broken = if d.pose.broken.is_empty() {
            String::new()
        } else {
            forge_ui::trf!(
                ", {n} not posed (loop or missing parent)",
                n = d.pose.broken.len()
            )
        };
        node.set_label(forge_ui::trf!(
            "Rig {name}: {bones_count} bones posed{broken}",
            name = d.name,
            bones_count = d.pose.bones.len(),
            broken
        ));
    }
}

// ---- meter bridge -----------------------------------------------------------------------

pub struct MeterBridge {
    backend: Rc<dyn AudioBuses>,
    /// `(bus id, name)` in display order (the panel sets it).
    order: Rc<RefCell<Vec<(String, String)>>>,
    shown: RefCell<BTreeMap<String, Meter>>,
    refreshes: Cell<u64>,
}

impl MeterBridge {
    pub fn new(backend: Rc<dyn AudioBuses>, order: Rc<RefCell<Vec<(String, String)>>>) -> Self {
        let shown = RefCell::new(backend.meters());
        Self {
            backend,
            order,
            shown,
            refreshes: Cell::new(0),
        }
    }
    /// How often the meters were re-read (only on a feed bump).
    pub fn refreshes(&self) -> u64 {
        self.refreshes.get()
    }
    /// A bus's meter as shown.
    pub fn shown(&self, bus: &str) -> Option<Meter> {
        self.shown.borrow().get(bus).copied()
    }
    fn frac(db: f64) -> f32 {
        ((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32
    }
}

impl Widget for MeterBridge {
    fn role(&self) -> Role {
        Role::Group
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::FeedChanged(_) => {
                self.refreshes.set(self.refreshes.get() + 1);
                let m = self.backend.meters();
                if *self.shown.borrow() != m {
                    *self.shown.borrow_mut() = m;
                    cx.request_paint();
                    cx.request_a11y();
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgSunken, 0.0);
        let order = self.order.borrow();
        let shown = self.shown.borrow();
        let style = small(cx);
        let col_w = 64.0;
        let bar_h = (r.h - 36.0).max(10.0);
        for (i, (id, name)) in order.iter().enumerate() {
            let x = r.x + 6.0 + i as f32 * col_w;
            if x > r.right() {
                break;
            }
            let m = shown.get(id).copied().unwrap_or(Meter::SILENT);
            for (j, db) in [m.peak_l_db, m.peak_r_db].into_iter().enumerate() {
                let bx = x + j as f32 * 10.0;
                // The meter's track is decorative: the level is carried by the fill and
                // by the dB readout below (colour and shape are never the only carrier).
                cx.tint(
                    Rect::new(bx, r.y + 4.0, 8.0, bar_h),
                    ColorRole::BgRaised,
                    1.0,
                    1.0,
                );
                let h = bar_h * Self::frac(db);
                let role = if db > -1.0 {
                    ColorRole::Danger
                } else {
                    ColorRole::Accent
                };
                cx.mark(
                    Rect::new(bx, r.y + 4.0 + bar_h - h, 8.0, h),
                    role,
                    ColorRole::BgRaised,
                    1.0,
                );
            }
            let rms_y = r.y + 4.0 + bar_h * (1.0 - Self::frac(m.rms_db));
            cx.mark(
                Rect::new(x - 2.0, rms_y, 22.0, 2.0),
                ColorRole::FgPrimary,
                ColorRole::BgSunken,
                0.0,
            );
            let db_text = if m.peak_db() <= -119.0 {
                "\u{2212}\u{221e}".to_string()
            } else {
                format!("{:.1}", m.peak_db())
            };
            cx.text(
                &db_text,
                &style,
                Point::new(x + 22.0, r.y + 4.0),
                ColorRole::FgPrimary,
            );
            let short: String = name.chars().take(8).collect();
            cx.text(
                &short,
                &style,
                Point::new(x, r.y + 8.0 + bar_h),
                ColorRole::FgPrimary,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut Node) {
        node.set_label(forge_ui::tr!("Bus meters"));
    }
    fn a11y_children(&self, _cx: &A11yCx, out: &mut Vec<(u64, Node)>) {
        let shown = self.shown.borrow();
        for (i, (id, name)) in self.order.borrow().iter().enumerate() {
            let m = shown.get(id).copied().unwrap_or(Meter::SILENT);
            let mut n = Node::new(Role::Meter);
            n.set_label(forge_ui::trf!("{name} level", name));
            n.set_numeric_value(m.peak_db());
            n.set_min_numeric_value(-120.0);
            n.set_max_numeric_value(12.0);
            n.set_value(forge_ui::trf!(
                "peak {peak} dB, RMS {rms} dB",
                peak = format!("{:.1}", m.peak_db()),
                rms = format!("{:.1}", m.rms_db)
            ));
            out.push((i as u64, n));
        }
    }
}

// ---- spatial preview --------------------------------------------------------------------

/// The preview source moved (session state: an audition, not an edit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceMoved {
    pub at: PreviewOffset,
    pub out: SpatialOut,
}

pub struct PadData {
    pub spatial: Spatial,
    pub source: PreviewOffset,
}

pub struct SpatialPad {
    data: Rc<RefCell<PadData>>,
    backend: Rc<dyn AudioBuses>,
    dragging: bool,
}

impl SpatialPad {
    pub fn new(data: Rc<RefCell<PadData>>, backend: Rc<dyn AudioBuses>) -> Self {
        Self {
            data,
            backend,
            dragging: false,
        }
    }
    /// Metres per logical pixel: the max distance fits the pad.
    fn scale(r: Rect, s: &Spatial) -> f64 {
        let half = f64::from(r.w.min(r.h) * 0.5 - 12.0).max(10.0);
        (s.max_distance * 1.1) / half
    }
    fn out(&self) -> SpatialOut {
        let d = self.data.borrow();
        self.backend.spatialize(&d.spatial, d.source)
    }
    fn set_source(&mut self, cx: &mut EventCx, at: PreviewOffset) {
        self.data.borrow_mut().source = at;
        let out = self.out();
        cx.action(SourceMoved { at, out });
        cx.request_paint();
        cx.request_a11y();
    }
    fn readout(&self) -> String {
        let o = self.out();
        let gain = if o.gain_db <= -119.0 {
            "silent".to_string()
        } else {
            format!("{:+.1} dB", o.gain_db)
        };
        forge_ui::trf!(
            "distance {distance} m, {gain}, pan {pan}",
            distance = format!("{:.1}", o.distance_m),
            gain,
            pan = format!("{:+.2}", o.pan)
        )
    }
}

impl Widget for SpatialPad {
    fn role(&self) -> Role {
        Role::Canvas
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let r = cx.rect();
        let k = Self::scale(r, &self.data.borrow().spatial);
        let at_of = |p: Point| PreviewOffset {
            right_m: f64::from(p.x - r.center().x) * k,
            ahead_m: f64::from(r.center().y - p.y) * k,
        };
        match ev {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                self.dragging = true;
                cx.capture_pointer();
                cx.request_focus();
                self.set_source(cx, at_of(*pos));
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if self.dragging => {
                self.set_source(cx, at_of(*pos));
                Handled::Yes
            }
            UiEvent::PointerUp { .. } if self.dragging => {
                self.dragging = false;
                cx.release_pointer();
                Handled::Yes
            }
            UiEvent::Key(key) if key.pressed && key.mods == Modifiers::NONE => {
                let s = self.data.borrow().source;
                let step = 0.5;
                let at = match key.code {
                    KeyCode::Left => PreviewOffset {
                        right_m: s.right_m - step,
                        ..s
                    },
                    KeyCode::Right => PreviewOffset {
                        right_m: s.right_m + step,
                        ..s
                    },
                    KeyCode::Up => PreviewOffset {
                        ahead_m: s.ahead_m + step,
                        ..s
                    },
                    KeyCode::Down => PreviewOffset {
                        ahead_m: s.ahead_m - step,
                        ..s
                    },
                    _ => return Handled::No,
                };
                self.set_source(cx, at);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgSunken, 0.0);
        let d = self.data.borrow();
        let k = Self::scale(r, &d.spatial);
        let c = r.center();
        for (dist, role) in [
            (d.spatial.min_distance, ColorRole::Accent),
            (d.spatial.max_distance, ColorRole::Border),
        ] {
            let rad = (dist / k) as f32;
            let mut p = PathBuilder::new();
            for i in 0..=48 {
                let a = i as f32 / 48.0 * std::f32::consts::TAU;
                let pt = Point::new(c.x + rad * a.cos(), c.y + rad * a.sin());
                if i == 0 {
                    p.move_to(pt);
                } else {
                    p.line_to(pt);
                }
            }
            cx.stroke_path(&p.build(), 1.5, role, ColorRole::BgSunken);
        }
        cx.mark(
            Rect::new(c.x - 5.0, c.y - 5.0, 10.0, 10.0),
            ColorRole::FgPrimary,
            ColorRole::BgSunken,
            5.0,
        );
        let s = Point::new(
            c.x + (d.source.right_m / k) as f32,
            c.y - (d.source.ahead_m / k) as f32,
        );
        cx.mark(
            Rect::new(s.x - 6.0, s.y - 6.0, 12.0, 12.0),
            ColorRole::Accent,
            ColorRole::BgSunken,
            6.0,
        );
        drop(d);
        let text = self.readout();
        cx.text(
            &text,
            &small(cx),
            Point::new(r.x + 6.0, r.y + 6.0),
            ColorRole::FgPrimary,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut Node) {
        node.set_label(forge_ui::tr!(
            "Spatial preview: source position around the listener"
        ));
        node.set_value(self.readout());
        node.set_description(forge_ui::tr!(
            "Arrows move the source half a metre; drag to place it"
        ));
        node.add_action(Action::Focus);
    }
}

// ---- bind capture -----------------------------------------------------------------------

/// Capture ended: the controls pressed, or `None` if cancelled (Esc).
#[derive(Clone, Debug, PartialEq)]
pub struct BindCaptured {
    pub controls: Option<Vec<Control>>,
}

const POLL: u64 = 0x6269_6e64;
/// How often a capture polls devices the window does not see (a gamepad).
pub const POLL_EVERY: Duration = Duration::from_millis(50);

/// Takes focus and records the next `need` controls pressed — keys and mouse buttons from
/// the window, gamepad controls polled from the input backend **only while capturing** —
/// then raises [`BindCaptured`]. Esc before anything is pressed cancels.
pub struct BindCapture {
    backend: Rc<dyn InputActions>,
    prompt: Signal<String>,
    need: usize,
    got: Vec<Control>,
    active: bool,
}

impl BindCapture {
    pub fn new(backend: Rc<dyn InputActions>, prompt: Signal<String>) -> Self {
        Self {
            backend,
            prompt,
            need: 1,
            got: Vec::new(),
            active: false,
        }
    }
    /// Start capturing `need` controls (the panel focuses the widget next).
    pub fn start(&mut self, need: usize) {
        self.need = need.max(1);
        self.got.clear();
        self.active = true;
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
    fn push(&mut self, cx: &mut EventCx, c: Control) {
        if !self.active {
            return;
        }
        self.got.push(c);
        if self.got.len() >= self.need {
            self.finish(cx, true);
        } else {
            cx.request_paint();
            cx.request_a11y();
        }
    }
    fn finish(&mut self, cx: &mut EventCx, ok: bool) {
        self.active = false;
        cx.cancel_timer(POLL);
        let controls = ok.then(|| std::mem::take(&mut self.got));
        self.got.clear();
        cx.action(BindCaptured { controls });
        cx.request_paint();
        cx.request_a11y();
    }
    fn text(&self, rt: &forge_ui::Runtime) -> String {
        let p = self.prompt.get(rt);
        if self.got.is_empty() {
            p
        } else {
            let so_far: Vec<String> = self.got.iter().map(Control::path).collect();
            format!("{p} \u{2014} {} \u{2026}", so_far.join(", "))
        }
    }
}

impl Widget for BindCapture {
    fn role(&self) -> Role {
        Role::Button
    }
    fn bind(&self, b: &mut forge_ui::widget::Binder) {
        b.watch(
            Some(self.prompt.any()),
            forge_ui::Dirty::PAINT | forge_ui::Dirty::A11Y,
        );
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::FocusGained { .. } if self.active => {
                cx.set_timer(POLL_EVERY, POLL);
                Handled::Yes
            }
            UiEvent::FocusLost if self.active => {
                self.finish(cx, false);
                Handled::Yes
            }
            UiEvent::Timer(POLL) if self.active => {
                if let Some(c) = self.backend.poll_capture() {
                    self.push(cx, c);
                }
                if self.active {
                    cx.set_timer(POLL_EVERY, POLL);
                }
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed && self.active => {
                if k.code == KeyCode::Escape && self.got.is_empty() {
                    self.finish(cx, false);
                } else if let Some(c) = self.backend.control_for_key(k.code.clone()) {
                    self.push(cx, c);
                }
                Handled::Yes
            }
            UiEvent::Key(_) if self.active => Handled::Yes,
            UiEvent::PointerDown { button, .. } if self.active => {
                if let Some(c) = self.backend.control_for_pointer(*button) {
                    self.push(cx, c);
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let (bg, fg) = if self.active {
            (ColorRole::Accent, ColorRole::FgOnAccent)
        } else {
            (ColorRole::BgRaised, ColorRole::FgPrimary)
        };
        cx.mark(r, bg, cx.parent_bg(), 4.0);
        let t = self.text(cx.rt());
        let style = TextStyle::body(cx.theme().type_scale.body);
        cx.text_on(&t, &style, Point::new(r.x + 8.0, r.y + 5.0), fg, bg);
    }
    fn a11y(&self, cx: &A11yCx, node: &mut Node) {
        node.set_label(self.text(cx.rt));
    }
}
