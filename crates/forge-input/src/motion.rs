//! Gyro aiming (Ch.28 §28.16). A gamepad's motion samples (angular velocity in degrees per
//! second, acceleration in g) become the `Gamepad/Gyro` control: the yaw and pitch turned
//! this frame, in degrees, scaled by the player's sensitivity. It binds like the mouse delta
//! (an `Axis2D` look action), so gyro aim and stick aim mix in one action.
//!
//! * **Calibration.** A gyro reads a small non-zero rate at rest (its bias), which drifts
//!   the aim. While the pad lies still (rate within `still_dps` of the running bias and
//!   the accelerometer steady near 1 g) for `still_time` s, the bias is re-estimated as a
//!   running average — continuous auto-calibration, so no "put your controller down" step.
//! * **Space.** `Local` uses the pad's own yaw axis (tilting the pad while turning mixes in
//!   roll). `Player` (the default; JoyShockLibrary's "player space") takes yaw about gravity
//!   and relaxes it toward the pad's yaw/roll plane, so turning works however the player
//!   holds the pad. `World` is yaw about gravity alone.
//! * **Tightening.** Below `tighten_dps` the output is scaled down in proportion (a steady
//!   hand's tremor does not shake the aim) — continuous, with no dead band.
//!
//! Where samples come from is a backend's job: [`MotionSource`] is the seam (a platform
//! backend for DualShock 4 / DualSense / Switch Pro motion reports plugs in there; gilrs
//! does not expose motion).

use crate::faults::InputFaults;

/// A resting rate above this is a turn, not a bias (deg/s).
pub const MAX_BIAS_DPS: f32 = 10.0;

/// One motion sample: `gyro` in deg/s about the pad's x (pitch), y (yaw) and z (roll) axes,
/// `accel` in g, `dt` seconds since the previous sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSample {
    pub gyro: [f32; 3],
    pub accel: [f32; 3],
    pub dt: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GyroSpace {
    Local,
    #[default]
    Player,
    World,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GyroSettings {
    pub space: GyroSpace,
    /// Output degrees per degree turned (x: yaw, y: pitch).
    pub sensitivity: [f32; 2],
    pub invert_x: bool,
    pub invert_y: bool,
    /// Rates below this are scaled down in proportion (deg/s).
    pub tighten_dps: f32,
    /// Auto-calibration: "still" is a rate within this of its running average (deg/s)...
    pub still_dps: f32,
    /// ...for this long (s).
    pub still_time: f32,
}

impl Default for GyroSettings {
    fn default() -> Self {
        Self {
            space: GyroSpace::Player,
            sensitivity: [1.0, 1.0],
            invert_x: false,
            invert_y: false,
            tighten_dps: 1.5,
            still_dps: 2.5,
            still_time: 0.5,
        }
    }
}

/// Per-device gyro state.
#[derive(Clone, Debug, PartialEq)]
pub struct GyroProcessor {
    pub settings: GyroSettings,
    bias: [f32; 3],
    /// A short running average of the raw rate (steadiness is measured against it).
    avg: [f32; 3],
    still: f32,
    samples: u32,
    gravity: [f32; 3],
}

fn norm3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

impl GyroProcessor {
    pub fn new(settings: GyroSettings) -> Self {
        Self {
            settings,
            bias: [0.0; 3],
            avg: [0.0; 3],
            still: 0.0,
            samples: 0,
            gravity: [0.0, 1.0, 0.0],
        }
    }

    /// The current bias estimate (deg/s).
    pub fn bias(&self) -> [f32; 3] {
        self.bias
    }

    /// One sample in; the (yaw, pitch) degrees turned out.
    pub fn process(&mut self, s: &MotionSample, f: &InputFaults) -> [f32; 2] {
        let cfg = self.settings;
        // Gravity: a slow low-pass of the accelerometer.
        let an = norm3(s.accel);
        if an > 0.5 && an < 1.5 {
            let k = (s.dt * 4.0).clamp(0.0, 1.0);
            for i in 0..3 {
                self.gravity[i] += (s.accel[i] / an - self.gravity[i]) * k;
            }
        }
        // Calibration while still.
        if !f.gyro_no_calibration() {
            let k = (s.dt / 0.25).clamp(0.0, 1.0);
            for (a, g) in self.avg.iter_mut().zip(s.gyro) {
                *a += (g - *a) * k;
            }
            let off = [
                s.gyro[0] - self.avg[0],
                s.gyro[1] - self.avg[1],
                s.gyro[2] - self.avg[2],
            ];
            let steady = (an - 1.0).abs() < 0.1;
            // Steady rate, steady gravity, and a rate small enough to be a bias (a slow
            // deliberate turn is not learnt away).
            if norm3(off) < cfg.still_dps && steady && norm3(self.avg) < MAX_BIAS_DPS {
                self.still += s.dt;
            } else {
                self.still = 0.0;
                self.samples = 0;
            }
            if self.still >= cfg.still_time {
                self.samples = self.samples.saturating_add(1);
                let k = 1.0 / (self.samples.min(200) as f32);
                for i in 0..3 {
                    self.bias[i] += (s.gyro[i] - self.bias[i]) * k;
                }
            }
        }
        let g = [
            s.gyro[0] - self.bias[0],
            s.gyro[1] - self.bias[1],
            s.gyro[2] - self.bias[2],
        ];
        let gn = norm3(self.gravity).max(1e-6);
        let grav = [
            self.gravity[0] / gn,
            self.gravity[1] / gn,
            self.gravity[2] / gn,
        ];
        let world_yaw = grav[1] * g[1] + grav[2] * g[2];
        let yaw = match cfg.space {
            GyroSpace::Local => g[1],
            GyroSpace::World => world_yaw,
            GyroSpace::Player => {
                let relax = 1.41;
                let cap = (g[1] * g[1] + g[2] * g[2]).sqrt();
                (world_yaw.abs() * relax).min(cap).copysign(world_yaw)
            }
        };
        let pitch = g[0];
        let tighten = |v: f32| {
            let t = cfg.tighten_dps;
            if t > 0.0 && v.abs() < t {
                v * v.abs() / t
            } else {
                v
            }
        };
        let (yaw, pitch) = (tighten(yaw), tighten(pitch));
        let sx = if cfg.invert_x { -1.0 } else { 1.0 };
        let sy = if cfg.invert_y { -1.0 } else { 1.0 };
        [
            yaw * s.dt * cfg.sensitivity[0] * sx,
            pitch * s.dt * cfg.sensitivity[1] * sy,
        ]
    }
}

/// Where motion samples come from (see the module docs): a backend fills `out` with
/// `(device stable key, sample)` pairs since the last poll.
pub trait MotionSource {
    fn describe(&self) -> String;
    fn poll(&mut self, out: &mut Vec<(String, MotionSample)>);
}
