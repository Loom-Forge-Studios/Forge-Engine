//! **The avian backend's narrowing to `f32` — the only place in forge-phys it happens**
//! (allow-listed in `tests/liveness/f32_allow.txt`, with the reason).
//!
//! avian3d 0.7 is `f64` in its `f64` build except in three places this backend must feed:
//! a collider's density (`ColliderDensity` is `f32`), and the direction and start of the ray
//! or sweep that walks its bounding-volume trees (bevy's `Dir3` / `Ray3d` and the `obvhs`
//! trees are `f32`). None of them positions anything: density feeds mass (avian's mass
//! properties are `f32` internally), and the tree walk only prunes — every hit this backend
//! reports is then computed again in `f64` against the collider's exact pose.

use avian3d::math::Vector;
use avian3d::prelude::ColliderDensity;
use bevy_math::{Dir3, Ray3d, Vec3};

fn narrow(v: Vector) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// A collider density for avian (kg/m³).
pub(super) fn density(d: f64) -> ColliderDensity {
    ColliderDensity(d as f32)
}

/// A unit direction for a tree sweep (`None` for a zero vector).
pub(super) fn dir(d: Vector) -> Option<Dir3> {
    Dir3::new(narrow(d)).ok()
}

/// A ray for a tree walk.
pub(super) fn ray(start: Vector, d: Vector) -> Option<Ray3d> {
    Some(Ray3d::new(narrow(start), dir(d)?))
}
