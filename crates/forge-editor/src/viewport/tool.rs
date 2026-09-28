//! The `ViewportTool` extension point (`forge.editor.viewport_tool`, Ch.21 §21.19): gizmos,
//! brushes, measurement and selection modes are tools a plugin can add, replace, remove or
//! chain — the first-party ones register through the same point (I16).
//!
//! A tool sees the viewport through a [`ToolCx`]: the camera, the scene, the mirror
//! (read-only), the session (selection) and the [`CommandEmitter`] — the only way it can
//! change the project (I7). A drag that edits is a **gesture**: one transaction, at most one
//! command per frame, one undo entry, Esc cancels (Ch.21 §21.18).
//!
//! This module also holds the built-in behaviours: [`SelectTool`] (click to pick),
//! [`TransformTool`] (the move / rotate / scale gizmo) and [`MeasureTool`].

use std::sync::Arc;

use forge_cmd::EntityKey;
use forge_frames::{DVec3, FramePos, FrameResolver, Tick};
use forge_plugin::ExtensionPoint;
use forge_ui::Modifiers;

use crate::emitter::{CommandEmitter, Gesture};
use crate::mirror::ProjectMirror;
use crate::session::SessionState;
use crate::viewport::camera::EditorCamera;
use crate::viewport::gizmo::{self, GizmoDrag, GizmoMode, GizmoSpace, Handle, Snap};
use crate::viewport::layer::ViewportLayers;
use crate::viewport::scene::{Drawable, LineSink, LineStyle, ViewportScene, pick};

/// What kind of tool (the toolbar groups by it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolCategory {
    Select,
    Transform,
    Brush,
    Measure,
}

/// The viewport's tool settings (session state: the viewport toolbar changes them directly;
/// a plugin's tool keeps its own settings in its layer, [`ToolView::layers`]).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ToolSettings {
    pub space: GizmoSpace,
    pub snap: Snap,
}

/// What a tool reads (everything camera-relative, `f64`).
pub struct ToolView<'a> {
    pub camera: &'a EditorCamera,
    /// Viewport size in logical pixels.
    pub size: (f64, f64),
    pub scene: &'a ViewportScene,
    pub frames: &'a dyn FrameResolver,
    pub tick: Tick,
    pub selection: &'a [EntityKey],
    pub settings: &'a ToolSettings,
    /// The layers plugins keep over the scene (a plugin's tool finds its own by type).
    pub layers: &'a ViewportLayers,
    pub mirror: &'a ProjectMirror,
}

impl ToolView<'_> {
    /// The selected drawables, pivot first.
    pub fn selected(&self) -> Vec<&Drawable> {
        self.selection
            .iter()
            .filter_map(|k| self.scene.get(*k))
            .collect()
    }

    /// Where the ray through pixel `p` meets the camera frame's `y = 0` plane, as a
    /// camera-relative offset.
    pub fn ground_hit(&self, p: (f64, f64)) -> Option<DVec3> {
        let d = self.camera.ray_dir(p.0, p.1, self.size.0, self.size.1);
        if d.y.abs() < 1e-9 {
            return None;
        }
        let s = -self.camera.pos.local.y / d.y;
        (s > 0.0).then_some(d * s)
    }
}

/// What a tool can do.
pub struct ToolCx<'a> {
    pub view: ToolView<'a>,
    /// The only way to change the project (I7).
    pub cmd: &'a CommandEmitter,
    /// Selection and notifications (session state).
    pub session: &'a mut SessionState,
}

/// Pointer input the viewport forwards to the active tool (pixels in the viewport).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToolInput {
    Hover((f64, f64)),
    /// The primary button went down. Reply `Captured` to own the drag.
    Press((f64, f64), Modifiers),
    /// The pointer moved while a drag the tool captured is held.
    Drag((f64, f64), Modifiers),
    /// The captured drag ended.
    Release((f64, f64)),
    /// A press and release without a drag the tool captured (and without navigation).
    Click((f64, f64), Modifiers),
    /// Esc, or the viewport lost the drag: undo whatever the drag did.
    Cancel,
}

/// A tool's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolReply {
    /// Not for this tool: the viewport navigates instead.
    Ignored,
    /// The tool owns the drag that started (or handled the input).
    Captured,
    /// Handled; the view needs a repaint (a hover highlight moved).
    Repaint,
}

/// A tool's behaviour. One instance per viewport, made by the item's factory.
pub trait ToolBehavior {
    fn input(&mut self, cx: &mut ToolCx<'_>, ev: &ToolInput) -> ToolReply;
    /// Its overlay (a gizmo, a brush footprint, a measurement).
    fn draw(&self, _view: &ToolView<'_>, _sink: &mut LineSink<'_>) {}
    /// A drag is in progress (the viewport keeps the pointer captured).
    fn dragging(&self) -> bool {
        false
    }
    /// One line for the viewport's status overlay (a distance, a brush's size).
    fn status(&self, _view: &ToolView<'_>) -> Option<String> {
        None
    }
}

/// Makes a fresh behaviour.
pub type ToolFactory = Arc<dyn Fn() -> Box<dyn ToolBehavior> + Send + Sync>;

/// One registered tool (the `ViewportTool` point's item). Its key is its id
/// (`forge.tool.move`).
#[derive(Clone)]
pub struct ViewportToolItem {
    pub title: String,
    pub category: ToolCategory,
    /// The key that picks it (shown in the toolbar tooltip; the keymap binds it).
    pub hint: String,
    pub make: ToolFactory,
}

impl std::fmt::Debug for ViewportToolItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewportToolItem")
            .field("title", &self.title)
            .field("category", &self.category)
            .finish_non_exhaustive()
    }
}

impl ViewportToolItem {
    pub fn new(
        title: &str,
        category: ToolCategory,
        hint: &str,
        make: impl Fn() -> Box<dyn ToolBehavior> + Send + Sync + 'static,
    ) -> Self {
        Self {
            title: title.to_string(),
            category,
            hint: hint.to_string(),
            make: Arc::new(make),
        }
    }
}

/// The `ViewportTool` extension point (`forge.editor.viewport_tool`).
pub struct ViewportTool;

impl ExtensionPoint for ViewportTool {
    type Item = ViewportToolItem;
    const ID: &'static str = "forge.editor.viewport_tool";
    const NAME: &'static str = "ViewportTool";
}

/// Pick at `p` and change the selection: plain click selects that entity (or clears on
/// empty space); Shift or Ctrl toggles it in the selection.
pub fn click_select(cx: &mut ToolCx<'_>, p: (f64, f64), mods: Modifiers) {
    let hit = pick(
        cx.view.scene,
        cx.view.camera,
        cx.view.frames,
        cx.view.tick,
        p,
        cx.view.size,
        false,
    );
    let mut sel = cx.session.selection.clone();
    match (hit, mods.shift || mods.ctrl) {
        (Some(h), true) => {
            if let Some(i) = sel.iter().position(|k| *k == h.key) {
                sel.remove(i);
            } else {
                sel.push(h.key);
            }
        }
        (Some(h), false) => sel = vec![h.key],
        (None, true) => {}
        (None, false) => sel.clear(),
    }
    cx.session.set_selection(sel);
}

// ---- select -------------------------------------------------------------------------------

/// Click to pick; drags navigate.
#[derive(Default)]
pub struct SelectTool;

impl ToolBehavior for SelectTool {
    fn input(&mut self, cx: &mut ToolCx<'_>, ev: &ToolInput) -> ToolReply {
        match ev {
            ToolInput::Click(p, m) => {
                click_select(cx, *p, *m);
                ToolReply::Captured
            }
            _ => ToolReply::Ignored,
        }
    }
}

// ---- transform ----------------------------------------------------------------------------

/// The move / rotate / scale gizmo (see [`crate::viewport::gizmo`]). Every drag is one
/// gesture: the commands of each frame go into one transaction, committed on release —
/// one undo entry — and Esc cancels it.
pub struct TransformTool {
    mode: GizmoMode,
    hot: Option<Handle>,
    drag: Option<(GizmoDrag, Gesture)>,
    /// Commands sent in this drag (tests read how many frames sent something).
    pub steps: u64,
    /// W2 fault switch for the gizmo guard's positive control: each frame of a drag is its
    /// own transaction (what a gizmo without a gesture does).
    fault_no_txn: crate::controls::Switch,
}

impl TransformTool {
    pub fn new(mode: GizmoMode) -> Self {
        Self {
            mode,
            hot: None,
            drag: None,
            steps: 0,
            fault_no_txn: crate::controls::Switch::default(),
        }
    }

    /// W2 positive control only: a gizmo whose drag frames are separate transactions.
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn without_gesture_for_control(mode: GizmoMode) -> Self {
        Self {
            fault_no_txn: crate::controls::Switch { on: true },
            ..Self::new(mode)
        }
    }

    fn frame_of(&self, view: &ToolView<'_>) -> Option<gizmo::GizmoFrame> {
        let sel = view.selected();
        let pivot = sel.first()?;
        gizmo::place(
            view.camera,
            view.frames,
            view.tick,
            pivot,
            match self.mode {
                // Scale acts on the entity's own axes whatever the space says.
                GizmoMode::Scale => GizmoSpace::Local,
                _ => view.settings.space,
            },
            view.size.1,
        )
    }

    fn label(&self, n: usize) -> String {
        match (self.mode, n == 1) {
            (GizmoMode::Translate, true) => forge_ui::trf!("Move {n} entity", n),
            (GizmoMode::Translate, false) => forge_ui::trf!("Move {n} entities", n),
            (GizmoMode::Rotate, true) => forge_ui::trf!("Rotate {n} entity", n),
            (GizmoMode::Rotate, false) => forge_ui::trf!("Rotate {n} entities", n),
            (GizmoMode::Scale, true) => forge_ui::trf!("Scale {n} entity", n),
            (GizmoMode::Scale, false) => forge_ui::trf!("Scale {n} entities", n),
        }
    }
}

impl ToolBehavior for TransformTool {
    fn input(&mut self, cx: &mut ToolCx<'_>, ev: &ToolInput) -> ToolReply {
        match ev {
            ToolInput::Hover(p) => {
                let hot = self.frame_of(&cx.view).and_then(|g| {
                    gizmo::hit(
                        cx.view.camera,
                        &g,
                        self.mode,
                        *p,
                        cx.view.size.0,
                        cx.view.size.1,
                    )
                });
                if hot != self.hot {
                    self.hot = hot;
                    return ToolReply::Repaint;
                }
                ToolReply::Ignored
            }
            ToolInput::Press(p, _) => {
                let Some(g) = self.frame_of(&cx.view) else {
                    return ToolReply::Ignored;
                };
                let Some(h) = gizmo::hit(
                    cx.view.camera,
                    &g,
                    self.mode,
                    *p,
                    cx.view.size.0,
                    cx.view.size.1,
                ) else {
                    return ToolReply::Ignored;
                };
                let sel: Vec<&Drawable> = cx
                    .view
                    .selected()
                    .into_iter()
                    .filter(|d| !d.locked)
                    .collect();
                if sel.is_empty() {
                    return ToolReply::Ignored;
                }
                let Some(drag) = GizmoDrag::begin(
                    cx.view.camera,
                    cx.view.frames,
                    cx.view.tick,
                    g,
                    self.mode,
                    h,
                    &sel,
                    *p,
                    cx.view.size,
                ) else {
                    return ToolReply::Ignored;
                };
                let label = self.label(sel.len());
                let gesture = cx.cmd.gesture_or_fault(&label, self.fault_no_txn);
                self.hot = Some(h);
                self.steps = 0;
                self.drag = Some((drag, gesture));
                ToolReply::Captured
            }
            ToolInput::Drag(p, _) => {
                let Some((drag, gesture)) = self.drag.as_mut() else {
                    return ToolReply::Ignored;
                };
                if let Some(to) =
                    drag.update(cx.view.camera, *p, cx.view.size, &cx.view.settings.snap)
                {
                    let cmds = drag.commands(&to);
                    if !cmds.is_empty() {
                        gesture.update_all(cmds);
                        self.steps += 1;
                    }
                }
                ToolReply::Captured
            }
            ToolInput::Release(_) => {
                if let Some((_, g)) = self.drag.take() {
                    if self.steps > 0 {
                        g.commit();
                    } else {
                        g.cancel();
                    }
                }
                ToolReply::Captured
            }
            ToolInput::Cancel => {
                if let Some((_, g)) = self.drag.take() {
                    g.cancel();
                    return ToolReply::Captured;
                }
                ToolReply::Ignored
            }
            ToolInput::Click(p, m) => {
                click_select(cx, *p, *m);
                ToolReply::Captured
            }
        }
    }

    fn draw(&self, view: &ToolView<'_>, sink: &mut LineSink<'_>) {
        if let Some(g) = self.frame_of(view) {
            gizmo::draw(sink, &g, self.mode, self.hot);
        }
    }

    fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn status(&self, view: &ToolView<'_>) -> Option<String> {
        let n = view.selection.len();
        (n > 0).then(|| {
            forge_ui::trf!(
                "{mode} \u{00b7} {space} space{snap}",
                mode = self.mode.label(),
                space = view.settings.space.label(),
                snap = if view.settings.snap.enabled {
                    forge_ui::tr!(" \u{00b7} snap")
                } else {
                    ""
                }
            )
        })
    }
}

// ---- measure ------------------------------------------------------------------------------

/// Click two points (on an entity's box or the ground) to read the distance between them.
/// The points are kept as `FramePos`, so a measurement across frames rebases, and the
/// distance is formed in `f64`.
#[derive(Default)]
pub struct MeasureTool {
    points: Vec<FramePos>,
}

impl MeasureTool {
    fn point_at(view: &ToolView<'_>, p: (f64, f64)) -> Option<FramePos> {
        let rel = match pick(
            view.scene,
            view.camera,
            view.frames,
            view.tick,
            p,
            view.size,
            false,
        ) {
            Some(h) => view.camera.ray_dir(p.0, p.1, view.size.0, view.size.1) * h.distance,
            None => view.ground_hit(p)?,
        };
        Some(FramePos::new(
            view.camera.pos.frame,
            view.camera.pos.local + rel,
        ))
    }

    /// The measured distance, metres.
    pub fn distance(&self, tree: &dyn FrameResolver, t: Tick) -> Option<f64> {
        let [a, b] = self.points.as_slice() else {
            return None;
        };
        let b = tree.resolve(*b, a.frame, t).ok()?;
        Some((b.local - a.local).length())
    }
}

impl ToolBehavior for MeasureTool {
    fn input(&mut self, cx: &mut ToolCx<'_>, ev: &ToolInput) -> ToolReply {
        match ev {
            ToolInput::Click(p, _) => {
                if self.points.len() >= 2 {
                    self.points.clear();
                }
                if let Some(q) = Self::point_at(&cx.view, *p) {
                    self.points.push(q);
                }
                ToolReply::Repaint
            }
            ToolInput::Cancel if !self.points.is_empty() => {
                self.points.clear();
                ToolReply::Repaint
            }
            _ => ToolReply::Ignored,
        }
    }

    fn draw(&self, view: &ToolView<'_>, sink: &mut LineSink<'_>) {
        let rel: Vec<DVec3> = self
            .points
            .iter()
            .filter_map(|p| view.camera.offset_of(view.frames, view.tick, *p))
            .collect();
        for r in &rel {
            let s = view.camera.metres_per_pixel(r.length(), view.size.1) * 5.0;
            sink.seg(*r - DVec3::X * s, *r + DVec3::X * s, LineStyle::Measure);
            sink.seg(*r - DVec3::Z * s, *r + DVec3::Z * s, LineStyle::Measure);
        }
        if let [a, b] = rel.as_slice() {
            sink.seg(*a, *b, LineStyle::Measure);
        }
    }

    fn status(&self, view: &ToolView<'_>) -> Option<String> {
        match self.points.len() {
            0 => Some(forge_ui::tr!("Measure: click the first point").into()),
            1 => Some(forge_ui::tr!("Measure: click the second point").into()),
            _ => self.distance(view.frames, view.tick).map(|d| {
                forge_ui::trf!(
                    "Distance {distance}",
                    distance = crate::viewport::format_metres(d)
                )
            }),
        }
    }
}

/// The first-party tools, `(id, item)`: select, move, rotate, scale, measure. The scene
/// panels plugin registers them through the ordinary point (I16).
pub fn builtin_tools() -> Vec<(&'static str, ViewportToolItem)> {
    vec![
        (
            "forge.tool.select",
            ViewportToolItem::new(
                forge_ui::tr_key!("Select"),
                ToolCategory::Select,
                "Q",
                || Box::new(SelectTool),
            ),
        ),
        (
            "forge.tool.move",
            ViewportToolItem::new(
                forge_ui::tr_key!("Move"),
                ToolCategory::Transform,
                "W",
                || Box::new(TransformTool::new(GizmoMode::Translate)),
            ),
        ),
        (
            "forge.tool.rotate",
            ViewportToolItem::new(
                forge_ui::tr_key!("Rotate"),
                ToolCategory::Transform,
                "E",
                || Box::new(TransformTool::new(GizmoMode::Rotate)),
            ),
        ),
        (
            "forge.tool.scale",
            ViewportToolItem::new(
                forge_ui::tr_key!("Scale"),
                ToolCategory::Transform,
                "R",
                || Box::new(TransformTool::new(GizmoMode::Scale)),
            ),
        ),
        (
            "forge.tool.measure",
            ViewportToolItem::new(
                forge_ui::tr_key!("Measure"),
                ToolCategory::Measure,
                "M",
                || Box::<MeasureTool>::default(),
            ),
        ),
    ]
}
