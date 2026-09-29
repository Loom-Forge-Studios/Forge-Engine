//! Debug drawing (the editor's physics overlay during Play, Ch.17): every collider's
//! wireframe and every joint's anchors as line segments in the world's frame, at the pose the
//! last step left (what Play shows). Built on demand from what the world recorded when the
//! collider or joint was added; the simulation never reads any of it.
//!
//! Primitives and height fields are drawn from their numbers; a convex hull from the edges
//! its backend computed (or, from a backend that gives none, its vertices' bounds); a
//! triangle mesh from its unique edges, found once when it is added.

use std::sync::Arc;

use forge_frames::{DQuat, DVec3};
use forge_num::det;

use crate::types::Shape;

/// What a debug line belongs to (the viewport picks its colour from it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DebugKind {
    /// A solid collider on an awake dynamic body.
    Dynamic,
    /// A solid collider on a dynamic body that sleeps.
    Sleeping,
    /// A solid collider on a kinematic body.
    Kinematic,
    /// A solid collider on a static body.
    Static,
    /// A trigger (sensor) collider, whatever its body.
    Trigger,
    /// A joint: a cross at each anchor, the line between them and the joint axis.
    Joint,
}

/// One segment of the debug drawing, in the world's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugLine {
    pub a: DVec3,
    pub b: DVec3,
    pub kind: DebugKind,
}

/// Segments per full circle.
pub const CIRCLE_SEGMENTS: usize = 24;

/// Half the size of the cross drawn at a joint anchor, and the drawn joint axis's length
/// (metres).
pub const ANCHOR_HALF: f64 = 0.05;
pub const AXIS_LENGTH: f64 = 0.3;

/// What the world keeps of a collider to draw it.
#[derive(Clone, Debug)]
pub(crate) enum Outline {
    /// A primitive or a height field, drawn from its numbers.
    Shape(Shape),
    /// Edges found once (a convex hull's, a triangle mesh's), in the shape's frame.
    Edges(Arc<[[DVec3; 2]]>),
}

#[derive(Clone, Debug)]
pub(crate) struct ColliderDraw {
    pub outline: Outline,
    pub offset: DVec3,
    pub rotation: DQuat,
    pub sensor: bool,
}

impl Outline {
    /// The outline of a collider of `shape`; `hull` is the backend's convex hull edges.
    pub fn of(shape: &Shape, hull: Option<Vec<[DVec3; 2]>>) -> Self {
        match shape {
            Shape::ConvexHull { vertices } => Self::Edges(match hull {
                Some(e) => e.into(),
                None => bounds_edges(vertices).into(),
            }),
            Shape::TriMesh { vertices, indices } => {
                Self::Edges(mesh_edges(vertices, indices).into())
            }
            s => Self::Shape(s.clone()),
        }
    }

    pub fn draw(&self, emit: &mut impl FnMut(DVec3, DVec3)) {
        match self {
            Self::Shape(s) => shape_wireframe(s, emit),
            Self::Edges(e) => {
                for [a, b] in e.iter() {
                    emit(*a, *b);
                }
            }
        }
    }
}

/// The wireframe of `shape` in its own frame, one `emit(a, b)` per segment (of a shape
/// [`Shape::check`] accepts; any other draws something, never panics). A convex hull is drawn
/// as its vertices' bounds here (the world draws the backend's hull edges instead).
pub fn shape_wireframe(shape: &Shape, emit: &mut impl FnMut(DVec3, DVec3)) {
    let (x, y, z) = (DVec3::X, DVec3::Y, DVec3::Z);
    let n = CIRCLE_SEGMENTS;
    match shape {
        Shape::Sphere { radius } => {
            for (u, v) in [(x, y), (y, z), (z, x)] {
                arc(DVec3::ZERO, u, v, *radius, n, emit);
            }
        }
        Shape::Cuboid { half_extents } => boxed(DVec3::ZERO, *half_extents, emit),
        Shape::Capsule {
            half_height,
            radius,
        } => {
            let (h, r) = (*half_height, *radius);
            barrel(h, r, r, emit);
            for (c, up) in [(y * h, y), (y * -h, y * -1.0)] {
                arc(c, x, up, r, n / 2, emit);
                arc(c, z, up, r, n / 2, emit);
            }
        }
        Shape::Cylinder {
            half_height,
            radius,
        } => barrel(*half_height, *radius, *radius, emit),
        Shape::Cone {
            half_height,
            radius,
        } => barrel(*half_height, *radius, 0.0, emit),
        Shape::ConvexHull { vertices } => {
            for [a, b] in bounds_edges(vertices) {
                emit(a, b);
            }
        }
        Shape::TriMesh { vertices, indices } => {
            for [a, b] in mesh_edges(vertices, indices) {
                emit(a, b);
            }
        }
        Shape::HeightField {
            rows,
            cols,
            heights,
            size,
        } => {
            let (r, c) = (*rows as usize, *cols as usize);
            let at = |i: usize, j: usize| {
                let u = |k: usize, m: usize| k as f64 / (m - 1) as f64 - 0.5;
                DVec3::new(
                    u(j, c) * size.x,
                    heights.get(i * c + j).copied().unwrap_or(0.0) * size.y,
                    u(i, r) * size.z,
                )
            };
            for i in 0..r {
                for j in 0..c {
                    if j + 1 < c {
                        emit(at(i, j), at(i, j + 1));
                    }
                    if i + 1 < r {
                        emit(at(i, j), at(i + 1, j));
                    }
                }
            }
        }
    }
}

/// `segments` of a circle about `c` of `radius` in the plane of unit `u` and `v`, starting
/// at `u` and turning toward `v` (a full circle is [`CIRCLE_SEGMENTS`]).
fn arc(
    c: DVec3,
    u: DVec3,
    v: DVec3,
    radius: f64,
    segments: usize,
    emit: &mut impl FnMut(DVec3, DVec3),
) {
    let at = |i: usize| {
        let a = std::f64::consts::TAU * i as f64 / CIRCLE_SEGMENTS as f64;
        c + (u * det::cos(a) + v * det::sin(a)) * radius
    };
    for i in 0..segments {
        emit(at(i), at(i + 1));
    }
}

/// A y-axis barrel: a ring of `bottom` radius at `-h`, one of `top` radius at `+h` (a point
/// for a cone's apex), and four lines joining them.
fn barrel(h: f64, bottom: f64, top: f64, emit: &mut impl FnMut(DVec3, DVec3)) {
    let (x, y, z) = (DVec3::X, DVec3::Y, DVec3::Z);
    for (r, c) in [(bottom, y * -h), (top, y * h)] {
        if r > 0.0 {
            arc(c, x, z, r, CIRCLE_SEGMENTS, emit);
        }
    }
    for d in [x, x * -1.0, z, z * -1.0] {
        emit(y * -h + d * bottom, y * h + d * top);
    }
}

/// The twelve edges of the box of `half` extents about `c`.
fn boxed(c: DVec3, half: DVec3, emit: &mut impl FnMut(DVec3, DVec3)) {
    let corner = |i: u8| {
        let s = |bit: u8, v: f64| if i & bit != 0 { v } else { -v };
        c + DVec3::new(s(1, half.x), s(2, half.y), s(4, half.z))
    };
    for (i, j) in [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        emit(corner(i), corner(j));
    }
}

/// The edges of the box bounding `vertices`.
fn bounds_edges(vertices: &[DVec3]) -> Vec<[DVec3; 2]> {
    let Some(first) = vertices.first() else {
        return Vec::new();
    };
    let (mut lo, mut hi) = (*first, *first);
    for p in vertices {
        lo = DVec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = DVec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    let mut out = Vec::with_capacity(12);
    boxed((lo + hi) * 0.5, (hi - lo) * 0.5, &mut |a, b| {
        out.push([a, b])
    });
    out
}

/// A triangle mesh's edges, each once.
fn mesh_edges(vertices: &[DVec3], indices: &[[u32; 3]]) -> Vec<[DVec3; 2]> {
    let mut pairs: Vec<(u32, u32)> = indices
        .iter()
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .map(|(a, b)| (a.min(b), a.max(b)))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    pairs
        .into_iter()
        .filter_map(|(a, b)| Some([*vertices.get(a as usize)?, *vertices.get(b as usize)?]))
        .collect()
}

/// A joint's drawing: a cross at each anchor (world-frame points `pa`, `pb`), the line
/// between them, and the joint axis (unit `axis`) from `pa`.
pub(crate) fn joint_lines(pa: DVec3, pb: DVec3, axis: DVec3, out: &mut Vec<DebugLine>) {
    let mut seg = |a: DVec3, b: DVec3| {
        out.push(DebugLine {
            a,
            b,
            kind: DebugKind::Joint,
        });
    };
    for p in [pa, pb] {
        for d in [DVec3::X, DVec3::Y, DVec3::Z] {
            seg(p - d * ANCHOR_HALF, p + d * ANCHOR_HALF);
        }
    }
    seg(pa, pb);
    seg(pa, pa + axis * AXIS_LENGTH);
}
