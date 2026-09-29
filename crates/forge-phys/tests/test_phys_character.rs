//! `test_phys_character` — **the kinematic character controller** (M4-3; gate
//! `C-phys-character`), on every backend.
//!
//! A 1.8 m capsule moved by `Character::move_and_slide`: it walks on flat ground and stays
//! on it; a wall stops it, and a diagonal walk into the wall slides along it; it steps up a
//! 0.25 m step and not a 0.6 m one (its `step_height` is 0.35 m); it walks up a 20-degree
//! ramp but not a 60-degree one (its `max_slope` is 45 degrees); going over an edge onto a
//! descending ramp it snaps down instead of walking out into the air; it lands when it falls; it pushes a free box it walks into.
//!
//! Positive controls (W2) beside each property: a step above `step_height`, a slope above
//! `max_slope`, `snap` 0, and no wall at all each give the other outcome.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use forge_frames::{DQuat, DVec3};
use forge_phys::{BodyDesc, Character, CharacterDesc, ColliderDesc, PhysicsWorld, Shape};

/// A capsule standing on the ground (its bottom at y = 0) at x, z.
fn person(w: &mut PhysicsWorld, x: f64, z: f64, desc: CharacterDesc) -> Character {
    let y = desc.half_height + desc.radius + desc.skin;
    Character::new(w, desc, at(x, y, z)).unwrap()
}

/// Walk `n` steps of 1/60 s at `v` (m/s), falling at gravity when not grounded; the world
/// steps each time. The character's positions.
fn walk(w: &mut PhysicsWorld, c: &mut Character, v: DVec3, n: u32) -> Vec<DVec3> {
    let dt = 1.0 / 60.0;
    let mut fall: f64 = 0.0;
    let mut out = Vec::new();
    for _ in 0..n {
        fall = if c.is_grounded() { 0.0 } else { fall - 9.81 * dt };
        let r = c
            .move_and_slide(w, DVec3::new(v.x * dt, v.y * dt + fall * dt, v.z * dt))
            .unwrap();
        if r.grounded {
            fall = 0.0;
        }
        w.step().unwrap();
        out.push(c.position().local);
    }
    out
}

fn box_at(w: &mut PhysicsWorld, centre: DVec3, half: DVec3) {
    let b = w.add_body(&BodyDesc::fixed(at(centre.x, centre.y, centre.z))).unwrap();
    w.add_collider(b, &ColliderDesc::new(Shape::Cuboid { half_extents: half }))
        .unwrap();
}

fn ramp(w: &mut PhysicsWorld, degrees: f64, x0: f64) {
    let a = degrees.to_radians();
    let b = w
        .add_body(&BodyDesc {
            rotation: DQuat::from_axis_angle(DVec3::Z, a),
            ..BodyDesc::fixed(at(x0 + 5.0 * a.cos(), 5.0 * a.sin() - 0.5 * a.cos(), 0.0))
        })
        .unwrap();
    w.add_collider(
        b,
        &ColliderDesc::new(Shape::Cuboid {
            half_extents: DVec3::new(5.0, 0.5, 3.0),
        }),
    )
    .unwrap();
}

fn walks_on_flat_ground_and_stays_on_it(b: &str) {
    let mut w = world(b);
    ground(&mut w);
    let d = CharacterDesc::default();
    let mut c = person(&mut w, 0.0, 0.0, d);
    let path = walk(&mut w, &mut c, DVec3::new(3.0, 0.0, 0.0), 60);
    let end = *path.last().unwrap();
    assert!(near(end.x, 3.0, 0.02), "{b}: walked to {end:?}, not 3 m");
    let stand = d.half_height + d.radius;
    assert!(path.iter().all(|p| (p.y - stand).abs() < 0.03), "{b}: left the ground");
    assert!(c.is_grounded(), "{b}");
    // Its body followed (the next step drove it there): queries and pushes see it.
    assert!((pos(&w, c.body()) - end).length() < 1e-6, "{b}: the body lags");
}

fn a_wall_stops_it_and_a_diagonal_slides_along(b: &str) {
    let mut w = world(b);
    ground(&mut w);
    box_at(&mut w, DVec3::new(2.0, 1.5, 0.0), DVec3::new(0.25, 1.5, 5.0));
    let mut c = person(&mut w, 0.0, 0.0, CharacterDesc::default());
    let end = *walk(&mut w, &mut c, DVec3::new(3.0, 0.0, 0.0), 90).last().unwrap();
    assert!(end.x < 2.0 - 0.25 - 0.3 + 0.02, "{b}: through the wall: {end:?}");
    let slid = *walk(&mut w, &mut c, DVec3::new(2.0, 0.0, 2.0), 60).last().unwrap();
    assert!(slid.z > 1.5, "{b}: did not slide along the wall: {slid:?}");
    assert!(slid.x < 2.0 - 0.25 - 0.3 + 0.02, "{b}: slid into the wall: {slid:?}");
    // The control: with no wall it walks on to x = 4.5.
    let mut w = world(b);
    ground(&mut w);
    let mut c = person(&mut w, 0.0, 0.0, CharacterDesc::default());
    let end = *walk(&mut w, &mut c, DVec3::new(3.0, 0.0, 0.0), 90).last().unwrap();
    assert!(end.x > 4.4, "{b}: the control stopped at {end:?}");
}

fn it_steps_up_a_low_step_and_not_a_high_one(b: &str) {
    let climb = |h: f64| {
        let mut w = world(b);
        ground(&mut w);
        box_at(&mut w, DVec3::new(3.0, 0.5 * h, 0.0), DVec3::new(1.5, 0.5 * h, 3.0));
        let d = CharacterDesc::default();
        let mut c = person(&mut w, 0.0, 0.0, d);
        let end = *walk(&mut w, &mut c, DVec3::new(2.0, 0.0, 0.0), 120).last().unwrap();
        (end, d.half_height + d.radius)
    };
    let (end, stand) = climb(0.25);
    assert!(end.x > 2.5 && near(end.y, stand + 0.25, 0.04), "{b}: {end:?}");
    let (end, stand) = climb(0.6);
    assert!(end.x < 1.5 && near(end.y, stand, 0.04), "{b}: climbed a 0.6 m step: {end:?}");
}

fn it_walks_up_gentle_slopes_and_not_steep_ones(b: &str) {
    let up = |deg: f64| {
        let mut w = world(b);
        ground(&mut w);
        ramp(&mut w, deg, 1.0);
        let mut c = person(&mut w, 0.0, 0.0, CharacterDesc::default());
        *walk(&mut w, &mut c, DVec3::new(2.0, 0.0, 0.0), 150).last().unwrap()
    };
    let gentle = up(20.0);
    assert!(gentle.x > 3.5 && gentle.y > 1.5, "{b}: stuck on a 20-degree ramp: {gentle:?}");
    let steep = up(60.0);
    assert!(steep.y < 1.3, "{b}: walked up a 60-degree ramp: {steep:?}");
}

/// Walking down a staircase of 0.15 m steps it stays on the ground (snapped down each
/// ledge); with `snap` 0 (the control) it walks out into the air off the first ledge.
fn it_snaps_down_stairs_instead_of_skipping(b: &str) {
    let down = |snap: f64| {
        let mut w = world(b);
        // Five steps descending along +x: step k spans x in [2k - 2, 2k] with its top at
        // -0.15 k.
        for k in 0..5 {
            let top = -0.15 * f64::from(k);
            box_at(
                &mut w,
                DVec3::new(2.0 * f64::from(k) - 1.0, top - 0.5, 0.0),
                DVec3::new(1.0, 0.5, 3.0),
            );
        }
        let d = CharacterDesc {
            snap,
            ..CharacterDesc::default()
        };
        let mut c = person(&mut w, -1.0, 0.0, d);
        c.move_and_slide(&mut w, DVec3::new(0.0, -0.05, 0.0)).unwrap();
        assert!(c.is_grounded(), "{b}: not standing on the top step");
        let mut airborne = 0;
        for _ in 0..90 {
            let r = c
                .move_and_slide(&mut w, DVec3::new(3.0 / 60.0, 0.0, 0.0))
                .unwrap();
            if !r.grounded {
                airborne += 1;
            }
            w.step().unwrap();
        }
        (airborne, c.position().local)
    };
    let (airborne, end) = down(0.3);
    assert_eq!(airborne, 0, "{b}: left the ground going down the stairs ({end:?})");
    // 4.5 m in 1.5 s: on the third step (x in [2, 4], its top at -0.30).
    assert!(
        end.x > 3.0 && near(end.y, 0.9 + 0.01 - 0.30, 0.02),
        "{b}: not standing on the third step: {end:?}"
    );
    let (airborne, _) = down(0.0);
    assert!(airborne > 10, "{b}: the control (no snap) stayed on the ground ({airborne})");
}

/// It lands when it falls. A free box in its way is pushed ahead by its kinematic body, and
/// the move reports touching it; a box dropped on its head rests on its capsule: its body is
/// in the world.
fn it_lands_reports_what_it_touches_and_holds_up_a_box(b: &str) {
    let mut w = world(b);
    ground(&mut w);
    let d = CharacterDesc::default();
    let mut c = Character::new(&mut w, d, at(0.0, 3.0, 0.0)).unwrap();
    let path = walk(&mut w, &mut c, DVec3::ZERO, 90);
    let end = *path.last().unwrap();
    assert!(c.is_grounded() && near(end.y, d.half_height + d.radius, 0.03), "{b}: {end:?}");
    let (bx, _) = body(&mut w, at(1.2, 0.25, 0.0), cube(0.25));
    run(&mut w, 10);
    let mut touched = false;
    for _ in 0..30 {
        let r = c.move_and_slide(&mut w, DVec3::new(2.0 / 60.0, 0.0, 0.0)).unwrap();
        touched |= r.touched.iter().flatten().any(|(t, _)| *t == bx);
        w.step().unwrap();
    }
    assert!(touched, "{b}: walking into the box never reported it");
    // Its kinematic body pushes the box ahead of it (never through it).
    let (cx, bxp) = (c.position().local.x, pos(&w, bx).x);
    assert!(bxp > 1.3, "{b}: the box was not pushed: {bxp}");
    assert!(bxp - cx > 0.25 + 0.3 - 0.05, "{b}: the character is inside the box ({cx} vs {bxp})");
    let top = d.half_height * 2.0 + d.radius * 2.0;
    let (lid, _) = body(&mut w, at(c.position().local.x, top + 1.0, 0.0), cube(0.1));
    run(&mut w, 120);
    assert!(pos(&w, lid).y > top, "{b}: the box fell through its head: {:?}", pos(&w, lid));
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
    walks_on_flat_ground_and_stays_on_it,
    a_wall_stops_it_and_a_diagonal_slides_along,
    it_steps_up_a_low_step_and_not_a_high_one,
    it_walks_up_gentle_slopes_and_not_steep_ones,
    it_snaps_down_stairs_instead_of_skipping,
    it_lands_reports_what_it_touches_and_holds_up_a_box,
);
