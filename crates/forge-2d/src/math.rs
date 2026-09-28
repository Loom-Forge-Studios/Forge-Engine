//! 2D math the pipeline shares: rotations, transforms, boxes and a counter-based random
//! stream. Every operation is IEEE `+ - * /`, `sqrt` and `forge_num::det` — bit-identical on
//! every platform (Ch.3), which is what lets physics, particles and skeletons join the I2
//! determinism corpus.

use forge_num::{DVec2, det};

/// A rotation stored as its cosine and sine (no angle wrapping, no drift from repeated
/// `sin`/`cos` of a growing angle).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rot2 {
    pub c: f64,
    pub s: f64,
}

impl Default for Rot2 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Rot2 {
    pub const IDENTITY: Self = Self { c: 1.0, s: 0.0 };

    /// The rotation by `angle` radians (counter-clockwise), through `det::sin`/`det::cos`.
    #[must_use]
    pub fn from_angle(angle: f64) -> Self {
        Self {
            c: det::cos(angle),
            s: det::sin(angle),
        }
    }

    /// The angle in `(-pi, pi]`.
    #[must_use]
    pub fn angle(self) -> f64 {
        det::atan2(self.s, self.c)
    }

    /// Rotate `v`.
    #[inline]
    #[must_use]
    pub fn apply(self, v: DVec2) -> DVec2 {
        DVec2::new(self.c * v.x - self.s * v.y, self.s * v.x + self.c * v.y)
    }

    /// Rotate `v` by the inverse.
    #[inline]
    #[must_use]
    pub fn apply_inv(self, v: DVec2) -> DVec2 {
        DVec2::new(self.c * v.x + self.s * v.y, -self.s * v.x + self.c * v.y)
    }

    /// `self` then `o` (angles add).
    #[inline]
    #[must_use]
    pub fn then(self, o: Self) -> Self {
        Self {
            c: o.c * self.c - o.s * self.s,
            s: o.s * self.c + o.c * self.s,
        }
    }

    /// Advance by `w * dt` radians with the first-order update used by rigid-body solvers,
    /// renormalised (Box2D's integrate-rotation): cheap, and exact to the bit everywhere.
    #[inline]
    #[must_use]
    pub fn integrate(self, delta: f64) -> Self {
        let c = self.c - delta * self.s;
        let s = self.s + delta * self.c;
        let l = det::sqrt(c * c + s * s);
        if l > 0.0 {
            Self { c: c / l, s: s / l }
        } else {
            Self::IDENTITY
        }
    }
}

/// A 2D affine transform: scale, then rotate, then translate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform2 {
    pub translation: DVec2,
    pub rot: Rot2,
    pub scale: DVec2,
}

impl Default for Xform2 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Xform2 {
    pub const IDENTITY: Self = Self {
        translation: DVec2::ZERO,
        rot: Rot2::IDENTITY,
        scale: DVec2 { x: 1.0, y: 1.0 },
    };

    #[must_use]
    pub fn new(translation: DVec2, angle: f64, scale: DVec2) -> Self {
        Self {
            translation,
            rot: Rot2::from_angle(angle),
            scale,
        }
    }

    /// Map a point of this transform's space into its parent's.
    #[inline]
    #[must_use]
    pub fn apply(&self, p: DVec2) -> DVec2 {
        self.rot.apply(p.mul_elem(self.scale)) + self.translation
    }

    /// Map a direction (no translation).
    #[inline]
    #[must_use]
    pub fn apply_vector(&self, v: DVec2) -> DVec2 {
        self.rot.apply(v.mul_elem(self.scale))
    }

    /// `parent * self`: this transform expressed in the parent's parent. Non-uniform scale
    /// under rotation is approximated as a scale then rotation (no shear), which is what a
    /// cutout rig wants: a squashed parent never skews its children.
    #[must_use]
    pub fn under(&self, parent: &Xform2) -> Xform2 {
        Xform2 {
            translation: parent.apply(self.translation),
            rot: self.rot.then(parent.rot),
            scale: self.scale.mul_elem(parent.scale),
        }
    }
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb2 {
    pub min: DVec2,
    pub max: DVec2,
}

impl Aabb2 {
    #[must_use]
    pub const fn new(min: DVec2, max: DVec2) -> Self {
        Self { min, max }
    }

    /// The box around `points` (empty input: a degenerate box at the origin).
    #[must_use]
    pub fn around(points: &[DVec2]) -> Self {
        let Some(first) = points.first() else {
            return Self::new(DVec2::ZERO, DVec2::ZERO);
        };
        let mut b = Self::new(*first, *first);
        for p in &points[1..] {
            b.min = b.min.min(*p);
            b.max = b.max.max(*p);
        }
        b
    }

    #[inline]
    #[must_use]
    pub fn overlaps(&self, o: &Aabb2) -> bool {
        self.min.x <= o.max.x
            && o.min.x <= self.max.x
            && self.min.y <= o.max.y
            && o.min.y <= self.max.y
    }

    #[inline]
    #[must_use]
    pub fn contains(&self, p: DVec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    #[inline]
    #[must_use]
    pub fn union(&self, o: &Aabb2) -> Aabb2 {
        Aabb2::new(self.min.min(o.min), self.max.max(o.max))
    }

    #[inline]
    #[must_use]
    pub fn inflate(&self, r: f64) -> Aabb2 {
        Aabb2::new(self.min - DVec2::splat(r), self.max + DVec2::splat(r))
    }

    #[inline]
    #[must_use]
    pub fn center(&self) -> DVec2 {
        (self.min + self.max) * 0.5
    }

    #[inline]
    #[must_use]
    pub fn size(&self) -> DVec2 {
        self.max - self.min
    }
}

/// A counter-based random stream: value `i` is a pure function of `(key, i)` (SplitMix64
/// finaliser), so a stream is never "advanced" state that depends on evaluation order —
/// the I3 discipline applied to particles and sampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream {
    key: u64,
}

impl Stream {
    #[must_use]
    pub const fn new(key: u64) -> Self {
        Self { key }
    }

    /// The `i`-th 64-bit value.
    #[must_use]
    pub fn bits(self, i: u64) -> u64 {
        let mut z = self
            .key
            .wrapping_add(i.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// The `i`-th value, uniform in `[0, 1)` (53 bits, no transcendental).
    #[must_use]
    pub fn unit(self, i: u64) -> f64 {
        (self.bits(i) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// The `i`-th value, uniform in `[a, b)`.
    #[must_use]
    pub fn range(self, i: u64, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit(i)
    }
}

/// Linear interpolation.
#[inline]
#[must_use]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// `a + (b - a) t` component-wise.
#[inline]
#[must_use]
pub fn lerp2(a: DVec2, b: DVec2, t: f64) -> DVec2 {
    a + (b - a) * t
}

/// Shortest signed difference `b - a` of two angles, in `[-pi, pi)`.
#[must_use]
pub fn angle_delta(a: f64, b: f64) -> f64 {
    let tau = core::f64::consts::TAU;
    let d = (b - a) % tau;
    if d < -core::f64::consts::PI {
        d + tau
    } else if d >= core::f64::consts::PI {
        d - tau
    } else {
        d
    }
}

/// Twice the signed area of the polygon (counter-clockwise positive).
#[must_use]
pub fn signed_area2(poly: &[DVec2]) -> f64 {
    let n = poly.len();
    let mut a = 0.0;
    for i in 0..n {
        let p = poly[i];
        let q = poly[(i + 1) % n];
        a += p.cross(q);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotations_compose_and_invert() {
        let a = Rot2::from_angle(0.7);
        let b = Rot2::from_angle(-0.2);
        let ab = a.then(b);
        assert!((ab.angle() - 0.5).abs() < 1e-15);
        let v = DVec2::new(3.0, -1.5);
        let w = a.apply_inv(a.apply(v));
        assert!((w - v).length() < 1e-14);
        let r = Rot2::IDENTITY.integrate(0.01);
        assert!((r.c * r.c + r.s * r.s - 1.0).abs() < 1e-15);
    }

    #[test]
    fn transforms_nest() {
        let parent = Xform2::new(
            DVec2::new(10.0, 0.0),
            core::f64::consts::FRAC_PI_2,
            DVec2::new(2.0, 2.0),
        );
        let child = Xform2::new(DVec2::new(1.0, 0.0), 0.0, DVec2::new(1.0, 1.0));
        let w = child.under(&parent);
        assert!((w.translation - DVec2::new(10.0, 2.0)).length() < 1e-12);
        assert!((w.apply(DVec2::new(1.0, 0.0)) - DVec2::new(10.0, 4.0)).length() < 1e-12);
    }

    #[test]
    fn streams_are_pure_and_uniform() {
        let s = Stream::new(42);
        assert_eq!(s.bits(7), s.bits(7));
        assert_ne!(s.bits(7), s.bits(8));
        let mean: f64 = (0..10_000).map(|i| s.unit(i)).sum::<f64>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.01, "{mean}");
        assert!((0..1000).all(|i| (0.0..1.0).contains(&s.unit(i))));
    }

    #[test]
    fn angle_delta_takes_the_short_way() {
        let pi = core::f64::consts::PI;
        assert!((angle_delta(pi - 0.1, -pi + 0.1) - 0.2).abs() < 1e-12);
        assert!((angle_delta(0.1, -0.1) + 0.2).abs() < 1e-12);
    }
}
