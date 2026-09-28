//! `DVec3` and `DQuat` — the engine's own minimal f64 vector and rotation types (ADR 0004).
//!
//! They live here, in the deterministic-math leaf, rather than coming from a general linear
//! algebra crate, because every operation on them must be bit-reproducible (Ch.3): each one
//! is a fixed sequence of IEEE `+ - * /` and `sqrt`, no FMA, and the only transcendental
//! (building a rotation from an angle) goes through [`crate::det`]. A general-purpose crate
//! offers angle constructors, slerp and `angle_between` that call the platform libm — every
//! one a silent cross-platform divergence below `forge-render`. This API has none of them.
//!
//! **A `DVec3` is never a position on its own** (invariant I1): a position is
//! `forge_frames::FramePos { frame, local }`. The I1 lint
//! (`tests/liveness/test_frame_liveness.rs`) rejects a bare `DVec3` in any position-shaped
//! public signature.

use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

use crate::det;

/// A 3-vector of `f64`: a displacement, velocity, axis or angular velocity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DVec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl DVec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub const fn splat(v: f64) -> Self {
        Self::new(v, v, v)
    }

    /// `x ox + y oy + z oz`, summed left to right.
    #[inline]
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    #[inline]
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    #[inline]
    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    /// Euclidean length (IEEE `sqrt`, exact to the bit).
    #[inline]
    pub fn length(self) -> f64 {
        det::sqrt(self.length_squared())
    }

    /// `self / |self|`, or `None` for a zero, non-finite or subnormal-length vector.
    #[inline]
    pub fn try_normalize(self) -> Option<Self> {
        let l = self.length();
        (l.is_finite() && l >= f64::MIN_POSITIVE).then(|| self / l)
    }

    /// The largest component magnitude.
    #[inline]
    pub fn max_abs(self) -> f64 {
        self.x.abs().max(self.y.abs()).max(self.z.abs())
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Bit patterns of the three components — for hashing and bit-exact comparison.
    #[inline]
    pub fn to_bits(self) -> [u64; 3] {
        [self.x.to_bits(), self.y.to_bits(), self.z.to_bits()]
    }
}

impl Add for DVec3 {
    type Output = Self;
    #[inline]
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl AddAssign for DVec3 {
    #[inline]
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

impl Sub for DVec3 {
    type Output = Self;
    #[inline]
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl SubAssign for DVec3 {
    #[inline]
    fn sub_assign(&mut self, o: Self) {
        *self = *self - o;
    }
}

impl Neg for DVec3 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Mul<f64> for DVec3 {
    type Output = Self;
    #[inline]
    fn mul(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Mul<DVec3> for f64 {
    type Output = DVec3;
    #[inline]
    fn mul(self, v: DVec3) -> DVec3 {
        v * self
    }
}

impl Div<f64> for DVec3 {
    type Output = Self;
    #[inline]
    fn div(self, s: f64) -> Self {
        Self::new(self.x / s, self.y / s, self.z / s)
    }
}

/// A 2-vector of `f64` (the 2D pipeline, Ch.35 §35.3): a displacement, velocity or
/// direction. Like [`DVec3`] it is never a position on its own — a 2D position is
/// `forge_frames::FramePos2 { frame, local }` (I1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DVec2 {
    pub x: f64,
    pub y: f64,
}

impl DVec2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);
    pub const X: Self = Self::new(1.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0);

    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[inline]
    pub const fn splat(v: f64) -> Self {
        Self::new(v, v)
    }

    /// `x ox + y oy`, summed left to right.
    #[inline]
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y
    }

    /// The z of the 3D cross product: `x oy - y ox` (positive when `o` is counter-clockwise
    /// of `self`).
    #[inline]
    pub fn cross(self, o: Self) -> f64 {
        self.x * o.y - self.y * o.x
    }

    /// `self` turned a quarter counter-clockwise: `(-y, x)`.
    #[inline]
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// `s x v` for a scalar (z-axis) angular quantity `s`: `(-s vy, s vx)`.
    #[inline]
    pub fn cross_scalar(s: f64, v: Self) -> Self {
        Self::new(-s * v.y, s * v.x)
    }

    #[inline]
    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    /// Euclidean length (IEEE `sqrt`, exact to the bit).
    #[inline]
    pub fn length(self) -> f64 {
        det::sqrt(self.length_squared())
    }

    /// `self / |self|`, or `None` for a zero, non-finite or subnormal-length vector.
    #[inline]
    pub fn try_normalize(self) -> Option<Self> {
        let l = self.length();
        (l.is_finite() && l >= f64::MIN_POSITIVE).then(|| self / l)
    }

    /// Component-wise minimum.
    #[inline]
    pub fn min(self, o: Self) -> Self {
        Self::new(self.x.min(o.x), self.y.min(o.y))
    }

    /// Component-wise maximum.
    #[inline]
    pub fn max(self, o: Self) -> Self {
        Self::new(self.x.max(o.x), self.y.max(o.y))
    }

    /// Component-wise product.
    #[inline]
    pub fn mul_elem(self, o: Self) -> Self {
        Self::new(self.x * o.x, self.y * o.y)
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// Bit patterns of the two components — for hashing and bit-exact comparison.
    #[inline]
    pub fn to_bits(self) -> [u64; 2] {
        [self.x.to_bits(), self.y.to_bits()]
    }
}

impl Add for DVec2 {
    type Output = Self;
    #[inline]
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl AddAssign for DVec2 {
    #[inline]
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

impl Sub for DVec2 {
    type Output = Self;
    #[inline]
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl SubAssign for DVec2 {
    #[inline]
    fn sub_assign(&mut self, o: Self) {
        *self = *self - o;
    }
}

impl Neg for DVec2 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl Mul<f64> for DVec2 {
    type Output = Self;
    #[inline]
    fn mul(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

impl Mul<DVec2> for f64 {
    type Output = DVec2;
    #[inline]
    fn mul(self, v: DVec2) -> DVec2 {
        v * self
    }
}

impl Div<f64> for DVec2 {
    type Output = Self;
    #[inline]
    fn div(self, s: f64) -> Self {
        Self::new(self.x / s, self.y / s)
    }
}

/// A rotation as a unit quaternion `w + xi + yj + zk`.
///
/// `rotate(v)` maps a vector expressed in the rotated (child) axes into the reference
/// (parent) axes; `inverse_rotate` maps back. Composition: `(a * b).rotate(v) ==
/// a.rotate(b.rotate(v))` up to rounding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DQuat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Default for DQuat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl DQuat {
    pub const IDENTITY: Self = Self::from_xyzw(0.0, 0.0, 0.0, 1.0);

    #[inline]
    pub const fn from_xyzw(x: f64, y: f64, z: f64, w: f64) -> Self {
        Self { x, y, z, w }
    }

    /// Rotation by `angle` radians about the unit vector `axis` (right-handed), with
    /// `sin`/`cos` from [`det`] so the result is identical on every platform. `axis` must
    /// be unit length; use [`DQuat::from_axis_angle_checked`] when it may not be.
    #[inline]
    pub fn from_axis_angle(axis: DVec3, angle: f64) -> Self {
        let h = 0.5 * angle;
        let (s, c) = (det::sin(h), det::cos(h));
        Self::from_xyzw(axis.x * s, axis.y * s, axis.z * s, c)
    }

    /// [`DQuat::from_axis_angle`] after normalising `axis`; `None` if `axis` is zero or not
    /// finite.
    #[inline]
    pub fn from_axis_angle_checked(axis: DVec3, angle: f64) -> Option<Self> {
        axis.try_normalize()
            .map(|a| Self::from_axis_angle(a, angle))
            .filter(|q| q.is_finite())
    }

    /// Rotation by `turns` full turns (`turns = 1` is `2 pi` radians) about unit `axis`.
    /// Taking the angle in turns lets callers reduce an exact phase (for example from an
    /// integer tick count) before any rounding by `2 pi`.
    #[inline]
    pub fn from_axis_turns(axis: DVec3, turns: f64) -> Self {
        Self::from_axis_angle(axis, turns * core::f64::consts::TAU)
    }

    #[inline]
    pub fn conjugate(self) -> Self {
        Self::from_xyzw(-self.x, -self.y, -self.z, self.w)
    }

    #[inline]
    pub fn length_squared(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w
    }

    /// `self / |self|`, or `None` for a zero or non-finite quaternion.
    #[inline]
    pub fn try_normalize(self) -> Option<Self> {
        let l = det::sqrt(self.length_squared());
        (l.is_finite() && l >= f64::MIN_POSITIVE)
            .then(|| Self::from_xyzw(self.x / l, self.y / l, self.z / l, self.w / l))
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite() && self.w.is_finite()
    }

    /// The vector part `(x, y, z)`.
    #[inline]
    pub fn xyz(self) -> DVec3 {
        DVec3::new(self.x, self.y, self.z)
    }

    /// Rotate `v` from the child axes into the parent axes:
    /// `v + 2w (q x v) + 2 q x (q x v)` with `q = (x, y, z)`.
    #[inline]
    pub fn rotate(self, v: DVec3) -> DVec3 {
        let q = self.xyz();
        let t = q.cross(v) * 2.0;
        v + t * self.w + q.cross(t)
    }

    /// Rotate `v` from the parent axes into the child axes (the inverse of [`DQuat::rotate`]
    /// for a unit quaternion).
    #[inline]
    pub fn inverse_rotate(self, v: DVec3) -> DVec3 {
        self.conjugate().rotate(v)
    }
}

impl Mul for DQuat {
    type Output = Self;
    /// Hamilton product: `(a * b).rotate(v) == a.rotate(b.rotate(v))`.
    #[inline]
    fn mul(self, b: Self) -> Self {
        let a = self;
        Self::from_xyzw(
            a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
            a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
            a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
            a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: DVec3, b: DVec3, tol: f64) -> bool {
        (a - b).max_abs() <= tol
    }

    #[test]
    fn cross_is_right_handed() {
        assert_eq!(DVec3::X.cross(DVec3::Y), DVec3::Z);
        assert_eq!(DVec3::Y.cross(DVec3::Z), DVec3::X);
        assert_eq!(DVec3::Z.cross(DVec3::X), DVec3::Y);
    }

    #[test]
    fn quarter_turn_about_z_maps_x_to_y() {
        let q = DQuat::from_axis_turns(DVec3::Z, 0.25);
        assert!(close(q.rotate(DVec3::X), DVec3::Y, 1e-15));
        assert!(close(q.inverse_rotate(DVec3::Y), DVec3::X, 1e-15));
    }

    #[test]
    fn composition_matches_sequential_rotation() {
        let a = DQuat::from_axis_angle_checked(DVec3::new(1.0, 2.0, 3.0), 0.7).unwrap();
        let b = DQuat::from_axis_angle_checked(DVec3::new(-2.0, 0.5, 1.0), -1.3).unwrap();
        let v = DVec3::new(3.0, -4.0, 5.0);
        assert!(close((a * b).rotate(v), a.rotate(b.rotate(v)), 1e-14));
        assert!(close(a.inverse_rotate(a.rotate(v)), v, 1e-14));
    }

    #[test]
    fn rotation_preserves_length() {
        let q = DQuat::from_axis_angle_checked(DVec3::new(0.3, -0.2, 0.9), 2.1).unwrap();
        let v = DVec3::new(1e11, -3e10, 7.0);
        let r = q.rotate(v);
        assert!((r.length() - v.length()).abs() <= 1e-15 * v.length() * 4.0);
    }

    #[test]
    fn degenerate_inputs_are_rejected_not_nan() {
        assert_eq!(DVec3::ZERO.try_normalize(), None);
        assert_eq!(DVec3::new(f64::NAN, 0.0, 0.0).try_normalize(), None);
        assert_eq!(DQuat::from_axis_angle_checked(DVec3::ZERO, 1.0), None);
        assert_eq!(DQuat::from_xyzw(0.0, 0.0, 0.0, 0.0).try_normalize(), None);
    }
}
