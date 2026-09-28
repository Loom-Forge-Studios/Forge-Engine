//! `test_physics_2d` — the 2D solver's guarantees (Ch.35 §35.2 "2D physics: a 2D solver",
//! Ch.17's determinism brief), each with a positive control that breaks it (W2):
//!
//! * **Stacking**: a 10-high pyramid of boxes stands for 10 s, its top box within 2 cm of
//!   where it started. Control: without warm starting (`Faults2d::no_warm_start`) the same
//!   pyramid sags or topples past that.
//! * **No tunnelling**: a 0.2-unit ball at 120 units/s against a 0.1-unit wall at 60 Hz
//!   (2 units a step) stops at the wall. Control: without speculative contacts
//!   (`Faults2d::no_speculative`) it goes through.
//! * **Determinism**: two runs are bit-identical, and a world cloned mid-run continues
//!   exactly as the original (the replay and rewind property). Control: a solver whose
//!   contact order depends on the step (`Faults2d::reverse_contact_order`) diverges from the
//!   canonical run; and a solver visiting contacts in `HashSet` iteration order
//!   (`Faults2d::hash_ordered_contacts`) makes two runs of the same world disagree, so the
//!   two-run check catches in-process nondeterminism, not only a different order.
//! * **Broad phase**: sort-and-sweep finds exactly the pairs brute force finds.

use forge_2d::math::Rot2;
use forge_2d::physics::{BodyDef, BodyKind, ColliderDef, PhysicsWorld2d, Shape};
use forge_2d::{DVec2, Faults2d, FrameId, FramePos2};

const F: FrameId = FrameId(0);

fn at(x: f64, y: f64) -> FramePos2 {
    FramePos2::new(F, DVec2::new(x, y))
}

fn pyramid(faults: Faults2d) -> (PhysicsWorld2d, forge_2d::physics::RigidBodyId) {
    let mut w = PhysicsWorld2d::new(F);
    w.faults = faults;
    let g = w
        .add_body(&BodyDef::new(BodyKind::Static, at(0.0, -0.5)))
        .expect("ground");
    w.add_collider(
        g,
        ColliderDef::new(Shape::Box {
            half: DVec2::new(40.0, 0.5),
        }),
    )
    .expect("ground box");
    let mut top = None;
    let rows = 10;
    for row in 0..rows {
        for i in 0..rows - row {
            let x = (f64::from(i) - f64::from(rows - row - 1) * 0.5) * 1.02;
            let y = 0.5 + f64::from(row) * 1.0;
            let b = w
                .add_body(&BodyDef::new(BodyKind::Dynamic, at(x, y)))
                .expect("box");
            w.add_collider(
                b,
                ColliderDef::new(Shape::Box {
                    half: DVec2::new(0.5, 0.5),
                }),
            )
            .expect("box collider");
            top = Some(b);
        }
    }
    (w, top.expect("a top box"))
}

fn top_drift(faults: Faults2d) -> f64 {
    let (mut w, top) = pyramid(faults);
    let start = w.position(top).expect("p").local;
    for _ in 0..600 {
        w.step();
    }
    (w.position(top).expect("p").local - start).length()
}

#[test]
fn a_pyramid_of_boxes_stands() {
    let d = top_drift(Faults2d::default());
    println!("pyramid top drift after 10 s: {d:.4}");
    assert!(d < 0.02, "the top box moved {d}");
}

#[test]
fn positive_control_no_warm_start_lets_the_pyramid_sag() {
    let d = top_drift(Faults2d {
        no_warm_start: true,
        ..Faults2d::default()
    });
    println!("pyramid top drift without warm starting: {d:.4}");
    assert!(
        d >= 0.02,
        "without warm starting the stack must sag past the guard: {d}"
    );
}

fn bullet(faults: Faults2d) -> f64 {
    let mut w = PhysicsWorld2d::with_settings(
        F,
        forge_2d::physics::PhysicsSettings {
            gravity: DVec2::ZERO,
            ..Default::default()
        },
    );
    w.faults = faults;
    let wall = w
        .add_body(&BodyDef::new(BodyKind::Static, at(10.0, 0.0)))
        .expect("wall");
    w.add_collider(
        wall,
        ColliderDef::new(Shape::Box {
            half: DVec2::new(0.05, 3.0),
        }),
    )
    .expect("wall box");
    let b = w
        .add_body(&BodyDef {
            linear_velocity: DVec2::new(120.0, 0.0),
            ..BodyDef::new(BodyKind::Dynamic, at(0.0, 0.0))
        })
        .expect("ball");
    w.add_collider(b, ColliderDef::new(Shape::Circle { radius: 0.1 }))
        .expect("ball collider");
    for _ in 0..30 {
        w.step();
    }
    w.position(b).expect("p").local.x
}

#[test]
fn a_fast_ball_does_not_tunnel_through_a_thin_wall() {
    let x = bullet(Faults2d::default());
    assert!(x < 9.95, "stopped at the wall's near face: x = {x}");
}

#[test]
fn positive_control_without_speculative_contacts_it_tunnels() {
    let x = bullet(Faults2d {
        no_speculative: true,
        ..Faults2d::default()
    });
    assert!(
        x > 10.05,
        "without speculative contacts the ball passes the wall: x = {x}"
    );
}

fn scene(faults: Faults2d) -> PhysicsWorld2d {
    let (mut w, _) = pyramid(faults);
    for i in 0..30 {
        let b = w
            .add_body(&BodyDef {
                angle: f64::from(i) * 0.3,
                angular_velocity: 1.0,
                ..BodyDef::new(
                    BodyKind::Dynamic,
                    at(-6.0 + f64::from(i % 6) * 2.1, 14.0 + f64::from(i / 6) * 1.3),
                )
            })
            .expect("b");
        let shape = match i % 3 {
            0 => Shape::Circle { radius: 0.35 },
            1 => Shape::Capsule {
                half_height: 0.3,
                radius: 0.2,
            },
            _ => Shape::Polygon {
                verts: vec![
                    DVec2::new(-0.4, -0.3),
                    DVec2::new(0.4, -0.3),
                    DVec2::new(0.0, 0.45),
                ],
                radius: 0.0,
            },
        };
        let mut c = ColliderDef::new(shape);
        c.restitution = 0.2;
        w.add_collider(b, c).expect("c");
    }
    w
}

/// Two independent runs of the same world, 200 steps each, in one process: their state bits.
fn two_runs(faults: Faults2d) -> (PhysicsWorld2d, Vec<u64>, Vec<u64>) {
    let mut a = scene(faults);
    let mut b = scene(faults);
    for _ in 0..200 {
        a.step();
        b.step();
    }
    let (sa, sb) = (a.state_bits(), b.state_bits());
    (a, sa, sb)
}

#[test]
fn runs_are_bit_identical_and_a_clone_continues_exactly() {
    let (mut a, sa, sb) = two_runs(Faults2d::default());
    assert_eq!(sa, sb);
    let mut fork = a.clone();
    for _ in 0..200 {
        a.step();
        fork.step();
    }
    assert_eq!(
        a.state_bits(),
        fork.state_bits(),
        "a snapshot replays the same future"
    );
    assert!(
        a.contact_count() > 10,
        "the scene is in contact: {}",
        a.contact_count()
    );
}

#[test]
fn positive_control_a_step_dependent_contact_order_diverges() {
    let mut a = scene(Faults2d::default());
    let mut b = scene(Faults2d {
        reverse_contact_order: true,
        ..Faults2d::default()
    });
    for _ in 0..200 {
        a.step();
        b.step();
    }
    assert_ne!(
        a.state_bits(),
        b.state_bits(),
        "the canonical order is what makes runs agree"
    );
}

/// The two-run comparison itself catches real nondeterminism: a solver that visits its
/// contacts in a `HashSet`'s iteration order (a fresh `RandomState` every step, as a
/// `HashMap`-keyed contact list would) makes two runs of the same world in the same process
/// disagree, through the very check `runs_are_bit_identical_and_a_clone_continues_exactly`
/// makes.
#[test]
fn positive_control_hash_map_contact_order_makes_two_runs_disagree() {
    let (a, sa, sb) = two_runs(Faults2d {
        hash_ordered_contacts: true,
        ..Faults2d::default()
    });
    assert!(a.contact_count() > 10, "{}", a.contact_count());
    assert_ne!(
        sa, sb,
        "two runs visiting contacts in hash order must not agree bit for bit"
    );
}

#[test]
fn sort_and_sweep_finds_the_same_pairs_as_brute_force() {
    let mut a = scene(Faults2d::default());
    let mut b = scene(Faults2d {
        brute_force_pairs: true,
        ..Faults2d::default()
    });
    for _ in 0..120 {
        a.step();
        b.step();
        assert_eq!(a.last_pairs, b.last_pairs);
    }
    assert_eq!(
        a.state_bits(),
        b.state_bits(),
        "same pairs, same order: same result"
    );
    let _ = Rot2::IDENTITY;
}
