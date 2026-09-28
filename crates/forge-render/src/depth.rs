//! Depth passes (Ch.2.4): the view is drawn **back to front** in the depth passes the frame
//! resolver's [`ViewDepth`] names, into one HDR target, each pass with its **own depth
//! clear** and a **reversed-Z** projection (near plane at depth 1, far plane at depth 0,
//! `Greater` test, cleared to 0). Beyond the last pass a mesh instance is culled, or drawn
//! as a sky sprite where the view depth says so.
//!
//! A single-frame world is drawn in one pass from 1 cm to 1e4 km
//! ([`ViewDepth::WORLD`]): reversed-Z with a 32-bit float depth buffer keeps ~1e-7 relative
//! precision over the whole range. A resolver whose frames reach further hands the renderer
//! more passes (at most [`MAX_DEPTH_PASSES`]); an object straddling a boundary between two
//! passes is drawn in both, each clipped exactly at the boundary, the nearer pass composited
//! over the farther one.

pub use forge_frames::{DepthSpan, ViewDepth};

use crate::last_mile::ViewOffset;

/// The most depth passes a view is drawn in (the pass-uniform blocks reserved for them).
pub const MAX_DEPTH_PASSES: usize = 3;

/// How depth is set up: the view depth (the resolver's own unless one is given) and the
/// direction of the depth test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthSetup {
    /// The passes to draw in; `None`: the frame resolver's [`ViewDepth`] (the engine).
    pub layout: Option<ViewDepth>,
    /// Reversed-Z (cleared to 0, `Greater`). Always `true` except in W2 positive controls,
    /// which set a conventional (0 near, 1 far, `Less`) depth to show what reversed-Z buys.
    pub reversed: bool,
}

impl Default for DepthSetup {
    fn default() -> Self {
        Self {
            layout: None,
            reversed: true,
        }
    }
}

impl DepthSetup {
    /// The view depth this setup draws a resolver's frames in.
    #[must_use]
    pub fn resolve(&self, resolver: ViewDepth) -> ViewDepth {
        self.layout.unwrap_or(resolver)
    }
}

/// The perspective projection of one depth pass (column-major, `f64`). Reversed: depth 1 at
/// `near`, 0 at `far`. Conventional: 0 at `near`, 1 at `far`. `clip.w` is the view depth.
#[must_use]
pub fn projection(
    tan_half_fov: f64,
    aspect: f64,
    near: f64,
    far: f64,
    reversed: bool,
) -> [[f64; 4]; 4] {
    let f = 1.0 / tan_half_fov;
    let (n, fa) = (near, far);
    let (a, b) = if reversed {
        (n / (fa - n), n * fa / (fa - n))
    } else {
        (-fa / (fa - n), -fa * n / (fa - n))
    };
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, a, -1.0],
        [0.0, 0.0, b, 0.0],
    ]
}

/// Normalised device depth of view depth `d` under [`projection`] (for tests and docs).
#[must_use]
pub fn ndc_depth(d: f64, span: &DepthSpan, reversed: bool) -> f64 {
    let p = projection(1.0, 1.0, span.near, span.far, reversed);
    let z = -d;
    (p[2][2] * z + p[3][2]) / d
}

/// Which depth passes (indices into `passes`) a bounding sphere at view depth `depth` with
/// radius `radius` overlaps, as a bit mask.
#[must_use]
pub fn overlapping(passes: &[DepthSpan], depth: f64, radius: f64) -> u32 {
    let (lo, hi) = (depth - radius, depth + radius);
    let mut mask = 0;
    for (i, r) in passes.iter().enumerate() {
        if hi > r.near && lo < r.far {
            mask |= 1 << i;
        }
    }
    mask
}

/// The pass-uniform block of pass `p` of `n` (back to front): the nearest pass is block 0.
#[must_use]
pub const fn pass_block(p: usize, n: usize) -> usize {
    n - 1 - p
}

/// Is a view-space sphere outside the side planes of the view frustum?
#[must_use]
pub fn outside_sides(offset: ViewOffset, radius: f64, tan_half_fov: f64, aspect: f64) -> bool {
    let center = offset.0;
    let side = |t: f64, lateral: f64| {
        // Plane through the eye: lateral * cos(h) - depth * sin(h) = 0, outward positive.
        let inv = 1.0 / forge_num::det::sqrt(1.0 + t * t);
        let (c, s) = (inv, t * inv);
        let depth = -center.z;
        lateral * c - depth * s > radius
    };
    let tx = tan_half_fov * aspect;
    side(tan_half_fov, center.y)
        || side(tan_half_fov, -center.y)
        || side(tx, center.x)
        || side(tx, -center.x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_num::DVec3;

    const TWO: [DepthSpan; 2] = [
        DepthSpan {
            near: 1e3,
            far: 1e7,
            pass: "b",
            attachment: "depth.b",
        },
        DepthSpan {
            near: 0.01,
            far: 1e3,
            pass: "a",
            attachment: "depth.a",
        },
    ];

    #[test]
    fn reversed_z_maps_near_to_one_and_far_to_zero() {
        for s in ViewDepth::WORLD.passes.iter().chain(&TWO) {
            assert!((ndc_depth(s.near, s, true) - 1.0).abs() < 1e-12);
            assert!(ndc_depth(s.far, s, true).abs() < 1e-9);
            assert!(ndc_depth(s.near, s, false).abs() < 1e-9);
            assert!((ndc_depth(s.far, s, false) - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn a_sphere_on_a_boundary_is_in_both_passes() {
        // passes are back to front: [1e3..1e7, 0.01..1e3]
        assert_eq!(overlapping(&TWO, 1e3, 10.0), 0b11);
        assert_eq!(overlapping(&TWO, 5.0, 1.0), 0b10);
        assert_eq!(overlapping(&TWO, 1e5, 1.0), 0b01);
        assert_eq!(overlapping(&TWO, 1e9, 1.0), 0);
        assert_eq!(overlapping(ViewDepth::WORLD.passes, 5e4, 1.0), 0b1);
        assert_eq!(overlapping(ViewDepth::WORLD.passes, 2e7, 1.0), 0);
    }

    #[test]
    fn the_nearest_pass_takes_block_zero() {
        assert_eq!(pass_block(0, 1), 0);
        assert_eq!(pass_block(0, 3), 2);
        assert_eq!(pass_block(2, 3), 0);
    }

    #[test]
    fn the_default_setup_is_the_resolvers_reversed_depth() {
        let d = DepthSetup::default();
        assert!(d.reversed);
        assert_eq!(d.resolve(ViewDepth::WORLD), ViewDepth::WORLD);
        let custom = ViewDepth {
            passes: &TWO,
            sprites_beyond: None,
        };
        let s = DepthSetup {
            layout: Some(custom),
            reversed: false,
        };
        assert_eq!(s.resolve(ViewDepth::WORLD), custom);
    }

    #[test]
    fn side_culling() {
        let t = 0.5;
        assert!(!outside_sides(
            ViewOffset(DVec3::new(0.0, 0.0, -10.0)),
            1.0,
            t,
            1.0
        ));
        assert!(outside_sides(
            ViewOffset(DVec3::new(20.0, 0.0, -10.0)),
            1.0,
            t,
            1.0
        ));
        assert!(!outside_sides(
            ViewOffset(DVec3::new(5.5, 0.0, -10.0)),
            1.0,
            t,
            1.0
        ));
        assert!(outside_sides(
            ViewOffset(DVec3::new(0.0, -8.0, -10.0)),
            1.0,
            t,
            1.0
        ));
    }
}
