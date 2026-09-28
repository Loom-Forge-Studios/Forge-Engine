//! The dock area widget: tab strips, splitter handles, panel frames and drop previews over
//! one dock tree (Ch.21 §21.17).
//!
//! * **Flat and keyed.** Every panel's frame is a direct child of the dock area keyed by its
//!   panel id, positioned absolutely from the tree's geometry ([`super::geometry`]). Moving
//!   a panel to another group, splitting, resizing or maximising only restyles and
//!   shows/hides children: **no panel is rebuilt**, so its scroll position, selection and
//!   focus survive a re-dock, and a window resize reflows in `taffy` with no widget code.
//! * **Requests, not mutations.** Every interaction raises a [`DockRequest`]. The handler
//!   [`dock_area`] installs applies the ones local to this area to the area's
//!   `Signal<DockTree>`; a tear-off ([`DockOp::Float`]) bubbles on to the application, which
//!   owns the multi-window [`super::Layout`] ([`super::DockController`]). After every change
//!   the area raises [`DockChanged`] so the application can persist the layout.
//! * **Drag to dock.** Dragging a tab starts a typed `Panel` drag. While it hovers the area,
//!   the drop target under the pointer (a tab position, a side of a group, an edge of the
//!   area) is previewed; releasing drops it there. Releasing outside every dock area tears
//!   the panel off into a floating window at that screen position.
//! * **Keyboard.** A tab strip is one tab stop: Left/Right/Home/End switch tabs,
//!   Ctrl+Shift+Left/Right reorder, Delete or Ctrl+W close, Shift+Space maximises. A
//!   splitter is a tab stop: arrows move it (Shift: ten times further). Shift+Space
//!   anywhere inside a panel maximises that panel; Escape restores.

use std::collections::BTreeMap;
use std::sync::Arc;

use accesskit::{Action, Role};
use taffy::prelude::{length, percent};

use super::geometry::{
    DropHit, Frac, Geometry, GroupGeom, HANDLE, HandleGeom, STRIP_H, abs_style, drop_at, geometry,
};
use super::model::{AreaId, Axis, DockNode, DropTarget, LAYOUT_VERSION, Layout, PanelId};
use crate::UiError;
use crate::damage::Dirty;
use crate::dnd::{DragPayload, DropVerdict};
use crate::geom::{Corners, Point, Rect, Size};
use crate::id::{Key, WidgetId};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::layout::{FocusScope, NodeStyle};
use crate::render::Primitive;
use crate::state::Signal;
use crate::style::ColorRole;
use crate::text::TextStyle;
use crate::ui::Ui;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};
use crate::widgets::{Build, Container, EmptyState};

/// What one dock area shows: its tree and, if the maximised panel is in it, that panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DockTree {
    pub root: DockNode,
    pub maximised: Option<PanelId>,
}

impl Default for DockNode {
    fn default() -> Self {
        DockNode::empty()
    }
}

impl DockTree {
    pub fn new(root: DockNode) -> Self {
        Self {
            root,
            maximised: None,
        }
    }
}

/// One dock operation.
#[derive(Clone, Debug, PartialEq)]
pub enum DockOp {
    /// Show this panel's tab.
    Activate(PanelId),
    /// Close the panel (the Window menu or the palette reopens it).
    Close(PanelId),
    /// Move the panel to this index within its tab group.
    Reorder(PanelId, usize),
    /// Maximise the panel, or restore it.
    ToggleMaximise(PanelId),
    /// Move the panel to a drop target in this area.
    Move { panel: PanelId, target: DropTarget },
    /// Set the ratios of the split at `path` (a splitter drag).
    Resize { path: Vec<usize>, ratios: Vec<f32> },
    /// Tear the panel off into a floating window at `rect` (desktop logical px) on
    /// `monitor`. Handled by the application (it opens the OS window).
    Float {
        panel: PanelId,
        rect: Rect,
        monitor: Option<String>,
    },
}

/// Action: a dock interaction in `area`.
#[derive(Clone, Debug, PartialEq)]
pub struct DockRequest {
    pub area: AreaId,
    pub op: DockOp,
}

/// Action: the tree of `area` changed (persist the layout: debounce, write-then-rename).
#[derive(Clone, Debug, PartialEq)]
pub struct DockChanged {
    pub area: AreaId,
    pub tree: DockTree,
}

/// Apply `op` to a single area's tree. `None` if the op changes nothing or is not local
/// (a [`DockOp::Float`] needs the application).
pub fn apply_local(tree: &DockTree, op: &DockOp) -> Option<DockTree> {
    let mut l = Layout {
        version: LAYOUT_VERSION,
        main: tree.root.clone(),
        floating: Vec::new(),
        maximised: tree.maximised.clone(),
    };
    let ok = match op {
        DockOp::Activate(p) => l.activate(p),
        DockOp::Close(p) => l.close(p),
        DockOp::Reorder(p, i) => l.reorder(p, *i),
        DockOp::ToggleMaximise(p) => l.toggle_maximise(p),
        DockOp::Move { panel, target } => l
            .move_panel(panel, &target.clone().in_area(AreaId::Main))
            .is_ok(),
        DockOp::Resize { path, ratios } => l.set_ratios(AreaId::Main, path, ratios.clone()).is_ok(),
        DockOp::Float { .. } => false,
    };
    let out = DockTree {
        root: l.main,
        maximised: l.maximised,
    };
    (ok && out != *tree).then_some(out)
}

/// What the dock needs from the application about panels: titles and content.
pub trait PanelHost {
    /// The tab title, or `None` for a panel the host does not know (its plugin is
    /// disabled): the tab then shows the id and the frame a placeholder.
    fn title(&self, panel: &PanelId) -> Option<String>;
    /// Build the panel's content under `parent` (a flex column that fills the panel's
    /// frame). `Ok(false)`: the host has no such panel; the dock shows a placeholder.
    fn build(
        &mut self,
        b: &mut dyn Build,
        parent: WidgetId,
        panel: &PanelId,
    ) -> Result<bool, UiError>;
    /// Why a panel is unavailable (shown in its placeholder).
    fn missing_reason(&self, panel: &PanelId) -> String {
        crate::trf!(
            "\u{26a0} The panel \u{201c}{panel}\u{201d} is not available: the plugin that provides it is not loaded. \
             Its place in the layout is kept and it comes back when the plugin is enabled.",
            panel
        )
    }
}

fn frame_key(p: &PanelId, rebuilds: u64) -> Key {
    if rebuilds == 0 {
        Key::Str(Arc::from(format!("panel:{p}")))
    } else {
        Key::Str(Arc::from(format!("panel:{p}:{rebuilds}")))
    }
}

/// The id a panel's frame has inside the dock area `dock` (stable wherever it is docked).
pub fn panel_frame_id(dock: WidgetId, panel: &PanelId) -> WidgetId {
    dock.child(&frame_key(panel, 0))
}

/// Add a dock area over `tree` under `parent` (it fills `style`'s box; its children are
/// positioned inside it). Installs the handler that applies local [`DockRequest`]s.
pub fn dock_area(
    ui: &mut Ui,
    parent: WidgetId,
    key: impl Into<Key>,
    style: NodeStyle,
    area: AreaId,
    tree: Signal<DockTree>,
    host: Box<dyn PanelHost>,
) -> Result<WidgetId, UiError> {
    let id = ui.add(
        parent,
        key,
        style.clip(),
        DockArea {
            area,
            tree,
            host,
            geom: Geometry::default(),
            frames: BTreeMap::new(),
            strips: Vec::new(),
            handles: Vec::new(),
            preview: None,
            last: None,
            hit: None,
            faults: DockFaults::default(),
            rebuilds: 0,
            placed: BTreeMap::new(),
        },
    )?;
    ui.on_action::<DockRequest>(id, move |cx, req| {
        if req.area != area {
            return false;
        }
        let cur = tree.get(cx.rt);
        match apply_local(&cur, &req.op) {
            Some(next) => {
                tree.set(cx.rt, next);
                true
            }
            // Float needs the application; a no-op is simply consumed.
            None => !matches!(req.op, DockOp::Float { .. }),
        }
    });
    ui.send(id, &UiEvent::BindingChanged);
    Ok(id)
}

forge_trace::control_switches! {
    /// Fault switches for the drag-dock tests' positive controls. All off in production.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct DockFaults {
        /// Re-dock by rebuilding every panel (what a naive dock does): panel state is lost.
        pub rebuild_panels_on_change: bool,
        /// Ignore drops (the drag-to-dock interaction does nothing).
        pub ignore_drops: bool,
    }
}

/// The dock area widget (see the module docs).
pub struct DockArea {
    area: AreaId,
    tree: Signal<DockTree>,
    host: Box<dyn PanelHost>,
    geom: Geometry,
    frames: BTreeMap<PanelId, WidgetId>,
    strips: Vec<WidgetId>,
    handles: Vec<WidgetId>,
    preview: Option<WidgetId>,
    last: Option<DockTree>,
    hit: Option<DropHit>,
    pub faults: DockFaults,
    rebuilds: u64,
    /// The placement last applied to each child, so a reconcile restyles only what moved
    /// (a tab switch or a splitter drag does not relayout every panel).
    placed: BTreeMap<WidgetId, Placed>,
}

/// A child's applied placement.
#[derive(Clone, Debug, PartialEq)]
enum Placed {
    Box(Frac, [f32; 4], Option<f32>),
    Handle(HandleGeom),
}

impl DockArea {
    /// The area this widget shows.
    pub fn area(&self) -> AreaId {
        self.area
    }
    /// The current geometry (groups and handles).
    pub fn geometry(&self) -> &Geometry {
        &self.geom
    }
    /// Panel frames by panel.
    pub fn frames(&self) -> &BTreeMap<PanelId, WidgetId> {
        &self.frames
    }
    /// Tab strip widgets, one per group (in geometry order).
    pub fn strips(&self) -> &[WidgetId] {
        &self.strips
    }
    /// Splitter handle widgets, one per handle (in geometry order).
    pub fn handles(&self) -> &[WidgetId] {
        &self.handles
    }
    /// The drop preview widget.
    pub fn preview(&self) -> Option<WidgetId> {
        self.preview
    }
    /// The drop target currently previewed, if a panel drag hovers the area.
    pub fn pending_drop(&self) -> Option<&DropHit> {
        self.hit.as_ref()
    }

    fn title(&self, p: &PanelId) -> String {
        self.host.title(p).unwrap_or_else(|| p.to_string())
    }

    /// Bring the children in line with the tree (see the module docs).
    fn reconcile(&mut self, cx: &mut EventCx) {
        let t = self.tree.get(cx.rt());
        self.geom = geometry(&t.root, t.maximised.as_ref());
        let me = cx.id();
        let theme_bg = ColorRole::BgBase;
        if self.faults.rebuild_panels_on_change() {
            for (_, f) in std::mem::take(&mut self.frames) {
                cx.remove_widget(f);
            }
            self.rebuilds += 1;
        }
        // Panel frames: one per panel in the tree, kept across re-docks.
        let wanted: Vec<(PanelId, usize, bool)> = self
            .geom
            .groups
            .iter()
            .enumerate()
            .flat_map(|(gi, g)| {
                let active = g.active_panel().cloned();
                g.panels
                    .iter()
                    .map(move |p| (p.clone(), gi, Some(p) == active.as_ref()))
                    .collect::<Vec<_>>()
            })
            .collect();
        // Panels hidden by maximise are still in the tree: keep their frames, hidden.
        let all = t.root.panels();
        let stale: Vec<PanelId> = self
            .frames
            .keys()
            .filter(|p| !all.contains(p))
            .cloned()
            .collect();
        for p in stale {
            if let Some(f) = self.frames.remove(&p) {
                cx.remove_widget(f);
            }
        }
        for p in &all {
            let placed = wanted.iter().find(|w| w.0 == *p);
            let id = match self.frames.get(p) {
                Some(id) => *id,
                None => {
                    let Some(id) = self.build_frame(cx, me, p, theme_bg) else {
                        continue;
                    };
                    self.frames.insert(p.clone(), id);
                    id
                }
            };
            match placed {
                Some((_, gi, active)) => {
                    let g = &self.geom.groups[*gi];
                    let mut m = g.frac.margins();
                    m[1] += STRIP_H;
                    let place = Placed::Box(g.frac, m, None);
                    if self.placed.get(&id) != Some(&place) {
                        cx.set_style(id, frame_style(g.frac, m, theme_bg));
                        self.placed.insert(id, place);
                    }
                    cx.set_hidden(id, !*active);
                }
                None => cx.set_hidden(id, true),
            }
        }
        // Tab strips: one per group, reused by position.
        self.sync_count(cx, me, true);
        for (gi, g) in self.geom.groups.iter().enumerate() {
            let sid = self.strips[gi];
            let tabs: Vec<(PanelId, String)> = g
                .panels
                .iter()
                .map(|p| (p.clone(), self.title(p)))
                .collect();
            let controls: Vec<WidgetId> = g
                .panels
                .iter()
                .filter_map(|p| self.frames.get(p).copied())
                .collect();
            let maximised = t.maximised.is_some();
            if let Some(s) = cx.widget_mut::<DockTabStrip>(sid) {
                let changed = s.tabs != tabs || s.active != g.active;
                s.tabs = tabs;
                s.active = g.active;
                s.group = gi;
                s.controls = controls;
                s.maximised = maximised;
                if changed {
                    cx.invalidate(sid, Dirty::LAYOUT);
                }
            }
            let place = Placed::Box(g.frac, g.frac.margins(), Some(STRIP_H));
            if self.placed.get(&sid) != Some(&place) {
                cx.set_style(sid, abs_style(g.frac, g.frac.margins(), Some(STRIP_H)));
                self.placed.insert(sid, place);
            }
        }
        // Splitter handles: one per boundary, reused by position. Only a handle whose
        // geometry changed is restyled and repainted.
        self.sync_count(cx, me, false);
        for (hi, h) in self.geom.handles.iter().enumerate() {
            let hid = self.handles[hi];
            let place = Placed::Handle(h.clone());
            if self.placed.get(&hid) == Some(&place) {
                continue;
            }
            if let Some(s) = cx.widget_mut::<DockSplitter>(hid) {
                s.handle = h.clone();
            }
            cx.set_style(hid, handle_style(h));
            cx.invalidate(hid, Dirty::PAINT | Dirty::A11Y);
            self.placed.insert(hid, place);
        }
        // Forget removed children.
        let live: Vec<WidgetId> = self
            .frames
            .values()
            .chain(&self.strips)
            .chain(&self.handles)
            .copied()
            .collect();
        self.placed.retain(|w, _| live.contains(w));
        // The drop preview paints above everything.
        let pv = match self.preview {
            Some(p) => p,
            None => {
                let Some(p) = cx.add_widget(
                    me,
                    Key::Static("preview"),
                    abs_style(Frac::FULL, [0.0; 4], None).no_hit_test(),
                    DropPreview,
                ) else {
                    return;
                };
                cx.set_hidden(p, true);
                self.preview = Some(p);
                p
            }
        };
        cx.bring_to_front(pv);
        if self.last.as_ref().is_some_and(|l| *l != t) {
            cx.action(DockChanged {
                area: self.area,
                tree: t.clone(),
            });
        }
        self.last = Some(t);
    }

    fn build_frame(
        &mut self,
        cx: &mut EventCx,
        me: WidgetId,
        p: &PanelId,
        bg: ColorRole,
    ) -> Option<WidgetId> {
        let title = self.title(p);
        let id = cx.add_widget(
            me,
            frame_key(p, self.rebuilds),
            frame_style(Frac::FULL, [0.0; 4], bg),
            Container::new(Role::TabPanel).labelled(&title),
        )?;
        let built = self.host.build(cx, id, p).unwrap_or(false);
        if !built {
            let msg = self.host.missing_reason(p);
            cx.add_widget(
                id,
                Key::Static("missing"),
                NodeStyle::leaf().grow(1.0),
                EmptyState::new(&msg),
            );
        }
        Some(id)
    }

    /// Make the strip (or handle) widgets match the geometry's count.
    fn sync_count(&mut self, cx: &mut EventCx, me: WidgetId, strips: bool) {
        let want = if strips {
            self.geom.groups.len()
        } else {
            self.geom.handles.len()
        };
        let list = if strips {
            &mut self.strips
        } else {
            &mut self.handles
        };
        while list.len() > want {
            if let Some(w) = list.pop() {
                cx.remove_widget(w);
            }
        }
        while list.len() < want {
            let i = list.len();
            let key = Key::Str(Arc::from(format!(
                "{}:{i}",
                if strips { "strip" } else { "handle" }
            )));
            let added = if strips {
                cx.add_widget(
                    me,
                    key,
                    NodeStyle::leaf(),
                    DockTabStrip {
                        area: self.area,
                        tabs: Vec::new(),
                        active: 0,
                        group: i,
                        controls: Vec::new(),
                        widths: Vec::new(),
                        press: None,
                        dragging: None,
                        maximised: false,
                    },
                )
            } else {
                cx.add_widget(
                    me,
                    key,
                    NodeStyle::leaf(),
                    DockSplitter {
                        area: self.area,
                        dock: me,
                        handle: self.geom.handles[i].clone(),
                        dragging: false,
                    },
                )
            };
            match added {
                Some(w) => list.push(w),
                None => break,
            }
        }
    }

    /// Tab boundaries (absolute x) of each strip, for the insertion index under a pointer.
    fn tab_edges(&mut self, cx: &mut EventCx) -> Vec<(f32, Vec<f32>)> {
        let mut out = Vec::with_capacity(self.strips.len());
        for s in &self.strips {
            let r = cx.rect_of(*s).unwrap_or_default();
            let x = r.x;
            let w = cx
                .widget_mut::<DockTabStrip>(*s)
                .map(|t| t.fitted(r.w))
                .unwrap_or_default();
            out.push((x, w));
        }
        out
    }

    fn show_preview(&mut self, cx: &mut EventCx, r: Option<Rect>) {
        let Some(pv) = self.preview else { return };
        match r {
            Some(r) => {
                let me = cx.rect();
                let mut s = NodeStyle::default().no_hit_test();
                s.layout.position = taffy::Position::Absolute;
                s.layout.inset = taffy::Rect {
                    left: length(r.x - me.x),
                    top: length(r.y - me.y),
                    right: taffy::prelude::auto(),
                    bottom: taffy::prelude::auto(),
                };
                s.layout.size = taffy::Size {
                    width: length(r.w),
                    height: length(r.h),
                };
                cx.set_style(pv, s);
                cx.set_hidden(pv, false);
            }
            None => cx.set_hidden(pv, true),
        }
    }

    /// The panel whose frame holds `w` (focus), if any.
    fn panel_holding(&self, cx: &EventCx, mut w: WidgetId) -> Option<PanelId> {
        let me = cx.id();
        loop {
            if let Some((p, _)) = self.frames.iter().find(|(_, f)| **f == w) {
                return Some(p.clone());
            }
            if let Some(gi) = self.strips.iter().position(|s| *s == w) {
                return self.geom.groups.get(gi)?.active_panel().cloned();
            }
            let parent = cx.parent_of(w)?;
            if parent == me {
                return None;
            }
            w = parent;
        }
    }
}

fn frame_style(frac: Frac, margins: [f32; 4], bg: ColorRole) -> NodeStyle {
    let mut s = abs_style(frac, margins, None)
        .clip()
        .background(bg)
        .scope(FocusScope::Panel);
    s.layout.display = taffy::Display::Flex;
    s.layout.flex_direction = taffy::FlexDirection::Column;
    s
}

fn handle_style(h: &HandleGeom) -> NodeStyle {
    let m = h.split.margins();
    let mut s = NodeStyle::default();
    s.layout.position = taffy::Position::Absolute;
    match h.axis {
        Axis::Horizontal => {
            s.layout.inset = taffy::Rect {
                left: percent(h.at),
                right: taffy::prelude::auto(),
                top: percent(h.split.y0),
                bottom: percent(1.0 - h.split.y1),
            };
            s.layout.margin = taffy::Rect {
                left: length(-HANDLE * 0.5),
                right: length(0.0),
                top: length(m[1]),
                bottom: length(m[3]),
            };
            s.layout.size.width = length(HANDLE);
        }
        Axis::Vertical => {
            s.layout.inset = taffy::Rect {
                top: percent(h.at),
                bottom: taffy::prelude::auto(),
                left: percent(h.split.x0),
                right: percent(1.0 - h.split.x1),
            };
            s.layout.margin = taffy::Rect {
                top: length(-HANDLE * 0.5),
                bottom: length(0.0),
                left: length(m[0]),
                right: length(m[2]),
            };
            s.layout.size.height = length(HANDLE);
        }
    }
    s
}

impl Widget for DockArea {
    fn role(&self) -> Role {
        Role::Group
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.tree.any()), Dirty::NONE);
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::BindingChanged => {
                self.reconcile(cx);
                Handled::Yes
            }
            UiEvent::DragOver(DragPayload::Panel(p)) => {
                let panel = PanelId::new(p);
                let (Some(pos), r) = (cx.pointer_pos(), cx.rect()) else {
                    return Handled::No;
                };
                let edges = self.tab_edges(cx);
                let tab_at = |gi: usize, x: f32| {
                    let (x0, w) = edges.get(gi)?;
                    let mut acc = *x0;
                    for (i, wi) in w.iter().enumerate() {
                        if x < acc + wi * 0.5 {
                            return Some(i);
                        }
                        acc += wi;
                    }
                    Some(w.len())
                };
                self.hit = drop_at(self.area, &self.geom, r, pos, &panel, &tab_at);
                match &self.hit {
                    Some(h) => {
                        let pr = h.preview;
                        cx.set_drop_verdict(DropVerdict::Accepted);
                        self.show_preview(cx, Some(pr));
                    }
                    None => {
                        // Over the area but nothing would change: say so, so the drag is
                        // not taken for a tear-off.
                        cx.set_drop_verdict(DropVerdict::Refused(
                            crate::tr!("the panel is already here").into(),
                        ));
                        self.show_preview(cx, None);
                    }
                }
                Handled::Yes
            }
            UiEvent::DragLeave => {
                self.hit = None;
                self.show_preview(cx, None);
                Handled::Yes
            }
            UiEvent::Drop(DragPayload::Panel(p)) => {
                let hit = self.hit.take();
                self.show_preview(cx, None);
                if self.faults.ignore_drops() {
                    return Handled::Yes;
                }
                if let Some(h) = hit {
                    let area = self.area;
                    cx.action(DockRequest {
                        area,
                        op: DockOp::Move {
                            panel: PanelId::new(p),
                            target: h.target,
                        },
                    });
                }
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => {
                let t = self.tree.get(cx.rt());
                match k.code {
                    KeyCode::Space if k.mods.shift && !k.mods.ctrl && !k.mods.alt => {
                        let target = cx
                            .focused()
                            .and_then(|f| self.panel_holding(cx, f))
                            .or_else(|| t.maximised.clone());
                        match target {
                            Some(p) => {
                                let area = self.area;
                                cx.action(DockRequest {
                                    area,
                                    op: DockOp::ToggleMaximise(p),
                                });
                                Handled::Yes
                            }
                            None => Handled::No,
                        }
                    }
                    KeyCode::Escape if t.maximised.is_some() => {
                        if let Some(p) = t.maximised {
                            let area = self.area;
                            cx.action(DockRequest {
                                area,
                                op: DockOp::ToggleMaximise(p),
                            });
                        }
                        Handled::Yes
                    }
                    _ => Handled::No,
                }
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Dock area"));
    }
}

// ---- the tab strip -------------------------------------------------------------------------

const CLOSE_W: f32 = 18.0;
const DRAG_START: f32 = 6.0;
/// The narrowest a tab shrinks to in a crowded strip.
const MIN_TAB_W: f32 = 44.0;
/// The size a torn-off panel's window opens at when nothing better is known.
pub const FLOAT_SIZE: Size = Size::new(520.0, 380.0);

/// One group's tab strip.
pub struct DockTabStrip {
    area: AreaId,
    tabs: Vec<(PanelId, String)>,
    active: usize,
    group: usize,
    controls: Vec<WidgetId>,
    /// Tab widths (logical px), cached at measure time.
    widths: Vec<f32>,
    press: Option<(usize, Point)>,
    dragging: Option<PanelId>,
    maximised: bool,
}

impl DockTabStrip {
    /// The panels of this group, in tab order.
    pub fn panels(&self) -> Vec<PanelId> {
        self.tabs.iter().map(|t| t.0.clone()).collect()
    }
    /// The active tab.
    pub fn active(&self) -> usize {
        self.active
    }
    /// The group index in the dock geometry.
    pub fn group(&self) -> usize {
        self.group
    }
    /// The rect of tab `i` inside the strip rect `r`.
    pub fn tab_rect(&self, r: Rect, i: usize) -> Option<Rect> {
        let w = self.fitted(r.w);
        let x: f32 = w.iter().take(i).sum();
        w.get(i).map(|w| Rect::new(r.x + x, r.y, *w, r.h))
    }

    /// Tab widths fitted into a strip `width` wide: with too many tabs every tab shrinks
    /// alike (titles clip), never below a grabbable minimum, so every tab stays reachable.
    pub fn fitted(&self, width: f32) -> Vec<f32> {
        let total: f32 = self.widths.iter().sum();
        if total <= width || total <= 0.0 {
            return self.widths.clone();
        }
        let k = width / total;
        self.widths.iter().map(|x| (x * k).max(MIN_TAB_W)).collect()
    }

    fn style(theme: &crate::style::Theme) -> TextStyle {
        TextStyle::body(theme.type_scale.body)
    }

    fn tab_at(&self, r: Rect, x: f32) -> Option<usize> {
        let mut acc = r.x;
        for (i, w) in self.fitted(r.w).iter().enumerate() {
            if x >= acc && x < acc + w {
                return Some(i);
            }
            acc += w;
        }
        None
    }

    fn request(&self, cx: &mut EventCx, op: DockOp) {
        let area = self.area;
        cx.action(DockRequest { area, op });
    }

    fn panel(&self, i: usize) -> Option<PanelId> {
        self.tabs.get(i).map(|t| t.0.clone())
    }
}

impl Widget for DockTabStrip {
    fn role(&self) -> Role {
        Role::TabList
    }
    fn focusable(&self) -> bool {
        !self.tabs.is_empty()
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let pad = cx.theme.space[3];
        let style = Self::style(cx.theme);
        self.widths = self
            .tabs
            .iter()
            .map(|(_, t)| cx.text.layout(t, &style, None).1.w + 2.0 * pad + CLOSE_W)
            .collect();
        let total: f32 = self.widths.iter().sum();
        Size::new(known.width.unwrap_or(total), STRIP_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let n = self.tabs.len();
        if n == 0 {
            return Handled::No;
        }
        let cur = self.active.min(n - 1);
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let (ctrl, shift) = (k.mods.ctrl, k.mods.shift);
                let op = match k.code {
                    KeyCode::Right if ctrl && shift => self
                        .panel(cur)
                        .map(|p| DockOp::Reorder(p, (cur + 1).min(n - 1))),
                    KeyCode::Left if ctrl && shift => self
                        .panel(cur)
                        .map(|p| DockOp::Reorder(p, cur.saturating_sub(1))),
                    KeyCode::Right => self.panel((cur + 1) % n).map(DockOp::Activate),
                    KeyCode::Left => self.panel((cur + n - 1) % n).map(DockOp::Activate),
                    KeyCode::Home => self.panel(0).map(DockOp::Activate),
                    KeyCode::End => self.panel(n - 1).map(DockOp::Activate),
                    KeyCode::Delete => self.panel(cur).map(DockOp::Close),
                    KeyCode::Char('w') if ctrl => self.panel(cur).map(DockOp::Close),
                    KeyCode::Space if shift => self.panel(cur).map(DockOp::ToggleMaximise),
                    _ => None,
                };
                match op {
                    Some(op) => {
                        self.request(cx, op);
                        Handled::Yes
                    }
                    None => Handled::No,
                }
            }
            UiEvent::PointerDown {
                pos,
                button,
                clicks,
            } => {
                let r = cx.rect();
                let Some(i) = self.tab_at(r, pos.x) else {
                    return Handled::Yes;
                };
                let tr = self.tab_rect(r, i).unwrap_or(r);
                match button {
                    PointerButton::Middle => {
                        if let Some(p) = self.panel(i) {
                            self.request(cx, DockOp::Close(p));
                        }
                    }
                    PointerButton::Primary if pos.x >= tr.right() - CLOSE_W => {
                        if let Some(p) = self.panel(i) {
                            self.request(cx, DockOp::Close(p));
                        }
                    }
                    PointerButton::Primary if *clicks >= 2 => {
                        if let Some(p) = self.panel(i) {
                            self.request(cx, DockOp::ToggleMaximise(p));
                        }
                    }
                    PointerButton::Primary => {
                        if i != cur
                            && let Some(p) = self.panel(i)
                        {
                            self.request(cx, DockOp::Activate(p));
                        }
                        self.press = Some((i, *pos));
                        cx.capture_pointer();
                    }
                    PointerButton::Secondary => {}
                }
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                if let Some((i, start)) = self.press
                    && self.dragging.is_none()
                    && ((pos.x - start.x).abs() > DRAG_START
                        || (pos.y - start.y).abs() > DRAG_START)
                    && let Some(p) = self.panel(i)
                {
                    cx.start_drag(DragPayload::Panel(p.to_string()));
                    self.dragging = Some(p);
                }
                Handled::Yes
            }
            UiEvent::PointerUp { pos, .. } => {
                self.press = None;
                cx.release_pointer();
                if let Some(p) = self.dragging.take() {
                    // Dropped on no dock area at all: tear the panel off into its own
                    // window at the drop point (desktop coordinates).
                    let landed = cx.drag().is_some_and(|d| d.over.is_some());
                    if !landed {
                        let (origin, monitor) = cx.window_origin();
                        let rect = Rect::new(
                            origin.x + pos.x - 40.0,
                            origin.y + pos.y - 12.0,
                            FLOAT_SIZE.w,
                            FLOAT_SIZE.h,
                        );
                        self.request(
                            cx,
                            DockOp::Float {
                                panel: p,
                                rect,
                                monitor,
                            },
                        );
                    }
                }
                Handled::Yes
            }
            UiEvent::DragCancelled => {
                self.press = None;
                self.dragging = None;
                cx.release_pointer();
                Handled::Yes
            }
            UiEvent::A11yChildAction(i, Action::Click) => {
                if let Some(p) = self.panel(*i as usize) {
                    self.request(cx, DockOp::Activate(p));
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgRaised, 0.0);
        let pad = cx.theme().space[3];
        let style = Self::style(cx.theme());
        let mut x = r.x;
        let fitted = self.fitted(r.w);
        for (i, (_, title)) in self.tabs.iter().enumerate() {
            let (run, ts) = cx.shape(title, &style);
            let w = fitted.get(i).copied().unwrap_or(ts.w + 2.0 * pad + CLOSE_W);
            let tr = Rect::new(x, r.y, w, r.h);
            let active = i == self.active;
            if active {
                cx.fill(tr, ColorRole::BgBase, 0.0);
            }
            let fg = if active {
                ColorRole::FgPrimary
            } else {
                ColorRole::FgMuted
            };
            let on = if active {
                ColorRole::BgBase
            } else {
                ColorRole::BgRaised
            };
            let clip = Rect::new(tr.x, tr.y, (tr.w - CLOSE_W).max(0.0), tr.h);
            cx.primitive(Primitive::PushClip {
                rect: clip,
                radii: Corners::all(0.0),
            });
            cx.run_on(
                run,
                Point::new(tr.x + pad, tr.y + (tr.h - ts.h) * 0.5),
                fg,
                on,
            );
            cx.primitive(Primitive::PopClip);
            if active {
                cx.mark(
                    Rect::new(tr.x + 4.0, tr.y, tr.w - 8.0, 2.0),
                    ColorRole::Accent,
                    ColorRole::BgBase,
                    1.0,
                );
            }
            let (crun, cs) = cx.shape("\u{2715}", &style);
            cx.run_on(
                crun,
                Point::new(
                    tr.right() - CLOSE_W - 2.0 + (CLOSE_W - cs.w) * 0.5,
                    tr.y + (tr.h - cs.h) * 0.5,
                ),
                ColorRole::FgMuted,
                on,
            );
            x += w;
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(if self.maximised {
            crate::tr!("Panel tabs (maximised)")
        } else {
            crate::tr!("Panel tabs")
        });
        if let Some((_, t)) = self.tabs.get(self.active) {
            node.set_value(t.as_str());
        }
    }
    fn a11y_children(&self, _cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let n = self.tabs.len();
        for (i, (_, t)) in self.tabs.iter().enumerate() {
            let mut node = accesskit::Node::new(Role::Tab);
            node.set_label(t.as_str());
            node.set_selected(i == self.active);
            node.set_position_in_set(i + 1);
            node.set_size_of_set(n);
            node.add_action(Action::Click);
            if let Some(c) = self.controls.get(i) {
                node.set_controls(vec![c.to_accesskit()]);
            }
            out.push((i as u64, node));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        (!self.tabs.is_empty()).then(|| self.active.min(self.tabs.len() - 1) as u64)
    }
}

// ---- the splitter handle -------------------------------------------------------------------

/// A splitter between two neighbouring children of a split.
pub struct DockSplitter {
    area: AreaId,
    dock: WidgetId,
    handle: HandleGeom,
    dragging: bool,
}

impl DockSplitter {
    /// The handle it drags.
    pub fn handle(&self) -> &HandleGeom {
        &self.handle
    }

    fn resize(&self, cx: &mut EventCx, ratios: Vec<f32>) {
        if ratios == self.handle.ratios {
            return;
        }
        let area = self.area;
        cx.action(DockRequest {
            area,
            op: DockOp::Resize {
                path: self.handle.path.clone(),
                ratios,
            },
        });
    }

    /// The boundary's position as a fraction of its split.
    fn boundary(&self) -> f32 {
        self.handle.ratios.iter().take(self.handle.index + 1).sum()
    }
}

impl Widget for DockSplitter {
    fn role(&self) -> Role {
        Role::Splitter
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::PointerDown {
                button: PointerButton::Primary,
                ..
            } => {
                self.dragging = true;
                cx.capture_pointer();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if self.dragging => {
                let area = cx.rect_of(self.dock).unwrap_or_default();
                let r = self.handle.drag_to(area, *pos);
                self.resize(cx, r);
                Handled::Yes
            }
            UiEvent::PointerUp { .. } if self.dragging => {
                self.dragging = false;
                cx.release_pointer();
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => {
                let step = if k.mods.shift { 0.1 } else { 0.01 };
                let dir = match (self.handle.axis, &k.code) {
                    (Axis::Horizontal, KeyCode::Left) | (Axis::Vertical, KeyCode::Up) => -1.0,
                    (Axis::Horizontal, KeyCode::Right) | (Axis::Vertical, KeyCode::Down) => 1.0,
                    _ => return Handled::No,
                };
                let r = self.handle.set_boundary(self.boundary() + dir * step);
                self.resize(cx, r);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let st = cx.state();
        let role = if self.dragging || st.hovered || st.focused {
            ColorRole::Accent
        } else {
            ColorRole::Border
        };
        let line = match self.handle.axis {
            Axis::Horizontal => Rect::new(r.x + r.w * 0.5 - 0.5, r.y, 1.0, r.h),
            Axis::Vertical => Rect::new(r.x, r.y + r.h * 0.5 - 0.5, r.w, 1.0),
        };
        let bg = cx.parent_bg();
        cx.mark(line, role, bg, 0.0);
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Resize panels"));
        node.set_orientation(match self.handle.axis {
            Axis::Horizontal => accesskit::Orientation::Vertical,
            Axis::Vertical => accesskit::Orientation::Horizontal,
        });
        node.set_numeric_value(f64::from(self.boundary()) * 100.0);
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(100.0);
    }
}

// ---- the drop preview ----------------------------------------------------------------------

/// The translucent rect showing where a dragged panel would land.
pub struct DropPreview;

impl Widget for DropPreview {
    fn role(&self) -> Role {
        Role::GenericContainer
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let bg = cx.parent_bg();
        cx.mark_alpha(r, ColorRole::Accent, bg, 4.0, 0.25);
        let w = 2.0;
        for e in [
            Rect::new(r.x, r.y, r.w, w),
            Rect::new(r.x, r.bottom() - w, r.w, w),
            Rect::new(r.x, r.y, w, r.h),
            Rect::new(r.right() - w, r.y, w, r.h),
        ] {
            cx.mark(e, ColorRole::Accent, bg, 0.0);
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_hidden();
    }
}

impl GroupGeom {
    /// The group's panels as a slice (tests).
    pub fn panel_ids(&self) -> &[PanelId] {
        &self.panels
    }
}
