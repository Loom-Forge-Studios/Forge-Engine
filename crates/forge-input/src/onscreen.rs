//! On-screen touch controls (Ch.28 §28.15): virtual sticks and buttons that drive a
//! **virtual gamepad**, so every action already bound to `Gamepad/LeftStick` or
//! `Gamepad/South` works on a phone or a touch laptop with no extra binding (Unity's
//! on-screen controls model). The game UI draws them (`forge-runtime`'s
//! `OnScreenControlsView`); this is their logic, headless and testable.
//!
//! * A finger that lands on a control is **captured** by it until it lifts, so a stick and a
//!   button work at once and a thumb sliding off a button does not press its neighbour.
//! * A **floating** stick recentres where the thumb lands inside its zone (twice its
//!   radius), the comfortable default on phones; a fixed stick measures from its centre.
//! * Stick deflection is the offset over the radius, clamped to the unit circle, with y up.
//!   The stick also drives its X and Y axes (`LeftStickX`/`LeftStickY`).
//! * Fingers that land on no control are not consumed: they go on to the touch device's
//!   gestures (camera swipes, pinch to zoom).

use crate::control::{Device, control_index};
use crate::glyph::PadFamily;
use crate::runtime::{DeviceDesc, DeviceId, InputRuntime};
use crate::touch::{TouchInput, TouchPhase};

#[derive(Clone, Debug, PartialEq)]
pub enum OnScreenKind {
    Stick {
        /// `LeftStick` or `RightStick`.
        control: String,
        radius: f32,
        floating: bool,
    },
    Button {
        /// A gamepad button (`South`, `Start`, ...).
        control: String,
        radius: f32,
    },
}

/// One on-screen control.
#[derive(Clone, Debug, PartialEq)]
pub struct OnScreenControl {
    pub id: String,
    pub kind: OnScreenKind,
    /// Its centre as a fraction of the screen (`[0.15, 0.8]`: lower left).
    pub anchor: [f32; 2],
    /// Its accessible name: a localisation key.
    pub label: String,
}

/// A control's visual state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OnScreenState {
    pub pressed: bool,
    /// The stick's deflection (y up), or zero.
    pub knob: [f32; 2],
    /// Where a floating stick's base sits now (px), or its centre.
    pub origin: [f32; 2],
}

/// The on-screen controls (see the module docs).
pub struct OnScreenControls {
    controls: Vec<OnScreenControl>,
    state: Vec<OnScreenState>,
    captures: Vec<(u64, usize)>,
    device: DeviceId,
    screen: [f32; 2],
    revision: u64,
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

impl OnScreenControls {
    /// Connect the virtual gamepad the controls drive.
    pub fn new(rt: &mut InputRuntime, controls: Vec<OnScreenControl>) -> Self {
        let device = rt.connect(
            DeviceDesc::new(Device::Gamepad, "On-screen controls", "onscreen")
                .family(PadFamily::Generic)
                .virtual_device(),
        );
        let n = controls.len();
        Self {
            controls,
            state: vec![OnScreenState::default(); n],
            captures: Vec::new(),
            device,
            screen: [1280.0, 720.0],
            revision: 0,
        }
    }

    /// A twin-stick layout: a floating move stick lower left, South and East lower right,
    /// Start upper right. Labels are localisation keys.
    pub fn default_layout() -> Vec<OnScreenControl> {
        vec![
            OnScreenControl {
                id: "move".into(),
                kind: OnScreenKind::Stick {
                    control: "LeftStick".into(),
                    radius: 70.0,
                    floating: true,
                },
                anchor: [0.15, 0.75],
                label: "onscreen.move".into(),
            },
            OnScreenControl {
                id: "south".into(),
                kind: OnScreenKind::Button {
                    control: "South".into(),
                    radius: 40.0,
                },
                anchor: [0.85, 0.8],
                label: "onscreen.south".into(),
            },
            OnScreenControl {
                id: "east".into(),
                kind: OnScreenKind::Button {
                    control: "East".into(),
                    radius: 40.0,
                },
                anchor: [0.93, 0.65],
                label: "onscreen.east".into(),
            },
            OnScreenControl {
                id: "start".into(),
                kind: OnScreenKind::Button {
                    control: "Start".into(),
                    radius: 28.0,
                },
                anchor: [0.93, 0.1],
                label: "onscreen.start".into(),
            },
        ]
    }

    pub fn device(&self) -> DeviceId {
        self.device
    }
    pub fn controls(&self) -> &[OnScreenControl] {
        &self.controls
    }
    pub fn state(&self, i: usize) -> OnScreenState {
        let mut s = self.state.get(i).copied().unwrap_or_default();
        if !s.pressed {
            s.origin = self.center(i);
        }
        s
    }
    /// Bumped whenever something visible changed (the view repaints on it).
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn set_screen(&mut self, w: f32, h: f32) {
        self.screen = [w.max(1.0), h.max(1.0)];
        self.revision += 1;
    }
    pub fn screen(&self) -> [f32; 2] {
        self.screen
    }
    /// A control's centre (px).
    pub fn center(&self, i: usize) -> [f32; 2] {
        self.controls.get(i).map_or([0.0; 2], |c| {
            [c.anchor[0] * self.screen[0], c.anchor[1] * self.screen[1]]
        })
    }
    pub fn radius(&self, i: usize) -> f32 {
        match self.controls.get(i).map(|c| &c.kind) {
            Some(OnScreenKind::Stick { radius, .. } | OnScreenKind::Button { radius, .. }) => {
                *radius
            }
            None => 0.0,
        }
    }

    fn hit(&self, p: [f32; 2]) -> Option<usize> {
        (0..self.controls.len()).find(|&i| {
            let zone = match &self.controls[i].kind {
                OnScreenKind::Stick {
                    radius, floating, ..
                } => {
                    if *floating {
                        radius * 2.0
                    } else {
                        radius * 1.2
                    }
                }
                OnScreenKind::Button { radius, .. } => radius * 1.1,
            };
            dist(p, self.center(i)) <= zone
        })
    }

    fn send_stick(&self, rt: &mut InputRuntime, control: &str, v: [f32; 2]) {
        rt.send(self.device, control, v);
        let (x, y) = match control {
            "LeftStick" => ("LeftStickX", "LeftStickY"),
            "RightStick" => ("RightStickX", "RightStickY"),
            _ => return,
        };
        if control_index(Device::Gamepad, x).is_some() {
            rt.send(self.device, x, [v[0], 0.0]);
            rt.send(self.device, y, [v[1], 0.0]);
        }
    }

    /// A finger event; `true` when a control took it (it does not reach the gestures).
    pub fn touch(&mut self, rt: &mut InputRuntime, t: TouchInput) -> bool {
        match t.phase {
            TouchPhase::Started => {
                let Some(i) = self.hit(t.pos) else {
                    return false;
                };
                if self.captures.iter().any(|(_, c)| *c == i) {
                    // One finger per control.
                    return true;
                }
                self.captures.push((t.id, i));
                let center = self.center(i);
                let kind = self.controls[i].kind.clone();
                let st = &mut self.state[i];
                st.pressed = true;
                match kind {
                    OnScreenKind::Stick {
                        control, floating, ..
                    } => {
                        st.origin = if floating { t.pos } else { center };
                        st.knob = [0.0; 2];
                        if !floating {
                            self.move_stick(rt, i, t.pos);
                        }
                        let _ = control;
                    }
                    OnScreenKind::Button { control, .. } => {
                        rt.send(self.device, &control, [1.0, 0.0]);
                    }
                }
                self.revision += 1;
                true
            }
            TouchPhase::Moved => {
                let Some(&(_, i)) = self.captures.iter().find(|(id, _)| *id == t.id) else {
                    return false;
                };
                if rt.faults().onscreen_no_capture() && self.hit(t.pos) != Some(i) {
                    // The control: no capture, so a thumb that slides off lets go.
                    let end = TouchInput {
                        phase: TouchPhase::Ended,
                        ..t
                    };
                    return self.touch(rt, end);
                }
                if matches!(self.controls[i].kind, OnScreenKind::Stick { .. }) {
                    self.move_stick(rt, i, t.pos);
                    self.revision += 1;
                }
                true
            }
            TouchPhase::Ended | TouchPhase::Canceled => {
                let Some(k) = self.captures.iter().position(|(id, _)| *id == t.id) else {
                    return false;
                };
                let (_, i) = self.captures.remove(k);
                self.state[i].pressed = false;
                self.state[i].knob = [0.0; 2];
                match self.controls[i].kind.clone() {
                    OnScreenKind::Stick { control, .. } => {
                        self.send_stick(rt, &control, [0.0; 2]);
                    }
                    OnScreenKind::Button { control, .. } => {
                        rt.send(self.device, &control, [0.0; 2]);
                    }
                }
                self.revision += 1;
                true
            }
        }
    }

    fn move_stick(&mut self, rt: &mut InputRuntime, i: usize, p: [f32; 2]) {
        let OnScreenKind::Stick {
            control, radius, ..
        } = self.controls[i].kind.clone()
        else {
            return;
        };
        let o = self.state[i].origin;
        let mut v = [(p[0] - o[0]) / radius, -(p[1] - o[1]) / radius];
        let m = (v[0] * v[0] + v[1] * v[1]).sqrt();
        if m > 1.0 {
            v = [v[0] / m, v[1] / m];
        }
        self.state[i].knob = v;
        self.send_stick(rt, &control, v);
    }
}
