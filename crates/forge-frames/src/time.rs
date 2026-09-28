//! Canonical time (Ch.2.5).

/// Canonical time: a signed count of **microseconds** from the epoch (ADR 0004).
///
/// One authoritative `Tick` is owned by the global-services layer; a client's wall clock
/// never feeds a physics-relevant evaluation (I-20). Microseconds give a range of
/// +-292,000 years (time-warp and long-lived servers never overflow it) and resolve any
/// simulation step; a rate that does not divide 10^6 (60 Hz) steps by the integer schedule
/// `floor(k * 10^6 / rate)`, which is exact and identical everywhere.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default, bevy_reflect::Reflect,
)]
#[reflect(opaque, Clone, Debug, PartialEq, Hash)]
pub struct Tick(pub i64);

/// Ticks per second of canonical time.
pub const TICKS_PER_SECOND: i64 = 1_000_000;

impl Tick {
    /// The epoch, `t = 0`.
    pub const EPOCH: Tick = Tick(0);

    /// Seconds since the epoch as `f64` (exact for `|t| < 2^53` ticks, then one rounding
    /// by the division).
    #[inline]
    pub fn seconds(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }

    /// `self + ticks`, or `None` on overflow.
    #[inline]
    pub fn checked_add(self, ticks: i64) -> Option<Tick> {
        self.0.checked_add(ticks).map(Tick)
    }

    /// The fraction of the way through the current cycle of a periodic motion with
    /// `period` ticks, in `[0, 1)` — reduced **exactly** in integers before any rounding,
    /// so a spin or orbit phase is as precise a million years out as at the epoch.
    /// `None` if `period <= 0`.
    #[inline]
    pub fn phase(self, period: i64) -> Option<f64> {
        (period > 0).then(|| self.0.rem_euclid(period) as f64 / period as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_is_exact_far_from_the_epoch() {
        let day = 86_400 * TICKS_PER_SECOND;
        let far = Tick(day * 365 * 250_000 + day / 4); // 250k years and a quarter day
        assert_eq!(far.phase(day), Some(0.25));
        assert_eq!(Tick(-day / 4).phase(day), Some(0.75)); // backwards in time
        assert_eq!(Tick(5).phase(0), None);
        assert_eq!(Tick(5).phase(-3), None);
    }

    #[test]
    fn seconds_and_overflow() {
        assert_eq!(Tick(1_500_000).seconds(), 1.5);
        assert_eq!(Tick(i64::MAX).checked_add(1), None);
        assert_eq!(Tick(1).checked_add(-2), Some(Tick(-1)));
    }
}
