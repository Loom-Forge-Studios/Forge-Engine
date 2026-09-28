//! The **node canvas** (§21.16 "Graph", WP-U8): the one canvas the graph editor draws the
//! blueprint, material, generator and PCG graphs on (Ch.24, E-11).
//!
//! It is a view with interaction, and knows nothing of graphs as data: the application
//! gives it nodes (title, typed pins), wires, comments and errors through
//! [`NodeCanvas::edit`], and it raises **typed actions** for every edit the user asks for
//! ([`NodesMoved`], [`ConnectRequested`], [`DeleteRequested`], ...). The editor turns each
//! into a command (I7); nothing here changes project state.
//!
//! * **Pan and zoom** — middle or right drag (or Alt+drag), the wheel zooms about the
//!   pointer, the arrow keys pan, `+` / `-` / `0` zoom, `F` frames the selection (or all).
//! * **Only what is on screen is recorded** (D-5, `ui_graph_2k_nodes`): a pan or zoom frame
//!   records the nodes and wires that intersect the view and nothing else, and below
//!   [`DETAIL_ZOOM`] a node is one quad (no text, no pin labels). The **minimap** is a child
//!   widget whose slice holds every node as a dot: it re-records when the graph changes,
//!   never on a pan — only its one-quad view frame does.
//! * **Wires** — drag from a pin to a pin. Every candidate is checked while the wire is
//!   dragged ([`NodeCanvas::set_connect_check`], e.g. unit-typed pins): an incompatible pin
//!   shows the **reason** next to it, and releasing on it raises [`ConnectRefused`] and
//!   leaves the reason on screen (also the pin's tooltip). A wire dropped on empty space
//!   opens the node search for that pin. Double-click a wire: a reroute
//!   ([`RerouteRequested`]). Alt+click an input: disconnect.
//! * **Node search** — Space, or a double click on empty space, opens a filtered pick list
//!   ([`crate::widgets::PickList`]) of the items the application supplies
//!   ([`NodeCanvas::set_search`]); choosing raises [`NodeChosen`] at that point.
//! * **Comments / groups** — framed regions behind the nodes; dragging a header moves the
//!   comment and every node inside it (one [`CommentEdited`]); the corner resizes it.
//!   `C` asks for a comment around the selection ([`CommentRequested`]).
//! * **Selection, copy/paste** — click, Shift/Ctrl+click, marquee; Ctrl+A; Ctrl+C / X / V /
//!   D raise [`ClipboardRequested`] (the application owns the clipboard format); Delete
//!   raises [`DeleteRequested`]; Ctrl+arrows nudge the selection.
//! * **Errors** — a node with an error is outlined in the danger colour, shows the message
//!   under it and as its tooltip (compile errors mapped back to nodes).
//! * **Accessibility** — one tab stop (a list box); every node on screen is a virtual
//!   option announcing its title, pins and error; `[` / `]` move the selection node by node.

use std::cell::{Cell, Ref, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use accesskit::{Action, Role};

use crate::UiError;
use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::id::{Key, WidgetId};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::layout::NodeStyle;
use crate::overlay::{Anchor, PopupSpec};
use crate::state::{Runtime, Signal};
use crate::style::ColorRole;
use crate::text::TextStyle;
use crate::ui::Ui;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::{Build, Container, PickItem, PickList};

/// Node header height (world units = logical px at zoom 1).
pub const HEADER_H: f32 = 26.0;
/// One pin row.
pub const PIN_ROW_H: f32 = 20.0;
/// Narrowest node.
pub const NODE_MIN_W: f32 = 120.0;
/// A reroute's size.
pub const REROUTE: f32 = 14.0;
pub const ZOOM_MIN: f32 = 0.1;
pub const ZOOM_MAX: f32 = 3.0;
/// Below this zoom a node is drawn as one quad: no title, no pins, no labels.
pub const DETAIL_ZOOM: f32 = 0.35;
/// Below this zoom pin labels are not drawn (titles still are).
pub const LABEL_ZOOM: f32 = 0.6;
/// How long after the last pan or zoom the accessibility tree gets the new node bounds.
const A11Y_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);
const TIMER_A11Y: u64 = 1;
/// Pointer tolerance for pins and wires (logical px on screen).
const HIT_PX: f32 = 7.0;
/// The minimap's size.
pub const MINIMAP: Size = Size::new(200.0, 130.0);

/// A pin: its node, which side, and its index on that side.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PinRef {
    pub node: u64,
    pub output: bool,
    pub index: u16,
}

impl PinRef {
    pub fn input(node: u64, index: u16) -> Self {
        Self {
            node,
            output: false,
            index,
        }
    }
    pub fn output(node: u64, index: u16) -> Self {
        Self {
            node,
            output: true,
            index,
        }
    }
}

/// One pin as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasPin {
    /// Shown next to the pin (`speed`).
    pub label: String,
    /// Its type and unit (`f64 · m/s`): the tooltip.
    pub detail: String,
    /// The pin's colour (one per kind of value).
    pub role: ColorRole,
}

impl CanvasPin {
    pub fn new(label: &str, detail: &str, role: ColorRole) -> Self {
        Self {
            label: label.to_string(),
            detail: detail.to_string(),
            role,
        }
    }
}

/// One node as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasNode {
    pub title: String,
    /// Top-left, in world units.
    pub pos: Point,
    pub inputs: Vec<CanvasPin>,
    pub outputs: Vec<CanvasPin>,
    /// Its tooltip (the node's doc comment).
    pub doc: String,
    /// A compile error mapped back to this node.
    pub error: Option<String>,
    /// A reroute: a dot with one input and one output, no title.
    pub reroute: bool,
    size: Size,
}

impl CanvasNode {
    pub fn new(title: &str, pos: Point) -> Self {
        let mut n = Self {
            title: title.to_string(),
            pos,
            inputs: Vec::new(),
            outputs: Vec::new(),
            doc: String::new(),
            error: None,
            reroute: false,
            size: Size::ZERO,
        };
        n.relayout();
        n
    }
    /// A reroute dot (its pins take the colour `role`).
    pub fn reroute(pos: Point, role: ColorRole, detail: &str) -> Self {
        let mut n = Self::new(crate::tr!("Reroute"), pos);
        n.reroute = true;
        n.inputs = vec![CanvasPin::new("", detail, role)];
        n.outputs = vec![CanvasPin::new("", detail, role)];
        n.relayout();
        n
    }
    pub fn with_inputs(mut self, pins: Vec<CanvasPin>) -> Self {
        self.inputs = pins;
        self.relayout();
        self
    }
    pub fn with_outputs(mut self, pins: Vec<CanvasPin>) -> Self {
        self.outputs = pins;
        self.relayout();
        self
    }
    pub fn with_doc(mut self, doc: &str) -> Self {
        self.doc = doc.to_string();
        self
    }
    /// Its size (world units), from its title and pin labels: no text is shaped for it.
    pub fn size(&self) -> Size {
        self.size
    }
    /// Its rect (world units).
    pub fn rect(&self) -> Rect {
        Rect::new(self.pos.x, self.pos.y, self.size.w, self.size.h)
    }
    fn relayout(&mut self) {
        if self.reroute {
            self.size = Size::new(REROUTE, REROUTE);
            return;
        }
        let chars = |s: &str| s.chars().count() as f32;
        let rows = self.inputs.len().max(self.outputs.len());
        let mut widest = chars(&self.title) * 7.5 + 24.0;
        for i in 0..rows {
            let l = self.inputs.get(i).map_or(0.0, |p| chars(&p.label));
            let r = self.outputs.get(i).map_or(0.0, |p| chars(&p.label));
            widest = widest.max((l + r) * 6.5 + 48.0);
        }
        self.size = Size::new(
            widest.clamp(NODE_MIN_W, 320.0),
            HEADER_H + PIN_ROW_H * rows as f32 + 6.0,
        );
    }
    /// Where a pin sits (world units).
    pub fn pin_pos(&self, output: bool, index: u16) -> Point {
        if self.reroute {
            return self.rect().center();
        }
        let x = if output {
            self.pos.x + self.size.w
        } else {
            self.pos.x
        };
        Point::new(
            x,
            self.pos.y + HEADER_H + PIN_ROW_H * f32::from(index) + PIN_ROW_H * 0.5,
        )
    }
    fn pin(&self, output: bool, index: u16) -> Option<&CanvasPin> {
        if output {
            self.outputs.get(usize::from(index))
        } else {
            self.inputs.get(usize::from(index))
        }
    }
}

/// A wire from an output pin to an input pin (an input has at most one).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CanvasWire {
    pub from: PinRef,
    pub to: PinRef,
    pub role: ColorRole,
}

/// A comment / group frame (world units).
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasComment {
    pub rect: Rect,
    pub text: String,
}

/// What the canvas shows: `pan` is the world point at the canvas's top-left corner.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CanvasView {
    pub pan: Point,
    pub zoom: f32,
}

impl Default for CanvasView {
    fn default() -> Self {
        Self {
            pan: Point::new(0.0, 0.0),
            zoom: 1.0,
        }
    }
}

/// Decides whether an output may feed an input (`Err`: the reason, shown to the user).
pub type ConnectCheck = Rc<dyn Fn(PinRef, PinRef) -> Result<(), String>>;
/// The node-search items for a request (`Some(pin)`: a wire was dropped from that pin).
pub type SearchItems = Rc<dyn Fn(Option<PinRef>) -> Rc<Vec<PickItem>>>;

// ---- actions ------------------------------------------------------------------------------

/// Nodes were dragged (or nudged): their new top-left positions.
#[derive(Clone, Debug, PartialEq)]
pub struct NodesMoved {
    pub canvas: WidgetId,
    pub moves: Vec<(u64, Point)>,
}

/// A comment was moved or resized; `moved` are the nodes that moved with it.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentEdited {
    pub canvas: WidgetId,
    pub comment: u64,
    pub rect: Rect,
    pub moved: Vec<(u64, Point)>,
}

/// Connect an output to an input (the check passed).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ConnectRequested {
    pub canvas: WidgetId,
    pub from: PinRef,
    pub to: PinRef,
}

/// A wire was released on a pin the check refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectRefused {
    pub canvas: WidgetId,
    pub from: PinRef,
    pub to: PinRef,
    pub reason: String,
}

/// Remove the wire into this input.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DisconnectRequested {
    pub canvas: WidgetId,
    pub to: PinRef,
}

/// Delete the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteRequested {
    pub canvas: WidgetId,
    pub nodes: Vec<u64>,
    pub comments: Vec<u64>,
    /// Wires, by the input they feed.
    pub wires: Vec<PinRef>,
}

/// Put a reroute on the wire into `to`, at `at` (world).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RerouteRequested {
    pub canvas: WidgetId,
    pub to: PinRef,
    pub at: Point,
}

/// The node search chose `op` (a [`PickItem::id`]) to add at `at` (world); `from` is the
/// pin a wire was dropped from.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeChosen {
    pub canvas: WidgetId,
    pub op: String,
    pub at: Point,
    pub from: Option<PinRef>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClipOp {
    Copy,
    Cut,
    Paste,
    Duplicate,
}

/// Copy / cut / paste / duplicate: the selection, and where a paste goes (world).
#[derive(Clone, Debug, PartialEq)]
pub struct ClipboardRequested {
    pub canvas: WidgetId,
    pub op: ClipOp,
    pub nodes: Vec<u64>,
    pub comments: Vec<u64>,
    pub at: Point,
}

/// Put a comment around these nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentRequested {
    pub canvas: WidgetId,
    pub rect: Rect,
    pub nodes: Vec<u64>,
}

/// The node selection changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanvasSelection {
    pub canvas: WidgetId,
    pub nodes: Vec<u64>,
}

// ---- the model ----------------------------------------------------------------------------

/// What the canvas and its minimap share (see [`NodeCanvas::model`]).
#[derive(Default)]
pub struct CanvasModel {
    nodes: BTreeMap<u64, CanvasNode>,
    /// By the input each wire feeds.
    wires: BTreeMap<PinRef, CanvasWire>,
    comments: BTreeMap<u64, CanvasComment>,
    view: CanvasView,
    selected: BTreeSet<u64>,
    selected_comment: Option<u64>,
    selected_wire: Option<PinRef>,
    /// World bounds of every node and comment (the minimap's extent).
    bounds: Option<Rect>,
    /// Canvas size (logical px) as last laid out.
    viewport: Size,
}

impl CanvasModel {
    pub fn node(&self, k: u64) -> Option<&CanvasNode> {
        self.nodes.get(&k)
    }
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = (&u64, &CanvasNode)> {
        self.nodes.iter()
    }
    pub fn wires(&self) -> impl ExactSizeIterator<Item = &CanvasWire> {
        self.wires.values()
    }
    pub fn wire_into(&self, to: PinRef) -> Option<&CanvasWire> {
        self.wires.get(&to)
    }
    pub fn comments(&self) -> impl ExactSizeIterator<Item = (&u64, &CanvasComment)> {
        self.comments.iter()
    }
    pub fn view(&self) -> CanvasView {
        self.view
    }
    pub fn selected(&self) -> Vec<u64> {
        self.selected.iter().copied().collect()
    }
    pub fn selected_comment(&self) -> Option<u64> {
        self.selected_comment
    }
    pub fn selected_wire(&self) -> Option<PinRef> {
        self.selected_wire
    }
    /// The world rect the canvas shows.
    pub fn visible_world(&self) -> Rect {
        let z = self.view.zoom;
        Rect::new(
            self.view.pan.x,
            self.view.pan.y,
            self.viewport.w / z,
            self.viewport.h / z,
        )
    }
    /// Nodes intersecting the view (what a frame records).
    pub fn visible_nodes(&self) -> usize {
        let v = self.visible_world();
        self.nodes
            .values()
            .filter(|n| n.rect().intersects(&v))
            .count()
    }
    fn recompute_bounds(&mut self) {
        let mut b: Option<Rect> = None;
        for r in self
            .nodes
            .values()
            .map(CanvasNode::rect)
            .chain(self.comments.values().map(|c| c.rect))
        {
            b = Some(b.map_or(r, |x| x.union(&r)));
        }
        self.bounds = b;
    }
    fn wire_points(&self, w: &CanvasWire) -> Option<[Point; 4]> {
        let a = self.nodes.get(&w.from.node)?.pin_pos(true, w.from.index);
        let b = self.nodes.get(&w.to.node)?.pin_pos(false, w.to.index);
        Some(bezier(a, b))
    }
    /// Place the view so `world` fills the canvas (with a margin).
    fn frame(&mut self, world: Rect) {
        let vp = self.viewport;
        if vp.w <= 0.0 || vp.h <= 0.0 {
            return;
        }
        let r = world.outset(40.0);
        let z = (vp.w / r.w.max(1.0))
            .min(vp.h / r.h.max(1.0))
            .clamp(ZOOM_MIN, 1.5);
        self.view = CanvasView {
            zoom: z,
            pan: Point::new(r.center().x - vp.w * 0.5 / z, r.center().y - vp.h * 0.5 / z),
        };
    }
}

/// The cubic from an output at `a` to an input at `b` (world).
fn bezier(a: Point, b: Point) -> [Point; 4] {
    let d = ((b.x - a.x).abs() * 0.5).max(30.0);
    [a, Point::new(a.x + d, a.y), Point::new(b.x - d, b.y), b]
}

fn bezier_at(p: &[Point; 4], t: f32) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        a * p[0].x + b * p[1].x + c * p[2].x + d * p[3].x,
        a * p[0].y + b * p[1].y + c * p[2].y + d * p[3].y,
    )
}

fn bezier_bounds(p: &[Point; 4]) -> Rect {
    let x0 = p.iter().map(|q| q.x).fold(f32::MAX, f32::min);
    let y0 = p.iter().map(|q| q.y).fold(f32::MAX, f32::min);
    let x1 = p.iter().map(|q| q.x).fold(f32::MIN, f32::max);
    let y1 = p.iter().map(|q| q.y).fold(f32::MIN, f32::max);
    // Outset: a straight horizontal or vertical wire has a zero-height (or -width) box,
    // which no rect would intersect.
    Rect::from_min_max(x0, y0, x1, y1).outset(1.0)
}

/// A screen-space cubic as a polyline: about one segment per 12 px of chord, 3..=24
/// (`detail` off: a straight line — the far-out overview).
fn flatten(s: &[Point; 4], detail: bool) -> Vec<Point> {
    if !detail {
        return vec![s[0], s[3]];
    }
    let chord = dist(s[0], s[1]) + dist(s[1], s[2]) + dist(s[2], s[3]);
    let n = ((chord / 12.0) as usize).clamp(3, 24);
    (0..=n).map(|i| bezier_at(s, i as f32 / n as f32)).collect()
}

fn dist(a: Point, b: Point) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Edits the canvas's model (see [`NodeCanvas::edit`]).
pub struct CanvasEdit<'a> {
    m: &'a mut CanvasModel,
    structure: bool,
}

impl CanvasEdit<'_> {
    /// Read the model.
    pub fn model(&self) -> &CanvasModel {
        self.m
    }
    /// Add or replace a node (its wires stay if its pins still exist).
    pub fn set_node(&mut self, k: u64, mut n: CanvasNode) {
        n.relayout();
        if self.m.nodes.get(&k) != Some(&n) {
            self.m.nodes.insert(k, n);
            self.structure = true;
        }
    }
    /// Move a node.
    pub fn set_pos(&mut self, k: u64, pos: Point) {
        if let Some(n) = self.m.nodes.get_mut(&k)
            && n.pos != pos
        {
            n.pos = pos;
            self.structure = true;
        }
    }
    /// Remove a node and every wire touching it.
    pub fn remove_node(&mut self, k: u64) {
        if self.m.nodes.remove(&k).is_some() {
            self.m
                .wires
                .retain(|_, w| w.from.node != k && w.to.node != k);
            self.m.selected.remove(&k);
            self.structure = true;
        }
    }
    /// Add or replace the wire into `w.to`.
    pub fn set_wire(&mut self, w: CanvasWire) {
        if self.m.wires.get(&w.to) != Some(&w) {
            self.m.wires.insert(w.to, w);
            self.structure = true;
        }
    }
    pub fn remove_wire(&mut self, to: PinRef) {
        if self.m.wires.remove(&to).is_some() {
            if self.m.selected_wire == Some(to) {
                self.m.selected_wire = None;
            }
            self.structure = true;
        }
    }
    /// Remove every wire into `node`'s inputs.
    pub fn remove_wires_into(&mut self, node: u64) {
        // Wires are keyed by the input they feed, so a node's are one contiguous range.
        let keys: Vec<PinRef> = self
            .m
            .wires
            .range(PinRef::input(node, 0)..=PinRef::input(node, u16::MAX))
            .map(|(k, _)| *k)
            .collect();
        for k in &keys {
            self.m.wires.remove(k);
        }
        self.structure |= !keys.is_empty();
    }
    pub fn set_comment(&mut self, k: u64, c: CanvasComment) {
        if self.m.comments.get(&k) != Some(&c) {
            self.m.comments.insert(k, c);
            self.structure = true;
        }
    }
    pub fn remove_comment(&mut self, k: u64) {
        if self.m.comments.remove(&k).is_some() {
            if self.m.selected_comment == Some(k) {
                self.m.selected_comment = None;
            }
            self.structure = true;
        }
    }
    /// Everything off.
    pub fn clear(&mut self) {
        self.m.nodes.clear();
        self.m.wires.clear();
        self.m.comments.clear();
        self.m.selected.clear();
        self.m.selected_comment = None;
        self.m.selected_wire = None;
        self.structure = true;
    }
    /// Map an error onto a node (`None` clears it).
    pub fn set_error(&mut self, k: u64, e: Option<String>) {
        if let Some(n) = self.m.nodes.get_mut(&k)
            && n.error != e
        {
            n.error = e;
            self.structure = true;
        }
    }
    /// Clear every node's error.
    pub fn clear_errors(&mut self) {
        for n in self.m.nodes.values_mut() {
            if n.error.take().is_some() {
                self.structure = true;
            }
        }
    }
    pub fn set_view(&mut self, v: CanvasView) {
        self.m.view = CanvasView {
            pan: v.pan,
            zoom: v.zoom.clamp(ZOOM_MIN, ZOOM_MAX),
        };
    }
    /// Frame these nodes (all when empty).
    pub fn frame_nodes(&mut self, nodes: &[u64]) {
        let mut b: Option<Rect> = None;
        for k in nodes {
            if let Some(n) = self.m.nodes.get(k) {
                let r = n.rect();
                b = Some(b.map_or(r, |x| x.union(&r)));
            }
        }
        if nodes.is_empty() {
            self.m.recompute_bounds();
            b = self.m.bounds;
        }
        if let Some(b) = b {
            self.m.frame(b);
        }
    }
    /// Select exactly these nodes.
    pub fn select(&mut self, nodes: &[u64]) {
        self.m.selected = nodes
            .iter()
            .copied()
            .filter(|k| self.m.nodes.contains_key(k))
            .collect();
    }
}

// ---- the widget ---------------------------------------------------------------------------

/// What a paint recorded (for the D-5 guards).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CanvasStats {
    /// Nodes recorded by the last paint of the canvas.
    pub nodes_recorded: usize,
    /// Wires recorded by the last paint.
    pub wires_recorded: usize,
    /// Canvas paints so far.
    pub canvas_paints: u64,
    /// Minimap (every-node) paints so far.
    pub minimap_paints: u64,
    /// Minimap view-frame paints so far.
    pub frame_paints: u64,
}

struct Shared {
    model: RefCell<CanvasModel>,
    stats: Cell<CanvasStats>,
}

impl Shared {
    fn bump(&self, f: impl FnOnce(&mut CanvasStats)) {
        let mut s = self.stats.get();
        f(&mut s);
        self.stats.set(s);
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    None,
    Pan,
    Nodes {
        start: Point,
        offset: Point,
    },
    Wire {
        from: PinRef,
        at: Point,
        over: Option<(PinRef, Result<(), String>)>,
    },
    Marquee {
        start: Point,
        at: Point,
    },
    Comment {
        id: u64,
        start: Point,
        orig: Rect,
        resize: bool,
        members: Vec<(u64, Point)>,
        offset: Point,
    },
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Hit {
    Pin(PinRef),
    Node(u64),
    Wire(PinRef),
    CommentHeader(u64),
    CommentResize(u64),
    Empty,
}

/// See the module docs. Build it with [`NodeCanvas::build`].
pub struct NodeCanvas {
    shared: Rc<Shared>,
    label: String,
    minimap: Option<WidgetId>,
    frame: Option<WidgetId>,
    check: Option<ConnectCheck>,
    search: Option<SearchItems>,
    search_sink: Signal<Option<String>>,
    /// Where the open search adds its node, and the pin a wire was dropped from.
    pending_search: Option<(Point, Option<PinRef>)>,
    drag: Drag,
    last_pointer: Point,
    hover: Hit,
    /// A refused connection's reason, left on screen at this world point.
    refusal: Option<(Point, String)>,
    minimap_shown: bool,
    /// Fault (W2 control for `ui_graph_2k_nodes`): record every node on every paint, not
    /// only the ones in view.
    record_every_node: crate::controls::Switch,
}

impl NodeCanvas {
    /// Build the canvas (and its minimap) under `parent`; returns the canvas's id.
    pub fn build(
        ui: &mut dyn Build,
        parent: WidgetId,
        key: impl Into<Key>,
        style: NodeStyle,
        label: &str,
    ) -> Result<WidgetId, UiError> {
        Self::build_inner(
            ui,
            parent,
            key,
            style,
            label,
            crate::controls::Switch::default(),
        )
    }

    /// As [`NodeCanvas::build`], with the recording fault on or off (guards only).
    #[cfg(any(test, feature = "controls"))]
    pub fn build_with(
        ui: &mut dyn Build,
        parent: WidgetId,
        key: impl Into<Key>,
        style: NodeStyle,
        label: &str,
        record_every_node: bool,
    ) -> Result<WidgetId, UiError> {
        let sw = crate::controls::Switch {
            on: record_every_node,
        };
        Self::build_inner(ui, parent, key, style, label, sw)
    }

    fn build_inner(
        ui: &mut dyn Build,
        parent: WidgetId,
        key: impl Into<Key>,
        style: NodeStyle,
        label: &str,
        record_every_node: crate::controls::Switch,
    ) -> Result<WidgetId, UiError> {
        let shared = Rc::new(Shared {
            model: RefCell::new(CanvasModel::default()),
            stats: Cell::new(CanvasStats::default()),
        });
        let search_sink = ui.runtime().signal(None::<String>);
        // The clip keeps nodes partly out of view from drawing over the neighbours.
        let clip = ui.add(parent, key, style.clip(), Container::group())?;
        // The parts' ids follow from the canvas's (stable ids, §21.4).
        let canvas_id = clip.child(&Key::Static("canvas"));
        let minimap_id = canvas_id.child(&Key::Static("minimap"));
        let frame_id = minimap_id.child(&Key::Static("view"));
        let canvas = ui.add(
            clip,
            Key::Static("canvas"),
            NodeStyle::default().fill(),
            NodeCanvas {
                shared: shared.clone(),
                label: label.to_string(),
                minimap: Some(minimap_id),
                frame: Some(frame_id),
                check: None,
                search: None,
                search_sink,
                pending_search: None,
                drag: Drag::None,
                last_pointer: Point::new(0.0, 0.0),
                hover: Hit::Empty,
                refusal: None,
                minimap_shown: true,
                record_every_node,
            },
        )?;
        let minimap = ui.add(
            canvas,
            Key::Static("minimap"),
            NodeStyle::absolute(None, None, Some(8.0), Some(8.0)).size(MINIMAP.w, MINIMAP.h),
            Minimap {
                shared: shared.clone(),
                canvas,
            },
        )?;
        let frame = ui.add(
            minimap,
            Key::Static("view"),
            NodeStyle::cover().no_hit_test(),
            MinimapFrame { shared },
        )?;
        debug_assert_eq!((canvas, minimap, frame), (canvas_id, minimap_id, frame_id));
        Ok(canvas)
    }

    /// Change what the canvas shows. Marks the canvas for repaint; the minimap re-records
    /// only when a node, wire, comment or error changed (not for a view change).
    pub fn edit<R>(ui: &mut Ui, id: WidgetId, f: impl FnOnce(&mut CanvasEdit) -> R) -> Option<R> {
        let size = ui.rect(id).map(|r| Size::new(r.w, r.h));
        let (r, minimap, frame, structure) = {
            let w = ui.widget_mut::<NodeCanvas>(id)?;
            let mut m = w.shared.model.borrow_mut();
            if let Some(s) = size
                && s.w > 0.0
            {
                m.viewport = s;
            }
            let mut e = CanvasEdit {
                m: &mut m,
                structure: false,
            };
            let r = f(&mut e);
            let structure = e.structure;
            if structure {
                m.recompute_bounds();
            }
            (r, w.minimap, w.frame, structure)
        };
        ui.invalidate(id, Dirty::PAINT | Dirty::A11Y);
        if structure && let Some(mm) = minimap {
            ui.invalidate(mm, Dirty::PAINT);
        }
        if let Some(f) = frame {
            ui.invalidate(f, Dirty::PAINT);
        }
        Some(r)
    }

    /// Read the model.
    pub fn model(&self) -> Ref<'_, CanvasModel> {
        self.shared.model.borrow()
    }
    /// What paints recorded so far (the D-5 guard's counters).
    pub fn stats(&self) -> CanvasStats {
        self.shared.stats.get()
    }
    /// The minimap's id (a child of the canvas).
    pub fn minimap(&self) -> Option<WidgetId> {
        self.minimap
    }
    /// The reason a refused connection is showing, if any.
    pub fn refusal(&self) -> Option<&str> {
        self.refusal.as_ref().map(|(_, r)| r.as_str())
    }
    /// Check every wire before it is offered (see the module docs).
    pub fn set_connect_check(&mut self, check: ConnectCheck) {
        self.check = Some(check);
    }
    /// The node search's items.
    pub fn set_search(&mut self, items: SearchItems) {
        self.search = Some(items);
    }
    /// The popup the node search is open in, if any: `(world point, from pin)`.
    pub fn pending_search(&self) -> Option<(Point, Option<PinRef>)> {
        self.pending_search
    }

    fn to_world(&self, r: Rect, s: Point) -> Point {
        let v = self.shared.model.borrow().view;
        Point::new(
            v.pan.x + (s.x - r.x) / v.zoom,
            v.pan.y + (s.y - r.y) / v.zoom,
        )
    }

    fn check_pair(&self, a: PinRef, b: PinRef) -> Result<(PinRef, PinRef), String> {
        if a.node == b.node {
            return Err(crate::tr!("A node cannot feed itself.").into());
        }
        if a.output == b.output {
            return Err(if a.output {
                crate::tr!("Both pins are outputs: connect an output to an input.").into()
            } else {
                crate::tr!("Both pins are inputs: connect an output to an input.").into()
            });
        }
        let (from, to) = if a.output { (a, b) } else { (b, a) };
        match &self.check {
            Some(c) => c(from, to).map(|()| (from, to)),
            None => Ok((from, to)),
        }
    }

    fn hit(&self, r: Rect, pos: Point) -> Hit {
        let m = self.shared.model.borrow();
        let w = self.to_world(r, pos);
        let tol = HIT_PX / m.view.zoom;
        for (k, n) in m.nodes.iter().rev() {
            let nr = n.rect();
            if !nr.outset(tol).contains(w) {
                continue;
            }
            if m.view.zoom >= DETAIL_ZOOM || n.reroute {
                for (output, pins) in [(false, &n.inputs), (true, &n.outputs)] {
                    for i in 0..pins.len() {
                        let idx = u16::try_from(i).unwrap_or(u16::MAX);
                        if dist(n.pin_pos(output, idx), w) <= tol.max(5.0) {
                            return Hit::Pin(PinRef {
                                node: *k,
                                output,
                                index: idx,
                            });
                        }
                    }
                }
            }
            if nr.contains(w) {
                return Hit::Node(*k);
            }
        }
        let v = m.visible_world();
        for wire in m.wires.values() {
            let Some(p) = m.wire_points(wire) else {
                continue;
            };
            if !bezier_bounds(&p).outset(tol).contains(w) || !bezier_bounds(&p).intersects(&v) {
                continue;
            }
            let near = (0..=24).any(|s| dist(bezier_at(&p, s as f32 / 24.0), w) <= tol);
            if near {
                return Hit::Wire(wire.to);
            }
        }
        for (k, c) in m.comments.iter().rev() {
            let grip = Rect::new(c.rect.right() - 16.0, c.rect.bottom() - 16.0, 16.0, 16.0);
            if grip.contains(w) {
                return Hit::CommentResize(*k);
            }
            if Rect::new(c.rect.x, c.rect.y, c.rect.w, 26.0).contains(w) {
                return Hit::CommentHeader(*k);
            }
        }
        Hit::Empty
    }

    fn view_changed(&self, cx: &mut EventCx) {
        cx.request_paint();
        if let Some(f) = self.frame {
            cx.invalidate(f, Dirty::PAINT);
        }
        // Node bounds for assistive technology follow once the view settles, not at every
        // pan frame: one timer, then idle.
        cx.cancel_timer(TIMER_A11Y);
        cx.set_timer(A11Y_SETTLE, TIMER_A11Y);
    }

    fn zoom_about(&self, cx: &mut EventCx, at: Point, factor: f32) {
        let r = cx.rect();
        let before = self.to_world(r, at);
        {
            let mut m = self.shared.model.borrow_mut();
            let z = (m.view.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
            m.view.zoom = z;
            m.view.pan = Point::new(before.x - (at.x - r.x) / z, before.y - (at.y - r.y) / z);
        }
        self.view_changed(cx);
    }

    fn pan_by(&self, cx: &mut EventCx, dx: f32, dy: f32) {
        {
            let mut m = self.shared.model.borrow_mut();
            let z = m.view.zoom;
            m.view.pan = Point::new(m.view.pan.x - dx / z, m.view.pan.y - dy / z);
        }
        self.view_changed(cx);
    }

    fn selection_changed(&self, cx: &mut EventCx) {
        let nodes = self.shared.model.borrow().selected();
        let canvas = cx.id();
        cx.action(CanvasSelection { canvas, nodes });
        cx.request_paint();
        cx.request_a11y();
    }

    fn open_search(&mut self, cx: &mut EventCx, world: Point, screen: Point, from: Option<PinRef>) {
        let Some(items) = self.search.as_ref().map(|f| f(from)) else {
            return;
        };
        self.search_sink.set(cx.rt_mut(), None);
        self.pending_search = Some((world, from));
        let list = PickList::into_sink(
            items,
            crate::tr!("Add node"),
            crate::tr!("Search nodes"),
            self.search_sink,
        );
        cx.open_popup(
            Key::Static("node-search"),
            PopupSpec::menu(Anchor::At(screen)).min_width(360.0),
            list,
        );
    }

    fn nudge(&self, cx: &mut EventCx, dx: f32, dy: f32) {
        let moves: Vec<(u64, Point)> = {
            let mut m = self.shared.model.borrow_mut();
            let sel: Vec<u64> = m.selected.iter().copied().collect();
            let mut out = Vec::new();
            for k in sel {
                if let Some(n) = m.nodes.get_mut(&k) {
                    n.pos = Point::new(n.pos.x + dx, n.pos.y + dy);
                    out.push((k, n.pos));
                }
            }
            out
        };
        if !moves.is_empty() {
            let canvas = cx.id();
            cx.action(NodesMoved { canvas, moves });
            self.structure_changed(cx);
        }
    }

    fn structure_changed(&self, cx: &mut EventCx) {
        self.shared.model.borrow_mut().recompute_bounds();
        cx.request_paint();
        cx.request_a11y();
        if let Some(mm) = self.minimap {
            cx.invalidate(mm, Dirty::PAINT);
        }
        if let Some(f) = self.frame {
            cx.invalidate(f, Dirty::PAINT);
        }
    }

    /// Step the selection to the next / previous node in reading order and bring it into
    /// view (keyboard and screen-reader navigation).
    fn step_selection(&self, cx: &mut EventCx, forward: bool) {
        {
            let mut m = self.shared.model.borrow_mut();
            let mut order: Vec<(i64, i64, u64)> = m
                .nodes
                .iter()
                .map(|(k, n)| (n.pos.y.round() as i64, n.pos.x.round() as i64, *k))
                .collect();
            order.sort_unstable();
            if order.is_empty() {
                return;
            }
            let cur = m
                .selected
                .iter()
                .next()
                .and_then(|k| order.iter().position(|(_, _, o)| o == k));
            let next = match (cur, forward) {
                (None, _) => 0,
                (Some(i), true) => (i + 1) % order.len(),
                (Some(i), false) => (i + order.len() - 1) % order.len(),
            };
            let k = order[next].2;
            m.selected = BTreeSet::from([k]);
            if let Some(r) = m.nodes.get(&k).map(CanvasNode::rect)
                && !m.visible_world().contains_rect(&r)
            {
                let z = m.view.zoom;
                let vp = m.viewport;
                m.view.pan =
                    Point::new(r.center().x - vp.w * 0.5 / z, r.center().y - vp.h * 0.5 / z);
            }
        }
        self.view_changed(cx);
        self.selection_changed(cx);
    }

    fn pointer_down(
        &mut self,
        cx: &mut EventCx,
        pos: Point,
        button: PointerButton,
        clicks: u8,
    ) -> Handled {
        cx.request_focus();
        let r = cx.rect();
        let mods = cx.modifiers();
        self.last_pointer = pos;
        if self.refusal.take().is_some() {
            cx.request_paint();
        }
        if matches!(button, PointerButton::Middle | PointerButton::Secondary)
            || (button == PointerButton::Primary && mods.alt && self.hit(r, pos) == Hit::Empty)
        {
            self.drag = Drag::Pan;
            cx.capture_pointer();
            return Handled::Yes;
        }
        let w = self.to_world(r, pos);
        let canvas = cx.id();
        match self.hit(r, pos) {
            Hit::Pin(p) => {
                let wired = self.shared.model.borrow().wires.contains_key(&p);
                if mods.alt && !p.output && wired {
                    cx.action(DisconnectRequested { canvas, to: p });
                    return Handled::Yes;
                }
                self.drag = Drag::Wire {
                    from: p,
                    at: w,
                    over: None,
                };
                cx.capture_pointer();
            }
            Hit::Node(k) => {
                {
                    let mut m = self.shared.model.borrow_mut();
                    m.selected_wire = None;
                    m.selected_comment = None;
                    if mods.ctrl {
                        if !m.selected.remove(&k) {
                            m.selected.insert(k);
                        }
                    } else if mods.shift {
                        m.selected.insert(k);
                    } else if !m.selected.contains(&k) {
                        m.selected = BTreeSet::from([k]);
                    }
                }
                self.selection_changed(cx);
                self.drag = Drag::Nodes {
                    start: w,
                    offset: Point::new(0.0, 0.0),
                };
                cx.capture_pointer();
            }
            Hit::Wire(to) => {
                if clicks >= 2 {
                    cx.action(RerouteRequested { canvas, to, at: w });
                } else {
                    let mut m = self.shared.model.borrow_mut();
                    m.selected_wire = Some(to);
                    m.selected.clear();
                    m.selected_comment = None;
                }
                self.selection_changed(cx);
            }
            Hit::CommentHeader(id) | Hit::CommentResize(id) => {
                let resize = matches!(self.hit(r, pos), Hit::CommentResize(_));
                let (orig, members) = {
                    let mut m = self.shared.model.borrow_mut();
                    m.selected_comment = Some(id);
                    m.selected.clear();
                    m.selected_wire = None;
                    let orig = m.comments.get(&id).map_or(Rect::default(), |c| c.rect);
                    let members = m
                        .nodes
                        .iter()
                        .filter(|(_, n)| orig.contains_rect(&n.rect()))
                        .map(|(k, n)| (*k, n.pos))
                        .collect();
                    (orig, members)
                };
                self.selection_changed(cx);
                self.drag = Drag::Comment {
                    id,
                    start: w,
                    orig,
                    resize,
                    members: if resize { Vec::new() } else { members },
                    offset: Point::new(0.0, 0.0),
                };
                cx.capture_pointer();
            }
            Hit::Empty => {
                if clicks >= 2 {
                    self.open_search(cx, w, pos, None);
                    return Handled::Yes;
                }
                if !(mods.shift || mods.ctrl) {
                    let mut m = self.shared.model.borrow_mut();
                    m.selected.clear();
                    m.selected_comment = None;
                    m.selected_wire = None;
                }
                self.selection_changed(cx);
                self.drag = Drag::Marquee { start: w, at: w };
                cx.capture_pointer();
            }
        }
        Handled::Yes
    }

    fn pointer_move(&mut self, cx: &mut EventCx, pos: Point) -> Handled {
        let r = cx.rect();
        let delta = Point::new(pos.x - self.last_pointer.x, pos.y - self.last_pointer.y);
        self.last_pointer = pos;
        let w = self.to_world(r, pos);
        match &mut self.drag {
            Drag::None => {
                self.hover = self.hit(r, pos);
                return Handled::No;
            }
            Drag::Pan => {
                self.pan_by(cx, delta.x, delta.y);
            }
            Drag::Nodes { start, offset } => {
                *offset = Point::new(w.x - start.x, w.y - start.y);
                cx.request_paint();
            }
            Drag::Marquee { at, .. } => {
                *at = w;
                cx.request_paint();
            }
            Drag::Comment { start, offset, .. } => {
                *offset = Point::new(w.x - start.x, w.y - start.y);
                cx.request_paint();
            }
            Drag::Wire { from, .. } => {
                let from = *from;
                let over = match self.hit(r, pos) {
                    Hit::Pin(p) if p != from => Some((p, self.check_pair(from, p).map(|_| ()))),
                    _ => None,
                };
                if let Drag::Wire { at, over: o, .. } = &mut self.drag {
                    *at = w;
                    *o = over;
                }
                cx.request_paint();
            }
        }
        Handled::Yes
    }

    fn pointer_up(&mut self, cx: &mut EventCx, pos: Point) -> Handled {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        cx.release_pointer();
        let canvas = cx.id();
        let r = cx.rect();
        match drag {
            Drag::None => return Handled::No,
            Drag::Pan => {}
            Drag::Nodes { offset, .. } => {
                if offset.x != 0.0 || offset.y != 0.0 {
                    let moves: Vec<(u64, Point)> = {
                        let mut m = self.shared.model.borrow_mut();
                        let sel: Vec<u64> = m.selected.iter().copied().collect();
                        let mut out = Vec::new();
                        for k in sel {
                            if let Some(n) = m.nodes.get_mut(&k) {
                                n.pos = Point::new(n.pos.x + offset.x, n.pos.y + offset.y);
                                out.push((k, n.pos));
                            }
                        }
                        out
                    };
                    cx.action(NodesMoved { canvas, moves });
                    self.structure_changed(cx);
                }
            }
            Drag::Wire { from, at, over } => match over {
                Some((p, Ok(()))) => {
                    if let Ok((from, to)) = self.check_pair(from, p) {
                        cx.action(ConnectRequested { canvas, from, to });
                    }
                }
                Some((p, Err(reason))) => {
                    let (f, t) = if from.output { (from, p) } else { (p, from) };
                    self.refusal = Some((at, reason.clone()));
                    cx.action(ConnectRefused {
                        canvas,
                        from: f,
                        to: t,
                        reason,
                    });
                }
                None => {
                    if self.hit(r, pos) == Hit::Empty {
                        self.open_search(cx, at, pos, Some(from));
                    }
                }
            },
            Drag::Marquee { start, at } => {
                let sel = Rect::from_min_max(
                    start.x.min(at.x),
                    start.y.min(at.y),
                    start.x.max(at.x),
                    start.y.max(at.y),
                );
                if sel.w > 0.0 || sel.h > 0.0 {
                    let mut m = self.shared.model.borrow_mut();
                    let hits: Vec<u64> = m
                        .nodes
                        .iter()
                        .filter(|(_, n)| n.rect().intersects(&sel))
                        .map(|(k, _)| *k)
                        .collect();
                    m.selected.extend(hits);
                }
                self.selection_changed(cx);
            }
            Drag::Comment {
                id,
                orig,
                resize,
                members,
                offset,
                ..
            } => {
                if offset.x != 0.0 || offset.y != 0.0 {
                    let rect = if resize {
                        Rect::new(
                            orig.x,
                            orig.y,
                            (orig.w + offset.x).max(80.0),
                            (orig.h + offset.y).max(60.0),
                        )
                    } else {
                        orig.translate(offset.x, offset.y)
                    };
                    let moved: Vec<(u64, Point)> = members
                        .iter()
                        .map(|(k, p)| (*k, Point::new(p.x + offset.x, p.y + offset.y)))
                        .collect();
                    {
                        let mut m = self.shared.model.borrow_mut();
                        if let Some(c) = m.comments.get_mut(&id) {
                            c.rect = rect;
                        }
                        for (k, p) in &moved {
                            if let Some(n) = m.nodes.get_mut(k) {
                                n.pos = *p;
                            }
                        }
                    }
                    cx.action(CommentEdited {
                        canvas,
                        comment: id,
                        rect,
                        moved,
                    });
                    self.structure_changed(cx);
                }
            }
        }
        cx.request_paint();
        Handled::Yes
    }

    fn key(&mut self, cx: &mut EventCx, k: &crate::input::KeyEvent) -> Handled {
        let canvas = cx.id();
        let step = if k.mods.shift { 160.0 } else { 40.0 };
        let r = cx.rect();
        let center = r.center();
        let (sel, sel_comment, sel_wire) = {
            let m = self.shared.model.borrow();
            (m.selected(), m.selected_comment, m.selected_wire)
        };
        match k.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if k.mods.ctrl => {
                if sel.is_empty() {
                    return Handled::No;
                }
                let g = if k.mods.shift { 64.0 } else { 16.0 };
                let (dx, dy) = match k.code {
                    KeyCode::Left => (-g, 0.0),
                    KeyCode::Right => (g, 0.0),
                    KeyCode::Up => (0.0, -g),
                    _ => (0.0, g),
                };
                self.nudge(cx, dx, dy);
            }
            KeyCode::Left => self.pan_by(cx, step, 0.0),
            KeyCode::Right => self.pan_by(cx, -step, 0.0),
            KeyCode::Up => self.pan_by(cx, 0.0, step),
            KeyCode::Down => self.pan_by(cx, 0.0, -step),
            KeyCode::Char('+' | '=') if !k.mods.ctrl => self.zoom_about(cx, center, 1.25),
            KeyCode::Char('-') if !k.mods.ctrl => self.zoom_about(cx, center, 0.8),
            KeyCode::Char('0') if !k.mods.ctrl => {
                let z = self.shared.model.borrow().view.zoom;
                self.zoom_about(cx, center, 1.0 / z);
            }
            KeyCode::Char('f') if !k.mods.ctrl => {
                {
                    let mut m = self.shared.model.borrow_mut();
                    let mut e = CanvasEdit {
                        m: &mut m,
                        structure: false,
                    };
                    e.frame_nodes(&sel);
                }
                self.view_changed(cx);
            }
            KeyCode::Char('a') if k.mods.ctrl => {
                {
                    let mut m = self.shared.model.borrow_mut();
                    m.selected = m.nodes.keys().copied().collect();
                }
                self.selection_changed(cx);
            }
            KeyCode::Char(c @ ('c' | 'x' | 'v' | 'd')) if k.mods.ctrl => {
                let op = match c {
                    'c' => ClipOp::Copy,
                    'x' => ClipOp::Cut,
                    'v' => ClipOp::Paste,
                    _ => ClipOp::Duplicate,
                };
                let at = self.to_world(r, self.last_pointer);
                let at = if r.contains(self.last_pointer) {
                    at
                } else {
                    self.to_world(r, center)
                };
                cx.action(ClipboardRequested {
                    canvas,
                    op,
                    nodes: sel,
                    comments: sel_comment.into_iter().collect(),
                    at,
                });
            }
            KeyCode::Char('c') if !k.mods.ctrl && !k.mods.alt => {
                if sel.is_empty() {
                    return Handled::No;
                }
                let m = self.shared.model.borrow();
                let mut b: Option<Rect> = None;
                for n in sel.iter().filter_map(|k| m.nodes.get(k)) {
                    let nr = n.rect();
                    b = Some(b.map_or(nr, |x| x.union(&nr)));
                }
                drop(m);
                if let Some(b) = b {
                    let rect = Rect::new(b.x - 20.0, b.y - 44.0, b.w + 40.0, b.h + 64.0);
                    cx.action(CommentRequested {
                        canvas,
                        rect,
                        nodes: sel,
                    });
                }
            }
            KeyCode::Char('m') if !k.mods.ctrl => {
                if let Some(mm) = self.minimap {
                    self.minimap_shown = !self.minimap_shown;
                    cx.set_hidden(mm, !self.minimap_shown);
                }
            }
            KeyCode::Char(']') => self.step_selection(cx, true),
            KeyCode::Char('[') => self.step_selection(cx, false),
            KeyCode::Space => {
                let at = self.to_world(r, center);
                self.open_search(cx, at, center, None);
            }
            KeyCode::Delete | KeyCode::Backspace => {
                if sel.is_empty() && sel_comment.is_none() && sel_wire.is_none() {
                    return Handled::No;
                }
                cx.action(DeleteRequested {
                    canvas,
                    nodes: sel,
                    comments: sel_comment.into_iter().collect(),
                    wires: sel_wire.into_iter().collect(),
                });
            }
            KeyCode::Escape => {
                if self.drag != Drag::None {
                    self.drag = Drag::None;
                    cx.release_pointer();
                    cx.request_paint();
                } else if !sel.is_empty() || sel_comment.is_some() || sel_wire.is_some() {
                    {
                        let mut m = self.shared.model.borrow_mut();
                        m.selected.clear();
                        m.selected_comment = None;
                        m.selected_wire = None;
                    }
                    self.selection_changed(cx);
                } else {
                    return Handled::No;
                }
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }
}

impl Widget for NodeCanvas {
    fn role(&self) -> Role {
        Role::ListBox
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.search_sink.any()), Dirty::NONE);
    }
    fn focusable(&self) -> bool {
        true
    }
    fn hover_sensitive(&self) -> bool {
        false
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
        Size::new(known.width.unwrap_or(640.0), known.height.unwrap_or(400.0))
    }
    fn tooltip(&self, _rt: &Runtime) -> Option<String> {
        let m = self.shared.model.borrow();
        match self.hover {
            Hit::Pin(p) => {
                if let Some((_, reason)) = &self.refusal {
                    return Some(reason.clone());
                }
                let n = m.nodes.get(&p.node)?;
                let pin = n.pin(p.output, p.index)?;
                Some(if pin.label.is_empty() {
                    pin.detail.clone()
                } else {
                    format!("{}: {}", pin.label, pin.detail)
                })
            }
            Hit::Node(k) => {
                let n = m.nodes.get(&k)?;
                match &n.error {
                    Some(e) => Some(format!("{}: {e}", n.title)),
                    None if !n.doc.is_empty() => Some(format!("{}: {}", n.title, n.doc)),
                    None => Some(n.title.clone()),
                }
            }
            Hit::Wire(_) => Some(
                crate::tr!("Double-click to add a reroute; select and Delete to remove").into(),
            ),
            _ => self.refusal.as_ref().map(|(_, r)| r.clone()),
        }
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::PointerDown {
                pos,
                button,
                clicks,
            } => self.pointer_down(cx, *pos, *button, *clicks),
            UiEvent::PointerMove { pos } => self.pointer_move(cx, *pos),
            UiEvent::PointerUp { pos, .. } => self.pointer_up(cx, *pos),
            UiEvent::Wheel { dx, dy } => {
                let r = cx.rect();
                let at = cx
                    .pointer_pos()
                    .filter(|p| r.contains(*p))
                    .unwrap_or(r.center());
                if cx.modifiers().shift {
                    self.pan_by(cx, *dy * 40.0, 0.0);
                } else {
                    if *dx != 0.0 {
                        self.pan_by(cx, *dx * 40.0, 0.0);
                    }
                    if *dy != 0.0 {
                        self.zoom_about(cx, at, 1.1f32.powf(dy.clamp(-3.0, 3.0)));
                    }
                }
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => self.key(cx, k),
            UiEvent::Timer(TIMER_A11Y) => {
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::BindingChanged => {
                if let Some(op) = self.search_sink.get(cx.rt()) {
                    self.search_sink.set(cx.rt_mut(), None);
                    if let Some((at, from)) = self.pending_search.take() {
                        let canvas = cx.id();
                        cx.action(NodeChosen {
                            canvas,
                            op,
                            at,
                            from,
                        });
                    }
                }
                Handled::Yes
            }
            UiEvent::A11yChildAction(k, Action::Click | Action::Focus) => {
                {
                    let mut m = self.shared.model.borrow_mut();
                    if m.nodes.contains_key(k) {
                        m.selected = BTreeSet::from([*k]);
                    }
                }
                self.selection_changed(cx);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let mut m = self.shared.model.borrow_mut();
        if r.w > 0.0 {
            m.viewport = Size::new(r.w, r.h);
        }
        let m = &*m;
        let z = m.view.zoom;
        let pan = m.view.pan;
        let sx = |p: Point| Point::new(r.x + (p.x - pan.x) * z, r.y + (p.y - pan.y) * z);
        let vis = Rect::new(pan.x, pan.y, r.w / z, r.h / z);
        cx.fill(r, ColorRole::BgSunken, 0.0);
        // Grid: every 32 world units, coarser as the view zooms out (≥ 12 px apart).
        let mut g = 32.0;
        while g * z < 12.0 {
            g *= 4.0;
        }
        let mut x = (pan.x / g).ceil() * g;
        while x < vis.right() {
            cx.mark_alpha(
                Rect::new(sx(Point::new(x, 0.0)).x, r.y, 1.0, r.h),
                ColorRole::Border,
                ColorRole::BgSunken,
                0.0,
                0.35,
            );
            x += g;
        }
        let mut y = (pan.y / g).ceil() * g;
        while y < vis.bottom() {
            cx.mark_alpha(
                Rect::new(r.x, sx(Point::new(0.0, y)).y, r.w, 1.0),
                ColorRole::Border,
                ColorRole::BgSunken,
                0.0,
                0.35,
            );
            y += g;
        }
        let detail = z >= DETAIL_ZOOM;
        let q = |s: f32| (s * z).round();
        let title_style = TextStyle::body(q(13.0)).strong();
        let label_style = TextStyle::body(q(11.0));
        // Comments, behind everything.
        for (k, c) in &m.comments {
            if !c.rect.intersects(&vis) && !self.record_every_node.on() {
                continue;
            }
            let mut cr = c.rect;
            if let Drag::Comment {
                id, offset, resize, ..
            } = &self.drag
                && id == k
            {
                cr = if *resize {
                    Rect::new(
                        cr.x,
                        cr.y,
                        (cr.w + offset.x).max(80.0),
                        (cr.h + offset.y).max(60.0),
                    )
                } else {
                    cr.translate(offset.x, offset.y)
                };
            }
            let tl = sx(Point::new(cr.x, cr.y));
            let sr = Rect::new(tl.x, tl.y, cr.w * z, cr.h * z);
            let sel = m.selected_comment == Some(*k);
            cx.tint(sr, ColorRole::BgRaised, 6.0 * z, 0.55);
            cx.panel(
                sr,
                None,
                Some(if sel {
                    ColorRole::Accent
                } else {
                    ColorRole::Border
                }),
                6.0 * z,
                None,
            );
            if detail {
                cx.text(
                    &c.text,
                    &title_style,
                    Point::new(sr.x + 8.0 * z, sr.y + 4.0 * z),
                    ColorRole::FgPrimary,
                );
                cx.mark_alpha(
                    Rect::new(
                        sr.right() - 12.0 * z,
                        sr.bottom() - 12.0 * z,
                        8.0 * z,
                        8.0 * z,
                    ),
                    ColorRole::FgMuted,
                    ColorRole::BgSunken,
                    1.0,
                    0.8,
                );
            }
        }
        // Where a node is drawn this frame (a node drag previews its offset).
        let node_offset = |k: u64| -> Point {
            match &self.drag {
                Drag::Nodes { offset, .. } if m.selected.contains(&k) => *offset,
                Drag::Comment {
                    members, offset, ..
                } if members.iter().any(|(mk, _)| *mk == k) => *offset,
                _ => Point::new(0.0, 0.0),
            }
        };
        let pin_at = |k: u64, output: bool, index: u16| -> Option<Point> {
            let n = m.nodes.get(&k)?;
            let p = n.pin_pos(output, index);
            let o = node_offset(k);
            Some(Point::new(p.x + o.x, p.y + o.y))
        };
        // Wires: one polyline mesh per colour, only those whose curve can reach the view,
        // each flattened to a few segments by its length on screen.
        let mut paths: Vec<(ColorRole, Vec<Vec<Point>>)> = Vec::new();
        let mut wires = 0usize;
        let mut selected_wire: Option<[Point; 4]> = None;
        for w in m.wires.values() {
            let (Some(a), Some(b)) = (
                pin_at(w.from.node, true, w.from.index),
                pin_at(w.to.node, false, w.to.index),
            ) else {
                continue;
            };
            let p = bezier(a, b);
            if !self.record_every_node.on() && !bezier_bounds(&p).intersects(&vis) {
                continue;
            }
            wires += 1;
            let s = [sx(p[0]), sx(p[1]), sx(p[2]), sx(p[3])];
            if m.selected_wire == Some(w.to) {
                selected_wire = Some(s);
            }
            let i = match paths.iter().position(|(role, _)| *role == w.role) {
                Some(i) => i,
                None => {
                    paths.push((w.role, Vec::new()));
                    paths.len() - 1
                }
            };
            paths[i].1.push(flatten(&s, detail));
        }
        let width = (2.0 * z).clamp(1.0, 3.0);
        for (role, lines) in &paths {
            cx.stroke_polylines(lines, width, *role, ColorRole::BgSunken);
        }
        if let Some(s) = selected_wire {
            cx.stroke_polylines(
                &[flatten(&s, true)],
                width + 2.0,
                ColorRole::FocusRing,
                ColorRole::BgSunken,
            );
        }
        // Nodes: only those in view (the D-5 property), detail by zoom.
        let mut recorded = 0usize;
        let radius = 6.0 * z;
        let focused = cx.state().focused;
        for (k, n) in &m.nodes {
            let o = node_offset(*k);
            let wr = n.rect().translate(o.x, o.y);
            if !self.record_every_node.on() && !wr.intersects(&vis) {
                continue;
            }
            recorded += 1;
            let tl = sx(Point::new(wr.x, wr.y));
            let sr = Rect::new(tl.x, tl.y, wr.w * z, wr.h * z);
            let sel = m.selected.contains(k);
            let border = if n.error.is_some() {
                ColorRole::Danger
            } else if sel {
                ColorRole::Accent
            } else {
                ColorRole::Border
            };
            if n.reroute {
                let role = n.inputs.first().map_or(ColorRole::FgMuted, |p| p.role);
                cx.mark(sr, role, ColorRole::BgSunken, sr.w * 0.5);
                if sel {
                    cx.panel(sr.outset(2.0), None, Some(ColorRole::Accent), sr.w, None);
                }
                continue;
            }
            if !detail {
                cx.mark(sr, border, ColorRole::BgSunken, radius.min(3.0));
                continue;
            }
            cx.panel(sr, Some(ColorRole::BgRaised), Some(border), radius, None);
            if sel && focused {
                cx.panel(
                    sr.outset(3.0),
                    None,
                    Some(ColorRole::FocusRing),
                    radius + 3.0,
                    None,
                );
            }
            cx.mark(
                Rect::new(sr.x, sr.y + HEADER_H * z - 1.0, sr.w, 1.0),
                ColorRole::Border,
                ColorRole::BgRaised,
                0.0,
            );
            if title_style.size >= 7.0 {
                cx.text(
                    &n.title,
                    &title_style,
                    Point::new(sr.x + 8.0 * z, sr.y + 5.0 * z),
                    ColorRole::FgPrimary,
                );
            }
            let pr = (5.0 * z).max(2.0);
            for (output, pins) in [(false, &n.inputs), (true, &n.outputs)] {
                for (i, p) in pins.iter().enumerate() {
                    let idx = u16::try_from(i).unwrap_or(u16::MAX);
                    let c = sx(Point::new(
                        n.pin_pos(output, idx).x + o.x,
                        n.pin_pos(output, idx).y + o.y,
                    ));
                    let wired = if output {
                        true
                    } else {
                        m.wires.contains_key(&PinRef::input(*k, idx))
                    };
                    let pin_r = Rect::new(c.x - pr, c.y - pr, pr * 2.0, pr * 2.0);
                    if wired {
                        cx.mark(pin_r, p.role, ColorRole::BgRaised, pr);
                    } else {
                        cx.panel(pin_r, None, Some(p.role), pr, None);
                    }
                    if z >= LABEL_ZOOM && label_style.size >= 7.0 && !p.label.is_empty() {
                        let lw = p.label.chars().count() as f32 * 6.5 * z;
                        let x = if output {
                            c.x - pr - 6.0 * z - lw
                        } else {
                            c.x + pr + 6.0 * z
                        };
                        cx.text(
                            &p.label,
                            &label_style,
                            Point::new(x, c.y - 8.0 * z),
                            ColorRole::FgMuted,
                        );
                    }
                }
            }
            if let Some(e) = &n.error
                && z >= LABEL_ZOOM
            {
                let st = TextStyle::body(q(11.0).max(9.0));
                let at = Point::new(sr.x, sr.bottom() + 4.0);
                let text = format!("\u{26a0} {e}");
                let size = Size::new(
                    (text.chars().count() as f32 * st.size * 0.55 + 12.0).min(420.0),
                    st.size * 1.4 + 6.0,
                );
                cx.panel(
                    Rect::new(at.x, at.y, size.w, size.h),
                    Some(ColorRole::BgRaised),
                    Some(ColorRole::Danger),
                    4.0,
                    None,
                );
                cx.text(
                    &text,
                    &st,
                    Point::new(at.x + 6.0, at.y + 3.0),
                    ColorRole::FgPrimary,
                );
            }
        }
        // A wire being dragged, and its verdict next to the pin under it.
        if let Drag::Wire { from, at, over } = &self.drag
            && let Some(a) = pin_at(from.node, from.output, from.index)
        {
            let (s, e) = if from.output { (a, *at) } else { (*at, a) };
            let p = bezier(s, e);
            let role = match over {
                Some((_, Err(_))) => ColorRole::Danger,
                _ => ColorRole::Accent,
            };
            cx.stroke_polylines(
                &[flatten(&[sx(p[0]), sx(p[1]), sx(p[2]), sx(p[3])], true)],
                width,
                role,
                ColorRole::BgSunken,
            );
            if let Some((_, Err(reason))) = over {
                bubble(cx, sx(*at), reason);
            }
        }
        if let Some((at, reason)) = &self.refusal {
            bubble(cx, sx(*at), reason);
        }
        if let Drag::Marquee { start, at } = &self.drag {
            let a = sx(*start);
            let b = sx(*at);
            let mr = Rect::from_min_max(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y));
            cx.panel(mr, None, Some(ColorRole::Accent), 0.0, None);
        }
        self.shared.bump(|s| {
            s.nodes_recorded = recorded;
            s.wires_recorded = wires;
            s.canvas_paints += 1;
        });
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_description(crate::tr!(
            "Arrows pan; + and - zoom; F frames; [ and ] step through nodes; Space adds a node; \
             Ctrl+arrows move the selection; Delete removes it; C adds a comment"
        ));
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let m = self.shared.model.borrow();
        let vis = m.visible_world();
        let z = m.view.zoom;
        let pan = m.view.pan;
        let r = cx.rect;
        for (k, n) in m.nodes.iter().filter(|(_, n)| n.rect().intersects(&vis)) {
            let mut node = accesskit::Node::new(Role::ListBoxOption);
            let ins: Vec<&str> = n.inputs.iter().map(|p| p.label.as_str()).collect();
            let outs: Vec<&str> = n.outputs.iter().map(|p| p.label.as_str()).collect();
            let mut label = crate::trf!(
                "{title}: inputs {inputs}; outputs {outputs}",
                title = n.title,
                inputs = if ins.is_empty() {
                    crate::tr!("none").to_string()
                } else {
                    ins.join(", ")
                },
                outputs = if outs.is_empty() {
                    crate::tr!("none").to_string()
                } else {
                    outs.join(", ")
                }
            );
            if let Some(e) = &n.error {
                label.push_str(&crate::trf!("; error: {e}", e));
            }
            node.set_label(label);
            node.set_selected(m.selected.contains(k));
            let nr = n.rect();
            node.set_bounds(cx.bounds(Rect::new(
                r.x + (nr.x - pan.x) * z,
                r.y + (nr.y - pan.y) * z,
                nr.w * z,
                nr.h * z,
            )));
            node.add_action(Action::Click);
            out.push((*k, node));
        }
    }
    fn a11y_focus(&self, _rt: &Runtime) -> Option<u64> {
        self.shared.model.borrow().selected.iter().next().copied()
    }
}

/// A reason next to the pointer (a refused connection).
fn bubble(cx: &mut PaintCx, at: Point, reason: &str) {
    let st = TextStyle::body(12.0);
    let w = (reason.chars().count() as f32 * 6.6 + 16.0).min(460.0);
    let r = Rect::new(at.x + 12.0, at.y + 10.0, w, 24.0);
    cx.panel(
        r,
        Some(ColorRole::BgRaised),
        Some(ColorRole::Danger),
        4.0,
        None,
    );
    cx.text(
        reason,
        &st,
        Point::new(r.x + 8.0, r.y + 4.0),
        ColorRole::FgPrimary,
    );
}

/// Every node as a dot: re-recorded when the graph changes, never on a pan.
struct Minimap {
    shared: Rc<Shared>,
    canvas: WidgetId,
}

impl Minimap {
    fn to_world(&self, r: Rect, p: Point) -> Option<Point> {
        let m = self.shared.model.borrow();
        let b = m.bounds?.outset(40.0);
        let s = (r.w / b.w).min(r.h / b.h);
        Some(Point::new(b.x + (p.x - r.x) / s, b.y + (p.y - r.y) / s))
    }
    fn center_on(&self, cx: &mut EventCx, pos: Point) {
        let Some(w) = self.to_world(cx.rect(), pos) else {
            return;
        };
        {
            let mut m = self.shared.model.borrow_mut();
            let z = m.view.zoom;
            let vp = m.viewport;
            m.view.pan = Point::new(w.x - vp.w * 0.5 / z, w.y - vp.h * 0.5 / z);
        }
        cx.invalidate(self.canvas, Dirty::PAINT | Dirty::A11Y);
        for c in cx.children_of(cx.id()) {
            cx.invalidate(c, Dirty::PAINT);
        }
    }
}

impl Widget for Minimap {
    fn role(&self) -> Role {
        Role::Image
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                cx.capture_pointer();
                self.center_on(cx, *pos);
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if cx.state().pressed => {
                self.center_on(cx, *pos);
                Handled::Yes
            }
            UiEvent::PointerUp { .. } => {
                cx.release_pointer();
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.panel(
            r,
            Some(ColorRole::BgRaised),
            Some(ColorRole::Border),
            4.0,
            None,
        );
        let m = self.shared.model.borrow();
        if let Some(b) = m.bounds {
            let b = b.outset(40.0);
            let s = (r.w / b.w).min(r.h / b.h);
            for c in m.comments.values() {
                let cr = Rect::new(
                    r.x + (c.rect.x - b.x) * s,
                    r.y + (c.rect.y - b.y) * s,
                    c.rect.w * s,
                    c.rect.h * s,
                );
                cx.mark_alpha(cr, ColorRole::FgMuted, ColorRole::BgRaised, 1.0, 0.3);
            }
            for n in m.nodes.values() {
                let nr = n.rect();
                let d = Rect::new(
                    r.x + (nr.x - b.x) * s,
                    r.y + (nr.y - b.y) * s,
                    (nr.w * s).max(2.0),
                    (nr.h * s).max(2.0),
                );
                let role = if n.error.is_some() {
                    ColorRole::Danger
                } else {
                    ColorRole::FgMuted
                };
                cx.mark(d, role, ColorRole::BgRaised, 0.0);
            }
        }
        drop(m);
        self.shared.bump(|s| s.minimap_paints += 1);
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Graph overview"));
    }
}

/// The view's outline on the minimap: one quad, re-recorded on every pan.
struct MinimapFrame {
    shared: Rc<Shared>,
}

impl Widget for MinimapFrame {
    fn role(&self) -> Role {
        Role::GenericContainer
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let m = self.shared.model.borrow();
        if let Some(b) = m.bounds {
            let b = b.outset(40.0);
            let s = (r.w / b.w).min(r.h / b.h);
            let v = m.visible_world();
            let vr = Rect::new(
                r.x + (v.x - b.x) * s,
                r.y + (v.y - b.y) * s,
                v.w * s,
                v.h * s,
            )
            .intersect(&r);
            if !vr.is_empty() {
                cx.panel(vr, None, Some(ColorRole::Accent), 0.0, None);
            }
        }
        drop(m);
        self.shared.bump(|s| s.frame_paints += 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_sizes_itself_from_its_labels_without_shaping() {
        let n = CanvasNode::new("Kinetic energy", Point::new(10.0, 20.0))
            .with_inputs(vec![
                CanvasPin::new("mass", "f64 · kg", ColorRole::Accent),
                CanvasPin::new("speed", "f64 · m/s", ColorRole::Accent),
            ])
            .with_outputs(vec![CanvasPin::new("return", "f64 · J", ColorRole::Accent)]);
        assert!(n.size().w >= NODE_MIN_W);
        assert_eq!(n.size().h, HEADER_H + 2.0 * PIN_ROW_H + 6.0);
        let p = n.pin_pos(false, 1);
        assert_eq!(p.x, 10.0);
        assert_eq!(p.y, 20.0 + HEADER_H + PIN_ROW_H * 1.5);
        assert_eq!(n.pin_pos(true, 0).x, 10.0 + n.size().w);
        let rr = CanvasNode::reroute(Point::new(0.0, 0.0), ColorRole::Accent, "f64");
        assert_eq!(rr.pin_pos(false, 0), rr.pin_pos(true, 0));
    }

    #[test]
    fn the_bezier_passes_through_its_ends() {
        let p = bezier(Point::new(0.0, 0.0), Point::new(100.0, 50.0));
        assert_eq!(bezier_at(&p, 0.0), Point::new(0.0, 0.0));
        let e = bezier_at(&p, 1.0);
        assert!((e.x - 100.0).abs() < 1e-4 && (e.y - 50.0).abs() < 1e-4);
        assert!(bezier_bounds(&p).contains(bezier_at(&p, 0.5)));
    }

    #[test]
    fn framing_fits_the_nodes_in_the_viewport() {
        let mut m = CanvasModel {
            viewport: Size::new(800.0, 600.0),
            ..CanvasModel::default()
        };
        m.frame(Rect::new(1000.0, 1000.0, 400.0, 300.0));
        let v = m.visible_world();
        assert!(
            v.contains_rect(&Rect::new(1000.0, 1000.0, 400.0, 300.0)),
            "{v:?}"
        );
    }
}
