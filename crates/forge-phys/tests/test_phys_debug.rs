//! `test_phys_debug` — the physics debug drawing (the editor's overlay during Play), on every
//! backend (gate row `C-phys-debug-draw`).
//!
//! * Every collider kind is drawn on its collider: a sphere's lines on its surface, a turned
//!   and offset box's lines between its corners, a height field's through its samples, a
//!   triangle mesh's once per edge.
//! * A convex hull is drawn from the hull its backend built: an octahedron's lines join its
//!   six vertices only, and a point inside it never appears. The control: drawn from its
//!   vertices' bounds (what a backend without hull edges gets), the lines are the box around
//!   it, which the same check refuses.
//! * The lines follow the body: after a fall they are drawn where the body is.
//! * Each line says what it belongs to: static, kinematic, trigger, an awake or a sleeping
//!   dynamic body, a joint (a cross on each anchor, the line between them and the axis).
//! * Touching contacts are drawn where bodies touch (a cross and the normal): a box resting
//!   on the ground shows its contacts on the ground's top with vertical normals; the same box
//!   in the air shows none (the control).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use forge_frames::{DQuat, DVec3};
use forge_phys::debug::{self, DebugKind, DebugLine};
use forge_phys::{
    BodyDesc, BodyKind, ColliderDesc, JointDesc, JointKind, PhysicsSettings, PhysicsWorld, Shape,
};

fn lines(w: &PhysicsWorld) -> Vec<DebugLine> {
    let mut out = Vec::new();
    w.debug_lines(&mut out).unwrap();
    out
}

fn ends(ls: &[DebugLine]) -> impl Iterator<Item = DVec3> + '_ {
    ls.iter().flat_map(|l| [l.a, l.b])
}

fn no_gravity() -> PhysicsSettings {
    PhysicsSettings {
        gravity: DVec3::ZERO,
        ..PhysicsSettings::default()
    }
}

fn octahedron() -> Vec<DVec3> {
    vec![
        DVec3::X,
        DVec3::X * -1.0,
        DVec3::Y,
        DVec3::Y * -1.0,
        DVec3::Z,
        DVec3::Z * -1.0,
        // Inside: the hull drops it.
        DVec3::new(0.1, 0.1, 0.1),
    ]
}

/// Whether every end of `ls` is one of `pts` (offset by `at`).
fn only_at(ls: &[DebugLine], at: DVec3, pts: &[DVec3]) -> bool {
    ends(ls).all(|e| pts.iter().any(|p| (e - (at + *p)).length() < 1e-9))
}

fn every_collider_kind_is_drawn_on_its_collider(b: &str) {
    let mut w = world_with(b, no_gravity());
    let fixed = |w: &mut PhysicsWorld, x: f64, c: ColliderDesc| {
        body_with(w, BodyDesc::fixed(at(x, 0.0, 0.0)), c);
        lines(w)
    };
    // A sphere: every end on its surface, three great circles.
    let ls = fixed(&mut w, 0.0, ColliderDesc::new(sphere(0.7)));
    assert_eq!(ls.len(), 3 * debug::CIRCLE_SEGMENTS, "{b}");
    assert!(
        ends(&ls).all(|e| ((e - DVec3::ZERO).length() - 0.7).abs() < 1e-9),
        "{b}"
    );
    assert!(ls.iter().all(|l| l.kind == DebugKind::Static), "{b}");
    // A box offset and turned on its body: twelve edges between its eight corners.
    let rot = DQuat::from_axis_angle(DVec3::Y, 0.5);
    let half = DVec3::new(0.5, 0.25, 1.0);
    let n = ls.len();
    let ls = fixed(
        &mut w,
        10.0,
        ColliderDesc {
            offset: DVec3::new(0.0, 2.0, 0.0),
            rotation: rot,
            ..ColliderDesc::new(Shape::Cuboid { half_extents: half })
        },
    );
    let boxed = &ls[n..];
    assert_eq!(boxed.len(), 12, "{b}");
    let corners: Vec<DVec3> = (0..8u8)
        .map(|i| {
            let s = |bit: u8, v: f64| if i & bit != 0 { v } else { -v };
            rot.rotate(DVec3::new(s(1, half.x), s(2, half.y), s(4, half.z)))
        })
        .collect();
    assert!(
        only_at(boxed, DVec3::new(10.0, 2.0, 0.0), &corners),
        "{b}: {boxed:?}"
    );
    // A convex hull: the backend's hull edges (see the module docs).
    let n = ls.len();
    let ls = fixed(
        &mut w,
        20.0,
        ColliderDesc::new(Shape::ConvexHull {
            vertices: octahedron(),
        }),
    );
    let hull = &ls[n..];
    assert!(hull.len() >= 12, "{b}: {} hull lines", hull.len());
    assert!(
        only_at(hull, DVec3::new(20.0, 0.0, 0.0), &octahedron()[..6]),
        "{b}: {hull:?}"
    );
    // A triangle mesh: two triangles sharing an edge are five lines.
    let n = ls.len();
    let ls = fixed(
        &mut w,
        30.0,
        ColliderDesc::new(Shape::TriMesh {
            vertices: vec![DVec3::ZERO, DVec3::X, DVec3::Z, DVec3::new(1.0, 0.0, 1.0)],
            indices: vec![[0, 2, 1], [1, 2, 3]],
        }),
    );
    assert_eq!(ls.len() - n, 5, "{b}");
    // A 3 x 3 height field: 12 lines through its samples.
    let n = ls.len();
    let heights = vec![0.0, 1.0, 0.0, 1.0, 2.0, 1.0, 0.0, 1.0, 0.0];
    let ls = fixed(
        &mut w,
        40.0,
        ColliderDesc::new(Shape::HeightField {
            rows: 3,
            cols: 3,
            heights,
            size: DVec3::new(4.0, 0.5, 2.0),
        }),
    );
    let hf = &ls[n..];
    assert_eq!(hf.len(), 12, "{b}");
    assert!(
        ends(hf).any(|e| (e - DVec3::new(40.0, 1.0, 0.0)).length() < 1e-9),
        "{b}: the centre sample (height 2 x 0.5) is not drawn"
    );
    // Capsule, cylinder and cone: every end within the shape's bounds.
    for (x, s, r, h) in [
        (
            50.0,
            Shape::Capsule {
                half_height: 0.5,
                radius: 0.3,
            },
            0.3,
            0.8,
        ),
        (
            60.0,
            Shape::Cylinder {
                half_height: 0.5,
                radius: 0.3,
            },
            0.3,
            0.5,
        ),
        (
            70.0,
            Shape::Cone {
                half_height: 0.5,
                radius: 0.3,
            },
            0.3,
            0.5,
        ),
    ] {
        let n = lines(&w).len();
        let ls = fixed(&mut w, x, ColliderDesc::new(s));
        let drawn = &ls[n..];
        assert!(drawn.len() > 20, "{b}");
        for e in ends(drawn) {
            let p = e - DVec3::new(x, 0.0, 0.0);
            let radial = DVec3::new(p.x, 0.0, p.z).length();
            assert!(radial <= r + 1e-9 && p.y.abs() <= h + 1e-9, "{b}: {p:?}");
        }
    }
}

fn a_convex_hull_is_drawn_from_its_hull_and_the_bounds_control_is_caught(b: &str) {
    let mut w = world(b);
    body_with(
        &mut w,
        BodyDesc::fixed(at(0.0, 0.0, 0.0)),
        ColliderDesc::new(Shape::ConvexHull {
            vertices: octahedron(),
        }),
    );
    let ls = lines(&w);
    assert!(only_at(&ls, DVec3::ZERO, &octahedron()[..6]), "{b}");
    // The control: drawn from its bounds, the check refuses it.
    let mut bounds = Vec::new();
    debug::shape_wireframe(
        &Shape::ConvexHull {
            vertices: octahedron(),
        },
        &mut |a, b| {
            bounds.push(DebugLine {
                a,
                b,
                kind: DebugKind::Static,
            });
        },
    );
    assert_eq!(bounds.len(), 12);
    assert!(!only_at(&bounds, DVec3::ZERO, &octahedron()[..6]));
}

fn the_lines_follow_the_body(b: &str) {
    let mut w = world(b);
    let (s, _) = body(&mut w, at(0.0, 10.0, 0.0), sphere(0.5));
    run(&mut w, 30);
    let p = pos(&w, s);
    assert!(p.y < 9.0, "{b}: did not fall");
    let ls = lines(&w);
    assert!(
        ends(&ls).all(|e| ((e - p).length() - 0.5).abs() < 1e-9),
        "{b}: drawn away from the body at {p:?}"
    );
    assert!(ls.iter().all(|l| l.kind == DebugKind::Dynamic), "{b}");
}

fn each_line_says_what_it_belongs_to(b: &str) {
    let mut w = world(b);
    ground(&mut w);
    let (sleeper, _) = body(&mut w, at(0.0, 0.5, 0.0), cube(0.5));
    body_with(
        &mut w,
        BodyDesc::new(BodyKind::Kinematic, at(5.0, 3.0, 0.0)),
        ColliderDesc::new(cube(0.5)),
    );
    body_with(
        &mut w,
        BodyDesc::fixed(at(-5.0, 3.0, 0.0)),
        ColliderDesc::sensor(cube(0.5)),
    );
    let (hook, _) = body_with(
        &mut w,
        BodyDesc::fixed(at(10.0, 5.0, 0.0)),
        ColliderDesc::new(cube(0.1)),
    );
    let (swing, _) = body(&mut w, at(11.0, 5.0, 0.0), cube(0.2));
    let mut j = JointDesc::new(hook, swing, JointKind::Hinge { limits: None });
    j.axis = DVec3::Z;
    j.anchor_b = DVec3::new(-1.0, 0.0, 0.0);
    w.add_joint(&j).unwrap();
    let count = |ls: &[DebugLine], k: DebugKind| ls.iter().filter(|l| l.kind == k).count();
    let ls = lines(&w);
    // Ground and hook static; the kinematic; the trigger; the sleeper and the swing awake.
    assert_eq!(count(&ls, DebugKind::Static), 24, "{b}");
    assert_eq!(count(&ls, DebugKind::Kinematic), 12, "{b}");
    assert_eq!(count(&ls, DebugKind::Trigger), 12, "{b}");
    assert_eq!(count(&ls, DebugKind::Dynamic), 24, "{b}");
    // A cross on each anchor, the line between them and the axis: both anchors on the hook.
    let joint: Vec<_> = ls.iter().filter(|l| l.kind == DebugKind::Joint).collect();
    assert_eq!(joint.len(), 8, "{b}");
    let hook_at = DVec3::new(10.0, 5.0, 0.0);
    assert!(
        joint[..6]
            .iter()
            .all(|l| ((l.a + l.b) * 0.5 - hook_at).length() < 1e-9),
        "{b}: {joint:?}"
    );
    assert!(
        (joint[7].b - (hook_at + DVec3::Z * debug::AXIS_LENGTH)).length() < 1e-9,
        "{b}: the axis {:?}",
        joint[7]
    );
    run(&mut w, 300);
    assert!(w.is_sleeping(sleeper).unwrap(), "{b}");
    let ls = lines(&w);
    assert_eq!(count(&ls, DebugKind::Sleeping), 12, "{b}");
}

fn contacts_are_drawn_where_bodies_touch(b: &str) {
    let contacts = |height: f64| {
        let mut w = world(b);
        ground(&mut w);
        body(&mut w, at(0.0, height, 0.0), cube(0.5));
        run(&mut w, 60);
        let ls: Vec<DebugLine> = lines(&w)
            .into_iter()
            .filter(|l| l.kind == DebugKind::Contact)
            .collect();
        assert_eq!(ls.len() % 4, 0, "{b}: a cross and a normal per contact");
        // Every fourth line is a normal, drawn from the contact point.
        ls.chunks(4)
            .map(|c| (c[3].a, c[3].b - c[3].a))
            .collect::<Vec<_>>()
    };
    let resting = contacts(0.5);
    assert!(!resting.is_empty(), "{b}: a resting box drew no contact");
    for (p, n) in &resting {
        assert!(
            p.y.abs() < 0.05 && p.x.abs() < 0.55 && p.z.abs() < 0.55,
            "{b}: a contact off the box's footprint: {p:?}"
        );
        assert!(
            (n.length() - debug::NORMAL_LENGTH).abs() < 1e-9 && n.y.abs() > 0.95 * n.length(),
            "{b}: a contact normal not vertical: {n:?}"
        );
    }
    assert!(
        contacts(20.0).is_empty(),
        "{b}: the control drew contacts for a box in the air"
    );
}

macro_rules! both {
    ($($t:ident),* $(,)?) => {
        mod avian3d {
            $( #[test] fn $t() { super::$t(forge_phys::AVIAN) } )*
        }
        mod rapier3d {
            $( #[test] fn $t() { super::$t(forge_phys::RAPIER) } )*
        }
    };
}

both!(
    every_collider_kind_is_drawn_on_its_collider,
    a_convex_hull_is_drawn_from_its_hull_and_the_bounds_control_is_caught,
    the_lines_follow_the_body,
    each_line_says_what_it_belongs_to,
    contacts_are_drawn_where_bodies_touch,
);
