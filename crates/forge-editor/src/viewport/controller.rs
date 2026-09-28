//! One viewport's controller: pointer, wheel and key input → camera navigation or the
//! active tool (Ch.21 §21.21). It is UI-toolkit-free so the navigation rules are tested
//! directly; the viewport widget turns its events into [`ViewInput`]s and the viewport
//! panel's handler runs them here with the handles a tool needs.
//!
//! **Input rules.** Secondary drag looks around (and W/A/S/D/Q/E fly while it is held, the
//! wheel then sets the fly speed); middle drag pans; Alt + primary drag orbits; the wheel
//! dollies toward the pivot. A primary press goes to the active tool first; if the tool
//! does not capture it, a drag navigates by the [`NavMode`] and a press-and-release without
//! movement is a click (select). F frames the selection; Esc cancels the tool's drag.
//!
//! Navigation is session state: it never emits a command. Only tools do, and only through
//! the emitter.

use forge_cmd::EntityKey;
use forge_frames::{FrameResolver, Tick};
use forge_ui::Modifiers;

use crate::emitter::CommandEmitter;
use crate::mirror::ProjectMirror;
use crate::session::SessionState;
use crate::viewport::camera::{EditorCamera, FlyInput, NavMode};
use crate::viewport::layer::ViewportLayers;
use crate::viewport::scene::ViewportScene;
use crate::viewport::tool::{ToolBehavior, ToolCx, ToolInput, ToolReply, ToolSettings, ToolView};

/// Pixels a press may travel and still be a click.
pub const CLICK_SLOP: f64 = 3.0;

/// A pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Primary,
    Secondary,
    Middle,
}

/// The fly keys (and the viewport's own shortcuts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKey {
    Forward,
    Back,
    Left,
    Right,
    Up,
    Down,
    /// F: frame the selection.
    Frame,
    /// Esc.
    Cancel,
}

/// Input for one viewport (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewInput {
    Down {
        at: (f64, f64),
        button: Button,
        mods: Modifiers,
    },
    Move {
        at: (f64, f64),
        mods: Modifiers,
    },
    Up {
        at: (f64, f64),
        button: Button,
    },
    /// Wheel notches (positive: away from the user = in).
    Wheel {
        notches: f64,
    },
    Key {
        key: ViewKey,
        pressed: bool,
        shift: bool,
    },
    /// An animation frame while flying: `dt` seconds since the last.
    Frame {
        dt: f64,
    },
    /// The viewport's size in logical pixels.
    Resize {
        w: f64,
        h: f64,
    },
}

/// What the viewport must do after an input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewReply {
    /// Redraw (the camera moved, a tool's overlay changed).
    pub repaint: bool,
    /// Keep receiving animation frames (keys held while flying).
    pub animate: bool,
    /// Capture the pointer (a drag in progress) or release it.
    pub capture: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// A primary press the tool did not take: a click until it moves.
    Pending {
        from: (f64, f64),
        last: (f64, f64),
        mods: Modifiers,
    },
    Orbit {
        last: (f64, f64),
    },
    Pan {
        last: (f64, f64),
    },
    Look {
        last: (f64, f64),
    },
    /// The tool owns it.
    Tool,
}

/// What the controller needs from the shell for one input.
pub struct Handles<'a> {
    pub mirror: &'a ProjectMirror,
    pub cmd: &'a CommandEmitter,
    pub session: &'a mut SessionState,
    pub frames: &'a dyn FrameResolver,
    pub tick: Tick,
    pub layers: &'a ViewportLayers,
}

/// One viewport's state (see the module docs).
pub struct ViewportController {
    pub camera: EditorCamera,
    pub nav: NavMode,
    pub size: (f64, f64),
    /// The active tool's id and behaviour.
    pub tool_id: String,
    pub tool: Box<dyn ToolBehavior>,
    pub settings: ToolSettings,
    drag: Option<Drag>,
    fly: FlyInput,
    /// Where the pointer is (for the seed-path overlay), if over the viewport.
    pub hover: Option<(f64, f64)>,
    /// Bumped whenever the camera moved.
    camera_rev: u64,
}

impl std::fmt::Debug for ViewportController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewportController")
            .field("camera", &self.camera)
            .field("nav", &self.nav)
            .field("tool", &self.tool_id)
            .finish_non_exhaustive()
    }
}

impl ViewportController {
    pub fn new(camera: EditorCamera, tool_id: &str, tool: Box<dyn ToolBehavior>) -> Self {
        Self {
            camera,
            nav: NavMode::Orbit,
            size: (1.0, 1.0),
            tool_id: tool_id.to_string(),
            tool,
            settings: ToolSettings::default(),
            drag: None,
            fly: FlyInput::default(),
            hover: None,
            camera_rev: 0,
        }
    }

    pub fn camera_revision(&self) -> u64 {
        self.camera_rev
    }

    /// Replace the camera (a fly-to); counts as a camera move.
    pub fn set_camera(&mut self, c: EditorCamera) {
        if c != self.camera {
            self.camera = c;
            self.camera_rev += 1;
        }
    }

    /// Switch tools. A drag in progress is cancelled first (its gesture undone).
    pub fn set_tool(
        &mut self,
        id: &str,
        tool: Box<dyn ToolBehavior>,
        h: Handles<'_>,
        scene: &ViewportScene,
    ) {
        if self.tool.dragging() {
            let _ = self.run_tool(ToolInput::Cancel, h, scene);
        }
        self.tool_id = id.to_string();
        self.tool = tool;
        self.drag = None;
    }

    /// Whether anything is held (a navigation drag or fly keys).
    pub fn busy(&self) -> bool {
        self.drag.is_some() || !self.fly.is_idle()
    }

    /// The view a tool reads.
    pub fn view<'a>(
        &'a self,
        scene: &'a ViewportScene,
        mirror: &'a ProjectMirror,
        frames: &'a dyn FrameResolver,
        tick: Tick,
        selection: &'a [EntityKey],
        layers: &'a ViewportLayers,
    ) -> ToolView<'a> {
        ToolView {
            camera: &self.camera,
            size: self.size,
            scene,
            frames,
            tick,
            selection,
            settings: &self.settings,
            layers,
            mirror,
        }
    }

    fn run_tool(&mut self, ev: ToolInput, h: Handles<'_>, scene: &ViewportScene) -> ToolReply {
        let selection = h.session.selection.clone();
        let view = ToolView {
            camera: &self.camera,
            size: self.size,
            scene,
            frames: h.frames,
            tick: h.tick,
            selection: &selection,
            settings: &self.settings,
            layers: h.layers,
            mirror: h.mirror,
        };
        let mut cx = ToolCx {
            view,
            cmd: h.cmd,
            session: h.session,
        };
        self.tool.input(&mut cx, &ev)
    }

    fn moved(&mut self) -> ViewReply {
        self.camera_rev += 1;
        ViewReply {
            repaint: true,
            ..ViewReply::default()
        }
    }

    /// Handle one input.
    pub fn input(&mut self, ev: ViewInput, h: Handles<'_>, scene: &ViewportScene) -> ViewReply {
        let mut r = ViewReply::default();
        match ev {
            ViewInput::Resize { w, h: hh } => {
                if (w, hh) != self.size && w > 0.0 && hh > 0.0 {
                    self.size = (w, hh);
                    r.repaint = true;
                }
            }
            ViewInput::Down { at, button, mods } => {
                self.hover = Some(at);
                let d = match button {
                    Button::Secondary => Drag::Look { last: at },
                    Button::Middle => Drag::Pan { last: at },
                    Button::Primary if mods.alt => Drag::Orbit { last: at },
                    Button::Primary => match self.run_tool(ToolInput::Press(at, mods), h, scene) {
                        ToolReply::Captured => {
                            r.repaint = true;
                            Drag::Tool
                        }
                        _ => Drag::Pending {
                            from: at,
                            last: at,
                            mods,
                        },
                    },
                };
                self.drag = Some(d);
                r.capture = Some(true);
            }
            ViewInput::Move { at, mods } => {
                self.hover = Some(at);
                match self.drag {
                    None => {
                        if self.run_tool(ToolInput::Hover(at), h, scene) != ToolReply::Ignored {
                            r.repaint = true;
                        }
                        // The seed-path readout follows the pointer.
                        r.repaint |= scene.iter().any(|d| d.seed_path.is_some());
                    }
                    Some(Drag::Tool) => {
                        self.run_tool(ToolInput::Drag(at, mods), h, scene);
                        r.repaint = true;
                    }
                    Some(Drag::Pending {
                        from,
                        last,
                        mods: m0,
                    }) => {
                        let dist = ((at.0 - from.0).powi(2) + (at.1 - from.1).powi(2)).sqrt();
                        if dist > CLICK_SLOP {
                            self.drag = Some(match self.nav {
                                NavMode::Orbit => Drag::Orbit { last },
                                NavMode::Pan => Drag::Pan { last },
                                NavMode::Fly => Drag::Look { last },
                            });
                            return self.input(ViewInput::Move { at, mods }, h, scene);
                        }
                        self.drag = Some(Drag::Pending {
                            from,
                            last,
                            mods: m0,
                        });
                    }
                    Some(Drag::Orbit { last }) => {
                        self.camera.orbit(at.0 - last.0, at.1 - last.1);
                        self.drag = Some(Drag::Orbit { last: at });
                        r = self.moved();
                    }
                    Some(Drag::Pan { last }) => {
                        self.camera.pan(at.0 - last.0, at.1 - last.1, self.size.1);
                        self.drag = Some(Drag::Pan { last: at });
                        r = self.moved();
                    }
                    Some(Drag::Look { last }) => {
                        self.camera.look(at.0 - last.0, at.1 - last.1);
                        self.drag = Some(Drag::Look { last: at });
                        r = self.moved();
                    }
                }
            }
            ViewInput::Up { at, button } => {
                let d = self.drag.take();
                r.capture = Some(false);
                match (d, button) {
                    (Some(Drag::Tool), _) => {
                        self.run_tool(ToolInput::Release(at), h, scene);
                        r.repaint = true;
                    }
                    (Some(Drag::Pending { mods, .. }), Button::Primary) => {
                        self.run_tool(ToolInput::Click(at, mods), h, scene);
                        r.repaint = true;
                    }
                    (Some(Drag::Look { .. }), _) => {
                        // Letting go of the look button stops flying.
                        self.fly = FlyInput::default();
                    }
                    _ => {}
                }
            }
            ViewInput::Wheel { notches } => {
                if matches!(self.drag, Some(Drag::Look { .. })) {
                    self.camera.adjust_speed(notches.signum() as i32);
                    r.repaint = true;
                } else {
                    self.camera.dolly(notches);
                    r = self.moved();
                }
            }
            ViewInput::Key {
                key,
                pressed,
                shift,
            } => {
                let v = if pressed { 1.0 } else { 0.0 };
                match key {
                    ViewKey::Forward => self.fly.forward = v,
                    ViewKey::Back => self.fly.forward = -v,
                    ViewKey::Right => self.fly.right = v,
                    ViewKey::Left => self.fly.right = -v,
                    ViewKey::Up => self.fly.up = v,
                    ViewKey::Down => self.fly.up = -v,
                    ViewKey::Frame if pressed => {
                        let sel = h.session.selection.clone();
                        self.frame_selection(&sel, scene, h.frames, h.tick);
                        return self.moved();
                    }
                    ViewKey::Cancel if pressed => {
                        if self.run_tool(ToolInput::Cancel, h, scene) != ToolReply::Ignored {
                            r.repaint = true;
                        }
                        if matches!(self.drag, Some(Drag::Tool)) {
                            self.drag = None;
                            r.capture = Some(false);
                        }
                    }
                    _ => {}
                }
                self.fly.boost = shift;
                r.animate = !self.fly.is_idle();
            }
            ViewInput::Frame { dt } => {
                if !self.fly.is_idle() {
                    self.camera.fly(self.fly, dt.clamp(0.0, 0.1));
                    r = self.moved();
                    r.animate = true;
                }
            }
        }
        r
    }

    /// Frame the selection (or everything when nothing is selected).
    pub fn frame_selection(
        &mut self,
        sel: &[EntityKey],
        scene: &ViewportScene,
        frames: &dyn FrameResolver,
        tick: Tick,
    ) {
        let items: Vec<_> = if sel.is_empty() {
            scene.iter().collect()
        } else {
            sel.iter().filter_map(|k| scene.get(*k)).collect()
        };
        let Some(first) = items.first() else {
            return;
        };
        // Bounding sphere around the first item's frame, all offsets in f64.
        let base = first.pos;
        let mut lo = forge_frames::DVec3::splat(f64::INFINITY);
        let mut hi = forge_frames::DVec3::splat(f64::NEG_INFINITY);
        for d in &items {
            let Ok(p) = frames.resolve(d.pos, base.frame, tick) else {
                continue;
            };
            let r = d.radius();
            let v = p.local - base.local;
            lo = forge_frames::DVec3::new(lo.x.min(v.x - r), lo.y.min(v.y - r), lo.z.min(v.z - r));
            hi = forge_frames::DVec3::new(hi.x.max(v.x + r), hi.y.max(v.y + r), hi.z.max(v.z + r));
        }
        if !lo.is_finite() || !hi.is_finite() {
            return;
        }
        let mid = (lo + hi) * 0.5;
        let radius = (hi - lo).length() * 0.5;
        let target = forge_frames::FramePos::new(base.frame, base.local + mid);
        self.camera.frame_sphere(frames, tick, target, radius);
        self.camera_rev += 1;
    }
}
