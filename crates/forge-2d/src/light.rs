//! 2D lights, shadows and normal maps (Ch.35 §35.2).
//!
//! A light is a point (or a spot: a cone) in the plane with a colour, an intensity, a
//! radius it fades to zero at, and a **height** above the plane: the renderer lights each
//! pixel by `N . L` with `N` from the sprite's normal map and `L` toward the light raised to
//! that height, so a flat sprite with a normal map shades like a relief.
//!
//! **Shadows are exact, hard and computed on the CPU in `f64`**: for each shadow-casting
//! light, [`visibility`] sweeps the occluders' edges inside its radius and returns the
//! polygon the light actually reaches. The renderer draws that polygon (a triangle fan) into
//! the light buffer — so a light costs one fan of triangles, all lights are one draw call,
//! and nothing outside the lit region is shaded at all. The polygon is a pure function of
//! the inputs (it joins the I2 corpus).

use std::sync::OnceLock;

use forge_frames::{FrameId, FramePos2};
use forge_num::{DVec2, det};

use crate::math::Aabb2;

/// A light's shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightShape {
    /// Every direction.
    Point,
    /// A cone around `direction` (radians): full intensity within `inner`, none beyond
    /// `outer` (half angles, radians).
    Spot {
        direction: f64,
        inner: f64,
        outer: f64,
    },
}

/// See the module docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light2d {
    pub pos: FramePos2,
    /// Linear RGB.
    pub color: [f64; 3],
    pub intensity: f64,
    /// Where it fades to nothing (world units).
    pub radius: f64,
    /// Height above the plane, for normal-mapped shading (world units).
    pub height: f64,
    pub shape: LightShape,
    /// Blocked by occluders.
    pub shadows: bool,
}

impl Light2d {
    /// A white shadow-casting point light.
    #[must_use]
    pub fn point(pos: FramePos2, radius: f64, intensity: f64) -> Self {
        Self {
            pos,
            color: [1.0; 3],
            intensity,
            radius,
            height: radius * 0.25,
            shape: LightShape::Point,
            shadows: true,
        }
    }
}

/// A shadow caster: an outline in `frame` (closed: a polygon; open: a line of edges).
#[derive(Clone, Debug, PartialEq)]
pub struct Occluder {
    pub frame: FrameId,
    pub outline: Vec<DVec2>,
    pub closed: bool,
}

impl Occluder {
    /// The box `[lo, hi]`.
    #[must_use]
    pub fn rect(frame: FrameId, lo: DVec2, hi: DVec2) -> Self {
        Self {
            frame,
            outline: vec![lo, DVec2::new(hi.x, lo.y), hi, DVec2::new(lo.x, hi.y)],
            closed: true,
        }
    }

    /// Its edges.
    pub fn edges(&self) -> impl Iterator<Item = (DVec2, DVec2)> + '_ {
        let n = self.outline.len();
        let m = if self.closed && n > 2 {
            n
        } else {
            n.saturating_sub(1)
        };
        (0..m).map(move |i| (self.outline[i], self.outline[(i + 1) % n]))
    }
}

/// Sides of the polygon that bounds an unshadowed light (circumscribing its radius).
pub const BOUND_SIDES: usize = 32;

fn bound_dirs() -> &'static [DVec2; BOUND_SIDES] {
    static DIRS: OnceLock<[DVec2; BOUND_SIDES]> = OnceLock::new();
    DIRS.get_or_init(|| {
        let mut d = [DVec2::ZERO; BOUND_SIDES];
        for (k, v) in d.iter_mut().enumerate() {
            let a = core::f64::consts::TAU * k as f64 / BOUND_SIDES as f64;
            *v = DVec2::new(det::cos(a), det::sin(a));
        }
        d
    })
}

/// The circumscribed radius factor `1 / cos(pi / BOUND_SIDES)`.
fn bound_scale() -> f64 {
    static S: OnceLock<f64> = OnceLock::new();
    *S.get_or_init(|| 1.0 / det::cos(core::f64::consts::PI / BOUND_SIDES as f64))
}

/// A monotonic stand-in for the angle of `d` in `[0, 4)`, counter-clockwise from +x, with
/// no transcendental (the "diamond angle").
fn pseudo_angle(d: DVec2) -> f64 {
    if d.y >= 0.0 {
        if d.x >= 0.0 {
            d.y / (d.x + d.y)
        } else {
            1.0 - d.x / (-d.x + d.y)
        }
    } else if d.x < 0.0 {
        2.0 - d.y / (-d.x - d.y)
    } else {
        3.0 + d.x / (d.x - d.y)
    }
}

/// Nearest `t > 0` where the ray `o + t d` crosses segment `a b`.
fn ray_hit(o: DVec2, d: DVec2, a: DVec2, b: DVec2) -> Option<f64> {
    let e = b - a;
    let den = d.cross(e);
    if den == 0.0 {
        return None;
    }
    let w = a - o;
    let t = w.cross(e) / den;
    let u = w.cross(d) / den;
    (t > 1e-12 && (-1e-12..=1.0 + 1e-12).contains(&u)).then_some(t)
}

/// Buffers [`visibility_with`] reuses between lights and frames (no allocation once warm).
#[derive(Clone, Debug, Default)]
pub struct VisibilityScratch {
    bound: Vec<DVec2>,
    near: Vec<(DVec2, DVec2)>,
    dirs: Vec<(f64, DVec2)>,
}

/// The region a light at `center` reaches within `radius`, blocked by `segments`: a
/// polygon, counter-clockwise, as offsets from `center` (the fan's rim), written to `out`.
/// With no segment in reach it is the circumscribing [`BOUND_SIDES`]-gon.
pub fn visibility(center: DVec2, radius: f64, segments: &[(DVec2, DVec2)], out: &mut Vec<DVec2>) {
    visibility_with(
        &mut VisibilityScratch::default(),
        center,
        radius,
        segments,
        out,
    );
}

/// [`visibility`] with reused buffers (what the renderer's last mile calls per light).
pub fn visibility_with(
    s: &mut VisibilityScratch,
    center: DVec2,
    radius: f64,
    segments: &[(DVec2, DVec2)],
    out: &mut Vec<DVec2>,
) {
    out.clear();
    let r = radius * bound_scale();
    s.bound.clear();
    s.bound.extend(bound_dirs().iter().map(|d| *d * r));
    let reach = Aabb2::new(center - DVec2::splat(r), center + DVec2::splat(r));
    s.near.clear();
    s.near.extend(
        segments
            .iter()
            .filter(|(a, b)| Aabb2::new(a.min(*b), a.max(*b)).overlaps(&reach))
            .map(|(a, b)| (*a - center, *b - center)),
    );
    if s.near.is_empty() {
        out.extend_from_slice(&s.bound);
        return;
    }
    // Rays toward every endpoint, a hair either side of it, and every bound vertex.
    let (ce, se) = (1.0 - 0.5e-8, 1e-4);
    let dirs = &mut s.dirs;
    dirs.clear();
    let mut push = |d: DVec2| {
        if let Some(u) = d.try_normalize() {
            dirs.push((pseudo_angle(u), u));
        }
    };
    for (a, b) in &s.near {
        for p in [*a, *b] {
            if p.length_squared() > r * r * 2.0 {
                continue;
            }
            push(p);
            push(DVec2::new(ce * p.x - se * p.y, se * p.x + ce * p.y));
            push(DVec2::new(ce * p.x + se * p.y, -se * p.x + ce * p.y));
        }
    }
    for d in &s.bound {
        push(*d);
    }
    // A total order (angle, then the direction's bits): an unstable sort is deterministic
    // and allocates nothing.
    s.dirs.sort_unstable_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.x.total_cmp(&b.1.x))
            .then(a.1.y.total_cmp(&b.1.y))
    });
    s.dirs.dedup_by(|a, b| a.0 == b.0);
    let nb = s.bound.len();
    for (_, d) in &s.dirs {
        let mut t = f64::INFINITY;
        for (a, b) in &s.near {
            if let Some(x) = ray_hit(DVec2::ZERO, *d, *a, *b) {
                t = t.min(x);
            }
        }
        for i in 0..nb {
            if let Some(x) = ray_hit(DVec2::ZERO, *d, s.bound[i], s.bound[(i + 1) % nb]) {
                t = t.min(x);
            }
        }
        if t.is_finite() {
            out.push(*d * t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(poly: &[DVec2]) -> f64 {
        crate::math::signed_area2(poly) * 0.5
    }

    #[test]
    fn an_unblocked_light_is_its_bound_and_counter_clockwise() {
        let mut out = Vec::new();
        visibility(DVec2::ZERO, 10.0, &[], &mut out);
        assert_eq!(out.len(), BOUND_SIDES);
        let a = area(&out);
        let circle = core::f64::consts::PI * 100.0;
        assert!(a > circle && a < circle * 1.02, "{a}");
    }

    #[test]
    fn a_wall_casts_a_shadow_behind_it() {
        let mut out = Vec::new();
        // A wall 2 units right of the light, from y -1 to 1.
        let wall = [(DVec2::new(2.0, -1.0), DVec2::new(2.0, 1.0))];
        visibility(DVec2::ZERO, 10.0, &wall, &mut out);
        // Straight right, the lit region stops at the wall.
        let right: Vec<&DVec2> = out
            .iter()
            .filter(|p| p.y.abs() < 0.5 && p.x > 0.0)
            .collect();
        assert!(!right.is_empty());
        assert!(right.iter().all(|p| p.x <= 2.0 + 1e-9), "{right:?}");
        // Just past the wall's end the light goes on to the bound.
        assert!(out.iter().any(|p| p.y > 1.0 && p.x > 5.0));
        let lit = area(&out);
        let unblocked = core::f64::consts::PI * 100.0;
        // The shadow is the wedge behind the wall: atan(1/2) each side of +x.
        let wedge_angle = 2.0 * det::atan2(1.0, 2.0);
        let expected = unblocked - 0.5 * wedge_angle * 100.0 + 0.5 * 2.0 * 2.0 * 2.0 * 0.5;
        assert!(
            (lit - expected).abs() / expected < 0.03,
            "lit {lit}, about {expected}"
        );
    }

    #[test]
    fn visibility_is_a_pure_function() {
        let segs: Vec<(DVec2, DVec2)> = (0..20)
            .map(|i| {
                let a = DVec2::new(f64::from(i) - 10.0, 3.0 + f64::from(i % 3));
                (a, a + DVec2::new(0.5, 0.5))
            })
            .collect();
        let (mut x, mut y) = (Vec::new(), Vec::new());
        visibility(DVec2::new(0.1, 0.2), 12.0, &segs, &mut x);
        visibility(DVec2::new(0.1, 0.2), 12.0, &segs, &mut y);
        assert_eq!(x, y);
        assert!(area(&x) > 0.0);
    }
}
