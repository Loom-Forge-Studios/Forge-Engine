//! The rapier3d backend (`rapier3d`, E-5's fallback, selectable per project): rapier's
//! `f64` build with `enhanced-determinism` (portable math, no SIMD, no parallel solver).
//!
//! Every joint is a rapier `GenericJoint` built from the world's joint frames: the joint
//! kinds are sets of locked, limited and coupled axes on one constraint type, so the six
//! kinds cost one code path. The cone-twist's swing is rapier's coupled angular limit (a true
//! cone about the joint axis), its twist a limit on the axis itself.

use std::sync::Mutex;

use forge_frames::{DQuat, DVec3, FrameId, FramePos};
use rapier3d_f64::math::{Pose, Rotation, Vector};
use rapier3d_f64::parry::query::ShapeCastOptions;
use rapier3d_f64::parry::shape::SharedShape;
use rapier3d_f64::parry::utils::Array2;
use rapier3d_f64::prelude::{
    ActiveEvents, CoefficientCombineRule, Collider, ColliderBuilder, ColliderHandle, ColliderSet,
    CollisionEvent, ContactPair, GenericJointBuilder, Group, ImpulseJointHandle, InteractionGroups,
    InteractionTestMode, JointAxesMask, JointAxis, LockedAxes, MotorModel, RigidBodyBuilder,
    RigidBodyHandle, RigidBodySet,
};

use crate::backend::{PhysicsBackend, RAPIER};
use crate::types::{
    AxisMotion, BodyDesc, BodyId, BodyKind, BodyState, ColliderDesc, ColliderId, CombineRule, Hit,
    JointDesc, JointFrames, JointId, JointKind, Overlap, PhysEvent, QueryFilter, Ray, Shape,
    ShapeCast,
};
use crate::{PhysError, PhysicsSettings};

fn v(a: DVec3) -> Vector {
    Vector::new(a.x, a.y, a.z)
}

fn fv(a: Vector) -> DVec3 {
    DVec3::new(a.x, a.y, a.z)
}

fn q(a: DQuat) -> Rotation {
    Rotation::from_xyzw(a.x, a.y, a.z, a.w)
}

fn fq(a: Rotation) -> DQuat {
    DQuat::from_xyzw(a.x, a.y, a.z, a.w)
}

fn pose(t: DVec3, r: DQuat) -> Pose {
    Pose::from_parts(v(t), q(r))
}

fn rule(r: CombineRule) -> CoefficientCombineRule {
    match r {
        CombineRule::Average => CoefficientCombineRule::Average,
        CombineRule::Min => CoefficientCombineRule::Min,
        CombineRule::Multiply => CoefficientCombineRule::Multiply,
        CombineRule::Max => CoefficientCombineRule::Max,
    }
}

/// The rapier shape of a [`Shape`] (checked by the world first).
pub(crate) fn shared_shape(s: &Shape) -> Result<SharedShape, PhysError> {
    Ok(match s {
        Shape::Sphere { radius } => SharedShape::ball(*radius),
        Shape::Cuboid { half_extents: h } => SharedShape::cuboid(h.x, h.y, h.z),
        Shape::Capsule {
            half_height,
            radius,
        } => SharedShape::capsule_y(*half_height, *radius),
        Shape::Cylinder {
            half_height,
            radius,
        } => SharedShape::cylinder(*half_height, *radius),
        Shape::Cone {
            half_height,
            radius,
        } => SharedShape::cone(*half_height, *radius),
        Shape::ConvexHull { vertices } => {
            let pts: Vec<Vector> = vertices.iter().map(|p| v(*p)).collect();
            SharedShape::convex_hull(&pts).ok_or_else(|| {
                PhysError::invalid("the convex hull's vertices are degenerate (coplanar)")
            })?
        }
        Shape::TriMesh { vertices, indices } => {
            SharedShape::trimesh(vertices.iter().map(|p| v(*p)).collect(), indices.clone())
                .map_err(|e| PhysError::invalid(format!("a triangle mesh was refused: {e:?}")))?
        }
        Shape::HeightField {
            rows,
            cols,
            heights,
            size,
        } => {
            let (r, c) = (*rows as usize, *cols as usize);
            SharedShape::heightfield(Array2::from_fn(r, c, |i, j| heights[i * c + j]), v(*size))
        }
    })
}

/// Collects the step's collision events (rapier reports them through `&self`).
#[derive(Default)]
struct Sink(Mutex<Vec<CollisionEvent>>);

impl rapier3d_f64::pipeline::EventHandler for Sink {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: CollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
        if let Ok(mut v) = self.0.lock() {
            v.push(event);
        }
    }

    fn handle_contact_force_event(
        &self,
        _dt: f64,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &ContactPair,
        _total_force_magnitude: f64,
    ) {
    }

    fn handle_soft_body_tear_event(
        &self,
        _soft_bodies: &rapier3d_f64::prelude::SoftBodySet,
        _event: &rapier3d_f64::prelude::SoftBodyTearEvent,
    ) {
    }
}

/// See the module docs.
pub struct RapierBackend {
    frame: FrameId,
    world: rapier3d_f64::prelude::PhysicsWorld,
    bodies: Vec<Option<RigidBodyHandle>>,
    colliders: Vec<Option<ColliderHandle>>,
    joints: Vec<Option<ImpulseJointHandle>>,
    sink: Sink,
    /// Kinematic targets for the next step: (body, target position, target rotation).
    targets: Vec<(RigidBodyHandle, DVec3, DQuat)>,
    /// Scratch for the post-step tree refresh (kept: no allocation per step).
    moved: Vec<ColliderHandle>,
}

fn get<H: Copy>(v: &[Option<H>], i: u32, what: &str) -> Result<H, PhysError> {
    v.get(i as usize)
        .copied()
        .flatten()
        .ok_or_else(|| PhysError::invalid(format!("no {what} {i}")))
}

fn put<H>(v: &mut Vec<Option<H>>, i: u32, h: H) {
    let i = i as usize;
    if v.len() <= i {
        v.resize_with(i + 1, || None);
    }
    v[i] = Some(h);
}

impl RapierBackend {
    pub fn new(frame: FrameId, s: &PhysicsSettings) -> Result<Self, PhysError> {
        let mut world = rapier3d_f64::prelude::PhysicsWorld::new();
        world.gravity = v(s.gravity);
        world.integration_parameters.dt = s.dt;
        if let Some(n) = s.substeps {
            world.integration_parameters.num_solver_iterations = n as usize;
        }
        if !s.continuous {
            // rapier sweeps every fast dynamic body against fixed colliders unless its CCD
            // passes are off altogether (`ccd_enabled` only widens the sweep to moving bodies).
            world.integration_parameters.max_ccd_substeps = 0;
        }
        Ok(Self {
            frame,
            world,
            bodies: Vec::new(),
            colliders: Vec::new(),
            joints: Vec::new(),
            sink: Sink::default(),
            targets: Vec::new(),
            moved: Vec::new(),
        })
    }

    /// Put these colliders' current boxes in the query tree (rapier's `set_aabb`: the next
    /// step's broad phase takes the leaves from there).
    fn refresh(&mut self, handles: &[ColliderHandle]) {
        let params = self.world.integration_parameters;
        for &h in handles {
            if let Some(co) = self.world.colliders.get(h) {
                let aabb = co.compute_broad_phase_aabb(&params, &self.world.bodies);
                self.world.broad_phase.set_aabb(&params, h, aabb);
            }
        }
    }

    fn body_handle(&self, id: BodyId) -> Result<RigidBodyHandle, PhysError> {
        get(&self.bodies, id.0, "body")
    }

    fn id_of(c: &Collider) -> ColliderId {
        ColliderId(c.user_data as u32)
    }

    fn filter_test(&self, f: &QueryFilter, c: &Collider) -> bool {
        if c.collision_groups().memberships.bits() & f.layers == 0 {
            return false;
        }
        if c.is_sensor() && !f.include_sensors {
            return false;
        }
        if let (Some(ex), Some(parent)) = (f.exclude_body, c.parent()) {
            if self.bodies.get(ex.0 as usize).copied().flatten() == Some(parent) {
                return false;
            }
        }
        true
    }

    fn hit(&self, h: ColliderHandle, distance: f64, at: Vector, normal: Vector) -> Option<Hit> {
        let c = self.world.colliders.get(h)?;
        let body = c
            .parent()
            .and_then(|p| self.world.bodies.get(p))
            .map(|b| BodyId(b.user_data as u32))?;
        Some(Hit {
            collider: Self::id_of(c),
            body,
            distance,
            point: FramePos::new(self.frame, fv(at)),
            normal: fv(normal),
        })
    }
}

fn locked(b: &BodyDesc) -> LockedAxes {
    let mut l = LockedAxes::empty();
    let t = [
        LockedAxes::TRANSLATION_LOCKED_X,
        LockedAxes::TRANSLATION_LOCKED_Y,
        LockedAxes::TRANSLATION_LOCKED_Z,
    ];
    let r = [
        LockedAxes::ROTATION_LOCKED_X,
        LockedAxes::ROTATION_LOCKED_Y,
        LockedAxes::ROTATION_LOCKED_Z,
    ];
    for i in 0..3 {
        if b.locks.translation[i] {
            l |= t[i];
        }
        if b.locks.rotation[i] {
            l |= r[i];
        }
    }
    l
}

const AXES: [JointAxis; 6] = [
    JointAxis::LinX,
    JointAxis::LinY,
    JointAxis::LinZ,
    JointAxis::AngX,
    JointAxis::AngY,
    JointAxis::AngZ,
];

impl PhysicsBackend for RapierBackend {
    fn id(&self) -> &str {
        RAPIER
    }

    fn set_gravity(&mut self, gravity: DVec3) {
        self.world.gravity = v(gravity);
    }

    fn add_body(&mut self, id: BodyId, d: &BodyDesc) -> Result<(), PhysError> {
        let b = match d.kind {
            BodyKind::Static => RigidBodyBuilder::fixed(),
            // Velocity-based, as on avian: a kinematic body moves by its velocity, and a
            // target is the velocity that reaches it in one step (`set_kinematic_target`).
            BodyKind::Kinematic => RigidBodyBuilder::kinematic_velocity_based(),
            BodyKind::Dynamic => RigidBodyBuilder::dynamic(),
        }
        .pose(pose(d.position.local, d.rotation))
        .linvel(v(d.linear_velocity))
        .angvel(v(d.angular_velocity))
        .linear_damping(d.linear_damping)
        .angular_damping(d.angular_damping)
        .gravity_scale(d.gravity_scale)
        .ccd_enabled(d.ccd)
        .locked_axes(locked(d))
        .can_sleep(d.can_sleep)
        .user_data(u128::from(id.0));
        let h = self.world.insert_body(b.build());
        put(&mut self.bodies, id.0, h);
        Ok(())
    }

    fn remove_body(&mut self, id: BodyId) -> Result<(), PhysError> {
        let h = self.body_handle(id)?;
        self.world.remove_body(h);
        self.bodies[id.0 as usize] = None;
        Ok(())
    }

    fn add_collider(
        &mut self,
        id: ColliderId,
        body: BodyId,
        d: &ColliderDesc,
    ) -> Result<(), PhysError> {
        let parent = self.body_handle(body)?;
        let m = &d.material;
        let c = ColliderBuilder::new(shared_shape(&d.shape)?)
            .position(pose(d.offset, d.rotation))
            .friction(m.friction)
            .restitution(m.restitution)
            .density(m.density)
            .friction_combine_rule(rule(m.friction_combine))
            .restitution_combine_rule(rule(m.restitution_combine))
            .collision_groups(InteractionGroups {
                memberships: Group::from_bits_retain(d.layers.memberships),
                filter: Group::from_bits_retain(d.layers.filters),
                test_mode: InteractionTestMode::And,
            })
            .sensor(d.sensor)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .user_data(u128::from(id.0))
            .build();
        let h = self.world.insert_collider(c, Some(parent));
        put(&mut self.colliders, id.0, h);
        // Into the query tree now (a query before the next step finds it). Done here, not at
        // the first query, so whether anything queries never changes what the step sees.
        self.refresh(&[h]);
        Ok(())
    }

    fn remove_collider(&mut self, id: ColliderId) -> Result<(), PhysError> {
        let h = get(&self.colliders, id.0, "collider")?;
        self.world.remove_collider(h);
        self.colliders[id.0 as usize] = None;
        Ok(())
    }

    fn add_joint(&mut self, id: JointId, d: &JointDesc, f: &JointFrames) -> Result<(), PhysError> {
        let a = self.body_handle(d.body_a)?;
        let b = self.body_handle(d.body_b)?;
        let frames = |j: GenericJointBuilder| {
            j.local_frame1(pose(f.anchor_a, f.basis_a))
                .local_frame2(pose(f.anchor_b, f.basis_b))
                .contacts_enabled(d.collide_connected)
        };
        let lin = JointAxesMask::LIN_X | JointAxesMask::LIN_Y | JointAxesMask::LIN_Z;
        let ang = JointAxesMask::ANG_X | JointAxesMask::ANG_Y | JointAxesMask::ANG_Z;
        let joint = match d.kind {
            JointKind::Fixed => frames(GenericJointBuilder::new(lin | ang)),
            JointKind::Hinge { limits } => {
                let j = frames(GenericJointBuilder::new(lin | ang - JointAxesMask::ANG_X));
                match limits {
                    Some((lo, hi)) => j.limits(JointAxis::AngX, [lo, hi]),
                    None => j,
                }
            }
            JointKind::Slider { limits } => {
                let j = frames(GenericJointBuilder::new(lin - JointAxesMask::LIN_X | ang));
                match limits {
                    Some((lo, hi)) => j.limits(JointAxis::LinX, [lo, hi]),
                    None => j,
                }
            }
            JointKind::Spring {
                rest_length,
                stiffness,
                damping,
            } => frames(
                GenericJointBuilder::new(JointAxesMask::empty())
                    .coupled_axes(lin)
                    .motor_position(JointAxis::LinX, rest_length, stiffness, damping)
                    .motor_model(JointAxis::LinX, MotorModel::ForceBased),
            ),
            JointKind::ConeTwist { swing, twist } => frames(
                GenericJointBuilder::new(lin)
                    .coupled_axes(JointAxesMask::ANG_Y | JointAxesMask::ANG_Z)
                    .limits(JointAxis::AngY, [-swing, swing])
                    .limits(JointAxis::AngX, [twist.0, twist.1]),
            ),
            JointKind::SixDof { axes } => {
                let mut mask = JointAxesMask::empty();
                for (i, m) in axes.iter().enumerate() {
                    if *m == AxisMotion::Locked {
                        mask |= JointAxesMask::from(AXES[i]);
                    }
                }
                let mut j = frames(GenericJointBuilder::new(mask));
                for (i, m) in axes.iter().enumerate() {
                    if let AxisMotion::Limited { min, max } = *m {
                        j = j.limits(AXES[i], [min, max]);
                    }
                }
                j
            }
        };
        let h = self.world.insert_impulse_joint(a, b, joint.build());
        put(&mut self.joints, id.0, h);
        Ok(())
    }

    fn remove_joint(&mut self, id: JointId) -> Result<(), PhysError> {
        let h = get(&self.joints, id.0, "joint")?;
        self.world.remove_impulse_joint(h);
        self.joints[id.0 as usize] = None;
        Ok(())
    }

    fn state(&self, id: BodyId) -> Result<BodyState, PhysError> {
        let b = self
            .world
            .bodies
            .get(self.body_handle(id)?)
            .ok_or_else(|| PhysError::Backend(format!("rapier lost body {}", id.0)))?;
        Ok(BodyState {
            position: FramePos::new(self.frame, fv(b.translation())),
            rotation: fq(*b.rotation()),
            linear_velocity: fv(b.linvel()),
            angular_velocity: fv(b.angvel()),
        })
    }

    fn set_state(&mut self, id: BodyId, s: &BodyState) -> Result<(), PhysError> {
        let h = self.body_handle(id)?;
        let b = self
            .world
            .bodies
            .get_mut(h)
            .ok_or_else(|| PhysError::Backend(format!("rapier lost body {}", id.0)))?;
        b.set_position(pose(s.position.local, s.rotation), true);
        b.set_linvel(v(s.linear_velocity), true);
        b.set_angvel(v(s.angular_velocity), true);
        let moved: Vec<ColliderHandle> = b.colliders().to_vec();
        // The colliders follow the body at the next step; queries before it see them here.
        let pose_now = *b.position();
        for c in &moved {
            if let Some(co) = self.world.colliders.get_mut(*c) {
                let rel = co.position_wrt_parent().copied().unwrap_or(Pose::IDENTITY);
                co.set_position(pose_now * rel);
            }
        }
        self.refresh(&moved);
        Ok(())
    }

    fn set_velocity(&mut self, id: BodyId, linear: DVec3, angular: DVec3) -> Result<(), PhysError> {
        let h = self.body_handle(id)?;
        if let Some(b) = self.world.bodies.get_mut(h) {
            b.set_linvel(v(linear), true);
            b.set_angvel(v(angular), true);
        }
        Ok(())
    }

    fn apply_impulse(
        &mut self,
        id: BodyId,
        linear: DVec3,
        angular: DVec3,
    ) -> Result<(), PhysError> {
        let h = self.body_handle(id)?;
        if let Some(b) = self.world.bodies.get_mut(h) {
            b.apply_impulse(v(linear), true);
            b.apply_torque_impulse(v(angular), true);
        }
        Ok(())
    }

    fn set_kinematic_target(
        &mut self,
        id: BodyId,
        target: FramePos,
        rotation: DQuat,
    ) -> Result<(), PhysError> {
        let h = self.body_handle(id)?;
        self.targets.retain(|t| t.0 != h);
        self.targets.push((h, target.local, rotation));
        Ok(())
    }

    fn mass(&self, id: BodyId) -> Result<f64, PhysError> {
        let b = self
            .world
            .bodies
            .get(self.body_handle(id)?)
            .ok_or_else(|| PhysError::Backend(format!("rapier lost body {}", id.0)))?;
        Ok(if b.is_dynamic() { b.mass() } else { 0.0 })
    }

    fn is_sleeping(&self, id: BodyId) -> Result<bool, PhysError> {
        Ok(self
            .world
            .bodies
            .get(self.body_handle(id)?)
            .is_some_and(|b| b.is_sleeping()))
    }

    fn step(&mut self, dt: f64, events: &mut Vec<PhysEvent>) -> Result<(), PhysError> {
        self.world.integration_parameters.dt = dt;
        let targets = std::mem::take(&mut self.targets);
        for &(h, at, rot) in &targets {
            if let Some(b) = self.world.bodies.get_mut(h) {
                let (lin, ang) =
                    crate::world::velocity_to(fv(b.translation()), fq(*b.rotation()), at, rot, dt);
                b.set_linvel(v(lin), true);
                b.set_angvel(v(ang), true);
            }
        }
        self.world.step_with_events(&(), &self.sink);
        for &(h, at, rot) in &targets {
            if let Some(b) = self.world.bodies.get_mut(h) {
                b.set_position(pose(at, rot), true);
                b.set_linvel(Vector::ZERO, true);
                b.set_angvel(Vector::ZERO, true);
            }
        }
        self.targets = targets;
        self.targets.clear();
        // rapier fits its query tree at the start of a step: bring the leaves of everything
        // that moved to where it is now, so a query after the step sees the step's result.
        // Every step, whether or not anything queries, so querying never changes the tree
        // the next step starts from (a replay does not depend on what was asked).
        let mut moved = std::mem::take(&mut self.moved);
        moved.clear();
        for h in self.world.islands.active_bodies() {
            if let Some(b) = self.world.bodies.get(h)
                && !b.is_fixed()
            {
                moved.extend_from_slice(b.colliders());
            }
        }
        self.refresh(&moved);
        self.moved = moved;
        let mut raw = self
            .sink
            .0
            .lock()
            .map_err(|_| PhysError::Backend("the event sink was poisoned".into()))?;
        for e in raw.drain(..) {
            let (h1, h2, started) = match e {
                CollisionEvent::Started(a, b, _) => (a, b, true),
                CollisionEvent::Stopped(a, b, _) => (a, b, false),
            };
            let (Some(c1), Some(c2)) = (self.world.colliders.get(h1), self.world.colliders.get(h2))
            else {
                // A collider removed this step: its contacts end silently (as on avian).
                continue;
            };
            let (i1, i2) = (Self::id_of(c1), Self::id_of(c2));
            events.push(match (c1.is_sensor(), c2.is_sensor(), started) {
                (false, false, true) => PhysEvent::ContactBegin(i1, i2),
                (false, false, false) => PhysEvent::ContactEnd(i1, i2),
                (true, _, true) => PhysEvent::TriggerEnter {
                    trigger: i1,
                    other: i2,
                },
                (true, _, false) => PhysEvent::TriggerExit {
                    trigger: i1,
                    other: i2,
                },
                (false, true, true) => PhysEvent::TriggerEnter {
                    trigger: i2,
                    other: i1,
                },
                (false, true, false) => PhysEvent::TriggerExit {
                    trigger: i2,
                    other: i1,
                },
            });
        }
        Ok(())
    }

    fn cast_ray(&self, r: &Ray, f: &QueryFilter) -> Result<Option<Hit>, PhysError> {
        let pred = |_h: ColliderHandle, c: &Collider| self.filter_test(f, c);
        let filter = rapier3d_f64::prelude::QueryFilter::new().predicate(&pred);
        let ray = rapier3d_f64::prelude::Ray::new(v(r.start.local), v(r.direction));
        Ok(self
            .world
            .cast_ray_and_get_normal(&ray, r.max_distance, true, filter)
            .and_then(|(h, i)| {
                self.hit(
                    h,
                    i.time_of_impact,
                    ray.point_at(i.time_of_impact),
                    i.normal,
                )
            }))
    }

    fn cast_shape(&self, c: &ShapeCast, f: &QueryFilter) -> Result<Option<Hit>, PhysError> {
        let shape = shared_shape(&c.shape)?;
        let pred = |_h: ColliderHandle, co: &Collider| self.filter_test(f, co);
        let filter = rapier3d_f64::prelude::QueryFilter::new().predicate(&pred);
        let opts = ShapeCastOptions {
            max_time_of_impact: c.max_distance,
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        };
        Ok(self
            .world
            .cast_shape(
                &pose(c.start.local, c.rotation),
                v(c.direction),
                shape.as_ref(),
                opts,
                filter,
            )
            .and_then(|(h, hit)| self.hit(h, hit.time_of_impact, hit.witness1, hit.normal1)))
    }

    fn overlap(
        &self,
        o: &Overlap,
        f: &QueryFilter,
        out: &mut Vec<ColliderId>,
    ) -> Result<(), PhysError> {
        let shape = shared_shape(&o.shape)?;
        let pred = |_h: ColliderHandle, c: &Collider| self.filter_test(f, c);
        let filter = rapier3d_f64::prelude::QueryFilter::new().predicate(&pred);
        out.extend(
            self.world
                .intersect_shape(pose(o.position.local, o.rotation), shape.as_ref(), filter)
                .map(|(_, c)| Self::id_of(c)),
        );
        Ok(())
    }

    fn contact_count(&self) -> usize {
        self.world
            .narrow_phase
            .contact_pairs()
            .filter(|p| p.has_any_active_contact())
            .count()
    }
}
