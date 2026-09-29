//! `test_phys_features` — queries, joints, zones and interpolation, on every backend (M7-9;
//! gate rows `C-phys-queries`, `C-phys-joints`, `C-phys-zones`, `C-phys-interpolation`).
//!
//! * **Queries**: ray, shape cast and overlap hit what they should, where they should, and
//!   respect filters (layers, triggers, the caster's own body); a batch answers exactly what
//!   the queries answer one at a time, in order, on several threads; an async query is
//!   answered at the next step boundary against the state that step produced.
//! * **Joints**: each of the six kinds keeps its constraint under gravity or a push, and the
//!   same scene without the joint breaks it (the control).
//! * **Zones**: gravity (directional and toward a point) and damping zones act on the bodies
//!   inside them only, by priority and layer; with the zone pass faulted off the body does
//!   what it would do without the zone (the control).
//! * **Interpolation**: the drawn pose moves smoothly at any frame rate; drawn at the last
//!   step's pose (the control fault) it stutters at the step rate.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use forge_frames::{DQuat, DVec3};
use forge_phys::{
    AxisMotion, BodyDesc, BodyId, ColliderDesc, JointDesc, JointKind, Layers, Overlap, PhysError,
    PhysFaults, PhysicsSettings, PhysicsWorld, Query, QueryFilter, QueryResult, Ray, ShapeCast,
    ZoneDesc, ZoneEffect, ZoneShape,
};

fn no_gravity() -> PhysicsSettings {
    PhysicsSettings {
        gravity: DVec3::ZERO,
        ..PhysicsSettings::default()
    }
}

fn ray(x: f64, y: f64, z: f64, d: DVec3, max: f64) -> Ray {
    Ray {
        start: at(x, y, z),
        direction: d,
        max_distance: max,
    }
}

// ---- queries --------------------------------------------------------------------------------

fn raycasts_hit_the_nearest_collider_and_respect_filters(b: &str) {
    let mut w = world_with(b, no_gravity());
    let near_box = w.add_body(&BodyDesc::fixed(at(5.0, 0.0, 0.0))).unwrap();
    let c1 = w
        .add_collider(
            near_box,
            &ColliderDesc {
                layers: Layers {
                    memberships: 2,
                    filters: u32::MAX,
                },
                ..ColliderDesc::new(cube(0.5))
            },
        )
        .unwrap();
    let (far_box, c2) = body(&mut w, at(10.0, 0.0, 0.0), cube(0.5));
    let trig = w.add_body(&BodyDesc::fixed(at(2.0, 0.0, 0.0))).unwrap();
    let t = w
        .add_collider(trig, &ColliderDesc::sensor(cube(0.25)))
        .unwrap();
    let r = ray(0.0, 0.0, 0.0, DVec3::X, 100.0);
    let f = QueryFilter::default();
    let h = w.raycast(&r, &f).unwrap().expect("hits the near box");
    assert_eq!((h.collider, h.body), (c1, near_box), "{b}");
    assert!(near(h.distance, 4.5, 1e-9), "{b}: {h:?}");
    assert!(
        (h.point.local - DVec3::new(4.5, 0.0, 0.0)).length() < 1e-9,
        "{b}: {h:?}"
    );
    assert_eq!(h.point.frame, F);
    assert!(
        (h.normal - DVec3::new(-1.0, 0.0, 0.0)).length() < 1e-9,
        "{b}: {h:?}"
    );
    // Not the near box's layer: the far box.
    let only_1 = QueryFilter { layers: 1, ..f };
    let h = w.raycast(&r, &only_1).unwrap().expect("hits the far box");
    assert_eq!((h.collider, h.body), (c2, far_box), "{b}");
    // The caster's own body is skipped.
    let not_near = QueryFilter {
        exclude_body: Some(near_box),
        ..f
    };
    assert_eq!(w.raycast(&r, &not_near).unwrap().unwrap().collider, c2);
    // Triggers only when asked.
    let with_triggers = QueryFilter {
        include_sensors: true,
        ..f
    };
    assert_eq!(w.raycast(&r, &with_triggers).unwrap().unwrap().collider, t);
    // Too short, or pointing away: nothing (the control of "hits").
    assert!(
        w.raycast(&ray(0.0, 0.0, 0.0, DVec3::X, 4.0), &f)
            .unwrap()
            .is_none()
    );
    assert!(
        w.raycast(&ray(0.0, 0.0, 0.0, DVec3::new(-1.0, 0.0, 0.0), 100.0), &f)
            .unwrap()
            .is_none()
    );
    // A zero direction is refused.
    assert!(matches!(
        w.raycast(&ray(0.0, 0.0, 0.0, DVec3::ZERO, 1.0), &f),
        Err(PhysError::Invalid(_))
    ));
}

fn shape_casts_and_overlaps_find_the_right_colliders(b: &str) {
    let mut w = world_with(b, no_gravity());
    let (bx, c1) = body(&mut w, at(5.0, 0.0, 0.0), cube(0.5));
    let (_, c2) = body(&mut w, at(10.0, 0.0, 0.0), sphere(0.5));
    let cast = ShapeCast {
        shape: sphere(0.5),
        start: at(0.0, 0.0, 0.0),
        rotation: DQuat::IDENTITY,
        direction: DVec3::X,
        max_distance: 100.0,
    };
    let f = QueryFilter::default();
    let h = w
        .shape_cast(&cast, &f)
        .unwrap()
        .expect("the sweep hits the box");
    assert_eq!((h.collider, h.body), (c1, bx), "{b}");
    // A sweep is solved iteratively (GJK): exact to its convergence tolerance, ~1e-6 m.
    assert!(near(h.distance, 4.0, 1e-5), "{b}: {h:?}");
    assert!(
        (h.point.local - DVec3::new(4.5, 0.0, 0.0)).length() < 1e-5,
        "{b}: {h:?}"
    );
    assert!(
        (h.normal - DVec3::new(-1.0, 0.0, 0.0)).length() < 1e-5,
        "{b}: {h:?}"
    );
    let over = |p: DVec3, r: f64| {
        w.overlap(
            &Overlap {
                shape: sphere(r),
                position: at(p.x, p.y, p.z),
                rotation: DQuat::IDENTITY,
            },
            &f,
        )
        .unwrap()
    };
    assert_eq!(over(DVec3::new(5.8, 0.0, 0.0), 0.5), vec![c1], "{b}");
    assert_eq!(over(DVec3::new(7.5, 0.0, 0.0), 2.5), vec![c1, c2], "{b}");
    assert!(over(DVec3::new(7.5, 0.0, 0.0), 1.5).is_empty(), "{b}");
}

/// A batch answers exactly what the queries answer one at a time, `out[i]` for
/// `queries[i]`, including when it is large enough to run on several threads.
fn a_batch_answers_like_one_query_at_a_time(b: &str) {
    let mut w = world_with(b, no_gravity());
    for i in 0..50 {
        let x = f64::from(i % 10) * 2.0;
        let z = f64::from(i / 10) * 2.0;
        body(
            &mut w,
            at(x, 0.0, z),
            if i % 2 == 0 { cube(0.4) } else { sphere(0.5) },
        );
    }
    let f = QueryFilter::default();
    let queries: Vec<Query> = (0..2048u32)
        .map(|i| {
            let (x, z) = (f64::from(i % 64) * 0.3 - 1.0, f64::from(i / 64) * 0.3 - 1.0);
            match i % 3 {
                0 => Query::Ray(ray(x, 5.0, z, DVec3::new(0.0, -1.0, 0.0), 10.0)),
                1 => Query::Shape(ShapeCast {
                    shape: sphere(0.2),
                    start: at(x, 5.0, z),
                    rotation: DQuat::IDENTITY,
                    direction: DVec3::new(0.0, -1.0, 0.0),
                    max_distance: 10.0,
                }),
                _ => Query::Overlap(Overlap {
                    shape: cube(0.3),
                    position: at(x, 0.0, z),
                    rotation: DQuat::IDENTITY,
                }),
            }
        })
        .collect();
    let mut out = Vec::new();
    w.query_batch(&queries, &f, &mut out);
    assert_eq!(out.len(), queries.len());
    let mut hits = 0;
    for (q, got) in queries.iter().zip(&out) {
        let one = match q {
            Query::Ray(r) => QueryResult::Hit(w.raycast(r, &f).unwrap()),
            Query::Shape(c) => QueryResult::Hit(w.shape_cast(c, &f).unwrap()),
            Query::Overlap(o) => QueryResult::Overlaps(w.overlap(o, &f).unwrap()),
        };
        if matches!(&one, QueryResult::Hit(Some(_))) {
            hits += 1;
        }
        assert_eq!(got.as_ref().unwrap(), &one, "{b}: {q:?}");
    }
    assert!(
        hits > 300,
        "{b}: the batch scene is too empty to mean anything ({hits} hits)"
    );
}

/// An async query is answered at the next step boundary, against the state the step
/// produced: a ray aimed where a moving box will be after the step hits it, although it
/// misses the box where it was when the query was submitted (the control).
fn an_async_query_answers_after_the_next_step(b: &str) {
    let mut w = world_with(b, no_gravity());
    let (bx, c) = body_with(
        &mut w,
        BodyDesc {
            linear_velocity: DVec3::new(60.0, 0.0, 0.0),
            ..BodyDesc::dynamic(at(0.0, 0.0, 0.0))
        },
        ColliderDesc::new(cube(0.25)),
    );
    let _ = bx;
    let down = ray(1.0, 5.0, 0.0, DVec3::new(0.0, -1.0, 0.0), 10.0);
    let f = QueryFilter::default();
    assert!(
        w.raycast(&down, &f).unwrap().is_none(),
        "{b}: the box is not there yet"
    );
    let t = w.submit(Query::Ray(down), f);
    assert!(w.poll(t).is_none(), "{b}: answered before the step");
    assert_eq!(w.pending_queries(), 1);
    w.step().unwrap();
    assert_eq!(w.pending_queries(), 0);
    match w.poll(t) {
        Some(Ok(QueryResult::Hit(Some(h)))) => assert_eq!(h.collider, c, "{b}"),
        other => panic!("{b}: expected the moved box, got {other:?}"),
    }
    assert!(w.poll(t).is_none(), "{b}: an answer is taken once");
}

// ---- joints ---------------------------------------------------------------------------------

/// A static hook at (0, 5, 0) and a 0.4 m box hanging from it.
fn hook_and_box(w: &mut PhysicsWorld, box_at: DVec3) -> (BodyId, BodyId) {
    let hook = w.add_body(&BodyDesc::fixed(at(0.0, 5.0, 0.0))).unwrap();
    let (bx, _) = body(w, at(box_at.x, box_at.y, box_at.z), cube(0.2));
    (hook, bx)
}

fn joint(w: &mut PhysicsWorld, a: BodyId, bb: BodyId, kind: JointKind, axis: DVec3) {
    let mut j = JointDesc::new(a, bb, kind);
    j.axis = axis;
    // Anchored at the hook's origin; B's anchor is where that point is in B's frame.
    let pa = pos(w, a);
    let pb = pos(w, bb);
    j.anchor_b = pa - pb;
    w.add_joint(&j).unwrap();
}

fn a_fixed_joint_holds_a_box_in_place(b: &str) {
    let hold = |with_joint: bool| {
        let mut w = world(b);
        let (hook, bx) = hook_and_box(&mut w, DVec3::new(1.0, 5.0, 0.0));
        if with_joint {
            joint(&mut w, hook, bx, JointKind::Fixed, DVec3::X);
        }
        run(&mut w, 120);
        (pos(&w, bx), angle(w.rotation(bx).unwrap()))
    };
    let (p, a) = hold(true);
    assert!(
        (p - DVec3::new(1.0, 5.0, 0.0)).length() < 0.02,
        "{b}: {p:?}"
    );
    assert!(a < 0.02, "{b}: turned {a} rad");
    let (p, _) = hold(false);
    assert!(p.y < 0.0, "{b}: the control did not fall: {p:?}");
}

/// A pendulum on a hinge about z swings in the xy plane at a fixed radius; its limit holds.
fn a_hinge_swings_about_its_axis_within_its_limits(b: &str) {
    let swing = |limits: Option<(f64, f64)>| {
        let mut w = world(b);
        let (hook, bx) = hook_and_box(&mut w, DVec3::new(1.0, 5.0, 0.0));
        joint(&mut w, hook, bx, JointKind::Hinge { limits }, DVec3::Z);
        let mut worst_r: f64 = 0.0;
        let mut worst_z: f64 = 0.0;
        let mut lowest: f64 = 5.0;
        for _ in 0..120 {
            w.step().unwrap();
            let p = pos(&w, bx) - DVec3::new(0.0, 5.0, 0.0);
            worst_r = worst_r.max((p.length() - 1.0).abs());
            worst_z = worst_z.max(p.z.abs());
            lowest = lowest.min(p.y);
        }
        (worst_r, worst_z, lowest)
    };
    let (r, z, lowest) = swing(None);
    assert!(
        r < 0.02 && z < 0.01,
        "{b}: left the hinge circle: dr {r}, z {z}"
    );
    assert!(
        lowest < -0.95,
        "{b}: the free pendulum never swung down ({lowest})"
    );
    // Limited to 0.3 rad either side: it never drops below sin(0.3) m (within 3 degrees:
    // limits are solved per substep, so a swinging body settles onto them).
    let (_, _, lowest) = swing(Some((-0.3, 0.3)));
    assert!(
        lowest > -(0.35f64.sin()),
        "{b}: the limited hinge dropped to {lowest}"
    );
}

/// A box on a 45-degree slider moves only along it and stops at its limit; without the
/// joint it falls straight down (the control).
fn a_slider_moves_only_along_its_axis_to_its_limit(b: &str) {
    let axis = DVec3::new(1.0, -1.0, 0.0) / 2f64.sqrt();
    let slide = |with_joint: bool| {
        let mut w = world(b);
        let (hook, bx) = hook_and_box(&mut w, DVec3::new(0.0, 5.0, 0.0));
        if with_joint {
            joint(
                &mut w,
                hook,
                bx,
                JointKind::Slider {
                    limits: Some((-1.0, 1.0)),
                },
                axis,
            );
        }
        run(&mut w, 120);
        let d = pos(&w, bx) - DVec3::new(0.0, 5.0, 0.0);
        let along = d.dot(axis);
        (along, (d - axis * along).length())
    };
    let (along, off) = slide(true);
    assert!(near(along, 1.0, 0.03), "{b}: slid {along} along, limit 1");
    assert!(off < 0.02, "{b}: left the axis by {off}");
    let (_, off) = slide(false);
    assert!(off > 1.0, "{b}: the control stayed on the axis ({off})");
}

/// A spring pulled to twice its rest length oscillates through the rest length and settles
/// near it; with no spring the body just flies away (the control).
fn a_spring_pulls_back_to_its_rest_length(b: &str) {
    let pull = |with_joint: bool| {
        let mut w = world_with(b, no_gravity());
        let (hook, bx) = hook_and_box(&mut w, DVec3::new(2.0, 5.0, 0.0));
        w.set_velocity(bx, DVec3::new(1.0, 0.0, 0.0), DVec3::ZERO)
            .unwrap();
        if with_joint {
            // Anchored at the two bodies' origins, 2 m apart: stretched to twice its rest.
            let mut j = JointDesc::new(
                hook,
                bx,
                JointKind::Spring {
                    rest_length: 1.0,
                    stiffness: 400.0,
                    // 64 kg: about half critical damping, so it overshoots once and settles.
                    damping: 160.0,
                },
            );
            j.axis = DVec3::X;
            w.add_joint(&j).unwrap();
        }
        let mut min_d = f64::MAX;
        for _ in 0..300 {
            w.step().unwrap();
            min_d = min_d.min((pos(&w, bx) - DVec3::new(0.0, 5.0, 0.0)).length());
        }
        (min_d, (pos(&w, bx) - DVec3::new(0.0, 5.0, 0.0)).length())
    };
    let (min_d, end) = pull(true);
    assert!(
        min_d < 1.05,
        "{b}: never came back to the rest length ({min_d})"
    );
    assert!(near(end, 1.0, 0.1), "{b}: settled at {end}, rest 1");
    let (_, end) = pull(false);
    assert!(end > 5.0, "{b}: the control came back ({end})");
}

/// A ball joint's cone keeps a pushed pendulum within its swing; the same push on a free
/// ball joint (6DOF, angular axes free) swings far past it (the control). Its twist limit
/// stops a spin about the joint axis.
fn a_cone_twist_keeps_swing_and_twist_within_limits(b: &str) {
    let down = DVec3::new(0.0, -1.0, 0.0);
    let push = |kind: JointKind, kick: DVec3, spin: DVec3| {
        let mut w = world(b);
        let (hook, bx) = hook_and_box(&mut w, DVec3::new(0.0, 4.0, 0.0));
        joint(&mut w, hook, bx, kind, down);
        w.set_velocity(bx, kick, spin).unwrap();
        let mut swing: f64 = 0.0;
        let mut twist: f64 = 0.0;
        for _ in 0..90 {
            w.step().unwrap();
            let d = (pos(&w, bx) - DVec3::new(0.0, 5.0, 0.0))
                .try_normalize()
                .unwrap();
            swing = swing.max(d.dot(down).clamp(-1.0, 1.0).acos());
            let q = w.rotation(bx).unwrap();
            // The twist: the rotation's component about the joint axis.
            let tw = 2.0 * (q.y).atan2(q.w).abs();
            twist = twist.max(tw.min(std::f64::consts::TAU - tw));
        }
        (swing, twist)
    };
    let cone = JointKind::ConeTwist {
        swing: 0.4,
        twist: (-0.2, 0.2),
    };
    let ball = JointKind::SixDof {
        axes: [
            AxisMotion::Locked,
            AxisMotion::Locked,
            AxisMotion::Locked,
            AxisMotion::Free,
            AxisMotion::Free,
            AxisMotion::Free,
        ],
    };
    // A 6 rad/s slam into the cone. rapier3d holds it exactly; avian3d's XPBD limits are
    // solved per substep and give up to ~0.08 rad at its default 6 substeps (ADR 0066).
    let kick = DVec3::new(6.0, 0.0, 0.0);
    let (s, _) = push(cone, kick, DVec3::ZERO);
    assert!(s < 0.4 + 0.1, "{b}: swung {s} rad past a 0.4 cone");
    let (s, _) = push(cone, DVec3::new(0.0, 0.0, 6.0), DVec3::ZERO);
    assert!(
        s > 0.3 && s < 0.4 + 0.1,
        "{b}: across the other swing axis: {s} rad"
    );
    let (s, _) = push(ball, kick, DVec3::ZERO);
    assert!(s > 0.8, "{b}: the free control swung only {s}");
    let spin = DVec3::new(0.0, 8.0, 0.0);
    let (_, t) = push(cone, DVec3::ZERO, spin);
    assert!(t < 0.2 + 0.05, "{b}: twisted {t} rad past 0.2");
    let (_, t) = push(ball, DVec3::ZERO, spin);
    assert!(t > 0.5, "{b}: the free control twisted only {t}");
}

/// 6DOF: a joint with one limited linear axis behaves as a limited slider on both backends.
/// A combination avian3d has no joint for (a cylindrical joint: slide and turn about one
/// axis) works on rapier3d and is refused on avian3d with `PHYS-0004` naming rapier3d.
fn a_six_dof_joint_holds_its_axes(b: &str) {
    let mut w = world(b);
    let (hook, bx) = hook_and_box(&mut w, DVec3::new(0.0, 5.0, 0.0));
    let limited = [
        AxisMotion::Limited {
            min: -0.5,
            max: 0.5,
        },
        AxisMotion::Locked,
        AxisMotion::Locked,
        AxisMotion::Locked,
        AxisMotion::Locked,
        AxisMotion::Locked,
    ];
    joint(
        &mut w,
        hook,
        bx,
        JointKind::SixDof { axes: limited },
        DVec3::new(0.0, -1.0, 0.0),
    );
    run(&mut w, 120);
    let d = pos(&w, bx) - DVec3::new(0.0, 5.0, 0.0);
    assert!(near(d.y, -0.5, 0.03) && d.x.abs() < 0.02, "{b}: {d:?}");
    // The cylindrical joint.
    let mut w = world(b);
    let (hook, bx) = hook_and_box(&mut w, DVec3::new(0.0, 5.0, 0.0));
    let mut j = JointDesc::new(
        hook,
        bx,
        JointKind::SixDof {
            axes: [
                AxisMotion::Free,
                AxisMotion::Locked,
                AxisMotion::Locked,
                AxisMotion::Free,
                AxisMotion::Locked,
                AxisMotion::Locked,
            ],
        },
    );
    j.axis = DVec3::new(0.0, -1.0, 0.0);
    j.anchor_b = DVec3::ZERO;
    let r = w.add_joint(&j);
    if b == forge_phys::AVIAN {
        let e = r.unwrap_err();
        assert!(matches!(e, PhysError::Unsupported(_)), "{e}");
        assert!(e.to_string().contains("rapier3d"), "{e}");
    } else {
        r.unwrap();
        w.set_velocity(bx, DVec3::ZERO, DVec3::new(0.0, 3.0, 0.0))
            .unwrap();
        run(&mut w, 60);
        let d = pos(&w, bx) - DVec3::new(0.0, 5.0, 0.0);
        assert!(
            d.y < -1.0 && d.x.abs() < 0.02 && d.z.abs() < 0.02,
            "{b}: {d:?}"
        );
        assert!(angle(w.rotation(bx).unwrap()) > 0.5, "{b}: did not turn");
    }
}

// ---- zones ----------------------------------------------------------------------------------

/// Inside a zone whose gravity points up the body rises; outside it falls; with the zone
/// pass faulted off it falls inside too (the control).
fn a_gravity_zone_replaces_gravity_inside_it(b: &str) {
    let fly = |faults: PhysFaults| {
        let mut w = world(b);
        w.faults = faults;
        w.add_zone(&ZoneDesc::new(
            at(0.0, 0.0, 0.0),
            ZoneShape::Box {
                half_extents: DVec3::new(2.0, 50.0, 2.0),
            },
            ZoneEffect::Gravity {
                acceleration: DVec3::new(0.0, 9.81, 0.0),
            },
        ))
        .unwrap();
        let (inside, _) = body(&mut w, at(0.0, 0.0, 0.0), sphere(0.2));
        let (outside, _) = body(&mut w, at(5.0, 0.0, 0.0), sphere(0.2));
        run(&mut w, 60);
        (pos(&w, inside).y, pos(&w, outside).y, w.zone_touches())
    };
    let (up, down, touches) = fly(PhysFaults::default());
    assert!(near(up, 4.95, 0.1), "{b}: rose to {up}");
    assert!(near(down, -4.99, 0.1), "{b}: outside fell to {down}");
    assert!(touches >= 60, "{b}: {touches}");
    let (up, _, _) = fly(PhysFaults {
        ignore_zones: true,
        ..PhysFaults::default()
    });
    assert!(up < -4.0, "{b}: with zones off it still rose ({up})");
}

/// A point-gravity zone (a small planet) pulls toward its centre; a higher-priority zone
/// overrides it; a zone limited to another layer leaves the body alone.
fn point_gravity_priority_and_layers(b: &str) {
    let mut w = world_with(b, no_gravity());
    w.add_zone(&ZoneDesc::new(
        at(0.0, 0.0, 0.0),
        ZoneShape::Sphere { radius: 50.0 },
        ZoneEffect::PointGravity { strength: 5.0 },
    ))
    .unwrap();
    let mut side = ZoneDesc::new(
        at(-20.0, 0.0, 0.0),
        ZoneShape::Sphere { radius: 3.0 },
        ZoneEffect::Gravity {
            acceleration: DVec3::new(0.0, 0.0, 5.0),
        },
    );
    side.priority = 1;
    w.add_zone(&side).unwrap();
    let mut other = ZoneDesc::new(
        at(0.0, 20.0, 0.0),
        ZoneShape::Sphere { radius: 3.0 },
        ZoneEffect::Gravity {
            acceleration: DVec3::new(0.0, 50.0, 0.0),
        },
    );
    other.priority = 5;
    other.layers = 1 << 4;
    w.add_zone(&other).unwrap();
    let (a, _) = body(&mut w, at(20.0, 0.0, 0.0), sphere(0.2));
    let (bb, _) = body(&mut w, at(-20.0, 0.0, 0.0), sphere(0.2));
    let (c, _) = body(&mut w, at(0.0, 20.0, 0.0), sphere(0.2));
    let (d, _) = body(&mut w, at(80.0, 0.0, 0.0), sphere(0.2));
    run(&mut w, 30);
    let (pa, pb, pc, pd) = (pos(&w, a), pos(&w, bb), pos(&w, c), pos(&w, d));
    // 5 m/s² toward the centre for half a second: 0.625 m.
    assert!(
        near(pa.x, 19.375, 0.01) && pa.y.abs() < 1e-9,
        "{b}: toward the centre: {pa:?}"
    );
    assert!(
        pb.z > 0.5 && near(pb.x, -20.0, 1e-9),
        "{b}: the priority zone: {pb:?}"
    );
    // Only the planet pulls it: the stronger zone it is in is for another layer.
    assert!(
        near(pc.y, 19.375, 0.01),
        "{b}: another layer's zone moved it: {pc:?}"
    );
    assert_eq!(pd, DVec3::new(80.0, 0.0, 0.0), "{b}: outside every zone");
}

/// A damping zone slows a body crossing it; without the zone (the control) it keeps its
/// speed.
fn a_damping_zone_slows_what_crosses_it(b: &str) {
    let cross = |zone: bool| {
        let mut w = world_with(b, no_gravity());
        if zone {
            w.add_zone(&ZoneDesc::new(
                at(5.0, 0.0, 0.0),
                ZoneShape::Box {
                    half_extents: DVec3::new(5.0, 2.0, 2.0),
                },
                ZoneEffect::Damping {
                    linear: 4.0,
                    angular: 4.0,
                },
            ))
            .unwrap();
        }
        let (s, _) = body_with(
            &mut w,
            BodyDesc {
                linear_velocity: DVec3::new(10.0, 0.0, 0.0),
                ..BodyDesc::dynamic(at(0.5, 0.0, 0.0))
            },
            ColliderDesc::new(sphere(0.2)),
        );
        run(&mut w, 60);
        vel(&w, s).x
    };
    let slowed = cross(true);
    let kept = cross(false);
    assert!(slowed < 1.0, "{b}: the damping zone left {slowed} m/s");
    assert!(
        near(kept, 10.0, 1e-9),
        "{b}: the control lost speed: {kept}"
    );
}

// ---- interpolation ----------------------------------------------------------------------------

/// At 144 frames per second over a 60 Hz simulation, the drawn position of a body moving
/// at constant speed advances by the same amount every frame (it is smooth); drawn at the
/// last step's pose instead (the control fault) it advances in jumps.
fn interpolation_draws_smooth_motion_between_steps(b: &str) {
    let draw = |faults: PhysFaults| {
        let mut w = world_with(b, no_gravity());
        w.faults = faults;
        let (s, _) = body_with(
            &mut w,
            BodyDesc {
                linear_velocity: DVec3::new(6.0, 0.0, 0.0),
                ..BodyDesc::dynamic(at(0.0, 0.0, 0.0))
            },
            ColliderDesc::new(sphere(0.2)),
        );
        let frame = 1.0 / 144.0;
        let mut xs = Vec::new();
        for _ in 0..144 {
            w.advance(frame).unwrap();
            // Interpolating between the last two steps draws one step behind: skip the
            // frames before the first step, when there is nothing to blend yet.
            if w.steps() > 0 {
                xs.push(w.render_pose(s).unwrap().0.local.x);
            }
        }
        let d: Vec<f64> = xs.windows(2).map(|p| p[1] - p[0]).collect();
        let lo = d.iter().copied().fold(f64::MAX, f64::min);
        let hi = d.iter().copied().fold(f64::MIN, f64::max);
        (lo, hi, w.steps())
    };
    let (lo, hi, steps) = draw(PhysFaults::default());
    assert!(
        near(f64::from(steps as u32), 59.0, 1.5),
        "{b}: {steps} steps in a second"
    );
    // 6 m/s at 144 Hz: 0.0417 m per frame, within rounding of the step boundary.
    assert!(
        near(lo, 6.0 / 144.0, 1e-6) && near(hi, 6.0 / 144.0, 1e-6),
        "{b}: {lo}..{hi}"
    );
    let (lo, hi, _) = draw(PhysFaults {
        no_interpolation: true,
        ..PhysFaults::default()
    });
    assert!(
        lo < 1e-12 && hi > 0.09,
        "{b}: the control drew smoothly: {lo}..{hi}"
    );
}

/// A long stall runs at most `max_catch_up` steps and drops the rest (no spiral of death).
fn a_long_stall_catches_up_a_bounded_number_of_steps(b: &str) {
    let mut w = world(b);
    body(&mut w, at(0.0, 10.0, 0.0), sphere(0.2));
    assert_eq!(w.advance(2.0).unwrap(), 5, "{b}");
    assert!(w.alpha() < 1.0);
    assert_eq!(w.advance(1.0 / 60.0).unwrap(), 1, "{b}");
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
    raycasts_hit_the_nearest_collider_and_respect_filters,
    shape_casts_and_overlaps_find_the_right_colliders,
    a_batch_answers_like_one_query_at_a_time,
    an_async_query_answers_after_the_next_step,
    a_fixed_joint_holds_a_box_in_place,
    a_hinge_swings_about_its_axis_within_its_limits,
    a_slider_moves_only_along_its_axis_to_its_limit,
    a_spring_pulls_back_to_its_rest_length,
    a_cone_twist_keeps_swing_and_twist_within_limits,
    a_six_dof_joint_holds_its_axes,
    a_gravity_zone_replaces_gravity_inside_it,
    point_gravity_priority_and_layers,
    a_damping_zone_slows_what_crosses_it,
    interpolation_draws_smooth_motion_between_steps,
    a_long_stall_catches_up_a_bounded_number_of_steps,
);
