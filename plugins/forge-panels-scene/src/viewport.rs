//! The **Viewport** (`forge.viewport`, Ch.21 §21.21, DoD M2-32).
//!
//! * **Camera** — `f64` and camera-relative ([`forge_editor::viewport::camera`]): orbit,
//!   fly and pan, log-scaled speed, usable from a centimetre to the far end of a large world.
//! * **Tools** — the `ViewportTool` registry's select, move, rotate, scale, measure (and a
//!   plugin's brushes, which its own panel picks). Gizmo drags are **gestures**: one transaction, one undo
//!   entry, Esc cancels (Ch.21 §21.18).
//! * **Picking** — click to select, Shift/Ctrl-click to toggle (session state).
//! * **Overlays** — stats, the camera's frame, the `SeedPath` under the pointer, the tool's
//!   status; the grid, frame axes and bounding boxes.
//! * **Multiple viewports** — one, two or four cells, each its own camera (perspective,
//!   top, front, side).
//! * **Rendering** — with a GPU scene renderer attached (the editor binary's `forge-render`
//!   host) each cell composites its rendered frame under the overlay lines; without one it
//!   shows the placeholder (grid, axes, boxes) and says so.
//! * **Worlds** — a plugin's world ([`ViewportWorld`], the `ViewportWorld` point) is shown
//!   in the cells instead of the project scene while it is shown: it paints the cells, takes
//!   a cell's camera to generated content a session camera focus names, and animates the
//!   move; "Back to the scene" leaves it. Without one, the cells show the project scene.
//! * **Redraws only on change** — a cell repaints when its camera, the scene, the selection
//!   or its tool changed, or while play-in-editor runs (§21.13); an idle viewport costs
//!   nothing (`test_viewport` checks 0 frames over 10 s idle).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use forge_editor::viewport::world::{CellCameras, ViewportWorld, WorldCell, WorldLaunch};
use forge_frames::FrameResolver;
use forge_ui::{LiveCell, LiveSource};

use forge_cmd::{EditorCommand, Value};
use forge_editor::panel_rt::ShellHandles;
use forge_editor::panels::PanelCx;
use forge_editor::play::PlayState;
use forge_editor::services::EditorServices;
use forge_editor::viewport::camera::{EditorCamera, NavMode, look_rotation};
use forge_editor::viewport::controller::{
    Button as VButton, Handles, ViewInput, ViewKey, ViewportController,
};
use forge_editor::viewport::format_metres;
use forge_editor::viewport::gizmo::{GizmoSpace, Snap};
use forge_editor::viewport::physics::{self as phys_draw, DebugKind, DebugLine};
use forge_editor::viewport::scene::{
    Drawable, LineSink, LineStyle, SceneVersion, Segment, ViewportScene, bounds, grid_and_axes,
    pick,
};
use forge_editor::viewport::surface::SurfaceFrame;
use forge_editor::viewport::tool::{SelectTool, ToolBehavior, ToolCategory, ToolView};
use forge_frames::{DVec3, FrameId, FramePos, Tick};
use forge_ui::input::{Handled, KeyCode, PointerButton, UiEvent};
use forge_ui::render::{ExternalTexture, PathBuilder, Primitive};
use forge_ui::text::TextStyle;
use forge_ui::widget::{A11yCx, EventCx, PaintCx, Widget};
use forge_ui::widgets::{
    Button, Container, Label, LabelKind, Pressed, SegmentedControl, SignalChanged, SignalRelay,
    Switch,
};
use forge_ui::{ColorRole, Dirty, NodeStyle, Point, Role, Signal, UiTime};

/// How many cells the viewport can show.
pub const MAX_CELLS: usize = 4;
/// Bounding boxes drawn per cell at most (nearest first); the rest are counted.
pub const MAX_BOXES: usize = 2000;

/// The camera placements of the four cells.
const CELL_NAMES: [&str; MAX_CELLS] = [
    forge_ui::tr_key!("Perspective"),
    forge_ui::tr_key!("Top"),
    forge_ui::tr_key!("Front"),
    forge_ui::tr_key!("Side"),
];

fn cell_camera(i: usize) -> EditorCamera {
    let f = FrameId(0);
    let mut c = EditorCamera::new(f);
    let (eye, up) = match i {
        1 => (DVec3::new(0.0, 12.0, 0.001), DVec3::new(0.0, 0.0, -1.0)),
        2 => (DVec3::new(0.0, 1.0, 12.0), DVec3::Y),
        3 => (DVec3::new(12.0, 1.0, 0.0), DVec3::Y),
        _ => return c,
    };
    c.pos = FramePos::new(f, eye);
    if let Some(q) = look_rotation(-eye, up) {
        c.orientation = q;
    }
    c.pivot_distance = eye.length();
    c
}

/// One cell: a camera and its controller, and its render surface.
pub struct Cell {
    pub ctl: ViewportController,
    pub surface: u32,
    pub name: &'static str,
    /// The seed path under the pointer (generated content), for the overlay.
    pub hover_seed: Option<String>,
}

/// Everything the viewport's cells share (the panel's model).
pub struct Model {
    handles: ShellHandles,
    services: Rc<EditorServices>,
    pub scene: ViewportScene,
    /// The play-in-editor sandbox's view of the scene, while a simulation exists.
    pub sim: Option<ViewportScene>,
    pub cells: Vec<Cell>,
    /// The cell that last had the pointer (fly-to and "Frame selection" act on it).
    pub active: usize,
    /// Cells shown: 1, 2 or 4.
    pub layout: usize,
    /// The drawables handed to a GPU host, rebuilt only when what the cells show changed:
    /// keyed on the shown scene's version (the edit world's revision and, while a
    /// simulation exists, the play core's), so each simulation step reaches the GPU.
    drawables: RefCell<Option<(SceneVersion, Rc<Vec<Drawable>>)>>,
    seen_play: u64,
    /// The sequencer preview last drawn (WP-U11).
    seen_preview: u64,
    seen_camera: u64,
    seen_tool: u64,
    seen_selection: u64,
    /// The team's state last drawn (presence, claims; WP-U10).
    seen_collab: u64,
    /// Paints per cell (the idle guard reads them).
    pub paints: RefCell<[u64; MAX_CELLS]>,
    /// Whether the cells draw the physics debug drawing while playing (the toolbar's
    /// "Colliders" switch; session state, on by default).
    pub show_colliders: bool,
    /// The play core's physics debug drawing after its last step (empty while stopped or
    /// with the switch off), each line in its region's frame.
    pub phys: Vec<(FrameId, DebugLine)>,
    /// Physics lines each cell drew in its last paint (the physics overlay's tests read it).
    pub phys_drawn: RefCell<[usize; MAX_CELLS]>,
    /// The world a plugin shows in the cells instead of the project scene, if one provides
    /// it ([`ViewportWorld`], made from the first `ViewportWorld` item).
    pub world: Option<Box<dyn ViewportWorld>>,
    /// Bumped when the GPU host has more to do for a frame it rendered (its uploads paced
    /// over frames): the panel takes a turn and requests the frame again.
    host_feed: Arc<LiveCell>,
}

/// The cells' cameras, lent to the world ([`CellCameras`]).
struct Cams<'a>(&'a mut [Cell]);

impl CellCameras for Cams<'_> {
    fn count(&self) -> usize {
        self.0.len()
    }
    fn camera(&self, cell: usize) -> Option<EditorCamera> {
        self.0.get(cell).map(|c| c.ctl.camera)
    }
    fn set_camera(&mut self, cell: usize, camera: EditorCamera) {
        if let Some(c) = self.0.get_mut(cell) {
            c.ctl.set_camera(camera);
        }
    }
}

impl Model {
    /// The world, while it is shown.
    fn shown_world(&self) -> Option<&dyn ViewportWorld> {
        self.world.as_deref().filter(|w| w.shown())
    }

    /// The frame tree the cells are in: a shown world's, else the project scene's.
    pub fn frames(&self) -> Arc<dyn FrameResolver> {
        self.shown_world()
            .and_then(|w| w.frames())
            .unwrap_or_else(|| Arc::clone(&self.services.frames))
    }

    /// The moment the cells show.
    pub fn tick(&self) -> Tick {
        self.shown_world().map_or(Tick(0), |w| w.tick())
    }

    /// Whether the world is moving a cell's camera.
    pub fn moving(&self) -> bool {
        self.world
            .as_deref()
            .is_some_and(|w| (0..self.cells.len()).any(|c| w.moving(c)))
    }

    /// Back to the project scene: every cell's camera to its first placement.
    fn leave_world(&mut self) {
        if let Some(w) = self.world.as_mut() {
            w.leave();
        }
        for (i, c) in self.cells.iter_mut().enumerate() {
            c.ctl.set_camera(cell_camera(i));
        }
    }

    /// Re-read the play core's physics debug drawing (nothing while stopped or switched off).
    fn refresh_physics(&mut self, play: &dyn forge_editor::play::PlayBackend) {
        self.phys.clear();
        if self.show_colliders && play.state() != PlayState::Stopped {
            play.physics_debug(&mut self.phys);
        }
    }

    /// The scene the cells show: the sandbox while playing, else the edit world.
    pub fn shown(&self) -> &ViewportScene {
        self.sim.as_ref().unwrap_or(&self.scene)
    }

    fn drawables(&self) -> Rc<Vec<Drawable>> {
        let scene = self.shown();
        let rev = scene.version();
        let mut d = self.drawables.borrow_mut();
        if let Some((r, v)) = d.as_ref()
            && *r == rev
        {
            return Rc::clone(v);
        }
        let v = Rc::new(scene.iter().cloned().collect::<Vec<_>>());
        *d = Some((rev, Rc::clone(&v)));
        v
    }

    /// Make a fresh behaviour for tool `id` (the select tool if it is not registered).
    fn make_tool(services: &EditorServices, id: &str) -> Box<dyn ToolBehavior> {
        services.viewport_tools.get(id).map_or_else(
            || Box::new(SelectTool) as Box<dyn ToolBehavior>,
            |t| (t.make)(),
        )
    }

    /// The snap settings: the project's grid and rotation snap (`editor.*`, project state
    /// every teammate shares).
    fn snap(mirror: &forge_editor::mirror::ProjectMirror) -> Snap {
        let f = |k: &str, d: f64| match mirror.setting(k) {
            Some(Value::Float(v)) if *v > 0.0 => *v,
            _ => d,
        };
        Snap {
            enabled: matches!(
                mirror.setting("editor.snap_translate"),
                Some(Value::Bool(true))
            ),
            translate: f("editor.grid_size", 1.0),
            rotate_deg: f("editor.snap_rotate", 15.0),
            scale: 0.1,
        }
    }
}

type Shared = Rc<RefCell<Model>>;

/// Input from a cell's canvas, for the panel's handler.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasInput {
    pub cell: usize,
    pub input: ViewInput,
}

/// A tool shortcut typed over a cell (Q select, W move, E rotate, R scale).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolShortcut(pub char);

/// One cell's canvas (see the module docs).
pub struct ViewportCanvas {
    model: Shared,
    cell: usize,
    /// Secondary button held (flying): the fly keys are live.
    looking: bool,
    fly_keys: u8,
    last_anim: Option<UiTime>,
    /// W2 fault (`PanelFaults::viewport_redraw_always`): a render loop.
    render_loop: bool,
    /// Animation frames requested for a world's camera move (or, under its idle control, after
    /// one).
    move_anim: bool,
}

impl ViewportCanvas {
    pub fn new(model: Shared, cell: usize) -> Self {
        let render_loop = model.borrow().services.faults.viewport_redraw_always();
        Self {
            model,
            cell,
            looking: false,
            fly_keys: 0,
            last_anim: None,
            render_loop,
            move_anim: false,
        }
    }

    /// Whether the world is moving this cell's camera.
    fn moving_here(&self) -> bool {
        self.model
            .borrow()
            .world
            .as_deref()
            .is_some_and(|w| w.moving(self.cell))
    }

    /// Whether a move of this cell's camera is being planned in the background.
    fn pending_here(&self) -> bool {
        self.model
            .borrow()
            .world
            .as_deref()
            .is_some_and(|w| w.pending(self.cell))
    }

    /// Whether any move is being planned in the world's background work.
    pub fn world_pending(&self) -> bool {
        let m = self.model.borrow();
        m.world
            .as_deref()
            .is_some_and(|w| (0..m.cells.len()).any(|c| w.pending(c)))
    }

    /// Whether the world's background work is still running for what it shows.
    pub fn world_busy(&self) -> bool {
        self.model
            .borrow()
            .world
            .as_deref()
            .is_some_and(ViewportWorld::busy)
    }

    /// Whether the cells show the world (not the project scene).
    pub fn world_shown(&self) -> bool {
        self.model.borrow().shown_world().is_some()
    }

    /// Run `f` on the world the cells show from, if the viewport has one (a plugin reads its
    /// own world's state through [`ViewportWorld::as_any`]).
    pub fn with_world<R>(&self, f: impl FnOnce(&dyn ViewportWorld) -> R) -> Option<R> {
        self.model.borrow().world.as_deref().map(f)
    }

    /// This cell's index among the viewport's cells.
    pub fn index(&self) -> usize {
        self.cell
    }

    /// This cell's render surface (its external-texture id).
    pub fn surface_id(&self) -> u32 {
        self.model.borrow().cells[self.cell].surface
    }

    /// Whether the world is moving any cell's camera.
    pub fn moving(&self) -> bool {
        self.model.borrow().moving()
    }

    /// This cell's camera now.
    pub fn camera(&self) -> EditorCamera {
        self.model.borrow().cells[self.cell].ctl.camera
    }

    /// How many times this cell has painted (the redraw guards read it).
    pub fn paints(&self) -> u64 {
        self.model.borrow().paints.borrow()[self.cell]
    }

    /// Physics debug lines this cell drew in its last paint.
    pub fn physics_lines(&self) -> usize {
        self.model.borrow().phys_drawn.borrow()[self.cell]
    }

    /// The active tool's id in this cell.
    pub fn tool(&self) -> String {
        self.model.borrow().cells[self.cell].ctl.tool_id.clone()
    }

    fn at(cx: &EventCx, p: Point) -> (f64, f64) {
        let r = cx.rect();
        (f64::from(p.x - r.x), f64::from(p.y - r.y))
    }

    fn send(&self, cx: &mut EventCx, input: ViewInput) {
        cx.action(CanvasInput {
            cell: self.cell,
            input,
        });
    }
}

fn role_of(style: LineStyle) -> (ColorRole, f32) {
    match style {
        LineStyle::GridMinor => (ColorRole::Border, 1.0),
        LineStyle::GridMajor => (ColorRole::FgMuted, 1.0),
        LineStyle::Axis(0)
        | LineStyle::Gizmo {
            axis: 0,
            hot: false,
        } => (ColorRole::Danger, 1.5),
        LineStyle::Axis(1)
        | LineStyle::Gizmo {
            axis: 1,
            hot: false,
        } => (ColorRole::Success, 1.5),
        LineStyle::Axis(_)
        | LineStyle::Gizmo {
            axis: 2,
            hot: false,
        } => (ColorRole::Accent, 1.5),
        LineStyle::Gizmo { hot: true, .. } => (ColorRole::Warning, 2.5),
        LineStyle::Gizmo { .. } => (ColorRole::FgPrimary, 1.5),
        LineStyle::Bounds => (ColorRole::FgMuted, 1.0),
        LineStyle::BoundsSelected => (ColorRole::Accent, 2.0),
        LineStyle::Cursor => (ColorRole::Warning, 1.5),
        LineStyle::Measure => (ColorRole::FgPrimary, 1.5),
        LineStyle::Layer => (ColorRole::Success, 1.0),
        LineStyle::Physics(k) => match k {
            DebugKind::Dynamic => (ColorRole::Accent, 1.5),
            DebugKind::Sleeping => (ColorRole::FgMuted, 1.0),
            DebugKind::Kinematic => (ColorRole::Warning, 1.5),
            DebugKind::Static => (ColorRole::Success, 1.0),
            DebugKind::Trigger => (ColorRole::Danger, 1.0),
            DebugKind::Joint => (ColorRole::FgPrimary, 2.0),
        },
    }
}

impl Widget for ViewportCanvas {
    fn role(&self) -> Role {
        Role::Canvas
    }
    fn focusable(&self) -> bool {
        true
    }
    fn hover_sensitive(&self) -> bool {
        false
    }

    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        // Taking the camera (a press, the wheel) or Esc stops a world's camera move where it
        // is.
        let takes_camera = matches!(ev, UiEvent::PointerDown { .. } | UiEvent::Wheel { .. });
        let esc = matches!(ev, UiEvent::Key(k) if k.pressed && k.code == KeyCode::Escape);
        if (takes_camera || esc) && (self.moving_here() || self.pending_here()) {
            let stopped = self
                .model
                .borrow_mut()
                .world
                .as_mut()
                .is_some_and(|w| w.cancel());
            if stopped {
                self.move_anim = false;
                self.last_anim = None;
                cx.stop_anim();
                cx.request_paint();
                if esc {
                    return Handled::Yes;
                }
            }
        }
        match ev {
            UiEvent::PointerDown { pos, button, .. } => {
                cx.request_focus();
                cx.capture_pointer();
                let button = match button {
                    PointerButton::Primary => VButton::Primary,
                    PointerButton::Secondary => {
                        self.looking = true;
                        VButton::Secondary
                    }
                    PointerButton::Middle => VButton::Middle,
                };
                let at = Self::at(cx, *pos);
                let mods = cx.modifiers();
                self.send(cx, ViewInput::Down { at, button, mods });
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                let at = Self::at(cx, *pos);
                let mods = cx.modifiers();
                self.send(cx, ViewInput::Move { at, mods });
                Handled::Yes
            }
            UiEvent::PointerUp { pos, button, .. } => {
                cx.release_pointer();
                let button = match button {
                    PointerButton::Primary => VButton::Primary,
                    PointerButton::Secondary => {
                        self.looking = false;
                        self.fly_keys = 0;
                        cx.stop_anim();
                        VButton::Secondary
                    }
                    PointerButton::Middle => VButton::Middle,
                };
                let at = Self::at(cx, *pos);
                self.send(cx, ViewInput::Up { at, button });
                Handled::Yes
            }
            UiEvent::Wheel { dy, .. } => {
                self.send(
                    cx,
                    ViewInput::Wheel {
                        notches: f64::from(*dy) / 40.0,
                    },
                );
                Handled::Yes
            }
            UiEvent::Key(k) => {
                let fly = match k.code {
                    KeyCode::Char('w') => Some((ViewKey::Forward, 1)),
                    KeyCode::Char('s') => Some((ViewKey::Back, 2)),
                    KeyCode::Char('a') => Some((ViewKey::Left, 4)),
                    KeyCode::Char('d') => Some((ViewKey::Right, 8)),
                    KeyCode::Char('e') => Some((ViewKey::Up, 16)),
                    KeyCode::Char('q') => Some((ViewKey::Down, 32)),
                    _ => None,
                };
                if let Some((key, bit)) = fly
                    && (self.looking || !k.pressed)
                    && !k.mods.ctrl
                {
                    if k.repeat {
                        return Handled::Yes;
                    }
                    if k.pressed {
                        self.fly_keys |= bit;
                    } else {
                        self.fly_keys &= !bit;
                    }
                    self.send(
                        cx,
                        ViewInput::Key {
                            key,
                            pressed: k.pressed,
                            shift: k.mods.shift,
                        },
                    );
                    if self.fly_keys != 0 {
                        self.last_anim = Some(cx.now());
                        cx.request_anim_frame();
                    } else {
                        cx.stop_anim();
                        self.last_anim = None;
                    }
                    return Handled::Yes;
                }
                if !k.pressed || k.mods.ctrl || k.mods.alt {
                    return Handled::No;
                }
                match k.code {
                    KeyCode::Char(c @ ('q' | 'w' | 'e' | 'r')) => {
                        cx.action(ToolShortcut(c));
                        Handled::Yes
                    }
                    KeyCode::Char('f') => {
                        self.send(
                            cx,
                            ViewInput::Key {
                                key: ViewKey::Frame,
                                pressed: true,
                                shift: false,
                            },
                        );
                        Handled::Yes
                    }
                    KeyCode::Escape => {
                        self.send(
                            cx,
                            ViewInput::Key {
                                key: ViewKey::Cancel,
                                pressed: true,
                                shift: false,
                            },
                        );
                        Handled::Yes
                    }
                    _ => Handled::No,
                }
            }
            UiEvent::VisibilityChanged(true) if self.render_loop => {
                cx.request_anim_frame();
                Handled::Yes
            }
            UiEvent::AnimFrame if self.render_loop => {
                // W2 positive control only: a viewport that redraws every display frame.
                cx.request_paint();
                cx.request_anim_frame();
                Handled::Yes
            }
            UiEvent::AnimFrame if self.moving_here() => {
                // A world moving this cell's camera: one step per display frame.
                let now = cx.now();
                let dt = self
                    .last_anim
                    .map_or(0.0, |t| now.saturating_sub(t).as_secs_f64());
                self.last_anim = Some(now);
                let step = {
                    let mut m = self.model.borrow_mut();
                    let Model { world, cells, .. } = &mut *m;
                    match world.as_mut() {
                        Some(w) => w.step(&mut Cams(cells), dt),
                        None => forge_editor::viewport::world::WorldStep {
                            done: true,
                            ..Default::default()
                        },
                    }
                };
                let (done, keep) = (step.done, step.keep_animating);
                cx.request_paint();
                if step.waiting {
                    // Holding for the world's background work: no frames until it is ready
                    // (the panel's world feed resumes the move then).
                    self.move_anim = false;
                    self.last_anim = None;
                    cx.stop_anim();
                } else if done && !keep {
                    self.move_anim = false;
                    self.last_anim = None;
                    cx.stop_anim();
                } else {
                    // A world's idle control keeps a finished move's frames coming.
                    self.move_anim = !done || keep;
                    cx.request_anim_frame();
                }
                Handled::Yes
            }
            UiEvent::AnimFrame if self.move_anim => {
                // A world's W2 idle control only: a finished move that keeps animating.
                cx.request_paint();
                cx.request_anim_frame();
                Handled::Yes
            }
            UiEvent::AnimFrame => {
                if self.fly_keys == 0 {
                    self.last_anim = None;
                    return Handled::Yes;
                }
                let now = cx.now();
                let dt = self
                    .last_anim
                    .map_or(0.0, |t| now.saturating_sub(t).as_secs_f64());
                self.last_anim = Some(now);
                self.send(cx, ViewInput::Frame { dt });
                cx.request_anim_frame();
                Handled::Yes
            }
            UiEvent::FocusLost => {
                if self.fly_keys != 0 {
                    self.fly_keys = 0;
                    cx.stop_anim();
                    for key in [ViewKey::Forward, ViewKey::Right, ViewKey::Up] {
                        self.send(
                            cx,
                            ViewInput::Key {
                                key,
                                pressed: false,
                                shift: false,
                            },
                        );
                    }
                }
                Handled::No
            }
            _ => Handled::No,
        }
    }

    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgSunken, 0.0);
        let m = self.model.borrow();
        if let Ok(mut p) = m.paints.try_borrow_mut() {
            p[self.cell] += 1;
        }
        let Some(cell) = m.cells.get(self.cell) else {
            return;
        };
        if let Some(w) = m.shown_world() {
            w.paint(
                cx,
                &WorldCell {
                    index: self.cell,
                    camera: cell.ctl.camera,
                    surface: cell.surface,
                },
            );
            return;
        }
        let services = &m.services;
        let (w, h) = (f64::from(r.w), f64::from(r.h));
        let scene = m.shown();
        let frames = services.frames.as_ref();
        let tick = Tick(0);
        let selection = m.handles.session().selection.clone();
        let mirror = m.handles.mirror();
        let host = services
            .viewport_surfaces
            .borrow()
            .host()
            .map(str::to_string);
        // The rendered scene, under the lines (a GPU host renders it right after this frame
        // is recorded and before it is drawn).
        if host.is_some() && w >= 1.0 && h >= 1.0 {
            let s = cx.scale();
            services.viewport_surfaces.borrow_mut().request(
                cell.surface,
                SurfaceFrame {
                    camera: cell.ctl.camera,
                    size_px: (
                        (r.w * s).round().max(1.0) as u32,
                        (r.h * s).round().max(1.0) as u32,
                    ),
                    drawables: m.drawables(),
                    selection: selection.clone(),
                    tick,
                    world: None,
                },
            );
            cx.primitive(Primitive::Viewport {
                tex: ExternalTexture(cell.surface),
                rect: r,
            });
        }
        // Overlay lines, all camera-relative in f64, narrowed only as pixel coordinates.
        let mut segs: Vec<Segment> = Vec::new();
        let mut sink = LineSink {
            cam: &cell.ctl.camera,
            w,
            h,
            out: &mut segs,
        };
        let grid = match mirror.setting("editor.grid_size") {
            Some(Value::Float(v)) if *v > 0.0 => *v,
            _ => 1.0,
        };
        grid_and_axes(&mut sink, grid);
        let boxes = bounds(&mut sink, scene, frames, tick, &selection, MAX_BOXES);
        // The physics bodies' colliders and joints while playing (empty otherwise).
        let phys = phys_draw::draw(&mut sink, &m.phys, frames, tick);
        if let Ok(mut d) = m.phys_drawn.try_borrow_mut() {
            d[self.cell] = phys;
        }
        let layers = services.viewport_layers.borrow();
        let view = ToolView {
            camera: &cell.ctl.camera,
            size: (w, h),
            scene,
            frames,
            tick,
            selection: &selection,
            settings: &cell.ctl.settings,
            layers: &layers,
            mirror: &mirror,
        };
        // What the plugins' layers draw over the scene, then the tool's footprint.
        layers.draw(&view, &mut sink);
        cell.ctl.tool.draw(&view, &mut sink);
        let status = cell.ctl.tool.status(&view);
        drop(layers);
        let mut by_style: BTreeMap<LineStyle, PathBuilder> = BTreeMap::new();
        for s in &segs {
            let pb = by_style.entry(s.style).or_default();
            pb.move_to(Point::new(r.x + s.a.0, r.y + s.a.1));
            pb.line_to(Point::new(r.x + s.b.0, r.y + s.b.1));
        }
        for (style, pb) in by_style {
            let (role, width) = role_of(style);
            cx.stroke_path(&pb.build(), width, role, ColorRole::BgSunken);
        }
        // Text overlays: stats, frame, seed path, tool status.
        let small = TextStyle::body(cx.theme().type_scale.small);
        let line = small.size * small.line_height;
        let pad = cx.theme().space[2];
        let cam = &cell.ctl.camera;
        let play = services.play.borrow();
        let mut top = vec![
            if scene.len() == 1 {
                forge_ui::trf!(
                    "{view} \u{00b7} {nav} \u{00b7} {n} entity ({boxes} drawn)",
                    view = forge_ui::l10n::tr(cell.name),
                    nav = cell.ctl.nav.label(),
                    n = scene.len(),
                    boxes
                )
            } else {
                forge_ui::trf!(
                    "{view} \u{00b7} {nav} \u{00b7} {n} entities ({boxes} drawn)",
                    view = forge_ui::l10n::tr(cell.name),
                    nav = cell.ctl.nav.label(),
                    n = scene.len(),
                    boxes
                )
            },
            forge_ui::trf!(
                "Frame {frame} \u{00b7} {distance} from its origin \u{00b7} pivot {pivot} \u{00b7} fly {speed}/s",
                frame = cam.pos.frame.0,
                distance = format_metres(cam.pos.local.length()),
                pivot = format_metres(cam.pivot_distance),
                speed = format_metres(cam.fly_speed())
            ),
            match &host {
                Some(hn) => match services.viewport_surfaces.borrow().error() {
                    Some(e) => {
                        forge_ui::trf!("Renderer: {hn} \u{2014} last frame failed: {e}", hn, e)
                    }
                    None => forge_ui::trf!("Renderer: {hn}", hn),
                },
                None => forge_ui::tr!("Placeholder view: grid, axes and bounding boxes (no GPU scene renderer attached)").into(),
            },
        ];
        if let Some(t) = teammates(services, &mirror) {
            top.push(t);
        }
        if play.state() != PlayState::Stopped {
            top.push(forge_ui::trf!(
                "Sandbox \u{00b7} tick {tick} \u{00b7} {backend}",
                tick = play.tick().0,
                backend = forge_ui::l10n::tr_str(play.backend())
            ));
        }
        drop(play);
        for (i, t) in top.iter().enumerate() {
            cx.text(
                t,
                &small,
                Point::new(r.x + pad, r.y + pad + line * i as f32),
                ColorRole::FgPrimary,
            );
        }
        let mut bottom: Vec<String> = Vec::new();
        if let Some(s) = &cell.hover_seed {
            bottom.push(forge_ui::trf!("Seed path: {s}", s));
        }
        if let Some(s) = status {
            bottom.push(s);
        }
        for (i, t) in bottom.iter().rev().enumerate() {
            cx.text(
                t,
                &small,
                Point::new(r.x + pad, r.bottom() - pad - line * (i as f32 + 1.0)),
                ColorRole::FgPrimary,
            );
        }
    }

    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        let m = self.model.borrow();
        if let Some(c) = m.cells.get(self.cell) {
            let team = teammates(&m.services, &m.handles.mirror())
                .map(|t| format!("; {t}"))
                .unwrap_or_default();
            node.set_label(forge_ui::trf!(
                "Viewport {viewport}: {name}, {shown} entities, tool {tool_id}{team}",
                viewport = self.cell + 1,
                name = forge_ui::l10n::tr(c.name),
                shown = m.shown().len(),
                tool_id = c.ctl.tool_id,
                team
            ));
        }
    }
}

/// Who else is here (Ch.37 §37.7, WP-U10): the teammates looking at something and what,
/// and what others have claimed (read-only for you). `None` without a team or teammates.
pub fn teammates(
    services: &EditorServices,
    mirror: &forge_editor::mirror::ProjectMirror,
) -> Option<String> {
    let c = &services.collab;
    if !c.attached() {
        return None;
    }
    let name = |k: u64| {
        mirror.entity(forge_cmd::EntityKey(k)).map_or_else(
            || format!("e{k}"),
            |e| format!("\u{201c}{}\u{201d}", e.name),
        )
    };
    let mut by_user: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (k, users) in c.viewers() {
        for u in users {
            by_user.entry(u).or_default().push(name(k));
        }
    }
    let me = c.me();
    let claims: Vec<String> = c
        .claims()
        .into_iter()
        .filter(|cl| cl.holder != me)
        .map(|cl| format!("{} by {}", name(cl.entity), cl.holder))
        .collect();
    if by_user.is_empty() && claims.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !by_user.is_empty() {
        parts.push(forge_ui::trf!(
            "\u{25cf} Teammates here: {who}",
            who = by_user
                .iter()
                .map(|(u, names)| format!("{u} \u{2192} {}", names.join(", ")))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if !claims.is_empty() {
        parts.push(forge_ui::trf!(
            "claimed (read-only for you): {what}",
            what = claims.join(", ")
        ));
    }
    Some(parts.join(" \u{b7} "))
}

fn cells_style(n: usize, gap: f32) -> NodeStyle {
    let (cols, rows): (u16, usize) = match n {
        1 => (1, 1),
        2 => (2, 1),
        _ => (2, 2),
    };
    let mut s = NodeStyle::grid(cols, gap).grow(1.0);
    s.layout.grid_template_rows = vec![taffy::prelude::fr(1.0); rows];
    s.layout.min_size = taffy::Size {
        width: taffy::prelude::length(0.0),
        height: taffy::prelude::length(0.0),
    };
    s
}

/// The tools the toolbar offers (everything but brushes, which a plugin's panel picks).
fn toolbar_tools(services: &EditorServices) -> Vec<(String, String)> {
    services
        .viewport_tools
        .iter()
        .filter(|(_, t)| t.category != ToolCategory::Brush)
        .map(|(k, t)| {
            (
                k.to_string(),
                forge_ui::trf!(
                    "{tool} ({key})",
                    tool = forge_ui::l10n::tr_str(&t.title),
                    key = t.hint
                ),
            )
        })
        .collect()
}

/// Build the viewport panel.
pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("the view of the world"));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let services = pb.services();
        let handles = pb.handles();
        let tools = toolbar_tools(&services);
        let shared = services.viewport.borrow().clone();

        // ---- the toolbar ----
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space[2]).padding(space[1]).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Viewport tools")),
        )?;
        let tool_idx = tools
            .iter()
            .position(|(k, _)| *k == shared.tool)
            .unwrap_or(usize::MAX);
        let tool_sig: Signal<usize> = pb.b.signal(tool_idx);
        let titles: Vec<&str> = tools.iter().map(|(_, t)| t.as_str()).collect();
        pb.b.add(
            bar,
            "tool",
            NodeStyle::leaf(),
            SegmentedControl::new(forge_ui::tr!("Tool"), &titles, tool_sig),
        )?;
        let tool_relay = pb.b.add(
            bar,
            "tool_relay",
            NodeStyle::leaf(),
            SignalRelay::new(tool_sig.any()),
        )?;
        let space_sig: Signal<usize> = pb.b.signal(
            GizmoSpace::ALL
                .iter()
                .position(|s| *s == shared.settings.space)
                .unwrap_or(1),
        );
        let space_names: Vec<&str> = GizmoSpace::ALL.iter().map(|s| s.label()).collect();
        pb.b.add(
            bar,
            "space",
            NodeStyle::leaf(),
            SegmentedControl::new(forge_ui::tr!("Space"), &space_names, space_sig),
        )?;
        let space_relay = pb.b.add(
            bar,
            "space_relay",
            NodeStyle::leaf(),
            SignalRelay::new(space_sig.any()),
        )?;
        let snap0 = Model::snap(&pb.mirror()).enabled;
        let snap_sig: Signal<bool> = pb.b.signal(snap0);
        pb.b.add(
            bar,
            "snap",
            NodeStyle::leaf(),
            Switch::new(snap_sig, forge_ui::tr!("Snap")),
        )?;
        let snap_relay = pb.b.add(
            bar,
            "snap_relay",
            NodeStyle::leaf(),
            SignalRelay::new(snap_sig.any()),
        )?;
        let nav_sig: Signal<usize> = pb.b.signal(0);
        let nav_names: Vec<&str> = NavMode::ALL.iter().map(|n| n.label()).collect();
        pb.b.add(
            bar,
            "nav",
            NodeStyle::leaf(),
            SegmentedControl::new(forge_ui::tr!("Navigate"), &nav_names, nav_sig),
        )?;
        let nav_relay = pb.b.add(
            bar,
            "nav_relay",
            NodeStyle::leaf(),
            SignalRelay::new(nav_sig.any()),
        )?;
        let colliders_sig: Signal<bool> = pb.b.signal(true);
        pb.b.add(
            bar,
            "colliders",
            NodeStyle::leaf(),
            Switch::new(colliders_sig, forge_ui::tr!("Colliders")),
        )?;
        let colliders_relay = pb.b.add(
            bar,
            "colliders_relay",
            NodeStyle::leaf(),
            SignalRelay::new(colliders_sig.any()),
        )?;
        let layout_sig: Signal<usize> = pb.b.signal(0);
        pb.b.add(
            bar,
            "layout",
            NodeStyle::leaf(),
            SegmentedControl::new(forge_ui::tr!("Viewports"), &["1", "2", "4"], layout_sig),
        )?;
        let layout_relay = pb.b.add(
            bar,
            "layout_relay",
            NodeStyle::leaf(),
            SignalRelay::new(layout_sig.any()),
        )?;
        let frame_btn = pb.b.add(
            bar,
            "frame",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Frame selection (F)")),
        )?;
        let leave_btn = pb.b.add(
            bar,
            "leave_world",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Back to the scene")),
        )?;
        pb.b.hide(leave_btn, true);
        // A scene opened in isolation ("Open base scene", WP-U20): what is edited, and the
        // way back to the world.
        let scene_label = pb.b.signal(String::new());
        let scene_note = pb.b.add(
            bar,
            "scene_open",
            NodeStyle::leaf(),
            Label::new(scene_label).kind(LabelKind::Small),
        )?;
        pb.b.hide(scene_note, true);
        let scene_back = pb.b.add(
            bar,
            "scene_back",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Back to the world")),
        )?;
        pb.b.hide(scene_back, true);
        pb.on(scene_back, |act, _: &Pressed| act.session.open_scene(None));

        // ---- the cells ----
        let grid = pb.b.add(
            pb.parent,
            "cells",
            cells_style(1, 2.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Viewports")),
        )?;
        let mut cells = Vec::with_capacity(MAX_CELLS);
        for (i, name) in CELL_NAMES.iter().enumerate() {
            let mut ctl = ViewportController::new(
                cell_camera(i),
                &shared.tool,
                Model::make_tool(&services, &shared.tool),
            );
            ctl.settings = shared.settings.clone();
            cells.push(Cell {
                ctl,
                surface: services.viewport_surfaces.borrow_mut().allocate(),
                name,
                hover_seed: None,
            });
        }
        let mut scene = ViewportScene::new();
        scene.sync(&pb.mirror());
        let model: Shared = Rc::new(RefCell::new(Model {
            handles: handles.clone(),
            services: Rc::clone(&services),
            scene,
            sim: None,
            cells,
            active: 0,
            layout: 1,
            drawables: RefCell::new(None),
            seen_play: u64::MAX,
            seen_preview: u64::MAX,
            seen_camera: pb.session().camera_revision(),
            seen_tool: shared.revision(),
            seen_selection: pb.session().selection_revision(),
            seen_collab: u64::MAX,
            paints: RefCell::new([0; MAX_CELLS]),
            show_colliders: true,
            phys: Vec::new(),
            phys_drawn: RefCell::new([0; MAX_CELLS]),
            world: services
                .viewport_worlds
                .iter()
                .next()
                .map(|(_, make)| make(&services)),
            host_feed: {
                let feed = LiveCell::new();
                let bump = Arc::clone(&feed);
                services
                    .viewport_surfaces
                    .borrow_mut()
                    .set_waker(Arc::new(move || bump.bump()));
                feed
            },
        }));
        let mut canvases = Vec::with_capacity(MAX_CELLS);
        for i in 0..MAX_CELLS {
            let mut st = NodeStyle::default();
            st.layout.min_size = taffy::Size {
                width: taffy::prelude::length(0.0),
                height: taffy::prelude::length(0.0),
            };
            let c = pb.b.add(
                grid,
                forge_ui::Key::Index(i as u32),
                st,
                ViewportCanvas::new(Rc::clone(&model), i),
            )?;
            if i > 0 {
                pb.b.hide(c, true);
            }
            canvases.push(c);
        }
        let canvases = Rc::new(canvases);

        // ---- handlers ----
        for (i, c) in canvases.iter().enumerate() {
            let (m, cv) = (Rc::clone(&model), Rc::clone(&canvases));
            pb.on(*c, move |act, e: &CanvasInput| {
                let mut model = m.borrow_mut();
                model.active = e.cell;
                let services = Rc::clone(&model.services);
                let Some(rect) = act.ui.rect(cv[i]) else {
                    return;
                };
                let snap = Model::snap(act.mirror);
                let layers = services.viewport_layers.borrow();
                let frames = model.frames();
                let tick = model.tick();
                let Model {
                    scene, sim, cells, ..
                } = &mut *model;
                let shown: &ViewportScene = sim.as_ref().unwrap_or(scene);
                let cell = &mut cells[e.cell];
                cell.ctl.settings.snap = snap;
                let _ = cell.ctl.input(
                    ViewInput::Resize {
                        w: f64::from(rect.w),
                        h: f64::from(rect.h),
                    },
                    Handles {
                        mirror: act.mirror,
                        cmd: act.cmd,
                        session: act.session,
                        frames: frames.as_ref(),
                        tick,
                        layers: &layers,
                    },
                    shown,
                );
                let reply = cell.ctl.input(
                    e.input,
                    Handles {
                        mirror: act.mirror,
                        cmd: act.cmd,
                        session: act.session,
                        frames: frames.as_ref(),
                        tick,
                        layers: &layers,
                    },
                    shown,
                );
                // The seed path under the pointer (generated content).
                if let ViewInput::Move { at, .. } = e.input {
                    let seed = pick(
                        shown,
                        &cell.ctl.camera,
                        frames.as_ref(),
                        tick,
                        at,
                        cell.ctl.size,
                        false,
                    )
                    .and_then(|p| shown.get(p.key))
                    .and_then(|d| d.seed_path.clone());
                    if seed != cell.hover_seed {
                        cell.hover_seed = seed;
                        act.ui.invalidate(cv[i], Dirty::PAINT);
                    }
                }
                if reply.repaint {
                    act.ui.invalidate(cv[i], Dirty::PAINT | Dirty::A11Y);
                }
            });
            let (m, cv) = (Rc::clone(&model), Rc::clone(&canvases));
            let tool_ids: Vec<(char, &'static str)> = vec![
                ('q', "forge.tool.select"),
                ('w', "forge.tool.move"),
                ('e', "forge.tool.rotate"),
                ('r', "forge.tool.scale"),
            ];
            pb.on(*c, move |act, e: &ToolShortcut| {
                let Some((_, id)) = tool_ids.iter().find(|(c, _)| *c == e.0) else {
                    return;
                };
                let services = Rc::clone(&m.borrow().services);
                if services.viewport_tools.get(id).is_some() {
                    services
                        .viewport
                        .borrow_mut()
                        .update(|s| s.tool = (*id).to_string());
                }
                for c in cv.iter() {
                    act.ui.invalidate(*c, Dirty::PAINT);
                }
            });
        }
        let s2 = Rc::clone(&services);
        let tool_keys: Vec<String> = tools.iter().map(|(k, _)| k.clone()).collect();
        pb.on(tool_relay, move |act, _: &SignalChanged| {
            let i = tool_sig.get(act.ui.rt());
            if let Some(k) = tool_keys.get(i) {
                s2.viewport.borrow_mut().update(|s| s.tool = k.clone());
            }
        });
        let s2 = Rc::clone(&services);
        pb.on(space_relay, move |act, _: &SignalChanged| {
            let i = space_sig.get(act.ui.rt());
            if let Some(sp) = GizmoSpace::ALL.get(i) {
                s2.viewport.borrow_mut().update(|s| s.settings.space = *sp);
            }
        });
        pb.on(snap_relay, move |act, _: &SignalChanged| {
            // Snapping is a project setting the team shares: a command (I7).
            let on = snap_sig.get(act.ui.rt());
            if Model::snap(act.mirror).enabled != on {
                act.cmd.emit(EditorCommand::SetSetting {
                    key: "editor.snap_translate".into(),
                    value: Some(Value::Bool(on)),
                });
            }
        });
        let (m, cv) = (Rc::clone(&model), Rc::clone(&canvases));
        pb.on(nav_relay, move |act, _: &SignalChanged| {
            let nav = NavMode::ALL
                .get(nav_sig.get(act.ui.rt()))
                .copied()
                .unwrap_or_default();
            for c in m.borrow_mut().cells.iter_mut() {
                c.ctl.nav = nav;
            }
            for c in cv.iter() {
                act.ui.invalidate(*c, Dirty::PAINT);
            }
        });
        let (m, cv, s2) = (
            Rc::clone(&model),
            Rc::clone(&canvases),
            Rc::clone(&services),
        );
        pb.on(colliders_relay, move |act, _: &SignalChanged| {
            // A view preference of this editor, like the navigation mode: session state.
            {
                let mut model = m.borrow_mut();
                model.show_colliders = colliders_sig.get(act.ui.rt());
                model.refresh_physics(&*s2.play.borrow());
            }
            for c in cv.iter() {
                act.ui.invalidate(*c, Dirty::PAINT);
            }
        });
        let (m, cv) = (Rc::clone(&model), Rc::clone(&canvases));
        pb.on(layout_relay, move |act, _: &SignalChanged| {
            let n = [1, 2, 4]
                .get(layout_sig.get(act.ui.rt()))
                .copied()
                .unwrap_or(1);
            {
                let mut model = m.borrow_mut();
                model.layout = n;
                // The hidden cells let go of what the world held for them.
                if let Some(w) = model.world.as_mut() {
                    for i in n..MAX_CELLS {
                        w.cell_hidden(i);
                    }
                }
            }
            let gap = act.ui.theme().space[0].max(2.0);
            let _ = act.ui.set_style(grid, cells_style(n, gap));
            for (i, c) in cv.iter().enumerate() {
                let _ = act.ui.set_hidden(*c, i >= n);
            }
        });
        let (m, cv) = (Rc::clone(&model), Rc::clone(&canvases));
        pb.on(frame_btn, move |act, _: &Pressed| {
            let mut model = m.borrow_mut();
            let a = model.active;
            let frames = model.frames();
            let sel = act.session.selection.clone();
            let Model { scene, cells, .. } = &mut *model;
            cells[a]
                .ctl
                .frame_selection(&sel, scene, frames.as_ref(), Tick(0));
            act.ui.invalidate(cv[a], Dirty::PAINT);
        });

        // Back to the project scene from a world.
        let (m2, cv2) = (Rc::clone(&model), Rc::clone(&canvases));
        pb.on(leave_btn, move |act, _: &Pressed| {
            m2.borrow_mut().leave_world();
            for c in cv2.iter() {
                act.ui.invalidate(*c, Dirty::PAINT | Dirty::A11Y);
            }
        });

        // ---- following the project, the session and the play core ----
        let (m, cv) = (model, canvases);
        let tool_keys: Vec<String> = tools.iter().map(|(k, _)| k.clone()).collect();
        let collab_relay = pb.b.add(
            bar,
            "collab_feed",
            NodeStyle::leaf(),
            forge_editor::feed::FeedRelay::new(),
        )?;
        pb.on(collab_relay, |act, _: &forge_editor::feed::FeedTicked| {
            act.want_turn()
        });
        // The world's background work and the GPU host's paced frames wake the panel.
        let world_relay = pb.b.add(
            bar,
            "world_feed",
            NodeStyle::leaf(),
            forge_editor::feed::FeedRelay::new(),
        )?;
        pb.on(world_relay, |act, _: &forge_editor::feed::FeedTicked| {
            act.want_turn()
        });
        let host_relay = pb.b.add(
            bar,
            "host_feed",
            NodeStyle::leaf(),
            forge_editor::feed::FeedRelay::new(),
        )?;
        pb.on(host_relay, |act, _: &forge_editor::feed::FeedTicked| {
            act.want_turn()
        });
        let mut feeds = false;
        let mut seen_world = 0u64;
        let mut seen_host = 0u64;
        let mut collab_feed = false;
        let mut shown_leave = false;
        let mut seen_scene_focus = u64::MAX;
        pb.sync(grid, move |sy| {
            let mut model = m.borrow_mut();
            let services = Rc::clone(&model.services);
            // The scene open in isolation, if it still exists (WP-U20).
            let focus = sy
                .session
                .scene_focus()
                .filter(|r| sy.mirror.entity(*r).is_some());
            if sy.session.scene_focus_revision() != seen_scene_focus
                || model.scene.isolation() != focus
            {
                seen_scene_focus = sy.session.scene_focus_revision();
                model.scene.set_isolation(focus);
                let name = focus
                    .and_then(|r| sy.mirror.entity(r))
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                scene_label.set(
                    sy.ui.rt_mut(),
                    forge_ui::trf!("Editing the scene \u{201c}{name}\u{201d}", name),
                );
                let _ = sy.ui.set_hidden(scene_note, focus.is_none());
                let _ = sy.ui.set_hidden(scene_back, focus.is_none());
            }
            let mut repaint = model.scene.sync(sy.mirror);
            repaint |= services.viewport_layers.borrow_mut().sync(sy.mirror);
            // The play sandbox; while none runs, the sequencer's preview (WP-U11: a clip
            // scrubbed or played through the play core, session state like play).
            {
                let play = services.play.borrow();
                let rev = play.revision();
                let preview = services.anim_preview.borrow();
                let prev = preview.revision();
                if rev != model.seen_play || prev != model.seen_preview || repaint {
                    model.seen_play = rev;
                    model.seen_preview = prev;
                    let sim = if play.state() != PlayState::Stopped {
                        Some(model.scene.with_overrides(play.transforms(), rev))
                    } else {
                        // The top bit keeps preview versions apart from play versions.
                        preview
                            .overrides()
                            .map(|(t, r)| model.scene.with_overrides(t, r | (1 << 63)))
                    };
                    if sim.is_some() || model.sim.is_some() {
                        repaint = true;
                    }
                    model.sim = sim;
                    model.refresh_physics(&*play);
                }
            }
            // The shared tool and settings.
            let shared = services.viewport.borrow().clone();
            if shared.revision() != model.seen_tool {
                model.seen_tool = shared.revision();
                for c in model.cells.iter_mut() {
                    if c.ctl.tool_id != shared.tool {
                        c.ctl.tool = Model::make_tool(&services, &shared.tool);
                        c.ctl.tool_id = shared.tool.clone();
                    }
                    let snap = c.ctl.settings.snap;
                    c.ctl.settings = shared.settings.clone();
                    c.ctl.settings.snap = snap;
                }
                let idx = tool_keys
                    .iter()
                    .position(|k| *k == shared.tool)
                    .unwrap_or(usize::MAX);
                tool_sig.set(sy.ui.rt_mut(), idx);
                repaint = true;
            }
            let snap = Model::snap(sy.mirror);
            snap_sig.set(sy.ui.rt_mut(), snap.enabled);
            // The world's feed (its background work) and the GPU host's (a frame it paced)
            // wake the panel only when they have something.
            if !feeds {
                feeds = true;
                if let Some(w) = model.world.as_deref() {
                    sy.ui.add_feed(
                        world_relay,
                        forge_ui::LiveFeed {
                            source: w.feed() as Arc<dyn forge_ui::LiveSource>,
                            max_hz: 120,
                            self_ui: false,
                        },
                    );
                }
                sy.ui.add_feed(
                    host_relay,
                    forge_ui::LiveFeed {
                        source: Arc::clone(&model.host_feed) as Arc<dyn forge_ui::LiveSource>,
                        max_hz: 120,
                        self_ui: false,
                    },
                );
            }
            // A session camera focus (a navigator's "Fly to", a console entry's seed path):
            // the world takes the camera there if it has such a place; otherwise frame the
            // placed entity with that seed path.
            let mut launched = None;
            if sy.session.camera_revision() != model.seen_camera {
                model.seen_camera = sy.session.camera_revision();
                if let Some(focus) = sy.session.camera_focus().cloned() {
                    let a = model.active;
                    let launch = {
                        let Model { world, cells, .. } = &mut *model;
                        match world.as_mut() {
                            Some(w) => w.focus(&mut Cams(cells), a, &focus.seed_path, &focus.name),
                            None => WorldLaunch::Nothing,
                        }
                    };
                    if launch == WorldLaunch::Started {
                        launched = Some(a);
                    }
                    if launch != WorldLaunch::Nothing {
                        repaint = true;
                    }
                    let shown = model.shown_world().is_some();
                    let target = model
                        .scene
                        .iter()
                        .find(|d| d.seed_path.as_deref() == Some(focus.seed_path.as_str()))
                        .map(|d| d.key)
                        .filter(|_| launch == WorldLaunch::Nothing && !shown);
                    let frames = services.frames.clone();
                    if let Some(k) = target {
                        let Model { scene, cells, .. } = &mut *model;
                        cells[a]
                            .ctl
                            .frame_selection(&[k], scene, frames.as_ref(), Tick(0));
                        repaint = true;
                    }
                }
            }
            // The world's background work finished something: a planned move to start, a move
            // to resume, new content to draw.
            let world_gen = model.world.as_deref().map_or(0, |w| w.feed().generation());
            if world_gen != seen_world {
                seen_world = world_gen;
                let Model { world, cells, .. } = &mut *model;
                if let Some(w) = world.as_mut() {
                    if let Some(a) = w.poll(&mut Cams(cells)) {
                        launched = launched.or(Some(a));
                    }
                    if w.shown() {
                        repaint = true;
                    }
                }
            }
            // The GPU host has more to do for a frame it rendered: request it again.
            let host_gen = model.host_feed.generation();
            if host_gen != seen_host {
                seen_host = host_gen;
                repaint = true;
            }
            // Selection changes show as highlighted boxes (the canvas reads the selection
            // when it paints).
            if sy.session.selection_revision() != model.seen_selection {
                model.seen_selection = sy.session.selection_revision();
                repaint = true;
            }
            // Teammates' presence and claims (WP-U10): drawn in the overlay.
            if services.collab.attached() {
                if !collab_feed {
                    collab_feed = true;
                    sy.ui.add_feed(
                        collab_relay,
                        forge_ui::LiveFeed {
                            source: services.collab.feed(),
                            max_hz: crate::hierarchy::COLLAB_HZ,
                            self_ui: false,
                        },
                    );
                }
                let rev = services.collab.revision();
                if rev != model.seen_collab {
                    model.seen_collab = rev;
                    repaint = true;
                }
            }
            let n = model.layout;
            let in_world = model.shown_world().is_some();
            drop(model);
            if in_world != shown_leave {
                shown_leave = in_world;
                let _ = sy.ui.set_hidden(leave_btn, !in_world);
            }
            if repaint {
                for c in cv.iter().take(n) {
                    sy.ui.invalidate(*c, Dirty::PAINT | Dirty::A11Y);
                }
            }
            // A flight animates its cell: its first frame is delivered now, the cell asks for
            // the rest (and stops asking when it arrives: idle again).
            if let Some(a) = launched {
                let _ = sy.ui.send(cv[a], &UiEvent::AnimFrame);
            }
            Ok(())
        });
        Ok(())
    });
}

/// The keys of a viewport's cells in the panel (tests reach them by these).
pub fn cell_key(i: usize) -> forge_ui::Key {
    forge_ui::Key::Index(i as u32)
}
