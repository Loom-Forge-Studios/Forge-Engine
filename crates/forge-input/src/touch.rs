//! Touch and gestures (Ch.28 §28.15). A touch device's fingers arrive as [`TouchInput`]s
//! (from winit's `Touch` events, or a test); the recogniser turns them into the Touch
//! class's controls, so a gesture binds like any button or axis (`Touch/DoubleTap`,
//! `Touch/Pinch`):
//!
//! * **Tap** — a finger up within `tap_time` s that moved less than `slop` px; **DoubleTap**
//!   — a second tap within `double_tap_gap` s and `double_tap_slop` px of the first (both
//!   taps also fire `Tap`: a game that wants only one of them binds only one).
//! * **LongPress** — a finger held still for `long_press` s (fires while held, once).
//! * **Swipe{Left,Right,Up,Down}** — a finger up within `swipe_time` s that travelled at
//!   least `swipe_min` px; the direction is the dominant axis (window y grows down).
//! * **Pinch** (the log-ratio of the two-finger spread this frame: positive spreads apart,
//!   and frames add up to the log of the total zoom), **Rotate** (radians, counter-clockwise
//!   on screen) and **TwoFingerPan** (the centroid's motion) while two fingers are down.
//! * **Primary**, **Position** and **Delta** — the first finger as a pointer.
//!
//! A gesture control is a one-frame pulse. Thresholds are in window pixels: scale them by
//! the display's DPI ([`GestureConfig::scaled`]).

use crate::faults::InputFaults;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Canceled,
}

/// One finger event (window px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchInput {
    pub id: u64,
    pub phase: TouchPhase,
    pub pos: [f32; 2],
}

impl TouchInput {
    pub fn new(id: u64, phase: TouchPhase, x: f32, y: f32) -> Self {
        Self {
            id,
            phase,
            pos: [x, y],
        }
    }
}

/// The recogniser's thresholds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureConfig {
    pub tap_time: f32,
    pub slop: f32,
    pub double_tap_gap: f32,
    pub double_tap_slop: f32,
    pub long_press: f32,
    pub swipe_min: f32,
    pub swipe_time: f32,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            tap_time: 0.25,
            slop: 12.0,
            double_tap_gap: 0.3,
            double_tap_slop: 40.0,
            long_press: 0.5,
            swipe_min: 60.0,
            swipe_time: 0.4,
        }
    }
}

impl GestureConfig {
    /// The pixel thresholds scaled (a display at 2x DPI: `scaled(2.0)`).
    pub fn scaled(mut self, k: f32) -> Self {
        self.slop *= k;
        self.double_tap_slop *= k;
        self.swipe_min *= k;
        self
    }
}

/// The gesture pulses of one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pulses(u16);

const PULSE_NAMES: [&str; 7] = [
    "Tap",
    "DoubleTap",
    "LongPress",
    "SwipeLeft",
    "SwipeRight",
    "SwipeUp",
    "SwipeDown",
];

impl Pulses {
    fn add(&mut self, name: &str) {
        if let Some(i) = PULSE_NAMES.iter().position(|n| *n == name) {
            self.0 |= 1 << i;
        }
    }
    pub(crate) fn names(self) -> impl Iterator<Item = &'static str> {
        PULSE_NAMES
            .iter()
            .enumerate()
            .filter(move |(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, n)| *n)
    }
    fn any(self) -> bool {
        self.0 != 0
    }
}

#[derive(Clone, Copy, Debug)]
struct Finger {
    id: u64,
    start: [f32; 2],
    pos: [f32; 2],
    t0: f64,
    moved_far: bool,
    long_fired: bool,
    /// Another finger joined while it was down (not a tap or a swipe).
    multi: bool,
}

#[derive(Clone, Copy, Debug)]
struct Two {
    dist: f32,
    angle: f32,
    centroid: [f32; 2],
}

/// One frame of recognised touch state.
pub(crate) struct GestureFrame {
    pub idle: bool,
    pub fingers: usize,
    pub position: [f32; 2],
    pub delta: [f32; 2],
    pub pinch: f32,
    pub rotate: f32,
    pub pan: [f32; 2],
    pub pulses: Pulses,
}

/// The recogniser (one per touch device).
#[derive(Debug, Default)]
pub(crate) struct Gestures {
    fingers: Vec<Finger>,
    pulses: Pulses,
    last_tap: Option<(f64, [f32; 2])>,
    two: Option<Two>,
    position: [f32; 2],
    delta: [f32; 2],
    pinch: f32,
    rotate: f32,
    pan: [f32; 2],
    was_active: bool,
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

impl Gestures {
    fn two_state(&self) -> Option<Two> {
        let (a, b) = (self.fingers.first()?, self.fingers.get(1)?);
        Some(Two {
            dist: dist(a.pos, b.pos).max(1e-3),
            // Window y grows down; negate it so positive is counter-clockwise on screen.
            angle: (-(b.pos[1] - a.pos[1])).atan2(b.pos[0] - a.pos[0]),
            centroid: [(a.pos[0] + b.pos[0]) * 0.5, (a.pos[1] + b.pos[1]) * 0.5],
        })
    }

    pub(crate) fn touch(&mut self, t: TouchInput, now: f64, cfg: &GestureConfig, f: &InputFaults) {
        match t.phase {
            TouchPhase::Started => {
                let multi = !self.fingers.is_empty();
                for x in &mut self.fingers {
                    x.multi = true;
                }
                self.fingers.push(Finger {
                    id: t.id,
                    start: t.pos,
                    pos: t.pos,
                    t0: now,
                    moved_far: false,
                    long_fired: false,
                    multi,
                });
                if self.fingers.len() == 1 {
                    self.position = t.pos;
                }
                self.two = self.two_state();
            }
            TouchPhase::Moved => {
                let Some(ix) = self.fingers.iter().position(|x| x.id == t.id) else {
                    return;
                };
                let old = self.fingers[ix].pos;
                self.fingers[ix].pos = t.pos;
                if dist(t.pos, self.fingers[ix].start) > cfg.slop {
                    self.fingers[ix].moved_far = true;
                }
                if ix == 0 {
                    self.delta[0] += t.pos[0] - old[0];
                    self.delta[1] += t.pos[1] - old[1];
                    self.position = t.pos;
                }
                if ix < 2
                    && let (Some(was), Some(now2)) = (self.two, self.two_state())
                {
                    self.pinch += (now2.dist / was.dist).ln();
                    let mut da = now2.angle - was.angle;
                    if da > std::f32::consts::PI {
                        da -= std::f32::consts::TAU;
                    } else if da < -std::f32::consts::PI {
                        da += std::f32::consts::TAU;
                    }
                    self.rotate += da;
                    self.pan[0] += now2.centroid[0] - was.centroid[0];
                    self.pan[1] += now2.centroid[1] - was.centroid[1];
                    self.two = Some(now2);
                }
            }
            TouchPhase::Ended | TouchPhase::Canceled => {
                let Some(ix) = self.fingers.iter().position(|x| x.id == t.id) else {
                    return;
                };
                let fg = self.fingers.remove(ix);
                self.two = self.two_state();
                if let Some(first) = self.fingers.first() {
                    self.position = first.pos;
                }
                if t.phase == TouchPhase::Canceled || fg.multi || fg.long_fired {
                    return;
                }
                let held = (now - fg.t0) as f32;
                let d = dist(t.pos, fg.start);
                let still = d <= cfg.slop || f.tap_no_slop();
                if held <= cfg.tap_time && still {
                    self.pulses.add("Tap");
                    match self.last_tap {
                        Some((t1, p1))
                            if ((now - t1) as f32) <= cfg.double_tap_gap
                                && dist(p1, t.pos) <= cfg.double_tap_slop =>
                        {
                            self.pulses.add("DoubleTap");
                            self.last_tap = None;
                        }
                        _ => self.last_tap = Some((now, t.pos)),
                    }
                } else if held <= cfg.swipe_time && d >= cfg.swipe_min {
                    let dx = t.pos[0] - fg.start[0];
                    let dy = t.pos[1] - fg.start[1];
                    let name = if dx.abs() >= dy.abs() {
                        if dx > 0.0 { "SwipeRight" } else { "SwipeLeft" }
                    } else if dy > 0.0 {
                        "SwipeDown"
                    } else {
                        "SwipeUp"
                    };
                    self.pulses.add(name);
                }
            }
        }
    }

    /// The frame's controls (and the long-press check).
    pub(crate) fn frame(
        &mut self,
        now: f64,
        cfg: &GestureConfig,
        _f: &InputFaults,
    ) -> GestureFrame {
        if self.fingers.len() == 1
            && let Some(x) = self.fingers.first_mut()
            && !x.long_fired
            && !x.moved_far
            && !x.multi
            && ((now - x.t0) as f32) >= cfg.long_press
        {
            x.long_fired = true;
            self.pulses.add("LongPress");
        }
        let active = !self.fingers.is_empty()
            || self.pulses.any()
            || self.delta != [0.0; 2]
            || self.pinch != 0.0
            || self.rotate != 0.0
            || self.pan != [0.0; 2];
        let out = GestureFrame {
            idle: !active && !self.was_active,
            fingers: self.fingers.len(),
            position: self.position,
            delta: self.delta,
            pinch: self.pinch,
            rotate: self.rotate,
            pan: self.pan,
            pulses: self.pulses,
        };
        self.was_active = active;
        self.pulses = Pulses::default();
        self.delta = [0.0; 2];
        self.pinch = 0.0;
        self.rotate = 0.0;
        self.pan = [0.0; 2];
        out
    }
}
