//! Haptics (Ch.28 §28.16): rumble with an envelope, per device or per player. The runtime
//! mixes every active effect of a device (the strongest per motor), and a backend applies
//! only the levels that **changed** ([`MotorCommand`]); an idle device costs nothing and a
//! finished effect always ends with a zero command, so no pad is left buzzing.
//!
//! Two motors, as every mainstream pad has: `low` (the heavy, low-frequency one) and `high`
//! (the light, high-frequency one), each `0..=1`.

use crate::faults::InputFaults;
use crate::runtime::DeviceId;

/// A rumble effect: levels, a duration and a linear attack and release (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rumble {
    pub low: f32,
    pub high: f32,
    pub duration: f32,
    pub attack: f32,
    pub release: f32,
}

impl Rumble {
    /// A constant rumble for `duration` seconds.
    pub fn new(low: f32, high: f32, duration: f32) -> Self {
        Self {
            low: low.clamp(0.0, 1.0),
            high: high.clamp(0.0, 1.0),
            duration: duration.max(0.0),
            attack: 0.0,
            release: 0.0,
        }
    }
    pub fn envelope(mut self, attack: f32, release: f32) -> Self {
        self.attack = attack.max(0.0);
        self.release = release.max(0.0);
        self
    }
    /// The envelope's gain `t` seconds in (0 after the end).
    pub fn gain(&self, t: f32) -> f32 {
        if t < 0.0 || t >= self.duration {
            return 0.0;
        }
        let a = if self.attack > 0.0 {
            (t / self.attack).min(1.0)
        } else {
            1.0
        };
        let r = if self.release > 0.0 {
            ((self.duration - t) / self.release).min(1.0)
        } else {
            1.0
        };
        a.min(r)
    }
}

/// New motor levels for a device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotorCommand {
    pub device: DeviceId,
    pub low: f32,
    pub high: f32,
}

#[derive(Default)]
pub(crate) struct HapticsState {
    effects: Vec<(DeviceId, Rumble, f64)>,
    /// Last levels sent per device.
    sent: Vec<(DeviceId, [f32; 2])>,
    commands: Vec<MotorCommand>,
}

impl HapticsState {
    pub(crate) fn start(&mut self, d: DeviceId, r: Rumble, now: f64) {
        self.effects.push((d, r, now));
    }
    pub(crate) fn stop(&mut self, d: DeviceId) {
        self.effects.retain(|e| e.0 != d);
    }
    pub(crate) fn forget(&mut self, d: DeviceId) {
        self.effects.retain(|e| e.0 != d);
        self.sent.retain(|e| e.0 != d);
    }
    pub(crate) fn commands(&self) -> &[MotorCommand] {
        &self.commands
    }

    pub(crate) fn update(&mut self, now: f64, faults: &InputFaults) {
        self.commands.clear();
        if self.effects.is_empty() && self.sent.iter().all(|s| s.1 == [0.0; 2]) {
            return;
        }
        if !faults.rumble_never_stops() {
            self.effects
                .retain(|(_, r, t0)| ((now - t0) as f32) < r.duration);
        }
        // Every device with an effect or a non-zero level last sent.
        for i in 0..self.sent.len() {
            let d = self.sent[i].0;
            if !self.effects.iter().any(|e| e.0 == d) && self.sent[i].1 != [0.0; 2] {
                self.sent[i].1 = [0.0; 2];
                self.commands.push(MotorCommand {
                    device: d,
                    low: 0.0,
                    high: 0.0,
                });
            }
        }
        for k in 0..self.effects.len() {
            let d = self.effects[k].0;
            if self.commands.iter().any(|c| c.device == d) {
                continue;
            }
            let mut lv = [0.0f32; 2];
            for (e, r, t0) in &self.effects {
                if *e != d {
                    continue;
                }
                let t = (now - t0) as f32;
                let g = if faults.rumble_never_stops() {
                    1.0
                } else {
                    r.gain(t)
                };
                lv[0] = lv[0].max(r.low * g);
                lv[1] = lv[1].max(r.high * g);
            }
            match self.sent.iter_mut().find(|s| s.0 == d) {
                Some(s) if s.1 == lv => {}
                Some(s) => {
                    s.1 = lv;
                    self.commands.push(MotorCommand {
                        device: d,
                        low: lv[0],
                        high: lv[1],
                    });
                }
                None => {
                    self.sent.push((d, lv));
                    self.commands.push(MotorCommand {
                        device: d,
                        low: lv[0],
                        high: lv[1],
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_rises_holds_and_falls() {
        let r = Rumble::new(1.0, 0.5, 1.0).envelope(0.2, 0.2);
        assert!((r.gain(0.1) - 0.5).abs() < 1e-6);
        assert!((r.gain(0.5) - 1.0).abs() < 1e-6);
        assert!((r.gain(0.9) - 0.5).abs() < 1e-5);
        assert_eq!(r.gain(1.0), 0.0);
    }
}
