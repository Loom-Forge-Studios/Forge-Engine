//! Narrow phase: contact manifolds between convex hulls with a rounding radius.
//!
//! Every shape is a [`Hull`]: 1 to [`MAX_VERTS`] core vertices (counter-clockwise) and a
//! radius. A circle is one vertex with a radius, a segment two vertices, a capsule two
//! vertices with a radius, a box four, a rounded box four with a radius. One set of routines
//! covers every pair, the way Box2D v3 does it:
//!
//! * **cores overlap** (deep contact): separating-axis search over both hulls' face normals,
//!   reference/incident faces, and the incident edge clipped to the reference face — up to
//!   two points;
//! * **cores apart** (rounded shapes touching through their radii, or a speculative gap):
//!   the closest points of the two cores; when the closest features are two nearly parallel
//!   faces the incident face is clipped as above (a capsule lying on the ground gets two
//!   points, so it does not rock), otherwise one point along the closest-point direction.
//!
//! Each point carries a feature id so the solver can match it to last step's point and warm
//! start it. Everything is `f64` `+ - * /` and `sqrt`: bit-identical on every platform.

use forge_num::{DVec2, det};

use crate::math::{Aabb2, Rot2};

/// Most vertices a hull has.
pub const MAX_VERTS: usize = 8;

/// A convex hull with a rounding radius (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hull {
    pub verts: [DVec2; MAX_VERTS],
    /// Outward unit normal of edge `i` (from `verts[i]` to `verts[i + 1]`).
    pub normals: [DVec2; MAX_VERTS],
    pub count: usize,
    pub radius: f64,
}

impl Hull {
    /// A circle.
    #[must_use]
    pub fn circle(center: DVec2, radius: f64) -> Hull {
        let mut h = Hull {
            verts: [DVec2::ZERO; MAX_VERTS],
            normals: [DVec2::ZERO; MAX_VERTS],
            count: 1,
            radius,
        };
        h.verts[0] = center;
        h
    }

    /// A segment `a`-`b` rounded by `radius` (a capsule; `radius` 0: a bare segment).
    #[must_use]
    pub fn segment(a: DVec2, b: DVec2, radius: f64) -> Option<Hull> {
        let n = (b - a).try_normalize()?.perp();
        let mut h = Hull {
            verts: [DVec2::ZERO; MAX_VERTS],
            normals: [DVec2::ZERO; MAX_VERTS],
            count: 2,
            radius,
        };
        h.verts[0] = a;
        h.verts[1] = b;
        // Edge 0 runs a->b with outward normal to its right; edge 1 runs back.
        h.normals[0] = -n;
        h.normals[1] = n;
        Some(h)
    }

    /// A convex polygon (counter-clockwise or clockwise; at most [`MAX_VERTS`] vertices),
    /// rounded by `radius`. `None` if it is not strictly convex or is degenerate.
    #[must_use]
    pub fn polygon(points: &[DVec2], radius: f64) -> Option<Hull> {
        let n = points.len();
        if !(3..=MAX_VERTS).contains(&n) {
            return None;
        }
        let area2 = crate::math::signed_area2(points);
        if area2.is_nan() || area2.abs() <= 1e-12 {
            return None;
        }
        let mut v = [DVec2::ZERO; MAX_VERTS];
        for i in 0..n {
            v[i] = if area2 > 0.0 {
                points[i]
            } else {
                points[n - 1 - i]
            };
        }
        let mut normals = [DVec2::ZERO; MAX_VERTS];
        for i in 0..n {
            let e = v[(i + 1) % n] - v[i];
            let nn = DVec2::new(e.y, -e.x).try_normalize()?;
            normals[i] = nn;
            // Strict convexity: every other vertex is strictly behind this edge.
            for j in 0..n {
                if j != i && j != (i + 1) % n && nn.dot(v[j] - v[i]) >= -1e-12 {
                    return None;
                }
            }
        }
        Some(Hull {
            verts: v,
            normals,
            count: n,
            radius,
        })
    }

    /// An axis-aligned box of half extents `half` centred at `center`, turned by `rot`.
    #[must_use]
    pub fn boxed(center: DVec2, half: DVec2, rot: Rot2, radius: f64) -> Hull {
        let c = [
            DVec2::new(-half.x, -half.y),
            DVec2::new(half.x, -half.y),
            DVec2::new(half.x, half.y),
            DVec2::new(-half.x, half.y),
        ];
        let mut h = Hull {
            verts: [DVec2::ZERO; MAX_VERTS],
            normals: [DVec2::ZERO; MAX_VERTS],
            count: 4,
            radius,
        };
        let n = [
            DVec2::new(0.0, -1.0),
            DVec2::X,
            DVec2::Y,
            DVec2::new(-1.0, 0.0),
        ];
        for i in 0..4 {
            h.verts[i] = center + rot.apply(c[i]);
            h.normals[i] = rot.apply(n[i]);
        }
        h
    }

    /// This hull moved by `rot` then `t`.
    #[must_use]
    pub fn transformed(&self, rot: Rot2, t: DVec2) -> Hull {
        let mut h = *self;
        for i in 0..self.count {
            h.verts[i] = rot.apply(self.verts[i]) + t;
            h.normals[i] = rot.apply(self.normals[i]);
        }
        h
    }

    /// The box around the hull, radius included.
    #[must_use]
    pub fn aabb(&self) -> Aabb2 {
        Aabb2::around(&self.verts[..self.count]).inflate(self.radius)
    }

    /// Area, centroid and second moment about the centroid, for `density`.
    #[must_use]
    pub fn mass(&self, density: f64) -> (f64, DVec2, f64) {
        let r = self.radius;
        let pi = core::f64::consts::PI;
        match self.count {
            1 => {
                let m = density * pi * r * r;
                (m, self.verts[0], m * 0.5 * r * r)
            }
            2 => {
                // Capsule: a rectangle and two half discs.
                let (a, b) = (self.verts[0], self.verts[1]);
                let l = (b - a).length();
                let rect = density * l * 2.0 * r;
                let disc = density * pi * r * r;
                let c = (a + b) * 0.5;
                let i_rect = rect * (l * l + 4.0 * r * r) / 12.0;
                // Two half discs = one disc, each half's centroid 4r/(3 pi) beyond the end.
                let d = l * 0.5 + 4.0 * r / (3.0 * pi);
                let i_disc = disc * (0.5 * r * r)
                    + disc * (d * d - (4.0 * r / (3.0 * pi)) * (4.0 * r / (3.0 * pi)));
                (rect + disc, c, i_rect + i_disc)
            }
            n => {
                // Polygon (rounded corners approximated by pushing each vertex out along its
                // bisector, as Box2D v2 does).
                let mut v = [DVec2::ZERO; MAX_VERTS];
                for (i, vi) in v.iter_mut().enumerate().take(n) {
                    if r > 0.0 {
                        let n0 = self.normals[(i + n - 1) % n];
                        let n1 = self.normals[i];
                        let mid = (n0 + n1).try_normalize().unwrap_or(n1);
                        let k = n1.dot(mid);
                        *vi = self.verts[i] + mid * (r / if k > 1e-6 { k } else { 1.0 });
                    } else {
                        *vi = self.verts[i];
                    }
                }
                let origin = v[0];
                let mut area = 0.0;
                let mut c = DVec2::ZERO;
                let mut inertia = 0.0;
                for i in 1..n - 1 {
                    let e1 = v[i] - origin;
                    let e2 = v[i + 1] - origin;
                    let d = e1.cross(e2);
                    let a = 0.5 * d;
                    area += a;
                    c += (e1 + e2) * (a / 3.0);
                    let intx2 = e1.x * e1.x + e2.x * e1.x + e2.x * e2.x;
                    let inty2 = e1.y * e1.y + e2.y * e1.y + e2.y * e2.y;
                    inertia += (0.25 / 3.0) * d * (intx2 + inty2);
                }
                let m = density * area;
                let cl = c / area;
                let centroid = origin + cl;
                // Inertia about `origin`, shifted to the centroid.
                let i_c = density * inertia - m * cl.dot(cl);
                (m, centroid, i_c)
            }
        }
    }
}

/// One contact point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManifoldPoint {
    /// World point midway between the two surfaces.
    pub point: DVec2,
    /// Negative when penetrating.
    pub separation: f64,
    /// Feature id (for warm starting).
    pub id: u32,
}

/// Up to two contact points with one normal, pointing from A to B.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Manifold {
    pub normal: DVec2,
    pub points: [ManifoldPoint; 2],
    pub count: usize,
}

impl Manifold {
    const EMPTY_POINT: ManifoldPoint = ManifoldPoint {
        point: DVec2::ZERO,
        separation: 0.0,
        id: 0,
    };

    fn one(normal: DVec2, p: ManifoldPoint) -> Manifold {
        Manifold {
            normal,
            points: [p, Self::EMPTY_POINT],
            count: 1,
        }
    }
}

/// Closest points of segments `p1 q1` and `p2 q2` (Ericson, Real-Time Collision Detection
/// 5.1.9): `(s, t, c1, c2)`.
fn closest_segments(p1: DVec2, q1: DVec2, p2: DVec2, q2: DVec2) -> (f64, f64, DVec2, DVec2) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let eps = 1e-24;
    let (s, t);
    if a <= eps && e <= eps {
        return (0.0, 0.0, p1, p2);
    }
    if a <= eps {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= eps {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut ss = if denom > eps {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut tt = (b * ss + f) / e;
            if tt < 0.0 {
                tt = 0.0;
                ss = (-c / a).clamp(0.0, 1.0);
            } else if tt > 1.0 {
                tt = 1.0;
                ss = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = ss;
            t = tt;
        }
    }
    (s, t, p1 + d1 * s, p2 + d2 * t)
}

/// Max over A's faces of the min separation of B's vertices: `(separation, edge)`.
fn max_separation(a: &Hull, b: &Hull) -> (f64, usize) {
    let mut best = f64::NEG_INFINITY;
    let mut edge = 0;
    for i in 0..a.count {
        let n = a.normals[i];
        let v = a.verts[i];
        let mut si = f64::INFINITY;
        for j in 0..b.count {
            let s = n.dot(b.verts[j] - v);
            if s < si {
                si = s;
            }
        }
        if si > best {
            best = si;
            edge = i;
        }
    }
    (best, edge)
}

/// Clip the incident edge of `inc` against reference face `edge` of `rf` (normal pointing
/// toward `inc`): up to two points `(core point on inc, separation of cores, id)`.
fn clip_face(rf: &Hull, edge: usize, inc: &Hull, flip: bool, margin: f64, out: &mut Manifold) {
    let nr = rf.normals[edge];
    let v1 = rf.verts[edge];
    let v2 = rf.verts[(edge + 1) % rf.count];
    // Incident edge: the most anti-parallel face of inc.
    let mut ie = 0;
    let mut md = f64::INFINITY;
    for i in 0..inc.count {
        let d = nr.dot(inc.normals[i]);
        if d < md {
            md = d;
            ie = i;
        }
    }
    let i1 = inc.verts[ie];
    let i2 = inc.verts[(ie + 1) % inc.count];
    let tangent = match (v2 - v1).try_normalize() {
        Some(t) => t,
        None => return,
    };
    // Clip [i1, i2] to the slab lo <= dot(t, p) <= hi.
    let lo = tangent.dot(v1);
    let hi = tangent.dot(v2);
    let mut pts = [(i1, ie as u32), (i2, ((ie + 1) % inc.count) as u32)];
    let (d1, d2) = (tangent.dot(pts[0].0), tangent.dot(pts[1].0));
    let clamp = |p: DVec2, dp: DVec2, dq: f64, dd: f64, bound: f64| {
        let t = (bound - dq) / (dd - dq);
        p + (dp - p) * t
    };
    // Lower bound.
    if d1 < lo && d2 < lo {
        return;
    }
    if d1 < lo {
        pts[0].0 = clamp(pts[0].0, pts[1].0, d1, d2, lo);
    } else if d2 < lo {
        pts[1].0 = clamp(pts[1].0, pts[0].0, d2, d1, lo);
    }
    let (d1, d2) = (tangent.dot(pts[0].0), tangent.dot(pts[1].0));
    if d1 > hi && d2 > hi {
        return;
    }
    if d1 > hi {
        pts[0].0 = clamp(pts[0].0, pts[1].0, d1, d2, hi);
    } else if d2 > hi {
        pts[1].0 = clamp(pts[1].0, pts[0].0, d2, d1, hi);
    }
    let (rr, ri) = (rf.radius, inc.radius);
    out.normal = if flip { -nr } else { nr };
    out.count = 0;
    for (p, vid) in pts {
        let sep_core = nr.dot(p - v1);
        let sep = sep_core - rr - ri;
        if sep > margin {
            continue;
        }
        // Midway between the incident surface (p - ri nr) and the reference surface.
        let on_inc = p - nr * ri;
        let on_ref = p - nr * (sep_core - rr);
        let id = ((flip as u32) << 16) | ((edge as u32) << 8) | vid;
        out.points[out.count] = ManifoldPoint {
            point: (on_inc + on_ref) * 0.5,
            separation: sep,
            id,
        };
        out.count += 1;
    }
}

/// The manifold between `a` and `b` (world space), keeping points separated by at most
/// `margin` (the speculative distance). `None`: farther apart than that.
#[must_use]
pub fn collide(a: &Hull, b: &Hull, margin: f64) -> Option<Manifold> {
    let total = a.radius + b.radius;
    // Circle vs circle.
    if a.count == 1 && b.count == 1 {
        let d = b.verts[0] - a.verts[0];
        let dist = d.length();
        let sep = dist - total;
        if sep > margin {
            return None;
        }
        let n = d.try_normalize().unwrap_or(DVec2::Y);
        let pa = a.verts[0] + n * a.radius;
        let pb = b.verts[0] - n * b.radius;
        return Some(Manifold::one(
            n,
            ManifoldPoint {
                point: (pa + pb) * 0.5,
                separation: sep,
                id: 0,
            },
        ));
    }
    // A point core (circle) against a polygon core.
    if a.count == 1 || b.count == 1 {
        let (poly, circ, flip) = if b.count == 1 {
            (a, b, false)
        } else {
            (b, a, true)
        };
        let m = poly_circle(poly, circ, margin)?;
        return Some(if flip {
            Manifold {
                normal: -m.normal,
                ..m
            }
        } else {
            m
        });
    }
    let (sa, ea) = max_separation(a, b);
    let (sb, eb) = max_separation(b, a);
    if sa > total + margin || sb > total + margin {
        return None;
    }
    let slop = 0.1 * 0.005;
    let mut m = Manifold {
        normal: DVec2::ZERO,
        points: [Manifold::EMPTY_POINT; 2],
        count: 0,
    };
    if sa.max(sb) > slop {
        // Cores apart: closest features.
        let mut best = (f64::INFINITY, DVec2::ZERO, DVec2::ZERO, 0usize, 0usize);
        for i in 0..a.count {
            let (a1, a2) = (a.verts[i], a.verts[(i + 1) % a.count]);
            for j in 0..b.count {
                let (b1, b2) = (b.verts[j], b.verts[(j + 1) % b.count]);
                let (_, _, ca, cb) = closest_segments(a1, a2, b1, b2);
                let d2 = (cb - ca).length_squared();
                if d2 < best.0 {
                    best = (d2, ca, cb, i, j);
                }
            }
        }
        let dist = det::sqrt(best.0);
        let sep = dist - total;
        if sep > margin {
            return None;
        }
        let n = (best.2 - best.1).try_normalize().unwrap_or(if sa > sb {
            a.normals[ea]
        } else {
            -b.normals[eb]
        });
        // Nearly parallel faces: clip for two points.
        let fa = a.support_face(n);
        let fb = b.support_face(-n);
        let (ca, cb) = (a.normals[fa].dot(n), b.normals[fb].dot(-n));
        let par = 0.999;
        if ca >= par || cb >= par {
            if ca >= cb {
                clip_face(a, fa, b, false, margin, &mut m);
            } else {
                clip_face(b, fb, a, true, margin, &mut m);
            }
            if m.count > 0 {
                return Some(m);
            }
        }
        let pa = best.1 + n * a.radius;
        let pb = best.2 - n * b.radius;
        let id = ((best.3 as u32) << 8) | best.4 as u32 | 0x8000_0000;
        return Some(Manifold::one(
            n,
            ManifoldPoint {
                point: (pa + pb) * 0.5,
                separation: sep,
                id,
            },
        ));
    }
    // Cores overlap: reference face with the larger separation (prefer A).
    if sb > sa + slop {
        clip_face(b, eb, a, true, margin, &mut m);
    } else {
        clip_face(a, ea, b, false, margin, &mut m);
    }
    (m.count > 0).then_some(m)
}

impl Hull {
    /// The face whose normal best matches `d`.
    fn support_face(&self, d: DVec2) -> usize {
        let mut best = 0;
        let mut bd = f64::NEG_INFINITY;
        for i in 0..self.count {
            let x = self.normals[i].dot(d);
            if x > bd {
                bd = x;
                best = i;
            }
        }
        best
    }
}

/// A polygon (count >= 2) against a circle (count 1): normal from polygon to circle.
fn poly_circle(p: &Hull, c: &Hull, margin: f64) -> Option<Manifold> {
    let center = c.verts[0];
    let total = p.radius + c.radius;
    let mut sep = f64::NEG_INFINITY;
    let mut edge = 0;
    for i in 0..p.count {
        let s = p.normals[i].dot(center - p.verts[i]);
        if s > sep {
            sep = s;
            edge = i;
        }
    }
    if sep > total + margin {
        return None;
    }
    let v1 = p.verts[edge];
    let v2 = p.verts[(edge + 1) % p.count];
    let mk = |n: DVec2, core_dist: f64, id: u32| {
        let s = core_dist - total;
        if s > margin {
            return None;
        }
        let on_c = center - n * c.radius;
        let on_p = center - n * (core_dist - p.radius);
        Some(Manifold::one(
            n,
            ManifoldPoint {
                point: (on_c + on_p) * 0.5,
                separation: s,
                id,
            },
        ))
    };
    if sep < 1e-12 {
        // Centre inside the core: push out along the face.
        return mk(p.normals[edge], sep, edge as u32);
    }
    let u1 = (center - v1).dot(v2 - v1);
    let u2 = (center - v2).dot(v1 - v2);
    if u1 <= 0.0 {
        let d = center - v1;
        let n = d.try_normalize()?;
        mk(n, d.length(), 0x100 | edge as u32)
    } else if u2 <= 0.0 {
        let d = center - v2;
        let n = d.try_normalize()?;
        mk(n, d.length(), 0x100 | ((edge + 1) % p.count) as u32)
    } else {
        mk(p.normals[edge], sep, edge as u32)
    }
}

/// A ray hit: distance along the (unit) direction and the surface normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub t: f64,
    pub normal: DVec2,
}

fn ray_circle(o: DVec2, d: DVec2, c: DVec2, r: f64, max_t: f64) -> Option<RayHit> {
    let m = o - c;
    let b = m.dot(d);
    let cc = m.dot(m) - r * r;
    if cc > 0.0 && b > 0.0 {
        return None;
    }
    let disc = b * b - cc;
    if disc < 0.0 {
        return None;
    }
    let t = -b - det::sqrt(disc);
    if t < 0.0 || t > max_t {
        return None;
    }
    let n = (o + d * t - c).try_normalize()?;
    Some(RayHit { t, normal: n })
}

fn ray_segment(o: DVec2, d: DVec2, a: DVec2, b: DVec2, max_t: f64) -> Option<RayHit> {
    let e = b - a;
    let den = d.cross(e);
    if den.abs() < 1e-18 {
        return None;
    }
    let t = (a - o).cross(e) / den;
    let u = (a - o).cross(d) / den;
    if !(0.0..=max_t).contains(&t) || !(0.0..=1.0).contains(&u) {
        return None;
    }
    let mut n = e.perp().try_normalize()?;
    if n.dot(d) > 0.0 {
        n = -n;
    }
    Some(RayHit { t, normal: n })
}

/// Cast a ray from `o` along unit `d` against `h`, up to `max_t`. A ray starting inside a
/// solid hull does not hit it.
#[must_use]
pub fn raycast(h: &Hull, o: DVec2, d: DVec2, max_t: f64) -> Option<RayHit> {
    if h.count == 1 {
        return ray_circle(o, d, h.verts[0], h.radius, max_t);
    }
    if h.radius == 0.0 && h.count >= 3 {
        // Slab clipping against every face.
        let (mut lower, mut upper) = (0.0, max_t);
        let mut index = None;
        for i in 0..h.count {
            let num = h.normals[i].dot(h.verts[i] - o);
            let den = h.normals[i].dot(d);
            if den == 0.0 {
                if num < 0.0 {
                    return None;
                }
            } else if den < 0.0 && num < lower * den {
                lower = num / den;
                index = Some(i);
            } else if den > 0.0 && num < upper * den {
                upper = num / den;
            }
            if upper < lower {
                return None;
            }
        }
        return index.map(|i| RayHit {
            t: lower,
            normal: h.normals[i],
        });
    }
    // Rounded hull or a bare segment: offset faces and corner circles.
    let mut best: Option<RayHit> = None;
    let mut take = |hit: Option<RayHit>| {
        if let Some(x) = hit
            && best.is_none_or(|b| x.t < b.t)
        {
            best = Some(x);
        }
    };
    let faces = if h.count == 2 { 2 } else { h.count };
    for i in 0..faces {
        let n = h.normals[i];
        let a = h.verts[i] + n * h.radius;
        let b = h.verts[(i + 1) % h.count] + n * h.radius;
        if h.radius > 0.0 && n.dot(d) >= 0.0 {
            continue;
        }
        take(ray_segment(o, d, a, b, max_t));
    }
    if h.radius > 0.0 {
        for i in 0..h.count {
            take(ray_circle(o, d, h.verts[i], h.radius, max_t));
        }
    }
    best
}

/// Is `p` inside `h` (radius included)?
#[must_use]
pub fn contains(h: &Hull, p: DVec2) -> bool {
    if h.count == 1 {
        return (p - h.verts[0]).length_squared() <= h.radius * h.radius;
    }
    if h.count >= 3 && (0..h.count).all(|i| h.normals[i].dot(p - h.verts[i]) <= 0.0) {
        return true;
    }
    // Outside the core: within the radius of it?
    let mut d2 = f64::INFINITY;
    for i in 0..h.count {
        let (_, _, c, _) = closest_segments(h.verts[i], h.verts[(i + 1) % h.count], p, p);
        d2 = d2.min((p - c).length_squared());
    }
    d2 <= h.radius * h.radius
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(x: f64, y: f64, hw: f64, hh: f64) -> Hull {
        Hull::boxed(DVec2::new(x, y), DVec2::new(hw, hh), Rot2::IDENTITY, 0.0)
    }

    #[test]
    fn box_resting_on_box_has_two_points_and_upward_normal() {
        let ground = bx(0.0, -0.5, 5.0, 0.5);
        let b = bx(0.0, 0.49, 0.5, 0.5);
        let m = collide(&ground, &b, 0.02).expect("touching");
        assert_eq!(m.count, 2);
        assert!((m.normal - DVec2::Y).length() < 1e-12, "{:?}", m.normal);
        for p in &m.points[..2] {
            assert!((p.separation + 0.01).abs() < 1e-12, "{}", p.separation);
        }
    }

    #[test]
    fn separated_beyond_the_margin_is_none_within_is_speculative() {
        let a = bx(0.0, 0.0, 0.5, 0.5);
        assert!(collide(&a, &bx(1.2, 0.0, 0.5, 0.5), 0.1).is_none());
        let m = collide(&a, &bx(1.05, 0.0, 0.5, 0.5), 0.1).expect("speculative");
        assert!(m.points[0].separation > 0.0);
        assert!((m.normal - DVec2::X).length() < 1e-12);
    }

    #[test]
    fn circle_cases() {
        let c1 = Hull::circle(DVec2::ZERO, 1.0);
        let c2 = Hull::circle(DVec2::new(1.5, 0.0), 1.0);
        let m = collide(&c1, &c2, 0.0).expect("overlap");
        assert!((m.points[0].separation + 0.5).abs() < 1e-12);
        let g = bx(0.0, -1.0, 5.0, 1.0);
        let ball = Hull::circle(DVec2::new(0.0, 0.4), 0.5);
        let m = collide(&g, &ball, 0.0).expect("ball on ground");
        assert!((m.normal - DVec2::Y).length() < 1e-12);
        assert!((m.points[0].separation + 0.1).abs() < 1e-12);
        // Reversed order flips the normal.
        let m2 = collide(&ball, &g, 0.0).expect("reversed");
        assert!((m2.normal + DVec2::Y).length() < 1e-12);
        // Corner region.
        let corner = Hull::circle(DVec2::new(5.3, 0.3), 0.5);
        let m = collide(&g, &corner, 0.0).expect("corner");
        let n = DVec2::new(0.3, 0.3).try_normalize().expect("n");
        assert!((m.normal - n).length() < 1e-12);
    }

    #[test]
    fn capsule_lying_on_ground_gets_two_points() {
        let g = bx(0.0, -0.5, 5.0, 0.5);
        let cap =
            Hull::segment(DVec2::new(-1.0, 0.25), DVec2::new(1.0, 0.25), 0.25).expect("capsule");
        let m = collide(&g, &cap, 0.02).expect("touch");
        assert_eq!(m.count, 2, "{m:?}");
        assert!((m.normal - DVec2::Y).length() < 1e-9);
        // Standing capsule: one point.
        let up =
            Hull::segment(DVec2::new(0.0, 0.25), DVec2::new(0.0, 1.25), 0.25).expect("capsule");
        let m = collide(&g, &up, 0.02).expect("touch");
        assert_eq!(m.count, 1);
    }

    #[test]
    fn rays_hit_polygons_circles_and_capsules() {
        let b = bx(5.0, 0.0, 1.0, 1.0);
        let h = raycast(&b, DVec2::ZERO, DVec2::X, 100.0).expect("hit");
        assert!((h.t - 4.0).abs() < 1e-12 && (h.normal + DVec2::X).length() < 1e-12);
        assert!(raycast(&b, DVec2::ZERO, DVec2::Y, 100.0).is_none());
        let c = Hull::circle(DVec2::new(0.0, 5.0), 1.0);
        let h = raycast(&c, DVec2::ZERO, DVec2::Y, 100.0).expect("hit");
        assert!((h.t - 4.0).abs() < 1e-12);
        let cap = Hull::segment(DVec2::new(3.0, -1.0), DVec2::new(3.0, 1.0), 0.5).expect("cap");
        let h = raycast(&cap, DVec2::ZERO, DVec2::X, 100.0).expect("hit");
        assert!((h.t - 2.5).abs() < 1e-12, "{}", h.t);
        assert!(contains(&cap, DVec2::new(3.2, 1.2)));
        assert!(!contains(&cap, DVec2::new(3.6, 1.4)));
    }

    #[test]
    fn polygon_mass_matches_the_box_formula() {
        let b = bx(0.0, 0.0, 1.0, 0.5);
        let (m, c, i) = b.mass(2.0);
        assert!((m - 4.0).abs() < 1e-12);
        assert!(c.length() < 1e-12);
        assert!((i - 4.0 * (4.0 + 1.0) / 12.0).abs() < 1e-12, "{i}");
        assert!(
            Hull::polygon(
                &[
                    DVec2::ZERO,
                    DVec2::X,
                    DVec2::new(1.0, 1.0),
                    DVec2::new(0.9, 0.1)
                ],
                0.0
            )
            .is_none(),
            "non-convex"
        );
    }
}
