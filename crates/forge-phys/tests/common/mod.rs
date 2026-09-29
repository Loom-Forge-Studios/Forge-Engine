//! Helpers shared by the forge-phys test suites.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use forge_frames::{DQuat, DVec3, FrameId, FramePos};
use forge_phys::{
    BodyDesc, BodyId, BodyKind, ColliderDesc, ColliderId, PhysicsSettings, PhysicsWorld, Shape,
};

/// Every suite runs in a frame other than the world frame: a physics world is region-local.
pub const F: FrameId = FrameId(7);

pub fn at(x: f64, y: f64, z: f64) -> FramePos {
    FramePos::new(F, DVec3::new(x, y, z))
}

pub fn world(backend: &str) -> PhysicsWorld {
    PhysicsWorld::first_party(backend, F, PhysicsSettings::default()).unwrap()
}

pub fn world_with(backend: &str, s: PhysicsSettings) -> PhysicsWorld {
    PhysicsWorld::first_party(backend, F, s).unwrap()
}

/// A static 40 x 1 x 40 box whose top face is at y = 0.
pub fn ground(w: &mut PhysicsWorld) -> (BodyId, ColliderId) {
    let g = w.add_body(&BodyDesc::fixed(at(0.0, -0.5, 0.0))).unwrap();
    let c = w
        .add_collider(
            g,
            &ColliderDesc::new(Shape::Cuboid {
                half_extents: DVec3::new(20.0, 0.5, 20.0),
            }),
        )
        .unwrap();
    (g, c)
}

/// A dynamic body with one collider of `shape` at `p`.
pub fn body(w: &mut PhysicsWorld, p: FramePos, shape: Shape) -> (BodyId, ColliderId) {
    body_with(w, BodyDesc::dynamic(p), ColliderDesc::new(shape))
}

pub fn body_with(w: &mut PhysicsWorld, b: BodyDesc, c: ColliderDesc) -> (BodyId, ColliderId) {
    let id = w.add_body(&b).unwrap();
    let c = w.add_collider(id, &c).unwrap();
    (id, c)
}

pub fn sphere(r: f64) -> Shape {
    Shape::Sphere { radius: r }
}

pub fn cube(half: f64) -> Shape {
    Shape::Cuboid {
        half_extents: DVec3::splat(half),
    }
}

pub fn run(w: &mut PhysicsWorld, n: u32) {
    for _ in 0..n {
        w.step().unwrap();
    }
}

pub fn pos(w: &PhysicsWorld, b: BodyId) -> DVec3 {
    w.position(b).unwrap().local
}

pub fn vel(w: &PhysicsWorld, b: BodyId) -> DVec3 {
    w.state(b).unwrap().linear_velocity
}

pub fn spin(w: &PhysicsWorld, b: BodyId) -> DVec3 {
    w.state(b).unwrap().angular_velocity
}

/// The angle (radians) of the rotation `q`.
pub fn angle(q: DQuat) -> f64 {
    let s = q.xyz().length();
    2.0 * s.atan2(q.w.abs())
}

pub fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// A kind of body for the determinism corpus and the budget scene.
pub fn kind(i: u32) -> BodyKind {
    if i.is_multiple_of(97) {
        BodyKind::Kinematic
    } else {
        BodyKind::Dynamic
    }
}
