//! Spline-based geometry — the Sprite-Shape equivalent (Ch.35 §35.2).
//!
//! A [`SplineShape`] is a closed or open curve through control points (centripetal
//! Catmull-Rom: no cusps or self-loops between close points). It produces:
//!
//! * a **fill** (closed shapes): the outline triangulated by ear clipping, textured in world
//!   space so neighbouring shapes' fills line up;
//! * an **edge**: a strip of `edge_width` along the outline, its texture repeating every
//!   `edge_uv_length` units of arc length (grass on a hill, a rope);
//! * a **collider outline** for the physics world (segments), and shadow occluders.
//!
//! Sampling is a fixed number of samples per span (no adaptive refinement whose count could
//! depend on rounding), so the geometry is a pure function of the points (I2 corpus row).

use forge_frames::FramePos2;
use forge_num::{DVec2, det};

use crate::Error2d;
use crate::math::signed_area2;
use crate::sprite::{Mesh2d, TextureId, Vertex2d};

/// See the module docs.
#[derive(Clone, Debug, PartialEq)]
pub struct SplineShape {
    /// The frame and origin the control points are offsets from.
    pub origin: FramePos2,
    /// Control points (offsets from `origin`).
    pub knots: Vec<DVec2>,
    pub closed: bool,
    /// Samples per span between two control points (at least 1).
    pub samples: u32,
    pub edge_width: f64,
    /// World units per repeat of the edge texture along the curve.
    pub edge_uv_length: f64,
    /// World units per repeat of the fill texture.
    pub fill_uv_size: f64,
    pub fill_texture: TextureId,
    pub edge_texture: TextureId,
    pub fill_color: [f64; 4],
    pub edge_color: [f64; 4],
    pub layer: i32,
    pub order: i32,
}

impl SplineShape {
    /// A closed shape through `knots` with 8 samples a span and a 0.25-unit edge.
    #[must_use]
    pub fn closed(origin: FramePos2, knots: Vec<DVec2>, fill: TextureId, edge: TextureId) -> Self {
        Self {
            origin,
            knots,
            closed: true,
            samples: 8,
            edge_width: 0.25,
            edge_uv_length: 1.0,
            fill_uv_size: 4.0,
            fill_texture: fill,
            edge_texture: edge,
            fill_color: [1.0; 4],
            edge_color: [1.0; 4],
            layer: 0,
            order: 0,
        }
    }

    fn check(&self) -> Result<(), Error2d> {
        let min = if self.closed { 3 } else { 2 };
        if self.knots.len() < min {
            return Err(Error2d::invalid(format!(
                "a {} spline needs at least {min} points",
                if self.closed { "closed" } else { "open" }
            )));
        }
        if self.knots.iter().any(|k| !k.is_finite()) {
            return Err(Error2d::invalid("a spline point is not finite"));
        }
        Ok(())
    }

    /// The sampled outline (offsets from `origin`); closed shapes do not repeat the first
    /// point.
    pub fn outline(&self) -> Result<Vec<DVec2>, Error2d> {
        self.check()?;
        let n = self.knots.len();
        let k = |i: i64| -> DVec2 {
            if self.closed {
                self.knots[i.rem_euclid(n as i64) as usize]
            } else {
                // Open: extrapolate the end tangents by mirroring.
                if i < 0 {
                    self.knots[0] * 2.0 - self.knots[1]
                } else if i as usize >= n {
                    self.knots[n - 1] * 2.0 - self.knots[n - 2]
                } else {
                    self.knots[i as usize]
                }
            }
        };
        let spans = if self.closed { n } else { n - 1 };
        let s = self.samples.max(1);
        let mut out = Vec::with_capacity(spans * s as usize + 1);
        for i in 0..spans as i64 {
            let (p0, p1, p2, p3) = (k(i - 1), k(i), k(i + 1), k(i + 2));
            for j in 0..s {
                out.push(centripetal(p0, p1, p2, p3, f64::from(j) / f64::from(s)));
            }
        }
        if !self.closed {
            out.push(self.knots[n - 1]);
        }
        out.dedup_by(|a, b| (*a - *b).length_squared() < 1e-24);
        Ok(out)
    }

    /// The fill mesh (closed shapes).
    pub fn fill(&self) -> Result<Mesh2d, Error2d> {
        if !self.closed {
            return Err(Error2d::invalid("only a closed spline has a fill"));
        }
        let pts = self.outline()?;
        let tris = triangulate(&pts)?;
        let s = if self.fill_uv_size > 0.0 {
            self.fill_uv_size
        } else {
            1.0
        };
        let o = self.origin.local;
        Ok(Mesh2d {
            origin: self.origin,
            verts: pts
                .iter()
                .map(|p| Vertex2d {
                    offset: *p,
                    uv: DVec2::new((o.x + p.x) / s, -(o.y + p.y) / s),
                    color: self.fill_color,
                })
                .collect(),
            indices: tris,
            texture: self.fill_texture,
            layer: self.layer,
            order: self.order,
        })
    }

    /// The edge strip.
    pub fn edge(&self) -> Result<Mesh2d, Error2d> {
        let mut pts = self.outline()?;
        if self.closed {
            // Counter-clockwise, so the strip's "outside" is to the right of travel.
            if signed_area2(&pts) < 0.0 {
                pts.reverse();
            }
            pts.push(pts[0]);
        }
        let n = pts.len();
        let half = self.edge_width * 0.5;
        let uvl = if self.edge_uv_length > 0.0 {
            self.edge_uv_length
        } else {
            1.0
        };
        let mut verts = Vec::with_capacity(n * 2);
        let mut along = 0.0;
        for i in 0..n {
            let prev = if i > 0 {
                pts[i - 1]
            } else if self.closed {
                pts[n - 2]
            } else {
                pts[0]
            };
            let next = if i + 1 < n {
                pts[i + 1]
            } else if self.closed {
                pts[1]
            } else {
                pts[n - 1]
            };
            if i > 0 {
                along += (pts[i] - pts[i - 1]).length();
            }
            let t_in = (pts[i] - prev).try_normalize();
            let t_out = (next - pts[i]).try_normalize();
            let tangent = match (t_in, t_out) {
                (Some(a), Some(b)) => (a + b).try_normalize().unwrap_or(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => DVec2::X,
            };
            // Outward normal (right of travel) with a miter, limited to 2x.
            let nrm = DVec2::new(tangent.y, -tangent.x);
            let miter = match t_out.or(t_in) {
                Some(t) => {
                    let side = DVec2::new(t.y, -t.x);
                    let c = nrm.dot(side);
                    if c > 0.5 { 1.0 / c } else { 2.0 }
                }
                None => 1.0,
            };
            let u = along / uvl;
            verts.push(Vertex2d {
                offset: pts[i] + nrm * (half * miter),
                uv: DVec2::new(u, 0.0),
                color: self.edge_color,
            });
            verts.push(Vertex2d {
                offset: pts[i] - nrm * (half * miter),
                uv: DVec2::new(u, 1.0),
                color: self.edge_color,
            });
        }
        let mut indices = Vec::with_capacity((n - 1) * 6);
        for i in 0..n as u32 - 1 {
            let (a, b, c, d) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
            indices.extend_from_slice(&[a, b, c, c, b, d]);
        }
        Ok(Mesh2d {
            origin: self.origin,
            verts,
            indices,
            texture: self.edge_texture,
            layer: self.layer,
            order: self.order + 1,
        })
    }

    /// Collider / occluder segments along the outline.
    pub fn segments(&self) -> Result<Vec<(DVec2, DVec2)>, Error2d> {
        let pts = self.outline()?;
        let n = pts.len();
        let m = if self.closed { n } else { n - 1 };
        Ok((0..m).map(|i| (pts[i], pts[(i + 1) % n])).collect())
    }
}

/// A point on the centripetal Catmull-Rom span `p1`-`p2` at `t` in `[0, 1]` (Barry-Goldman).
fn centripetal(p0: DVec2, p1: DVec2, p2: DVec2, p3: DVec2, t: f64) -> DVec2 {
    let knot = |a: DVec2, b: DVec2| det::sqrt((b - a).length()).max(1e-9);
    let t0 = 0.0;
    let t1 = t0 + knot(p0, p1);
    let t2 = t1 + knot(p1, p2);
    let t3 = t2 + knot(p2, p3);
    let tt = t1 + (t2 - t1) * t;
    let lerp = |a: DVec2, b: DVec2, ta: f64, tb: f64| {
        a * ((tb - tt) / (tb - ta)) + b * ((tt - ta) / (tb - ta))
    };
    let a1 = lerp(p0, p1, t0, t1);
    let a2 = lerp(p1, p2, t1, t2);
    let a3 = lerp(p2, p3, t2, t3);
    let b1 = lerp(a1, a2, t0, t2);
    let b2 = lerp(a2, a3, t1, t3);
    lerp(b1, b2, t1, t2)
}

/// Does no edge of the closed polygon cross a non-adjacent one?
#[must_use]
pub fn is_simple(pts: &[DVec2]) -> bool {
    let n = pts.len();
    let orient = |a: DVec2, b: DVec2, c: DVec2| (b - a).cross(c - a);
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        for j in i + 1..n {
            if j == i || (j + 1) % n == i || (i + 1) % n == j {
                continue;
            }
            let (c, d) = (pts[j], pts[(j + 1) % n]);
            let (o1, o2) = (orient(a, b, c), orient(a, b, d));
            let (o3, o4) = (orient(c, d, a), orient(c, d, b));
            if o1 * o2 < 0.0 && o3 * o4 < 0.0 {
                return false;
            }
        }
    }
    true
}

/// Triangulate a simple polygon by ear clipping: indices into `pts`, counter-clockwise
/// triangles. Fails on fewer than 3 points or a polygon it cannot clip (self-intersecting).
pub fn triangulate(pts: &[DVec2]) -> Result<Vec<u32>, Error2d> {
    let n = pts.len();
    if n < 3 {
        return Err(Error2d::invalid("a polygon needs 3 points"));
    }
    if !is_simple(pts) {
        return Err(Error2d::invalid(
            "the shape crosses itself; it cannot be filled",
        ));
    }
    let mut idx: Vec<usize> = (0..n).collect();
    if signed_area2(pts) < 0.0 {
        idx.reverse();
    }
    let mut out = Vec::with_capacity((n - 2) * 3);
    let mut guard = 0;
    while idx.len() > 3 {
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            if (b - a).cross(c - b) <= 0.0 {
                continue; // reflex or flat
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = pts[j];
                (b - a).cross(p - a) >= 0.0
                    && (c - b).cross(p - b) >= 0.0
                    && (a - c).cross(p - c) >= 0.0
            });
            if inside {
                continue;
            }
            out.extend_from_slice(&[ia as u32, ib as u32, ic as u32]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Drop a collinear vertex if there is one; otherwise the polygon is not simple.
            if let Some(i) = (0..m).find(|&i| {
                let (a, b, c) = (
                    pts[idx[(i + m - 1) % m]],
                    pts[idx[i]],
                    pts[idx[(i + 1) % m]],
                );
                (b - a).cross(c - b) == 0.0
            }) {
                idx.remove(i);
            } else {
                return Err(Error2d::invalid(
                    "the shape crosses itself; it cannot be filled",
                ));
            }
        }
        guard += 1;
        if guard > n * n {
            return Err(Error2d::invalid("triangulation did not finish"));
        }
    }
    out.extend_from_slice(&[idx[0] as u32, idx[1] as u32, idx[2] as u32]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_frames::FrameId;

    fn blob() -> SplineShape {
        SplineShape::closed(
            FramePos2::new(FrameId(0), DVec2::ZERO),
            vec![
                DVec2::new(0.0, 0.0),
                DVec2::new(4.0, -0.5),
                DVec2::new(6.0, 2.0),
                DVec2::new(3.0, 3.0),
                DVec2::new(2.5, 1.5),
                DVec2::new(0.5, 2.5),
            ],
            TextureId(1),
            TextureId(2),
        )
    }

    #[test]
    fn the_curve_passes_through_its_points() {
        let s = blob();
        let o = s.outline().expect("outline");
        assert_eq!(o.len(), 48);
        for (i, k) in s.knots.iter().enumerate() {
            assert!((o[i * 8] - *k).length() < 1e-12, "knot {i}");
        }
    }

    #[test]
    fn the_fill_covers_the_outline_area_exactly() {
        let s = blob();
        let m = s.fill().expect("fill");
        let pts = s.outline().expect("outline");
        let area: f64 = m
            .indices
            .chunks(3)
            .map(|t| {
                let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
                let x = (b - a).cross(c - a) * 0.5;
                assert!(x > 0.0, "counter-clockwise triangle");
                x
            })
            .sum();
        let want = signed_area2(&pts).abs() * 0.5;
        assert!((area - want).abs() < 1e-9, "{area} vs {want}");
        assert_eq!(m.indices.len(), (pts.len() - 2) * 3);
    }

    #[test]
    fn the_edge_strip_follows_the_curve_with_arc_length_uvs() {
        let mut s = blob();
        s.edge_uv_length = 0.5;
        let e = s.edge().expect("edge");
        let pts = s.outline().expect("outline");
        assert_eq!(e.verts.len(), (pts.len() + 1) * 2);
        let perimeter: f64 = (0..pts.len())
            .map(|i| (pts[(i + 1) % pts.len()] - pts[i]).length())
            .sum();
        let last_u = e.verts[e.verts.len() - 1].uv.x;
        assert!((last_u - perimeter / 0.5).abs() < 1e-9);
        for w in e.verts.chunks(2) {
            let width = (w[0].offset - w[1].offset).length();
            assert!((0.25 - 1e-9..=0.5 + 1e-9).contains(&width), "{width}");
        }
    }

    #[test]
    fn a_self_crossing_shape_is_refused_and_an_open_one_has_no_fill() {
        let bow = SplineShape {
            samples: 1,
            ..SplineShape::closed(
                FramePos2::new(FrameId(0), DVec2::ZERO),
                vec![
                    DVec2::new(0.0, 0.0),
                    DVec2::new(2.0, 2.0),
                    DVec2::new(2.0, 0.0),
                    DVec2::new(0.0, 2.0),
                ],
                TextureId(0),
                TextureId(0),
            )
        };
        assert!(bow.fill().is_err());
        let open = SplineShape {
            closed: false,
            ..blob()
        };
        assert!(open.fill().is_err());
        assert_eq!(
            open.segments().expect("segs").len(),
            open.outline().expect("o").len() - 1
        );
    }
}
