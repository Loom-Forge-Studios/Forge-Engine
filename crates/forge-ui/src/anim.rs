//! Animation and transitions (Ch.21 §21.15).
//!
//! Tweens and critically-damped springs on style properties only. The clock is the
//! presentation clock ([`crate::damage::UiTime`]), never the canonical `Tick`. An active
//! animation requests frames until it settles, then stops; infinite animations
//! (spinners) run only while visible (§21.3). Reduced motion makes durations 0 and
//! replaces motion with a cross-fade no longer than [`REDUCED_MOTION_FADE`].

use std::time::Duration;

use crate::damage::UiTime;

/// The longest cross-fade reduced motion allows.
pub const REDUCED_MOTION_FADE: Duration = Duration::from_millis(80);

/// Easing curves (theme tokens name them).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseOut,
    EaseInOut,
}

impl Easing {
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

/// A finite tween from `from` to `to`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Tween {
    pub from: f32,
    pub to: f32,
    pub start: UiTime,
    pub duration: Duration,
    pub easing: Easing,
}

impl Tween {
    /// A tween honouring reduced motion (duration capped to the cross-fade).
    pub fn new(
        from: f32,
        to: f32,
        start: UiTime,
        duration: Duration,
        easing: Easing,
        reduced_motion: bool,
    ) -> Self {
        let duration = if reduced_motion {
            duration.min(REDUCED_MOTION_FADE)
        } else {
            duration
        };
        Self {
            from,
            to,
            start,
            duration,
            easing,
        }
    }
    pub fn value(&self, now: UiTime) -> f32 {
        if self.duration.is_zero() {
            return self.to;
        }
        let t = now.saturating_sub(self.start).as_secs_f32() / self.duration.as_secs_f32();
        self.from + (self.to - self.from) * self.easing.apply(t)
    }
    pub fn done(&self, now: UiTime) -> bool {
        now >= self.start + self.duration
    }
}

/// A critically-damped spring (smooth scroll, drag settle).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    /// Angular frequency; higher settles faster.
    pub omega: f32,
}

impl Spring {
    pub fn new(value: f32, omega: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            omega,
        }
    }
    /// Advance by `dt` (exact solution of the critically damped oscillator).
    pub fn step(&mut self, dt: Duration) {
        let t = dt.as_secs_f32();
        let x0 = self.value - self.target;
        let v0 = self.velocity;
        let w = self.omega;
        let e = (-w * t).exp();
        let x = (x0 + (v0 + w * x0) * t) * e;
        let v = (v0 - (v0 + w * x0) * w * t) * e;
        self.value = self.target + x;
        self.velocity = v;
    }
    /// Settled to within half a physical pixel.
    pub fn settled(&self) -> bool {
        (self.value - self.target).abs() < 0.25 && self.velocity.abs() < 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tween_ends_and_reduced_motion_caps() {
        let t = Tween::new(
            0.0,
            1.0,
            Duration::ZERO,
            Duration::from_millis(200),
            Easing::Linear,
            false,
        );
        assert!((t.value(Duration::from_millis(100)) - 0.5).abs() < 1e-6);
        assert!(t.done(Duration::from_millis(200)));
        let r = Tween::new(
            0.0,
            1.0,
            Duration::ZERO,
            Duration::from_millis(200),
            Easing::Linear,
            true,
        );
        assert_eq!(r.duration, REDUCED_MOTION_FADE);
    }

    #[test]
    fn spring_settles() {
        let mut s = Spring::new(0.0, 20.0);
        s.target = 100.0;
        for _ in 0..120 {
            s.step(Duration::from_millis(16));
        }
        assert!(s.settled());
    }
}
