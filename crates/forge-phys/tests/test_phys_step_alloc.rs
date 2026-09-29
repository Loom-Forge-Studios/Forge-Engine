//! `test_phys_step_alloc` — **a steady `PhysicsWorld::step` makes no heap allocation of its
//! own** (gate `C-phys-step-no-alloc`).
//!
//! Everything the world does around the backend every step — the zone pass (half a kick
//! before and after), the interpolation poses of every moving body, the events' canonical
//! order, the async queue — reuses buffers sized once. Measured with a counting allocator
//! (thread-local, so sibling tests cannot disturb it) over a test-double backend that
//! integrates velocities and allocates nothing, so what is counted is the world's own work:
//! 1,000 dynamic bodies, three zones, an async ray query answered every step.
//!
//! The first-party backends' own allocations are printed for the record, not gated (their
//! solvers are upstream code; a regression there shows in the `phys.step.*` budget rows).
//!
//! Positive control (W2): `positive_control_a_rebuilt_moving_list_is_counted` — the same
//! run with `PhysFaults::rebuild_moving` (the moving-body list rebuilt every step, as a naive
//! world would) is counted.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use forge_frames::{DQuat, DVec3, FrameId, FramePos};
use forge_phys::{
    BodyDesc, BodyId, BodyState, ColliderDesc, ColliderId, Hit, JointDesc, JointFrames, JointId,
    Overlap, PhysError, PhysEvent, PhysFaults, PhysicsBackend, PhysicsSettings, PhysicsWorld,
    Query, QueryFilter, Ray, Shape, ShapeCast, ZoneDesc, ZoneEffect, ZoneShape,
};

const F: FrameId = FrameId(2);

/// Velocities integrated, nothing collides; no allocation after a body is added.
struct Null {
    bodies: Vec<BodyState>,
    gravity: DVec3,
}

impl PhysicsBackend for Null {
    fn id(&self) -> &str {
        "test.null"
    }
    fn set_gravity(&mut self, gravity: DVec3) {
        self.gravity = gravity;
    }
    fn add_body(&mut self, id: BodyId, d: &BodyDesc) -> Result<(), PhysError> {
        assert_eq!(id.0 as usize, self.bodies.len());
        self.bodies.push(BodyState {
            position: d.position,
            rotation: d.rotation,
            linear_velocity: d.linear_velocity,
            angular_velocity: d.angular_velocity,
        });
        Ok(())
    }
    fn remove_body(&mut self, _: BodyId) -> Result<(), PhysError> {
        Ok(())
    }
    fn add_collider(&mut self, _: ColliderId, _: BodyId, _: &ColliderDesc) -> Result<(), PhysError> {
        Ok(())
    }
    fn remove_collider(&mut self, _: ColliderId) -> Result<(), PhysError> {
        Ok(())
    }
    fn add_joint(&mut self, _: JointId, _: &JointDesc, _: &JointFrames) -> Result<(), PhysError> {
        Ok(())
    }
    fn remove_joint(&mut self, _: JointId) -> Result<(), PhysError> {
        Ok(())
    }
    fn state(&self, id: BodyId) -> Result<BodyState, PhysError> {
        Ok(self.bodies[id.0 as usize])
    }
    fn set_state(&mut self, id: BodyId, s: &BodyState) -> Result<(), PhysError> {
        self.bodies[id.0 as usize] = *s;
        Ok(())
    }
    fn set_velocity(&mut self, id: BodyId, l: DVec3, a: DVec3) -> Result<(), PhysError> {
        let b = &mut self.bodies[id.0 as usize];
        b.linear_velocity = l;
        b.angular_velocity = a;
        Ok(())
    }
    fn apply_impulse(&mut self, _: BodyId, _: DVec3, _: DVec3) -> Result<(), PhysError> {
        Ok(())
    }
    fn set_kinematic_target(&mut self, _: BodyId, _: FramePos, _: DQuat) -> Result<(), PhysError> {
        Ok(())
    }
    fn mass(&self, _: BodyId) -> Result<f64, PhysError> {
        Ok(1.0)
    }
    fn is_sleeping(&self, _: BodyId) -> Result<bool, PhysError> {
        Ok(false)
    }
    fn step(&mut self, dt: f64, _: &mut Vec<PhysEvent>) -> Result<(), PhysError> {
        for b in &mut self.bodies {
            b.linear_velocity += self.gravity * dt;
            b.position.local += b.linear_velocity * dt;
        }
        Ok(())
    }
    fn cast_ray(&self, _: &Ray, _: &QueryFilter) -> Result<Option<Hit>, PhysError> {
        Ok(None)
    }
    fn cast_shape(&self, _: &ShapeCast, _: &QueryFilter) -> Result<Option<Hit>, PhysError> {
        Ok(None)
    }
    fn overlap(&self, _: &Overlap, _: &QueryFilter, _: &mut Vec<ColliderId>) -> Result<(), PhysError> {
        Ok(())
    }
    fn contact_count(&self) -> usize {
        0
    }
}

fn at(x: f64, y: f64, z: f64) -> FramePos {
    FramePos::new(F, DVec3::new(x, y, z))
}

fn world(faults: PhysFaults) -> PhysicsWorld {
    let null = Null {
        bodies: Vec::new(),
        gravity: DVec3::ZERO,
    };
    let mut w = PhysicsWorld::with_backend(F, PhysicsSettings::default(), Box::new(null));
    w.faults = faults;
    w.set_gravity(DVec3::new(0.0, -9.81, 0.0)).unwrap();
    for i in 0..1000 {
        let b = w
            .add_body(&BodyDesc::dynamic(at(f64::from(i % 10) * 3.0, 50.0, f64::from(i / 10))))
            .unwrap();
        w.add_collider(b, &ColliderDesc::new(Shape::Sphere { radius: 0.5 }))
            .unwrap();
    }
    let zone = |x: f64, effect| ZoneDesc::new(at(x, 30.0, 50.0), ZoneShape::Box {
        half_extents: DVec3::new(5.0, 40.0, 60.0),
    }, effect);
    w.add_zone(&zone(0.0, ZoneEffect::Gravity {
        acceleration: DVec3::new(0.0, 3.0, 0.0),
    }))
    .unwrap();
    w.add_zone(&zone(12.0, ZoneEffect::Damping {
        linear: 1.0,
        angular: 1.0,
    }))
    .unwrap();
    w.add_zone(&zone(24.0, ZoneEffect::PointGravity { strength: 2.0 }))
        .unwrap();
    w
}

/// Allocations over 30 steady steps (after 30 of warm-up), each with an async ray query
/// submitted and taken.
fn count(faults: PhysFaults) -> u64 {
    let mut w = world(faults);
    let q = Query::Ray(Ray {
        start: at(0.0, 100.0, 0.0),
        direction: DVec3::new(0.0, -1.0, 0.0),
        max_distance: 200.0,
    });
    let frame = |w: &mut PhysicsWorld| {
        let t = w.submit(q.clone(), QueryFilter::default());
        w.advance(1.0 / 60.0).unwrap();
        assert!(w.poll(t).is_some());
        let _ = w.render_pose(BodyId(500)).unwrap();
    };
    for _ in 0..30 {
        frame(&mut w);
    }
    let info = allocation_counter::measure(|| {
        for _ in 0..30 {
            frame(&mut w);
        }
    });
    assert!(w.zone_touches() > 0, "the zones never acted: the scene is vacuous");
    info.count_total
}

#[test]
fn a_steady_step_makes_no_allocation_of_its_own() {
    let n = count(PhysFaults::default());
    assert_eq!(n, 0, "{n} allocations in 30 steady steps");
}

#[test]
fn positive_control_a_rebuilt_moving_list_is_counted() {
    let n = count(PhysFaults {
        rebuild_moving: true,
        ..PhysFaults::default()
    });
    assert!(n >= 30, "the control allocated only {n} times in 30 steps");
}

/// For the record: the first-party backends' own allocations per steady step (their
/// solvers' scratch), in the determinism corpus.
#[test]
fn backend_allocations_for_the_record() {
    for b in forge_phys::FIRST_PARTY {
        let (mut w, platform) = forge_phys::scenes::corpus(b, 0).unwrap();
        for i in 0..60 {
            forge_phys::scenes::drive(&mut w, platform, i).unwrap();
        }
        let info = allocation_counter::measure(|| {
            for i in 60..90 {
                forge_phys::scenes::drive(&mut w, platform, i).unwrap();
            }
        });
        println!(
            "{b}: {:.1} allocations, {:.0} bytes per steady corpus step",
            info.count_total as f64 / 30.0,
            info.bytes_total as f64 / 30.0
        );
    }
}
