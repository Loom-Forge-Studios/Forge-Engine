//! The avian3d backend (`avian3d`, E-5, the default): avian's `f64` build with parry-f64
//! collision, the XPBD joint solver and `enhanced-determinism`.
//!
//! **A private Bevy world, stepped by hand.** avian is a set of Bevy plugins. This backend
//! builds them once into its own `World` (never the engine's ECS: Ch.5.1 keeps the framework
//! out of the engine, and a region's physics is its own world, Ch.17) with a schedule of its
//! own, and runs that schedule once per fixed step with the step's `dt` — no app loop, no
//! clock, no Transform sync (avian's `Position`/`Rotation` are the `f64` truth; Bevy's `f32`
//! `Transform` is never written), no interpolation plugin (the world above does it for every
//! backend), no async tasks (the collider-tree optimiser runs inline: same bits every run).
//!
//! **Colliders** are child entities of their body with an exact `f64` `ColliderTransform`;
//! avian moves them with the body at the start of each step. Queries derive a collider's
//! pose from its body's current pose, so a query right after a step sees where things are.
//!
//! **Queries** walk avian's collider trees (read-only, so a batch can run on several threads)
//! and compute every hit in `f64`; the trees themselves are `f32` and only prune (see
//! [`narrow`]).
//!
//! **Joints.** avian 0.7 has fixed, revolute, prismatic, distance and spherical joints and no
//! generic 6DOF joint. The world's joint kinds map onto them; a 6DOF joint whose axes one of
//! them expresses (all locked, one free angular axis, one free linear axis, a free or
//! twist-limited ball, all free) is built as that joint, and any other combination is refused
//! with `PHYS-0004` naming the rapier3d backend, which has a generic joint (ADR 0064).

mod narrow;

use std::time::Duration;

use avian3d::collider_tree::{ColliderTreeOptimization, ColliderTrees};
use avian3d::math::{Quaternion, Scalar, Vector};
use avian3d::parry::query::ShapeCastOptions;
use avian3d::parry::shape::SharedShape;
use avian3d::parry::utils::Array2;
use avian3d::prelude::*;
use bevy_app::{App, Plugin, PluginGroup};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::hierarchy::ChildOf;
use bevy_ecs::message::Messages;
use bevy_ecs::schedule::ScheduleLabel;
use bevy_ecs::world::World;
use bevy_transform::components::GlobalTransform;
use forge_frames::{DQuat, DVec3, FrameId, FramePos};

use crate::backend::{AVIAN, PhysicsBackend};
use crate::types::{
    AxisMotion, BodyDesc, BodyId, BodyKind, BodyState, ColliderDesc, ColliderId, CombineRule,
    Hit, JointDesc, JointFrames, JointId, JointKind, Overlap, PhysEvent, QueryFilter, Ray, Shape,
    ShapeCast,
};
use crate::{PhysError, PhysicsSettings};

/// The schedule one fixed step runs.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct ForgeStep;

/// Our id on a body entity.
#[derive(Component, Clone, Copy)]
struct BodyTag(u32);

/// Our id on a collider entity.
#[derive(Component, Clone, Copy)]
struct ColliderTag(u32);

fn v(a: DVec3) -> Vector {
    Vector::new(a.x, a.y, a.z)
}

fn fv(a: Vector) -> DVec3 {
    DVec3::new(a.x, a.y, a.z)
}

fn q(a: DQuat) -> Quaternion {
    Quaternion::from_xyzw(a.x, a.y, a.z, a.w)
}

fn fq(a: Quaternion) -> DQuat {
    DQuat::from_xyzw(a.x, a.y, a.z, a.w)
}

fn rule(r: CombineRule) -> CoefficientCombine {
    match r {
        CombineRule::Average => CoefficientCombine::Average,
        CombineRule::Min => CoefficientCombine::Min,
        CombineRule::Multiply => CoefficientCombine::Multiply,
        CombineRule::Max => CoefficientCombine::Max,
    }
}

/// The parry shape of a [`Shape`] (checked by the world first).
fn shared_shape(s: &Shape) -> Result<SharedShape, PhysError> {
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
        Shape::TriMesh { vertices, indices } => SharedShape::trimesh(
            vertices.iter().map(|p| v(*p)).collect(),
            indices.clone(),
        )
        .map_err(|e| PhysError::invalid(format!("a triangle mesh was refused: {e:?}")))?,
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

/// What a 6DOF joint is on avian (see the module docs).
enum SixDof {
    Nothing,
    Fixed,
    Hinge(Option<(f64, f64)>),
    Slider(Option<(f64, f64)>),
    Ball(Option<(f64, f64)>),
}

fn six_dof(axes: &[AxisMotion; 6]) -> Result<SixDof, PhysError> {
    use AxisMotion::{Free, Limited, Locked};
    let lim = |m: AxisMotion| match m {
        Limited { min, max } => Some((min, max)),
        _ => None,
    };
    let (l, a) = (&axes[..3], &axes[3..]);
    let all = |s: &[AxisMotion], m: AxisMotion| s.iter().all(|x| *x == m);
    let free_or_limited = |m: AxisMotion| matches!(m, Free | Limited { .. });
    if all(axes, Free) {
        return Ok(SixDof::Nothing);
    }
    if all(l, Locked) {
        if all(a, Locked) {
            return Ok(SixDof::Fixed);
        }
        if a[1] == Locked && a[2] == Locked && free_or_limited(a[0]) {
            return Ok(SixDof::Hinge(lim(a[0])));
        }
        if a[1] == Free && a[2] == Free && free_or_limited(a[0]) {
            return Ok(SixDof::Ball(lim(a[0])));
        }
    }
    if l[1] == Locked && l[2] == Locked && free_or_limited(l[0]) && all(a, Locked) {
        return Ok(SixDof::Slider(lim(l[0])));
    }
    Err(PhysError::Unsupported(format!(
        "avian3d has no generic 6DOF joint: these axes {axes:?} are not a fixed, hinge, slider or ball joint; use the rapier3d backend (physics.backend = \"rapier3d\") for any combination"
    )))
}

/// See the module docs.
pub struct AvianBackend {
    frame: FrameId,
    world: World,
    bodies: Vec<Option<Entity>>,
    colliders: Vec<Option<Entity>>,
    joints: Vec<Option<Entity>>,
    /// Kinematic targets for the next step: (body, target position, target rotation).
    targets: Vec<(Entity, DVec3, DQuat)>,
    /// Bodies or colliders were added since the last step (mass properties are stale).
    dirty: bool,
    starts: Vec<CollisionStart>,
    ends: Vec<CollisionEnd>,
    /// `PhysicsSettings::continuous`.
    continuous: bool,
}

fn get(v: &[Option<Entity>], i: u32, what: &str) -> Result<Entity, PhysError> {
    v.get(i as usize)
        .copied()
        .flatten()
        .ok_or_else(|| PhysError::invalid(format!("no {what} {i}")))
}

fn put(v: &mut Vec<Option<Entity>>, i: u32, e: Entity) {
    let i = i as usize;
    if v.len() <= i {
        v.resize_with(i + 1, || None);
    }
    v[i] = Some(e);
}

/// Configures avian for a region world stepped by hand (see the module docs).
struct Setup {
    gravity: Vector,
    substeps: Option<u32>,
}

impl Plugin for Setup {
    fn build(&self, app: &mut App) {
        app.insert_resource(avian3d::physics_transform::PhysicsTransformConfig {
            propagate_before_physics: false,
            transform_to_position: false,
            position_to_transform: false,
            transform_to_collider_scale: false,
        })
        .insert_resource(ColliderTreeOptimization {
            use_async_tasks: false,
            ..Default::default()
        })
        .insert_resource(bevy_time::Time::<()>::default())
        .insert_resource(Gravity(self.gravity));
        if let Some(n) = self.substeps {
            app.insert_resource(SubstepCount(n));
        }
    }
}

impl AvianBackend {
    pub fn new(frame: FrameId, s: &PhysicsSettings) -> Result<Self, PhysError> {
        // avian's broad phase and tree optimiser reach for bevy's task pools; without bevy's
        // multi-threaded feature they run inline on this thread.
        let _ = bevy_tasks::ComputeTaskPool::get_or_init(bevy_tasks::TaskPool::default);
        let _ = bevy_tasks::AsyncComputeTaskPool::get_or_init(bevy_tasks::TaskPool::default);
        let mut app = App::new();
        app.add_plugins(Setup {
            gravity: v(s.gravity),
            substeps: s.substeps,
        });
        app.add_plugins(
            PhysicsPlugins::new(ForgeStep)
                .build()
                .disable::<PhysicsInterpolationPlugin>(),
        );
        app.finish();
        app.cleanup();
        let world = std::mem::take(app.world_mut());
        let mut b = Self {
            frame,
            world,
            bodies: Vec::new(),
            colliders: Vec::new(),
            joints: Vec::new(),
            targets: Vec::new(),
            dirty: false,
            starts: Vec::new(),
            ends: Vec::new(),
            continuous: s.continuous,
        };
        // One pass with no time: every system initialised, no simulation.
        b.run(Duration::ZERO)?;
        Ok(b)
    }

    fn run(&mut self, dt: Duration) -> Result<(), PhysError> {
        self.world
            .resource_mut::<bevy_time::Time>()
            .advance_by(dt);
        self.world
            .try_run_schedule(ForgeStep)
            .map_err(|e| PhysError::Backend(format!("avian's step schedule: {e}")))?;
        self.world.flush();
        Ok(())
    }

    /// Bring mass properties up to date after bodies or colliders were added (a pass with no
    /// time).
    fn settle(&mut self) -> Result<(), PhysError> {
        if self.dirty {
            self.run(Duration::ZERO)?;
            self.dirty = false;
        }
        Ok(())
    }

    fn body(&self, id: BodyId) -> Result<Entity, PhysError> {
        get(&self.bodies, id.0, "body")
    }

    fn pose_of(&self, e: Entity) -> Option<(Vector, Quaternion)> {
        Some((
            self.world.get::<Position>(e)?.0,
            self.world.get::<Rotation>(e)?.0,
        ))
    }

    /// A collider's current world pose: its body's pose after its offset.
    fn collider_pose(&self, e: Entity) -> Option<(Vector, Quaternion)> {
        let body = self.world.get::<ColliderOf>(e)?.body;
        let (p, r) = self.pose_of(body)?;
        let t = self.world.get::<ColliderTransform>(e)?;
        Some((p + r * t.translation, (r * t.rotation.0).normalize()))
    }

    /// Whether the collider entity `e` passes `f`; its id and body id if so.
    fn accept(&self, e: Entity, f: &QueryFilter) -> Option<(ColliderId, BodyId)> {
        let id = ColliderId(self.world.get::<ColliderTag>(e)?.0);
        let layers = self.world.get::<CollisionLayers>(e)?;
        if layers.memberships.0 & f.layers == 0 {
            return None;
        }
        if !f.include_sensors && self.world.get::<Sensor>(e).is_some() {
            return None;
        }
        let body_e = self.world.get::<ColliderOf>(e)?.body;
        let body = BodyId(self.world.get::<BodyTag>(body_e)?.0);
        if f.exclude_body == Some(body) {
            return None;
        }
        Some((id, body))
    }

    fn hit(&self, c: ColliderId, b: BodyId, d: Scalar, at: Vector, n: Vector) -> Hit {
        Hit {
            collider: c,
            body: b,
            distance: d,
            point: FramePos::new(self.frame, fv(at)),
            normal: fv(n),
        }
    }

    fn joint_frame(anchor: DVec3, basis: DQuat) -> JointFrame {
        JointFrame {
            anchor: JointAnchor::Local(v(anchor)),
            basis: JointBasis::Local(q(basis)),
        }
    }
}

fn pose(p: Vector, r: Quaternion) -> avian3d::parry::math::Pose {
    avian3d::parry::math::Pose::from_parts(p, r)
}

impl PhysicsBackend for AvianBackend {
    fn id(&self) -> &str {
        AVIAN
    }

    fn set_gravity(&mut self, gravity: DVec3) {
        self.world.insert_resource(Gravity(v(gravity)));
    }

    fn add_body(&mut self, id: BodyId, d: &BodyDesc) -> Result<(), PhysError> {
        let kind = match d.kind {
            BodyKind::Static => RigidBody::Static,
            BodyKind::Kinematic => RigidBody::Kinematic,
            BodyKind::Dynamic => RigidBody::Dynamic,
        };
        let mut locks = LockedAxes::new();
        let [tx, ty, tz] = d.locks.translation;
        let [rx, ry, rz] = d.locks.rotation;
        if tx {
            locks = locks.lock_translation_x();
        }
        if ty {
            locks = locks.lock_translation_y();
        }
        if tz {
            locks = locks.lock_translation_z();
        }
        if rx {
            locks = locks.lock_rotation_x();
        }
        if ry {
            locks = locks.lock_rotation_y();
        }
        if rz {
            locks = locks.lock_rotation_z();
        }
        let mut e = self.world.spawn((
            kind,
            Position(v(d.position.local)),
            Rotation(q(d.rotation)),
            LinearVelocity(v(d.linear_velocity)),
            AngularVelocity(v(d.angular_velocity)),
            LinearDamping(d.linear_damping),
            AngularDamping(d.angular_damping),
            GravityScale(d.gravity_scale),
            locks,
            BodyTag(id.0),
            // avian links a collider to its body in `ColliderOf`'s insert hook, which reads
            // both entities' `GlobalTransform`s. Identity (exact in any precision) on every
            // entity, and no `Transform` anywhere: nothing propagates, and the collider's
            // real `f64` offset is written afterwards (`add_collider`).
            GlobalTransform::IDENTITY,
        ));
        if !self.continuous {
            // No sweeping at all: avian's unbounded speculative margin is its linear CCD.
            e.insert(SpeculativeMargin(0.0));
        } else if d.ccd {
            e.insert(SweptCcd::default());
        }
        if !d.can_sleep {
            e.insert(SleepingDisabled);
        }
        let e = e.id();
        put(&mut self.bodies, id.0, e);
        self.world.flush();
        self.dirty = true;
        Ok(())
    }

    fn remove_body(&mut self, id: BodyId) -> Result<(), PhysError> {
        let e = self.body(id)?;
        self.world.despawn(e);
        self.world.flush();
        self.bodies[id.0 as usize] = None;
        self.targets.retain(|t| t.0 != e);
        Ok(())
    }

    fn add_collider(
        &mut self,
        id: ColliderId,
        body: BodyId,
        d: &ColliderDesc,
    ) -> Result<(), PhysError> {
        let parent = self.body(body)?;
        let collider = Collider::from(shared_shape(&d.shape)?);
        let (bp, br) = self
            .pose_of(parent)
            .ok_or_else(|| PhysError::Backend(format!("avian lost body {}", body.0)))?;
        let offset = v(d.offset);
        let rot = q(d.rotation);
        let (cp, cr) = (bp + br * offset, (br * rot).normalize());
        // The tree takes the collider in when it gets its body (an observer on `ColliderOf`):
        // give it its real box now, so a query before the first step finds it.
        let aabb = collider.aabb(cp, Rotation(cr));
        let m = &d.material;
        let parts = (
            collider,
            aabb,
            avian3d::collision::collider::EnlargedAabb::new(aabb),
            Friction {
                dynamic_coefficient: m.friction,
                static_coefficient: m.friction,
                combine_rule: rule(m.friction_combine),
            },
            Restitution {
                coefficient: m.restitution,
                combine_rule: rule(m.restitution_combine),
            },
            narrow::density(m.density),
            CollisionLayers::from_bits(d.layers.memberships, d.layers.filters),
            CollisionEventsEnabled,
            ColliderTag(id.0),
        );
        // A body's first collider at its origin lives on the body entity itself (avian's
        // swept CCD sees only that one, and it is one entity fewer); others are children.
        let on_body = d.offset == DVec3::ZERO
            && d.rotation == DQuat::IDENTITY
            && self.world.get::<Collider>(parent).is_none();
        let e = if on_body {
            let mut e = self
                .world
                .get_entity_mut(parent)
                .map_err(|_| PhysError::Backend(format!("avian lost body {}", body.0)))?;
            e.insert(parts);
            if d.sensor {
                e.insert(Sensor);
            }
            e.insert(ColliderOf { body: parent });
            parent
        } else {
            let mut e = self.world.spawn((parts, Position(cp), Rotation(cr)));
            if d.sensor {
                e.insert(Sensor);
            }
            e.insert((GlobalTransform::IDENTITY, ChildOf(parent)));
            e.insert(ColliderOf { body: parent });
            e.id()
        };
        // Every hook and observer has run: now the exact offset (the link above set identity).
        self.world.flush();
        if let Ok(mut ent) = self.world.get_entity_mut(e) {
            ent.insert(ColliderTransform {
                translation: offset,
                rotation: Rotation(rot),
                scale: Vector::ONE,
            });
        }
        put(&mut self.colliders, id.0, e);
        self.world.flush();
        self.dirty = true;
        Ok(())
    }

    fn remove_collider(&mut self, id: ColliderId) -> Result<(), PhysError> {
        let e = get(&self.colliders, id.0, "collider")?;
        if self.world.get::<BodyTag>(e).is_some() {
            // The collider on the body entity: take it off, keep the body.
            if let Ok(mut ent) = self.world.get_entity_mut(e) {
                ent.remove::<(
                    ColliderOf,
                    Collider,
                    ColliderTag,
                    Sensor,
                    CollisionEventsEnabled,
                    Friction,
                    Restitution,
                    CollisionLayers,
                )>();
            }
        } else {
            self.world.despawn(e);
        }
        self.world.flush();
        self.colliders[id.0 as usize] = None;
        self.dirty = true;
        Ok(())
    }

    fn add_joint(
        &mut self,
        id: JointId,
        d: &JointDesc,
        f: &JointFrames,
    ) -> Result<(), PhysError> {
        let a = self.body(d.body_a)?;
        let b = self.body(d.body_b)?;
        let f1 = Self::joint_frame(f.anchor_a, f.basis_a);
        let f2 = Self::joint_frame(f.anchor_b, f.basis_b);
        let fixed = || {
            let mut j = FixedJoint::new(a, b);
            j.frame1 = f1;
            j.frame2 = f2;
            j
        };
        let hinge = |limits: Option<(f64, f64)>| {
            let mut j = RevoluteJoint::new(a, b).with_hinge_axis(Vector::X);
            j.frame1 = f1;
            j.frame2 = f2;
            j.angle_limit = limits.map(|(lo, hi)| AngleLimit::new(lo, hi));
            j
        };
        let slider = |limits: Option<(f64, f64)>| {
            let mut j = PrismaticJoint::new(a, b).with_slider_axis(Vector::X);
            j.frame1 = f1;
            j.frame2 = f2;
            j.limits = limits.map(|(lo, hi)| DistanceLimit::new(lo, hi));
            j
        };
        // avian's spherical joint limits the cone about `twist_axis.any_orthonormal_vector()`
        // (its XPBD swing limit aligns those vectors; its twist limit turns about them). With
        // `twist_axis = Y` that is -Z: turn the frames so avian's -Z is the joint axis (X).
        let cone = q(DQuat::from_axis_angle(DVec3::Y, -std::f64::consts::FRAC_PI_2));
        let ball = |swing: Option<f64>, twist: Option<(f64, f64)>| {
            let mut j = SphericalJoint::new(a, b).with_twist_axis(Vector::Y);
            j.frame1 = Self::joint_frame(f.anchor_a, fq(q(f.basis_a) * cone));
            j.frame2 = Self::joint_frame(f.anchor_b, fq(q(f.basis_b) * cone));
            j.swing_limit = swing.map(|s| AngleLimit::new(-s, s));
            j.twist_limit = twist.map(|(lo, hi)| AngleLimit::new(lo, hi));
            j
        };
        let mut e = match d.kind {
            JointKind::Fixed => self.world.spawn(fixed()),
            JointKind::Hinge { limits } => self.world.spawn(hinge(limits)),
            JointKind::Slider { limits } => self.world.spawn(slider(limits)),
            JointKind::ConeTwist { swing, twist } => {
                self.world.spawn(ball(Some(swing), Some(twist)))
            }
            JointKind::Spring {
                rest_length,
                stiffness,
                damping,
            } => {
                self.settle()?;
                let inv = |e: Entity| {
                    self.world
                        .get::<ComputedMass>(e)
                        .map_or(0.0, |m| m.inverse())
                };
                let (wa, wb) = (inv(a), inv(b));
                let j = DistanceJoint::new(a, b)
                    .with_local_anchor1(v(f.anchor_a))
                    .with_local_anchor2(v(f.anchor_b))
                    .with_limits(rest_length, rest_length)
                    .with_compliance(1.0 / stiffness);
                // avian damps the relative velocity by `linear dt` per step as an impulse
                // over the reduced mass: `linear = c (1/m_a + 1/m_b)` is a damper of `c`.
                self.world.spawn((
                    j,
                    JointDamping {
                        linear: damping * (wa + wb),
                        angular: 0.0,
                    },
                ))
            }
            JointKind::SixDof { axes } => match six_dof(&axes)? {
                SixDof::Nothing => self.world.spawn(()),
                SixDof::Fixed => self.world.spawn(fixed()),
                SixDof::Hinge(l) => self.world.spawn(hinge(l)),
                SixDof::Slider(l) => self.world.spawn(slider(l)),
                SixDof::Ball(twist) => self.world.spawn(ball(None, twist)),
            },
        };
        if !d.collide_connected {
            e.insert(JointCollisionDisabled);
        }
        let e = e.id();
        put(&mut self.joints, id.0, e);
        self.world.flush();
        Ok(())
    }

    fn remove_joint(&mut self, id: JointId) -> Result<(), PhysError> {
        let e = get(&self.joints, id.0, "joint")?;
        self.world.despawn(e);
        self.world.flush();
        self.joints[id.0 as usize] = None;
        Ok(())
    }

    fn state(&self, id: BodyId) -> Result<BodyState, PhysError> {
        let e = self.body(id)?;
        let lost = || PhysError::Backend(format!("avian lost body {}", id.0));
        let (p, r) = self.pose_of(e).ok_or_else(lost)?;
        let lin = self.world.get::<LinearVelocity>(e).ok_or_else(lost)?.0;
        let ang = self.world.get::<AngularVelocity>(e).ok_or_else(lost)?.0;
        Ok(BodyState {
            position: FramePos::new(self.frame, fv(p)),
            rotation: fq(r),
            linear_velocity: fv(lin),
            angular_velocity: fv(ang),
        })
    }

    fn set_state(&mut self, id: BodyId, s: &BodyState) -> Result<(), PhysError> {
        let e = self.body(id)?;
        let mut ent = self
            .world
            .get_entity_mut(e)
            .map_err(|_| PhysError::Backend(format!("avian lost body {}", id.0)))?;
        ent.insert((
            Position(v(s.position.local)),
            Rotation(q(s.rotation)),
            LinearVelocity(v(s.linear_velocity)),
            AngularVelocity(v(s.angular_velocity)),
        ));
        ent.remove::<Sleeping>();
        self.world.flush();
        self.dirty = true;
        Ok(())
    }

    fn set_velocity(
        &mut self,
        id: BodyId,
        linear: DVec3,
        angular: DVec3,
    ) -> Result<(), PhysError> {
        let e = self.body(id)?;
        let mut ent = self
            .world
            .get_entity_mut(e)
            .map_err(|_| PhysError::Backend(format!("avian lost body {}", id.0)))?;
        ent.insert((LinearVelocity(v(linear)), AngularVelocity(v(angular))));
        ent.remove::<Sleeping>();
        self.world.flush();
        Ok(())
    }

    fn apply_impulse(
        &mut self,
        id: BodyId,
        linear: DVec3,
        angular: DVec3,
    ) -> Result<(), PhysError> {
        // The mass properties must be current for the impulse's velocity change.
        self.settle()?;
        let e = self.body(id)?;
        let mut forces = self.world.query::<Forces>();
        let mut f = forces
            .get_mut(&mut self.world, e)
            .map_err(|_| PhysError::Backend(format!("avian lost body {}", id.0)))?;
        f.apply_linear_impulse(v(linear));
        f.apply_angular_impulse(v(angular));
        self.world.flush();
        Ok(())
    }

    fn set_kinematic_target(
        &mut self,
        id: BodyId,
        target: FramePos,
        rotation: DQuat,
    ) -> Result<(), PhysError> {
        let e = self.body(id)?;
        self.targets.retain(|t| t.0 != e);
        self.targets.push((e, target.local, rotation));
        Ok(())
    }

    fn mass(&self, id: BodyId) -> Result<f64, PhysError> {
        let e = self.body(id)?;
        let dynamic = self.world.get::<RigidBody>(e) == Some(&RigidBody::Dynamic);
        Ok(if dynamic {
            self.world.get::<ComputedMass>(e).map_or(0.0, |m| m.value())
        } else {
            0.0
        })
    }

    fn is_sleeping(&self, id: BodyId) -> Result<bool, PhysError> {
        Ok(self.world.get::<Sleeping>(self.body(id)?).is_some())
    }

    fn sync(&mut self) -> Result<(), PhysError> {
        self.settle()
    }

    fn step(&mut self, dt: f64, events: &mut Vec<PhysEvent>) -> Result<(), PhysError> {
        // Kinematic targets: the velocity that reaches the pose in one step (so what rides
        // on the body is carried), then the exact pose once the step is done.
        let targets = std::mem::take(&mut self.targets);
        for &(e, at, rot) in &targets {
            let Some((p, r)) = self.pose_of(e) else {
                continue;
            };
            let (lin, ang) = crate::world::velocity_to(fv(p), fq(r), at, rot, dt);
            if let Ok(mut ent) = self.world.get_entity_mut(e) {
                ent.insert((LinearVelocity(v(lin)), AngularVelocity(v(ang))));
            }
        }
        self.world.flush();
        self.run(Duration::from_secs_f64(dt))?;
        self.dirty = false;
        for (e, at, rot) in targets {
            if let Ok(mut ent) = self.world.get_entity_mut(e) {
                ent.insert((
                    Position(v(at)),
                    Rotation(q(rot)),
                    LinearVelocity(Vector::ZERO),
                    AngularVelocity(Vector::ZERO),
                ));
            }
        }
        self.world.flush();
        self.starts.clear();
        self.ends.clear();
        self.starts
            .extend(self.world.resource_mut::<Messages<CollisionStart>>().drain());
        self.ends
            .extend(self.world.resource_mut::<Messages<CollisionEnd>>().drain());
        let tag = |w: &World, e: Entity| w.get::<ColliderTag>(e).map(|t| ColliderId(t.0));
        let sensor = |w: &World, e: Entity| w.get::<Sensor>(e).is_some();
        let pairs = self
            .starts
            .iter()
            .map(|s| (s.collider1, s.collider2, true))
            .chain(self.ends.iter().map(|s| (s.collider1, s.collider2, false)));
        for (e1, e2, started) in pairs {
            // A despawned collider's contacts end silently (as on rapier).
            let (Some(i1), Some(i2)) = (tag(&self.world, e1), tag(&self.world, e2)) else {
                continue;
            };
            let (s1, s2) = (sensor(&self.world, e1), sensor(&self.world, e2));
            events.push(match (s1, s2, started) {
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
        let (start, dir) = (v(r.start.local), v(r.direction));
        let Some(ray) = narrow::ray(start, dir) else {
            return Ok(None);
        };
        let mut best: Option<(Scalar, ColliderId, BodyId, Vector)> = None;
        for tree in self.world.resource::<ColliderTrees>().iter_trees() {
            let mut max = best.map_or(r.max_distance, |b| b.0);
            tree.ray_traverse_closest(ray, max, |pid| {
                let Some(proxy) = tree.get_proxy(pid) else {
                    return Scalar::MAX;
                };
                let e = proxy.collider;
                let (Some((c, b)), Some((p, rot)), Some(col)) = (
                    self.accept(e, f),
                    self.collider_pose(e),
                    self.world.get::<Collider>(e),
                ) else {
                    return Scalar::MAX;
                };
                match col.cast_ray(p, Rotation(rot), start, dir, max, true) {
                    Some((d, n)) if d <= max => {
                        if best.is_none_or(|x| d < x.0 || (d == x.0 && c < x.1)) {
                            best = Some((d, c, b, n));
                        }
                        max = d;
                        d
                    }
                    _ => Scalar::MAX,
                }
            });
        }
        Ok(best.map(|(d, c, b, n)| self.hit(c, b, d, start + dir * d, n)))
    }

    fn cast_shape(&self, c: &ShapeCast, f: &QueryFilter) -> Result<Option<Hit>, PhysError> {
        let shape = Collider::from(shared_shape(&c.shape)?);
        let (start, rot, dir) = (v(c.start.local), q(c.rotation), v(c.direction));
        let Some(sweep_dir) = narrow::dir(dir) else {
            return Ok(None);
        };
        let aabb = shape.aabb(start, Rotation(rot));
        let pose2 = pose(start, rot);
        let mut best: Option<(Scalar, ColliderId, BodyId, Vector, Vector)> = None;
        for tree in self.world.resource::<ColliderTrees>().iter_trees() {
            let mut max = best.map_or(c.max_distance, |b| b.0);
            tree.sweep_traverse_closest(aabb.into(), sweep_dir, max, 0.0, |pid| {
                let Some(proxy) = tree.get_proxy(pid) else {
                    return Scalar::MAX;
                };
                let e = proxy.collider;
                let (Some((cid, b)), Some((p, r)), Some(col)) = (
                    self.accept(e, f),
                    self.collider_pose(e),
                    self.world.get::<Collider>(e),
                ) else {
                    return Scalar::MAX;
                };
                let pose1 = pose(p, r);
                let hit = avian3d::parry::query::cast_shapes(
                    &pose1,
                    Vector::ZERO,
                    col.shape_scaled().as_ref(),
                    &pose2,
                    dir,
                    shape.shape_scaled().as_ref(),
                    ShapeCastOptions {
                        max_time_of_impact: max,
                        target_distance: 0.0,
                        stop_at_penetration: true,
                        compute_impact_geometry_on_penetration: true,
                    },
                );
                match hit {
                    Ok(Some(h)) if h.time_of_impact <= max => {
                        let d = h.time_of_impact;
                        if best.is_none_or(|x| d < x.0 || (d == x.0 && cid < x.1)) {
                            best = Some((d, cid, b, pose1 * h.witness1, r * h.normal1));
                        }
                        max = d;
                        d
                    }
                    _ => Scalar::MAX,
                }
            });
        }
        Ok(best.map(|(d, cid, b, at, n)| self.hit(cid, b, d, at, n)))
    }

    fn overlap(
        &self,
        o: &Overlap,
        f: &QueryFilter,
        out: &mut Vec<ColliderId>,
    ) -> Result<(), PhysError> {
        let shape = Collider::from(shared_shape(&o.shape)?);
        let (at, rot) = (v(o.position.local), q(o.rotation));
        let aabb = shape.aabb(at, Rotation(rot));
        for tree in self.world.resource::<ColliderTrees>().iter_trees() {
            tree.aabb_traverse(aabb.into(), |pid| {
                let Some(proxy) = tree.get_proxy(pid) else {
                    return true;
                };
                let e = proxy.collider;
                if let (Some((cid, _)), Some((p, r)), Some(col)) = (
                    self.accept(e, f),
                    self.collider_pose(e),
                    self.world.get::<Collider>(e),
                ) && matches!(
                    avian3d::collision::collider::contact_query::intersection_test(
                        col,
                        Position(p),
                        Rotation(r),
                        &shape,
                        Position(at),
                        Rotation(rot),
                    ),
                    Ok(true)
                ) {
                    out.push(cid);
                }
                true
            });
        }
        Ok(())
    }

    fn contact_count(&self) -> usize {
        let g = self.world.resource::<ContactGraph>();
        g.iter_active_touching().count() + g.iter_sleeping_touching().count()
    }
}
