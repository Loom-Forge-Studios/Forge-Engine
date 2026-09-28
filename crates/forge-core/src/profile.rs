//! Profiles (Ch.1.4): a property of a **region**, selected by measurement, with hysteresis.
//!
//! A profile may change *cost*, never *outcome* (I5). Nothing in this module decides
//! gameplay; it only picks which cost model a region runs under.
//!
//! **Status: measurement stub.** The four Foundations measurements (plus a fluid fraction)
//! are the inputs and the hysteresis machinery is complete, but the default thresholds in
//! [`ProfileThresholds::default`] are provisional placeholders until M4 measures real
//! scenes. Tuning them changes no outcome, by I5.

/// The five region profiles (Ch.1.4). One binary; a region carries one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Profile {
    /// Open terrain under the sky — the default.
    #[default]
    Surface,
    /// Enclosed: most of the sky is occluded.
    Cave,
    /// Many mutually visible agents.
    Crowd,
    /// Few entities spread thinly over a large loaded volume.
    Sparse,
    /// Dominated by simulated fluid.
    Fluid,
}

impl Profile {
    /// Every profile, in declaration order.
    pub const ALL: [Profile; 5] = [
        Profile::Surface,
        Profile::Cave,
        Profile::Crowd,
        Profile::Sparse,
        Profile::Fluid,
    ];
}

/// One measurement of a region (Foundations: "selected by measurement, not label").
///
/// Measured values are plain `f64` — they select a cost model and never feed an outcome (I5),
/// so they do not need the Ch.3 determinism contract, though IEEE comparison is exact anyway.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ProfileMetrics {
    /// Fraction of the sky hemisphere occluded, averaged over the region's viewers, `0..=1`.
    pub sky_occlusion: f64,
    /// Number of distinct agents mutually visible to the region's viewers (the crowd size).
    pub mutual_visibility: u32,
    /// Loaded surface area ÷ loaded volume, in 1/m. High = thin shells (surface), low =
    /// voluminous (sparse space, deep fluid). Measured and carried, but the stub classifier
    /// does not weigh it yet (M4 decides how it combines with `dispersion`).
    pub surface_per_volume: f64,
    /// How thinly the region's entities are spread, `0..=1`: mean nearest-neighbour distance
    /// ÷ region extent.
    pub dispersion: f64,
    /// Fraction of the loaded volume that is simulated fluid, `0..=1`. Not one of the
    /// Foundations' four; `Fluid` has no other honest signal (ADR 0005).
    pub fluid_fraction: f64,
}

impl ProfileMetrics {
    fn is_finite(&self) -> bool {
        self.sky_occlusion.is_finite()
            && self.surface_per_volume.is_finite()
            && self.dispersion.is_finite()
            && self.fluid_fraction.is_finite()
    }
}

/// A threshold with a hysteresis band: a region **enters** a profile when the measurement
/// crosses `enter`, and **stays** until it falls back past `exit`. For an "at least"
/// threshold `exit <= enter`; the gap is the band that stops a player standing in a cave
/// mouth from thrashing the profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    /// Value at which the profile is entered.
    pub enter: f64,
    /// Value at which a region already in the profile leaves it.
    pub exit: f64,
}

impl Band {
    /// `value >= enter` normally, `value >= exit` when already in the profile.
    fn holds(self, value: f64, currently_in: bool) -> bool {
        value >= if currently_in { self.exit } else { self.enter }
    }

    /// A band of zero width (no hysteresis) — used by the positive control.
    #[must_use]
    pub const fn sharp(at: f64) -> Self {
        Self {
            enter: at,
            exit: at,
        }
    }
}

/// Thresholds for the stub classifier. Evaluated in a fixed priority order:
/// Fluid, Cave, Crowd, Sparse, else Surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfileThresholds {
    /// `fluid_fraction` at least this ⇒ `Fluid`.
    pub fluid: Band,
    /// `sky_occlusion` at least this ⇒ `Cave`.
    pub cave: Band,
    /// `mutual_visibility` at least this ⇒ `Crowd`.
    pub crowd: Band,
    /// `dispersion` at least this ⇒ `Sparse`.
    pub sparse: Band,
}

impl Default for ProfileThresholds {
    /// Provisional values (see module docs); each band is ~10% wide.
    fn default() -> Self {
        Self {
            fluid: Band {
                enter: 0.5,
                exit: 0.4,
            },
            cave: Band {
                enter: 0.85,
                exit: 0.75,
            },
            crowd: Band {
                enter: 64.0,
                exit: 48.0,
            },
            sparse: Band {
                enter: 0.8,
                exit: 0.7,
            },
        }
    }
}

impl ProfileThresholds {
    /// Zero-width bands at the default `enter` values — no hysteresis at all.
    #[must_use]
    pub fn sharp() -> Self {
        let d = Self::default();
        Self {
            fluid: Band::sharp(d.fluid.enter),
            cave: Band::sharp(d.cave.enter),
            crowd: Band::sharp(d.crowd.enter),
            sparse: Band::sharp(d.sparse.enter),
        }
    }

    /// The profile these measurements indicate for a region currently in `current`.
    /// The current profile's own test uses its `exit` value (the band); every other test
    /// uses `enter`.
    #[must_use]
    pub fn classify(&self, m: &ProfileMetrics, current: Profile) -> Profile {
        let tests = [
            (Profile::Fluid, self.fluid, m.fluid_fraction),
            (Profile::Cave, self.cave, m.sky_occlusion),
            (Profile::Crowd, self.crowd, f64::from(m.mutual_visibility)),
            (Profile::Sparse, self.sparse, m.dispersion),
        ];
        for (p, band, v) in tests {
            if band.holds(v, current == p) {
                return p;
            }
        }
        Profile::Surface
    }
}

/// Picks a region's profile from a stream of measurements, with two layers of hysteresis:
/// the value bands in [`ProfileThresholds`], and a **dwell**: a different profile must be
/// indicated by `dwell` consecutive measurements before the region switches.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileSelector {
    thresholds: ProfileThresholds,
    dwell: u32,
    current: Profile,
    /// A profile that has been indicated, and for how many consecutive observations.
    pending: Option<(Profile, u32)>,
}

impl ProfileSelector {
    /// Default dwell: four consecutive measurements.
    pub const DEFAULT_DWELL: u32 = 4;

    /// A selector starting in `initial`, with the default thresholds and dwell.
    #[must_use]
    pub fn new(initial: Profile) -> Self {
        Self::with(initial, ProfileThresholds::default(), Self::DEFAULT_DWELL)
    }

    /// A selector with explicit thresholds and dwell (`0` is treated as `1`).
    #[must_use]
    pub fn with(initial: Profile, thresholds: ProfileThresholds, dwell: u32) -> Self {
        Self {
            thresholds,
            dwell: dwell.max(1),
            current: initial,
            pending: None,
        }
    }

    /// No hysteresis at all: sharp bands, dwell 1. Exists so the thrash test has a positive
    /// control; never use it for a real region.
    #[must_use]
    pub fn without_hysteresis(initial: Profile) -> Self {
        Self::with(initial, ProfileThresholds::sharp(), 1)
    }

    /// The profile in force.
    #[must_use]
    pub fn current(&self) -> Profile {
        self.current
    }

    /// Force a profile (e.g. inherited on split), clearing any pending switch.
    pub fn set(&mut self, p: Profile) {
        self.current = p;
        self.pending = None;
    }

    /// Feed one measurement; returns the profile in force afterwards. A measurement with a
    /// non-finite value is no evidence and changes nothing.
    pub fn observe(&mut self, m: &ProfileMetrics) -> Profile {
        if !m.is_finite() {
            return self.current;
        }
        let indicated = self.thresholds.classify(m, self.current);
        if indicated == self.current {
            self.pending = None;
            return self.current;
        }
        let streak = match self.pending {
            Some((p, n)) if p == indicated => n + 1,
            _ => 1,
        };
        if streak >= self.dwell {
            self.current = indicated;
            self.pending = None;
        } else {
            self.pending = Some((indicated, streak));
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cave(occ: f64) -> ProfileMetrics {
        ProfileMetrics {
            sky_occlusion: occ,
            ..ProfileMetrics::default()
        }
    }

    /// A player in a cave mouth: occlusion jitters ±0.03 around the `enter` threshold.
    fn cave_mouth(n: usize) -> Vec<ProfileMetrics> {
        (0..n)
            .map(|i| cave(if i % 2 == 0 { 0.87 } else { 0.83 }))
            .collect()
    }

    fn switches(sel: &mut ProfileSelector, stream: &[ProfileMetrics]) -> usize {
        let mut last = sel.current();
        let mut n = 0;
        for m in stream {
            let p = sel.observe(m);
            if p != last {
                n += 1;
                last = p;
            }
        }
        n
    }

    #[test]
    fn cave_mouth_does_not_thrash() {
        let mut sel = ProfileSelector::new(Profile::Surface);
        // Walk in: sustained occlusion switches exactly once, after the dwell.
        let walk_in = vec![cave(0.95); 10];
        assert_eq!(switches(&mut sel, &walk_in), 1);
        assert_eq!(sel.current(), Profile::Cave);
        // Stand in the mouth: jitter inside the band never leaves Cave.
        assert_eq!(switches(&mut sel, &cave_mouth(200)), 0);
        // Walk out: sustained low occlusion switches back once.
        assert_eq!(switches(&mut sel, &vec![cave(0.1); 10]), 1);
        assert_eq!(sel.current(), Profile::Surface);
    }

    #[test]
    fn dwell_needs_consecutive_indications() {
        let mut sel = ProfileSelector::new(Profile::Surface);
        for _ in 0..(ProfileSelector::DEFAULT_DWELL - 1) {
            assert_eq!(sel.observe(&cave(0.95)), Profile::Surface);
        }
        // An interruption resets the streak.
        assert_eq!(sel.observe(&cave(0.1)), Profile::Surface);
        for _ in 0..(ProfileSelector::DEFAULT_DWELL - 1) {
            assert_eq!(sel.observe(&cave(0.95)), Profile::Surface);
        }
        assert_eq!(sel.observe(&cave(0.95)), Profile::Cave);
    }

    /// Positive control (W2): with the hysteresis removed, the same cave-mouth stream
    /// thrashes on every sample — so `cave_mouth_does_not_thrash` is not vacuous.
    #[test]
    fn positive_control_no_hysteresis_thrashes() {
        let mut sel = ProfileSelector::without_hysteresis(Profile::Surface);
        let n = switches(&mut sel, &cave_mouth(200));
        assert_eq!(n, 200, "every sample should flip the profile");
    }

    #[test]
    fn non_finite_measurements_are_ignored() {
        let mut sel = ProfileSelector::with(Profile::Surface, ProfileThresholds::default(), 1);
        assert_eq!(sel.observe(&cave(f64::NAN)), Profile::Surface);
        assert_eq!(sel.observe(&cave(f64::INFINITY)), Profile::Surface);
    }

    #[test]
    fn priority_order() {
        let t = ProfileThresholds::default();
        let all = ProfileMetrics {
            sky_occlusion: 1.0,
            mutual_visibility: 1000,
            surface_per_volume: 1.0,
            dispersion: 1.0,
            fluid_fraction: 1.0,
        };
        assert_eq!(t.classify(&all, Profile::Surface), Profile::Fluid);
        let crowd = ProfileMetrics {
            mutual_visibility: 100,
            dispersion: 0.9,
            ..ProfileMetrics::default()
        };
        assert_eq!(t.classify(&crowd, Profile::Surface), Profile::Crowd);
        let sparse = ProfileMetrics {
            dispersion: 0.9,
            ..ProfileMetrics::default()
        };
        assert_eq!(t.classify(&sparse, Profile::Surface), Profile::Sparse);
        assert_eq!(
            t.classify(&ProfileMetrics::default(), Profile::Cave),
            Profile::Surface
        );
    }
}
