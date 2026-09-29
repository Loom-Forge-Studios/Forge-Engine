//! `test_phys_behaviour` — **one behaviour suite, every backend** (M4-3, M7-9; gate
//! `C-phys-behaviour`).
//!
//! Each scenario below is a plain function of the backend id; the `both!` macro at the end
//! runs every one on `avian3d` and on `rapier3d`. A project that switches `physics.backend`
//! keeps every behaviour asserted here: gravity, resting contact on every collider kind
//! (primitives, a convex hull generated from mesh vertices, a triangle mesh, a height field),
//! materials, collision layers, triggers and contact events, CCD, axis locks, kinematic
//! platforms, sleeping, the region-local frame and `f64` far from the origin.
//!
//! The bounds are behavioural (a body rests within 3 cm of where it should; a bounce reaches
//! at least half its drop): the two solvers differ in their last digits, and the assertion is
//! about what a game sees. Positive controls (W2) sit beside the property they guard: each
//! scenario runs the broken configuration too (no restitution, no friction, disjoint layers,
//! a solid instead of a trigger, continuous collision off, unlocked axes) and asserts it
//! fails the same check.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use forge_frames::{DQuat, DVec3, FrameId, FramePos};
use forge_phys::{
    AxisLocks, BodyDesc, BodyKind, BodyState, ColliderDesc, Layers, Material, PhysError, PhysEvent,
    PhysicsSettings, PhysicsWorld, QueryFilter, Ray, Shape,
};

/// One second of free fall drops `g/2` metres. Both backends integrate semi-implicitly per
/// solver substep (rapier 4, avian 6 by default), which converges on the exact `g t²/2`:
/// within 2.5 cm after 60 steps, and the speed exact.
fn free_fall_matches_the_fixed_step_integrator(b: &str) {
    let mut w = world(b);
    let (s, _) = body(&mut w, at(0.0, 100.0, 0.0), sphere(0.5));
    run(&mut w, 60);
    let expect = 100.0 - 0.5 * 9.81;
    let y = pos(&w, s).y;
    assert!(near(y, expect, 0.025), "{b}: y = {y}, expected {expect}");
    assert!(near(vel(&w, s).y, -9.81, 0.01), "{b}: {:?}", vel(&w, s));
    assert_eq!(w.steps(), 60);
}

fn a_sphere_comes_to_rest_on_a_box(b: &str) {
    let mut w = world(b);
    ground(&mut w);
    let (s, _) = body(&mut w, at(0.0, 2.0, 0.0), sphere(0.5));
    run(&mut w, 240);
    let p = pos(&w, s);
    assert!(near(p.y, 0.5, 0.03), "{b}: rests at {p:?}");
    assert!(
        vel(&w, s).length() < 0.05,
        "{b}: still moving {:?}",
        vel(&w, s)
    );
    assert!(w.contact_count() >= 1, "{b}: no contact reported");
}

/// Every collider kind a dynamic body can carry comes to rest on the ground at its own
/// half height — a convex hull generated from a cube mesh with interior points included.
fn every_collider_kind_rests_on_the_ground(b: &str) {
    let hull: Vec<DVec3> = [-0.5, 0.5]
        .iter()
        .flat_map(|&x| {
            [-0.5, 0.5]
                .iter()
                .flat_map(move |&y| [-0.5, 0.5].iter().map(move |&z| DVec3::new(x, y, z)))
        })
        .chain([DVec3::ZERO, DVec3::new(0.1, -0.2, 0.3)])
        .collect();
    let cases: Vec<(&str, Shape, f64)> = vec![
        ("sphere", sphere(0.4), 0.4),
        ("cuboid", cube(0.3), 0.3),
        (
            "capsule",
            Shape::Capsule {
                half_height: 0.3,
                radius: 0.2,
            },
            0.5,
        ),
        (
            "cylinder",
            Shape::Cylinder {
                half_height: 0.25,
                radius: 0.4,
            },
            0.25,
        ),
        (
            "cone",
            Shape::Cone {
                half_height: 0.3,
                radius: 0.5,
            },
            0.3,
        ),
        ("convex hull", Shape::ConvexHull { vertices: hull }, 0.5),
    ];
    for (name, shape, rest) in cases {
        let mut w = world(b);
        ground(&mut w);
        let (id, _) = body_with(
            &mut w,
            BodyDesc {
                locks: AxisLocks::ROTATION,
                ..BodyDesc::dynamic(at(0.0, rest + 0.5, 0.0))
            },
            ColliderDesc::new(shape),
        );
        run(&mut w, 240);
        let y = pos(&w, id).y;
        assert!(
            near(y, rest, 0.03),
            "{b}: a {name} rests at {y}, not {rest}"
        );
    }
}

/// Static level geometry: a triangle mesh floor and a height field both hold a sphere; the
/// height field's rows run along z and its columns along x (the documented layout).
fn a_sphere_rests_on_a_triangle_mesh_and_a_height_field(b: &str) {
    let mut w = world(b);
    let floor = w.add_body(&BodyDesc::fixed(at(0.0, 0.0, 0.0))).unwrap();
    let v = vec![
        DVec3::new(-10.0, 0.0, -10.0),
        DVec3::new(10.0, 0.0, -10.0),
        DVec3::new(10.0, 0.0, 10.0),
        DVec3::new(-10.0, 0.0, 10.0),
    ];
    w.add_collider(
        floor,
        &ColliderDesc::new(Shape::TriMesh {
            vertices: v,
            indices: vec![[0, 2, 1], [0, 3, 2]],
        }),
    )
    .unwrap();
    let (s, _) = body(&mut w, at(0.0, 1.0, 0.0), sphere(0.5));
    run(&mut w, 180);
    assert!(
        near(pos(&w, s).y, 0.5, 0.03),
        "{b}: on the mesh {:?}",
        pos(&w, s)
    );

    // A 5 x 9 height field whose height is its column index: rising along +x.
    let mut w = world(b);
    let (rows, cols) = (5u32, 9u32);
    let heights = (0..rows)
        .flat_map(|_| (0..cols).map(f64::from))
        .collect::<Vec<_>>();
    let hf = w.add_body(&BodyDesc::fixed(at(0.0, 0.0, 0.0))).unwrap();
    w.add_collider(
        hf,
        &ColliderDesc::new(Shape::HeightField {
            rows,
            cols,
            heights,
            size: DVec3::new(8.0, 0.25, 4.0),
        }),
    )
    .unwrap();
    // Column c is at x = -4 + c, height c * 0.25.
    for c in [1.0, 4.0, 7.0] {
        let x = -4.0 + c + 0.5;
        let h = w
            .raycast(
                &Ray {
                    start: at(x, 10.0, 0.3),
                    direction: DVec3::new(0.0, -1.0, 0.0),
                    max_distance: 20.0,
                },
                &QueryFilter::default(),
            )
            .unwrap()
            .expect("the ray hits the height field");
        let expect = (c + 0.5) * 0.25;
        assert!(
            near(h.point.local.y, expect, 1e-9),
            "{b}: height at x = {x} is {}, not {expect}",
            h.point.local.y
        );
    }
}

/// Restitution bounces a dropped ball back up; without it the ball stays down (the control).
/// Friction stops a sliding box; without it the box keeps sliding (the control).
fn restitution_bounces_and_friction_stops_a_slide(b: &str) {
    let bounce = |restitution: f64| {
        let mut w = world(b);
        let (_, g) = ground(&mut w);
        let _ = g;
        let mat = Material {
            restitution,
            restitution_combine: forge_phys::CombineRule::Max,
            ..Material::default()
        };
        let (s, _) = body_with(
            &mut w,
            BodyDesc::dynamic(at(0.0, 2.5, 0.0)),
            ColliderDesc {
                material: mat,
                ..ColliderDesc::new(sphere(0.5))
            },
        );
        // Fall (2 m to contact), then track the highest point after the first bounce.
        let mut touched = false;
        let mut peak: f64 = 0.0;
        for _ in 0..150 {
            w.step().unwrap();
            let y = pos(&w, s).y;
            if y < 0.6 {
                touched = true;
            }
            if touched {
                peak = peak.max(y);
            }
        }
        assert!(touched, "{b}: the ball never reached the ground");
        peak - 0.5
    };
    let high = bounce(0.9);
    let dead = bounce(0.0);
    assert!(
        high > 1.0,
        "{b}: restitution 0.9 bounced only {high} m of 2 m"
    );
    assert!(dead < 0.05, "{b}: restitution 0 bounced {dead} m");

    let slide = |friction: f64, rule: forge_phys::CombineRule| {
        let mut w = world(b);
        ground(&mut w);
        let (bx, _) = body_with(
            &mut w,
            BodyDesc {
                linear_velocity: DVec3::new(4.0, 0.0, 0.0),
                ..BodyDesc::dynamic(at(0.0, 0.25, 0.0))
            },
            ColliderDesc {
                material: Material {
                    friction,
                    friction_combine: rule,
                    ..Material::default()
                },
                ..ColliderDesc::new(cube(0.25))
            },
        );
        run(&mut w, 120);
        (pos(&w, bx).x, vel(&w, bx).x)
    };
    // The combine rule picks the box's coefficient over the ground's 0.5 either way.
    let (x_grip, v_grip) = slide(1.0, forge_phys::CombineRule::Max);
    let (x_ice, v_ice) = slide(0.0, forge_phys::CombineRule::Min);
    assert!(
        v_grip.abs() < 0.05,
        "{b}: friction 1 still slides at {v_grip}"
    );
    assert!(x_grip < 1.5, "{b}: friction 1 slid {x_grip} m");
    assert!(v_ice > 3.9, "{b}: friction 0 slowed to {v_ice}");
    assert!(x_ice > 7.5, "{b}: friction 0 slid only {x_ice} m");
}

/// A body on a layer the ground does not collide with falls through; on a shared layer it
/// rests (the control).
fn collision_layers_let_bodies_pass(b: &str) {
    let fall = |layers: Layers| {
        let mut w = world(b);
        ground(&mut w);
        let (s, _) = body_with(
            &mut w,
            BodyDesc::dynamic(at(0.0, 1.0, 0.0)),
            ColliderDesc {
                layers,
                ..ColliderDesc::new(sphere(0.5))
            },
        );
        run(&mut w, 120);
        pos(&w, s).y
    };
    let ghost = fall(Layers {
        memberships: 1 << 3,
        filters: 1 << 3,
    });
    let solid = fall(Layers::default());
    assert!(
        ghost < -2.0,
        "{b}: a body on another layer did not fall through: {ghost}"
    );
    assert!(near(solid, 0.5, 0.03), "{b}: the control rests at {solid}");
}

/// A trigger reports a body entering and leaving and never pushes it; the same shape as a
/// solid stops the body (the control). Solid contacts report begin (and, when the body is
/// lifted away, end).
fn triggers_report_enter_and_exit_and_never_push(b: &str) {
    let run_through = |sensor: bool| {
        let mut w = world(b);
        let zone = w.add_body(&BodyDesc::fixed(at(0.0, 0.0, 0.0))).unwrap();
        let t = w
            .add_collider(
                zone,
                &ColliderDesc {
                    sensor,
                    ..ColliderDesc::new(cube(1.0))
                },
            )
            .unwrap();
        let (s, c) = body(&mut w, at(0.0, 3.0, 0.0), sphere(0.25));
        let mut events = Vec::new();
        for _ in 0..90 {
            w.step().unwrap();
            events.extend(w.events().iter().copied());
        }
        (pos(&w, s).y, events, t, c)
    };
    let (y, events, t, c) = run_through(true);
    assert!(y < -3.0, "{b}: the trigger stopped the body at {y}");
    let enter = events.iter().position(|e| {
        *e == PhysEvent::TriggerEnter {
            trigger: t,
            other: c,
        }
    });
    let exit = events.iter().position(|e| {
        *e == PhysEvent::TriggerExit {
            trigger: t,
            other: c,
        }
    });
    assert!(
        matches!((enter, exit), (Some(i), Some(j)) if i < j),
        "{b}: enter then exit expected: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PhysEvent::ContactBegin(..))),
        "{b}: a trigger made a contact: {events:?}"
    );
    let (y, events, t, c) = run_through(false);
    assert!(
        near(y, 1.25, 0.03),
        "{b}: the control solid let the body through: {y}"
    );
    let (lo, hi) = if t < c { (t, c) } else { (c, t) };
    assert!(
        events.contains(&PhysEvent::ContactBegin(lo, hi)),
        "{b}: no contact begin: {events:?}"
    );
}

/// Contact end: a resting body lifted off the ground reports the contact ending.
fn contacts_end_when_bodies_part(b: &str) {
    let mut w = world(b);
    let (_, g) = ground(&mut w);
    let (s, c) = body(&mut w, at(0.0, 0.6, 0.0), sphere(0.5));
    let mut events = Vec::new();
    for _ in 0..60 {
        w.step().unwrap();
        events.extend(w.events().iter().copied());
    }
    w.set_velocity(s, DVec3::new(0.0, 10.0, 0.0), DVec3::ZERO)
        .unwrap();
    for _ in 0..30 {
        w.step().unwrap();
        events.extend(w.events().iter().copied());
    }
    let pair = (g.min(c), g.max(c));
    let begin = events
        .iter()
        .position(|e| *e == PhysEvent::ContactBegin(pair.0, pair.1));
    let end = events
        .iter()
        .position(|e| *e == PhysEvent::ContactEnd(pair.0, pair.1));
    assert!(
        matches!((begin, end), (Some(i), Some(j)) if i < j),
        "{b}: begin then end expected: {events:?}"
    );
}

/// A 1 cm bullet at 400 m/s (6.7 m per step): with continuous collision (the default) it
/// cannot pass a 2 cm static wall, and with `ccd` it cannot pass a 2 cm plank that is itself
/// moving; with continuous collision off (the control) it tunnels through the wall.
fn continuous_collision_stops_a_bullet(b: &str) {
    let shoot = |continuous: bool, plank: bool| {
        let mut w = world_with(
            b,
            PhysicsSettings {
                gravity: DVec3::ZERO,
                continuous,
                ..PhysicsSettings::default()
            },
        );
        let slab = forge_phys::Shape::Cuboid {
            half_extents: DVec3::new(0.01, 5.0, 5.0),
        };
        if plank {
            body_with(
                &mut w,
                BodyDesc {
                    linear_velocity: DVec3::new(0.0, 0.5, 0.0),
                    ..BodyDesc::dynamic(at(10.0, 0.0, 0.0))
                },
                ColliderDesc::new(slab),
            );
        } else {
            let wall = w.add_body(&BodyDesc::fixed(at(10.0, 0.0, 0.0))).unwrap();
            w.add_collider(wall, &ColliderDesc::new(slab)).unwrap();
        }
        let (s, _) = body_with(
            &mut w,
            BodyDesc {
                linear_velocity: DVec3::new(400.0, 0.0, 0.0),
                ccd: plank,
                ..BodyDesc::dynamic(at(0.0, 0.0, 0.0))
            },
            ColliderDesc::new(sphere(0.01)),
        );
        run(&mut w, 10);
        pos(&w, s).x
    };
    let wall = shoot(true, false);
    let plank = shoot(true, true);
    let tunnel = shoot(false, false);
    assert!(wall < 10.0, "{b}: through the wall: x = {wall}");
    assert!(plank < 11.0, "{b}: through the moving plank: x = {plank}");
    assert!(
        tunnel > 10.0,
        "{b}: the control (continuous off) did not tunnel: x = {tunnel}"
    );
}

/// A body with every rotation locked hit off-centre does not turn; unlocked, it does (the
/// control).
fn axis_locks_hold_rotation(b: &str) {
    let hit = |locks: AxisLocks| {
        let mut w = world_with(
            b,
            PhysicsSettings {
                gravity: DVec3::ZERO,
                ..PhysicsSettings::default()
            },
        );
        let (bx, _) = body_with(
            &mut w,
            BodyDesc {
                locks,
                ..BodyDesc::dynamic(at(0.0, 0.0, 0.0))
            },
            ColliderDesc::new(cube(0.5)),
        );
        let (_, _) = body_with(
            &mut w,
            BodyDesc {
                linear_velocity: DVec3::new(-5.0, 0.0, 0.0),
                ..BodyDesc::dynamic(at(2.0, 0.4, 0.0))
            },
            ColliderDesc::new(sphere(0.2)),
        );
        run(&mut w, 60);
        angle(w.rotation(bx).unwrap())
    };
    let locked = hit(AxisLocks::ROTATION);
    let free = hit(AxisLocks::NONE);
    assert!(
        locked < 1e-9,
        "{b}: a rotation-locked body turned {locked} rad"
    );
    assert!(free > 0.05, "{b}: the control did not turn: {free} rad");
}

/// A kinematic platform driven by targets moves exactly to them and carries a box resting on
/// it (the box inherits the platform's velocity through friction).
fn a_kinematic_platform_carries_what_rests_on_it(b: &str) {
    let mut w = world(b);
    let (p, _) = body_with(
        &mut w,
        BodyDesc::new(BodyKind::Kinematic, at(0.0, 0.0, 0.0)),
        ColliderDesc::new(Shape::Cuboid {
            half_extents: DVec3::new(3.0, 0.25, 3.0),
        }),
    );
    let (bx, _) = body_with(
        &mut w,
        BodyDesc::dynamic(at(0.0, 0.5, 0.0)),
        ColliderDesc {
            material: Material {
                friction: 1.0,
                ..Material::default()
            },
            ..ColliderDesc::new(cube(0.25))
        },
    );
    run(&mut w, 30);
    let x0 = pos(&w, bx).x;
    for i in 1..=90 {
        let x = f64::from(i) * 0.02;
        w.set_kinematic_target(p, at(x, 0.0, 0.0), DQuat::IDENTITY)
            .unwrap();
        w.step().unwrap();
        assert!(
            near(pos(&w, p).x, x, 1e-9),
            "{b}: the platform is at {:?}, its target {x}",
            pos(&w, p)
        );
    }
    let carried = pos(&w, bx).x - x0;
    assert!(
        near(carried, 1.8, 0.1),
        "{b}: the box was carried {carried} m of 1.8"
    );
    // A dynamic body is not kinematic: refused with a code, never a panic.
    let e = w
        .set_kinematic_target(bx, at(0.0, 0.0, 0.0), DQuat::IDENTITY)
        .unwrap_err();
    assert!(matches!(e, PhysError::Invalid(_)), "{e}");
}

/// A body left at rest falls asleep; one that may not sleep never does (the control).
fn resting_bodies_sleep(b: &str) {
    let settle = |can_sleep: bool| {
        let mut w = world(b);
        ground(&mut w);
        let (s, _) = body_with(
            &mut w,
            BodyDesc {
                can_sleep,
                ..BodyDesc::dynamic(at(0.0, 0.5, 0.0))
            },
            ColliderDesc::new(cube(0.5)),
        );
        run(&mut w, 300);
        w.is_sleeping(s).unwrap()
    };
    assert!(settle(true), "{b}: a resting box never slept");
    assert!(!settle(false), "{b}: a box that may not sleep slept");
}

/// f64 far from the frame origin (Ch.17: region-local frames keep numbers small, but a
/// region can still be large): the same drop 1,000 km out rests where it rests at the
/// origin, to the millimetre.
fn far_from_the_origin_is_as_exact_as_at_it(b: &str) {
    let drop = |o: f64| {
        let mut w = world(b);
        let g = w.add_body(&BodyDesc::fixed(at(o, -0.5, o))).unwrap();
        w.add_collider(
            g,
            &ColliderDesc::new(Shape::Cuboid {
                half_extents: DVec3::new(20.0, 0.5, 20.0),
            }),
        )
        .unwrap();
        let (s, _) = body(&mut w, at(o + 0.25, 2.0, o - 0.25), sphere(0.5));
        run(&mut w, 240);
        let p = pos(&w, s);
        DVec3::new(p.x - o, p.y, p.z - o)
    };
    let here = drop(0.0);
    let there = drop(1.0e6);
    assert!(
        (here - there).length() < 1e-3,
        "{b}: at the origin {here:?}, 1,000 km out {there:?}"
    );
}

/// A position in another frame is refused (`PHYS-0002`), so are bad shapes and numbers
/// (`PHYS-0001`), with codes and never a panic.
fn bad_inputs_are_refused_with_codes(b: &str) {
    let mut w = world(b);
    let e = w
        .add_body(&BodyDesc::dynamic(FramePos::new(FrameId(8), DVec3::ZERO)))
        .unwrap_err();
    assert!(matches!(e, PhysError::Frame(_)), "{e}");
    assert!(e.to_string().starts_with("PHYS-0002"));
    let s = w.add_body(&BodyDesc::dynamic(at(0.0, 0.0, 0.0))).unwrap();
    for bad in [
        sphere(0.0),
        sphere(f64::NAN),
        Shape::ConvexHull {
            vertices: vec![DVec3::ZERO; 3],
        },
        Shape::TriMesh {
            vertices: vec![DVec3::ZERO],
            indices: vec![[0, 1, 2]],
        },
        Shape::HeightField {
            rows: 2,
            cols: 2,
            heights: vec![0.0; 3],
            size: DVec3::splat(1.0),
        },
    ] {
        let e = w
            .add_collider(s, &ColliderDesc::new(bad.clone()))
            .unwrap_err();
        assert!(e.to_string().starts_with("PHYS-0001"), "{bad:?}: {e}");
    }
    let e = w
        .set_state(
            s,
            &BodyState {
                position: at(0.0, 0.0, 0.0),
                rotation: DQuat::IDENTITY,
                linear_velocity: DVec3::new(f64::INFINITY, 0.0, 0.0),
                angular_velocity: DVec3::ZERO,
            },
        )
        .unwrap_err();
    assert!(e.to_string().starts_with("PHYS-0001"), "{e}");
    assert!(
        PhysicsWorld::first_party("no-such-backend", F, PhysicsSettings::default())
            .unwrap_err()
            .to_string()
            .starts_with("PHYS-0003")
    );
    // A removed body is gone for good, with its colliders.
    w.remove_body(s).unwrap();
    assert!(w.state(s).is_err());
    assert_eq!(w.body_count(), 0);
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
    free_fall_matches_the_fixed_step_integrator,
    a_sphere_comes_to_rest_on_a_box,
    every_collider_kind_rests_on_the_ground,
    a_sphere_rests_on_a_triangle_mesh_and_a_height_field,
    restitution_bounces_and_friction_stops_a_slide,
    collision_layers_let_bodies_pass,
    triggers_report_enter_and_exit_and_never_push,
    contacts_end_when_bodies_part,
    continuous_collision_stops_a_bullet,
    axis_locks_hold_rotation,
    a_kinematic_platform_carries_what_rests_on_it,
    resting_bodies_sleep,
    far_from_the_origin_is_as_exact_as_at_it,
    bad_inputs_are_refused_with_codes,
);
