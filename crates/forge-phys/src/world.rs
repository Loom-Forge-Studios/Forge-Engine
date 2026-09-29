//! The physics world: one region-local simulation (Ch.17) over a backend from
//! `forge.phys.backend`, and everything that must behave the same whichever backend runs.
//!
//! * **The fixed step.** [`PhysicsWorld::step`] advances exactly `settings.dt`;
//!   [`PhysicsWorld::advance`] turns elapsed time into fixed steps (at most `max_catch_up`
//!   per call, the rest dropped rather than spiralling) and keeps the remainder as the
//!   interpolation factor.
//! * **Render interpolation.** Every moving body's pose before and after the last step is
//!   kept; [`PhysicsWorld::render_pose`] blends them by the remainder, so a 60 Hz simulation
//!   draws smoothly at any frame rate.
//! * **Gravity and damping zones** are applied here, before each step, as velocity changes a
//!   backend integrates like its own gravity: identical on every backend, and nothing at all
//!   when a world has no zones.
//! * **Events** come out in one canonical order (kind, then ids): the same list on every
//!   backend and machine.
//! * **Queries** (ray, shape cast, overlap) are checked here (frame, finiteness), answered by
//!   the backend, and put in canonical order; a batch runs across threads when it is large
//!   (each query is independent, the output order is the input order); an async query is
//!   answered at the next step boundary, against the state that step produced.
//! * **The state hash** ([`PhysicsWorld::state_hash`]) covers every body's pose and
//!   velocities to the bit: the determinism check's measure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use forge_frames::{DQuat, DVec3, FrameId, FramePos};

use crate::backend::{PhysicsBackend, PhysicsBackendPoint};
use crate::debug::{ColliderDraw, DebugKind, DebugLine, Outline};
use crate::types::{
    BodyDesc, BodyId, BodyKind, BodyState, ColliderDesc, ColliderId, Hit, JointDesc, JointFrames,
    JointId, Overlap, PhysEvent, Query, QueryFilter, QueryResult, Ray, ShapeCast, ZoneDesc,
    ZoneEffect, ZoneId, ZoneShape,
};
use crate::{PhysError, PhysFaults};

/// A world's settings. The step and gravity may change mid-run (deterministically: a replay
/// must change them at the same step).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicsSettings {
    /// m/s², frame axes.
    pub gravity: DVec3,
    /// The fixed step, seconds.
    pub dt: f64,
    /// Solver substeps (avian) or iterations (rapier) per step; `None`: the backend's own
    /// default.
    pub substeps: Option<u32>,
    /// Most fixed steps one [`PhysicsWorld::advance`] runs (a slow frame drops time rather
    /// than falling further behind).
    pub max_catch_up: u32,
    /// Continuous collision: fast bodies are swept against static geometry, so they cannot
    /// pass through a thin wall between steps (both backends do this by default). Off, a
    /// step is cheapest and a fast body may tunnel. Set at creation.
    pub continuous: bool,
}

impl Default for PhysicsSettings {
    fn default() -> Self {
        Self {
            gravity: DVec3::new(0.0, -9.81, 0.0),
            dt: 1.0 / 60.0,
            substeps: None,
            max_catch_up: 5,
            continuous: true,
        }
    }
}

/// Queries per thread below which a batch runs on the calling thread (a spawn costs more
/// than this many ray casts).
const BATCH_PER_THREAD: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    at: DVec3,
    rot: DQuat,
}

#[derive(Clone, Debug)]
struct BodyRec {
    kind: BodyKind,
    gravity_scale: f64,
    /// Union of its colliders' memberships (zones select bodies by layer).
    layers: u32,
    colliders: Vec<ColliderId>,
    joints: Vec<JointId>,
    prev: Pose,
    curr: Pose,
    user: u64,
}

#[derive(Clone, Copy, Debug)]
struct ColliderRec {
    body: BodyId,
    memberships: u32,
    user: u64,
}

#[derive(Clone, Copy, Debug)]
struct JointRec {
    a: BodyId,
    b: BodyId,
    frames: JointFrames,
}

/// A pending async query's ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QueryTicket(pub u64);

/// The physics world (see the module docs).
pub struct PhysicsWorld {
    frame: FrameId,
    settings: PhysicsSettings,
    /// The backend. `&mut self` methods reach it without locking (`RwLock::get_mut`); a
    /// query takes the read lock, so a batch reads it from several threads at once.
    backend: RwLock<Box<dyn PhysicsBackend>>,
    backend_id: String,
    /// Bodies or colliders were added, removed or teleported since the backend last
    /// updated its query structures; the next query syncs it first (once).
    stale: AtomicBool,
    bodies: Vec<Option<BodyRec>>,
    colliders: Vec<Option<ColliderRec>>,
    /// What the debug drawing draws of each collider (by collider id).
    drawn: Vec<Option<ColliderDraw>>,
    joints: Vec<Option<JointRec>>,
    zones: Vec<Option<ZoneDesc>>,
    /// Dynamic bodies in id order (what zones visit), kept as bodies come and go.
    dynamic: Vec<BodyId>,
    /// Bodies whose pose is tracked for interpolation (every non-static body), id order.
    moving: Vec<BodyId>,
    events: Vec<PhysEvent>,
    steps: u64,
    /// Seconds of simulated time not yet stepped (< dt after [`Self::advance`]).
    accumulator: f64,
    /// The interpolation factor in [0, 1] between the last two steps.
    alpha: f64,
    next_ticket: u64,
    pending: Vec<(QueryTicket, Query, QueryFilter)>,
    /// Scratch for answering `pending` (kept: no allocation per step).
    checked: Vec<(QueryTicket, Result<Query, PhysError>, QueryFilter)>,
    ready: Vec<(QueryTicket, Result<QueryResult, PhysError>)>,
    /// Velocity changes the zone pass made so far (a probe for tests).
    zone_touches: u64,
    /// W2 fault switches (test builds only).
    pub faults: PhysFaults,
}

impl std::fmt::Debug for PhysicsWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhysicsWorld")
            .field("backend", &self.backend_id)
            .field("frame", &self.frame)
            .field("bodies", &self.body_count())
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
}

fn slot<T>(v: &[Option<T>], i: u32) -> Option<&T> {
    v.get(i as usize).and_then(Option::as_ref)
}

/// `q` normalised, or an error naming `what`.
fn unit(q: DQuat, what: &str) -> Result<DQuat, PhysError> {
    if !q.is_finite() {
        return Err(PhysError::invalid(format!("{what} is not finite")));
    }
    q.try_normalize()
        .ok_or_else(|| PhysError::invalid(format!("{what} is a zero quaternion")))
}

/// The rotation taking `+x` to the unit vector `d` (the shortest arc; a half turn about `y`
/// for `-x`).
#[must_use]
pub fn rotation_from_x(d: DVec3) -> DQuat {
    let x = DVec3::X;
    let c = x.dot(d);
    if c < -1.0 + 1e-12 {
        return DQuat::from_xyzw(0.0, 1.0, 0.0, 0.0);
    }
    let axis = x.cross(d);
    DQuat::from_xyzw(axis.x, axis.y, axis.z, 1.0 + c)
        .try_normalize()
        .unwrap_or(DQuat::IDENTITY)
}

/// The linear and angular velocity that take a body from one pose to another in `dt`
/// (the shorter way round; deterministic: `forge_num::det` for the angle). How both
/// backends drive a kinematic body to its target.
#[must_use]
pub fn velocity_to(p0: DVec3, r0: DQuat, p1: DVec3, r1: DQuat, dt: f64) -> (DVec3, DVec3) {
    let lin = (p1 - p0) / dt;
    let d = (r1 * r0.conjugate())
        .try_normalize()
        .unwrap_or(DQuat::IDENTITY);
    let d = if d.w < 0.0 {
        DQuat::from_xyzw(-d.x, -d.y, -d.z, -d.w)
    } else {
        d
    };
    let s = d.xyz().length();
    let ang = if s > 0.0 {
        let angle = 2.0 * forge_num::det::atan2(s, d.w);
        d.xyz() * (angle / s / dt)
    } else {
        DVec3::ZERO
    };
    (lin, ang)
}

/// Normalised blend of two rotations along the shorter arc (deterministic: no
/// transcendental).
#[must_use]
pub fn nlerp(a: DQuat, b: DQuat, t: f64) -> DQuat {
    let dot = a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w;
    let s = if dot < 0.0 { -1.0 } else { 1.0 };
    let u = 1.0 - t;
    DQuat::from_xyzw(
        a.x * u + b.x * s * t,
        a.y * u + b.y * s * t,
        a.z * u + b.z * s * t,
        a.w * u + b.w * s * t,
    )
    .try_normalize()
    .unwrap_or(b)
}

impl PhysicsWorld {
    /// A world in `frame` on the backend `id` of `reg` (`None`: the registry's default).
    pub fn new(
        reg: &forge_plugin::Registry<PhysicsBackendPoint>,
        id: Option<&str>,
        frame: FrameId,
        settings: PhysicsSettings,
    ) -> Result<Self, PhysError> {
        Self::check_settings(&settings)?;
        let backend = crate::backend::create(reg, id, frame, &settings)?;
        Ok(Self::with_backend(frame, settings, backend))
    }

    /// A world on the first-party backend `id` (`avian3d`, `rapier3d`).
    pub fn first_party(
        id: &str,
        frame: FrameId,
        settings: PhysicsSettings,
    ) -> Result<Self, PhysError> {
        Self::check_settings(&settings)?;
        let f = crate::backend::first_party_factory(id)
            .ok_or_else(|| PhysError::UnknownBackend(id.to_owned()))?;
        Ok(Self::with_backend(frame, settings, f(frame, &settings)?))
    }

    /// A world over an already-built backend.
    #[must_use]
    pub fn with_backend(
        frame: FrameId,
        settings: PhysicsSettings,
        backend: Box<dyn PhysicsBackend>,
    ) -> Self {
        Self {
            frame,
            settings,
            backend_id: backend.id().to_owned(),
            backend: RwLock::new(backend),
            stale: AtomicBool::new(false),
            bodies: Vec::new(),
            colliders: Vec::new(),
            drawn: Vec::new(),
            joints: Vec::new(),
            zones: Vec::new(),
            dynamic: Vec::new(),
            moving: Vec::new(),
            events: Vec::new(),
            steps: 0,
            accumulator: 0.0,
            alpha: 1.0,
            next_ticket: 0,
            pending: Vec::new(),
            checked: Vec::new(),
            ready: Vec::new(),
            zone_touches: 0,
            faults: PhysFaults::default(),
        }
    }

    fn check_settings(s: &PhysicsSettings) -> Result<(), PhysError> {
        if !s.gravity.is_finite() || !s.dt.is_finite() || s.dt <= 0.0 {
            return Err(PhysError::invalid(
                "the gravity must be finite and the step > 0",
            ));
        }
        if s.substeps == Some(0) {
            return Err(PhysError::invalid("substeps must be at least 1"));
        }
        Ok(())
    }

    /// The backend's id.
    #[must_use]
    pub fn backend_id(&self) -> &str {
        &self.backend_id
    }

    fn be(&self) -> RwLockReadGuard<'_, Box<dyn PhysicsBackend>> {
        self.backend.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn be_mut(&mut self) -> &mut dyn PhysicsBackend {
        self.backend
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
    }

    /// The backend, synced first if structure changed since it last was.
    fn synced(&self) -> Result<RwLockReadGuard<'_, Box<dyn PhysicsBackend>>, PhysError> {
        if self.stale.load(Ordering::Acquire) {
            let mut g = self.backend.write().unwrap_or_else(PoisonError::into_inner);
            if self.stale.load(Ordering::Acquire) {
                g.sync()?;
                self.stale.store(false, Ordering::Release);
            }
        }
        Ok(self.be())
    }

    fn touch(&mut self) {
        *self.stale.get_mut() = true;
    }

    /// The frame everything is in.
    #[must_use]
    pub fn frame(&self) -> FrameId {
        self.frame
    }

    #[must_use]
    pub fn settings(&self) -> &PhysicsSettings {
        &self.settings
    }

    /// Change the gravity (takes effect at the next step).
    pub fn set_gravity(&mut self, gravity: DVec3) -> Result<(), PhysError> {
        if !gravity.is_finite() {
            return Err(PhysError::invalid("the gravity is not finite"));
        }
        self.settings.gravity = gravity;
        self.be_mut().set_gravity(gravity);
        Ok(())
    }

    /// Steps taken.
    #[must_use]
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Live bodies.
    #[must_use]
    pub fn body_count(&self) -> usize {
        self.bodies.iter().flatten().count()
    }

    /// Live bodies in id order.
    pub fn bodies(&self) -> impl Iterator<Item = BodyId> + '_ {
        self.bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_some())
            .map(|(i, _)| BodyId(i as u32))
    }

    fn check_pos(&self, p: FramePos, what: &str) -> Result<(), PhysError> {
        if p.frame != self.frame {
            return Err(PhysError::Frame(format!(
                "{what} is in frame {}; this physics world is frame {} (a physics world is region-local)",
                p.frame.0, self.frame.0
            )));
        }
        if !p.local.is_finite() {
            return Err(PhysError::invalid(format!("{what} is not finite")));
        }
        Ok(())
    }

    fn body(&self, id: BodyId) -> Result<&BodyRec, PhysError> {
        slot(&self.bodies, id.0).ok_or_else(|| PhysError::invalid(format!("no body {}", id.0)))
    }

    // ---- bodies, colliders, joints, zones --------------------------------------------------

    /// Add a body. A dynamic body's mass comes from its colliders (volume x density): give it
    /// one before it is meant to move.
    pub fn add_body(&mut self, desc: &BodyDesc) -> Result<BodyId, PhysError> {
        self.check_pos(desc.position, "a body's position")?;
        let rotation = unit(desc.rotation, "a body's rotation")?;
        let finite = desc.linear_velocity.is_finite()
            && desc.angular_velocity.is_finite()
            && desc.linear_damping.is_finite()
            && desc.linear_damping >= 0.0
            && desc.angular_damping.is_finite()
            && desc.angular_damping >= 0.0
            && desc.gravity_scale.is_finite();
        if !finite {
            return Err(PhysError::invalid(
                "a body's velocity, damping or gravity scale is not finite (or damping < 0)",
            ));
        }
        let id = BodyId(
            u32::try_from(self.bodies.len()).map_err(|_| PhysError::invalid("too many bodies"))?,
        );
        let desc = BodyDesc { rotation, ..*desc };
        self.be_mut().add_body(id, &desc)?;
        self.touch();
        let pose = Pose {
            at: desc.position.local,
            rot: rotation,
        };
        self.bodies.push(Some(BodyRec {
            kind: desc.kind,
            gravity_scale: desc.gravity_scale,
            layers: 0,
            colliders: Vec::new(),
            joints: Vec::new(),
            prev: pose,
            curr: pose,
            user: desc.user,
        }));
        if desc.kind == BodyKind::Dynamic {
            self.dynamic.push(id);
        }
        if desc.kind != BodyKind::Static {
            self.moving.push(id);
        }
        Ok(id)
    }

    /// Remove a body with its colliders and joints.
    pub fn remove_body(&mut self, id: BodyId) -> Result<(), PhysError> {
        let rec = self.body(id)?.clone();
        for j in rec.joints {
            if slot(&self.joints, j.0).is_some() {
                self.remove_joint(j)?;
            }
        }
        for c in rec.colliders {
            self.remove_collider(c)?;
        }
        self.be_mut().remove_body(id)?;
        self.touch();
        self.bodies[id.0 as usize] = None;
        self.dynamic.retain(|b| *b != id);
        self.moving.retain(|b| *b != id);
        Ok(())
    }

    /// Attach a collider to `body`.
    pub fn add_collider(
        &mut self,
        body: BodyId,
        desc: &ColliderDesc,
    ) -> Result<ColliderId, PhysError> {
        self.body(body)?;
        desc.check()?;
        let rotation = unit(desc.rotation, "a collider's rotation")?;
        let desc = ColliderDesc {
            rotation,
            ..desc.clone()
        };
        let id = ColliderId(
            u32::try_from(self.colliders.len())
                .map_err(|_| PhysError::invalid("too many colliders"))?,
        );
        self.be_mut().add_collider(id, body, &desc)?;
        self.touch();
        let hull = match desc.shape {
            crate::Shape::ConvexHull { .. } => self.be().hull_edges(id),
            _ => None,
        };
        self.drawn.push(Some(ColliderDraw {
            outline: Outline::of(&desc.shape, hull),
            offset: desc.offset,
            rotation: desc.rotation,
            sensor: desc.sensor,
        }));
        self.colliders.push(Some(ColliderRec {
            body,
            memberships: desc.layers.memberships,
            user: desc.user,
        }));
        if let Some(Some(b)) = self.bodies.get_mut(body.0 as usize) {
            b.colliders.push(id);
            b.layers |= desc.layers.memberships;
        }
        Ok(id)
    }

    /// Detach and drop a collider.
    pub fn remove_collider(&mut self, id: ColliderId) -> Result<(), PhysError> {
        let rec = *slot(&self.colliders, id.0)
            .ok_or_else(|| PhysError::invalid(format!("no collider {}", id.0)))?;
        self.be_mut().remove_collider(id)?;
        self.touch();
        self.colliders[id.0 as usize] = None;
        self.drawn[id.0 as usize] = None;
        let layers = self
            .body(rec.body)
            .map(|b| b.colliders.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| *c != id)
            .filter_map(|c| slot(&self.colliders, c.0).map(|r| r.memberships))
            .fold(0, |a, m| a | m);
        if let Some(Some(b)) = self.bodies.get_mut(rec.body.0 as usize) {
            b.colliders.retain(|c| *c != id);
            b.layers = layers;
        }
        Ok(())
    }

    /// The body a collider is attached to.
    pub fn collider_body(&self, id: ColliderId) -> Result<BodyId, PhysError> {
        slot(&self.colliders, id.0)
            .map(|c| c.body)
            .ok_or_else(|| PhysError::invalid(format!("no collider {}", id.0)))
    }

    /// A collider's game value.
    pub fn collider_user(&self, id: ColliderId) -> Result<u64, PhysError> {
        slot(&self.colliders, id.0)
            .map(|c| c.user)
            .ok_or_else(|| PhysError::invalid(format!("no collider {}", id.0)))
    }

    /// Join two bodies. The joint frames are fixed here, from the bodies' current poses, so
    /// the joint starts at rest (see [`JointDesc`]).
    pub fn add_joint(&mut self, desc: &JointDesc) -> Result<JointId, PhysError> {
        if desc.body_a == desc.body_b {
            return Err(PhysError::invalid("a joint needs two different bodies"));
        }
        self.body(desc.body_a)?;
        self.body(desc.body_b)?;
        let axis = desc
            .axis
            .try_normalize()
            .filter(|a| a.is_finite())
            .ok_or_else(|| PhysError::invalid("a joint's axis is zero or not finite"))?;
        if !desc.anchor_a.is_finite() || !desc.anchor_b.is_finite() {
            return Err(PhysError::invalid("a joint's anchor is not finite"));
        }
        check_joint_kind(&desc.kind)?;
        let sa = self.be_mut().state(desc.body_a)?;
        let sb = self.be_mut().state(desc.body_b)?;
        let basis_a = rotation_from_x(axis);
        // B's basis is A's, seen from B at creation: rot_b * basis_b == rot_a * basis_a.
        let basis_b = (sb.rotation.conjugate() * sa.rotation * basis_a)
            .try_normalize()
            .unwrap_or(DQuat::IDENTITY);
        let frames = JointFrames {
            anchor_a: desc.anchor_a,
            basis_a,
            anchor_b: desc.anchor_b,
            basis_b,
        };
        let id = JointId(
            u32::try_from(self.joints.len()).map_err(|_| PhysError::invalid("too many joints"))?,
        );
        let desc = JointDesc { axis, ..*desc };
        self.be_mut().add_joint(id, &desc, &frames)?;
        self.joints.push(Some(JointRec {
            a: desc.body_a,
            b: desc.body_b,
            frames,
        }));
        for b in [desc.body_a, desc.body_b] {
            if let Some(Some(r)) = self.bodies.get_mut(b.0 as usize) {
                r.joints.push(id);
            }
        }
        Ok(id)
    }

    /// Remove a joint.
    pub fn remove_joint(&mut self, id: JointId) -> Result<(), PhysError> {
        let rec = *slot(&self.joints, id.0)
            .ok_or_else(|| PhysError::invalid(format!("no joint {}", id.0)))?;
        self.be_mut().remove_joint(id)?;
        self.joints[id.0 as usize] = None;
        for b in [rec.a, rec.b] {
            if let Some(Some(r)) = self.bodies.get_mut(b.0 as usize) {
                r.joints.retain(|j| *j != id);
            }
        }
        Ok(())
    }

    /// Add a gravity or damping zone.
    pub fn add_zone(&mut self, desc: &ZoneDesc) -> Result<ZoneId, PhysError> {
        self.check_pos(desc.position, "a zone's position")?;
        let rotation = unit(desc.rotation, "a zone's rotation")?;
        let ok = match desc.shape {
            ZoneShape::Sphere { radius } => radius.is_finite() && radius > 0.0,
            ZoneShape::Box { half_extents: h } => {
                h.is_finite() && h.x > 0.0 && h.y > 0.0 && h.z > 0.0
            }
        } && match desc.effect {
            ZoneEffect::Gravity { acceleration } => acceleration.is_finite(),
            ZoneEffect::PointGravity { strength } => strength.is_finite(),
            ZoneEffect::Damping { linear, angular } => {
                linear.is_finite() && linear >= 0.0 && angular.is_finite() && angular >= 0.0
            }
        };
        if !ok {
            return Err(PhysError::invalid(format!(
                "a zone needs a positive finite size and a finite effect: {desc:?}"
            )));
        }
        let id = ZoneId(
            u32::try_from(self.zones.len()).map_err(|_| PhysError::invalid("too many zones"))?,
        );
        self.zones.push(Some(ZoneDesc { rotation, ..*desc }));
        Ok(id)
    }

    /// Remove a zone.
    pub fn remove_zone(&mut self, id: ZoneId) -> Result<(), PhysError> {
        match self.zones.get_mut(id.0 as usize) {
            Some(z @ Some(_)) => {
                *z = None;
                Ok(())
            }
            _ => Err(PhysError::invalid(format!("no zone {}", id.0))),
        }
    }

    // ---- state -------------------------------------------------------------------------------

    /// A body's pose and velocities after the last step.
    pub fn state(&self, id: BodyId) -> Result<BodyState, PhysError> {
        self.body(id)?;
        self.be().state(id)
    }

    /// A body's position after the last step.
    pub fn position(&self, id: BodyId) -> Result<FramePos, PhysError> {
        Ok(self.state(id)?.position)
    }

    /// A body's rotation after the last step.
    pub fn rotation(&self, id: BodyId) -> Result<DQuat, PhysError> {
        Ok(self.state(id)?.rotation)
    }

    /// A body's game value.
    pub fn user(&self, id: BodyId) -> Result<u64, PhysError> {
        Ok(self.body(id)?.user)
    }

    /// A dynamic body's mass (kg); 0 for static and kinematic bodies.
    pub fn mass(&self, id: BodyId) -> Result<f64, PhysError> {
        self.body(id)?;
        self.synced()?.mass(id)
    }

    /// Whether a body is asleep.
    pub fn is_sleeping(&self, id: BodyId) -> Result<bool, PhysError> {
        self.body(id)?;
        self.be().is_sleeping(id)
    }

    /// The debug drawing ([`crate::debug`]): every collider's wireframe, every joint's anchors
    /// and every touching contact at the pose the last step left, in this world's frame,
    /// appended to `out`.
    pub fn debug_lines(&self, out: &mut Vec<DebugLine>) -> Result<(), PhysError> {
        let be = self.be();
        for (c, d) in self.colliders.iter().zip(&self.drawn) {
            let (Some(c), Some(d)) = (c, d) else {
                continue;
            };
            let Some(b) = slot(&self.bodies, c.body.0) else {
                continue;
            };
            let kind = match b.kind {
                _ if d.sensor => DebugKind::Trigger,
                BodyKind::Static => DebugKind::Static,
                BodyKind::Kinematic => DebugKind::Kinematic,
                BodyKind::Dynamic if be.is_sleeping(c.body)? => DebugKind::Sleeping,
                BodyKind::Dynamic => DebugKind::Dynamic,
            };
            let rot = b.curr.rot * d.rotation;
            let at = b.curr.at + b.curr.rot.rotate(d.offset);
            d.outline.draw(&mut |p, q| {
                out.push(DebugLine {
                    a: at + rot.rotate(p),
                    b: at + rot.rotate(q),
                    kind,
                });
            });
        }
        for j in self.joints.iter().flatten() {
            let (Some(a), Some(b)) = (slot(&self.bodies, j.a.0), slot(&self.bodies, j.b.0)) else {
                continue;
            };
            let f = &j.frames;
            let pa = a.curr.at + a.curr.rot.rotate(f.anchor_a);
            let pb = b.curr.at + b.curr.rot.rotate(f.anchor_b);
            let axis = (a.curr.rot * f.basis_a).rotate(DVec3::X);
            crate::debug::joint_lines(pa, pb, axis, out);
        }
        let mut contacts = Vec::new();
        be.contacts(&mut contacts);
        for c in contacts.iter().filter(|c| c.point.frame == self.frame) {
            crate::debug::contact_lines(c, out);
        }
        Ok(())
    }

    /// Teleport a body and set its velocities.
    pub fn set_state(&mut self, id: BodyId, s: &BodyState) -> Result<(), PhysError> {
        self.body(id)?;
        self.check_pos(s.position, "a body's position")?;
        let rotation = unit(s.rotation, "a body's rotation")?;
        if !s.linear_velocity.is_finite() || !s.angular_velocity.is_finite() {
            return Err(PhysError::invalid("a velocity is not finite"));
        }
        let s = BodyState { rotation, ..*s };
        self.be_mut().set_state(id, &s)?;
        self.touch();
        // A teleport is not a motion to blend across.
        if let Some(Some(b)) = self.bodies.get_mut(id.0 as usize) {
            let p = Pose {
                at: s.position.local,
                rot: rotation,
            };
            b.prev = p;
            b.curr = p;
        }
        Ok(())
    }

    /// Set a body's velocities.
    pub fn set_velocity(
        &mut self,
        id: BodyId,
        linear: DVec3,
        angular: DVec3,
    ) -> Result<(), PhysError> {
        self.body(id)?;
        if !linear.is_finite() || !angular.is_finite() {
            return Err(PhysError::invalid("a velocity is not finite"));
        }
        self.be_mut().set_velocity(id, linear, angular)
    }

    /// Apply an impulse (N·s at the centre of mass; N·m·s about it).
    pub fn apply_impulse(
        &mut self,
        id: BodyId,
        linear: DVec3,
        angular: DVec3,
    ) -> Result<(), PhysError> {
        self.body(id)?;
        if !linear.is_finite() || !angular.is_finite() {
            return Err(PhysError::invalid("an impulse is not finite"));
        }
        self.be_mut().apply_impulse(id, linear, angular)
    }

    /// Drive a kinematic body to a pose by the end of the next step.
    pub fn set_kinematic_target(
        &mut self,
        id: BodyId,
        target: FramePos,
        rotation: DQuat,
    ) -> Result<(), PhysError> {
        if self.body(id)?.kind != BodyKind::Kinematic {
            return Err(PhysError::invalid(format!(
                "body {} is not kinematic: move a dynamic body with velocities or impulses",
                id.0
            )));
        }
        self.check_pos(target, "a kinematic target")?;
        let rotation = unit(rotation, "a kinematic target's rotation")?;
        self.be_mut().set_kinematic_target(id, target, rotation)
    }

    /// Solid contact pairs touching after the last step.
    #[must_use]
    pub fn contact_count(&self) -> usize {
        self.be().contact_count()
    }

    /// This step's events, in canonical order.
    #[must_use]
    pub fn events(&self) -> &[PhysEvent] {
        &self.events
    }

    // ---- stepping ----------------------------------------------------------------------------

    /// Run one fixed step: zones, the backend's step, the poses for interpolation, the
    /// events in canonical order, then the async queries against the new state.
    pub fn step(&mut self) -> Result<(), PhysError> {
        let dt = self.settings.dt;
        // Zones kick half their change before the step and half after (kick-drift-kick):
        // second-order, like the backends' own gravity, and the same on every backend.
        self.apply_zones(0.5 * dt)?;
        self.events.clear();
        let repeats = self.faults.step_repeats();
        let backend = self
            .backend
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for _ in 0..=repeats {
            backend.step(dt, &mut self.events)?;
        }
        *self.stale.get_mut() = false;
        self.apply_zones(0.5 * dt)?;
        for (a, b) in self.events.iter_mut().filter_map(|e| match e {
            PhysEvent::ContactBegin(a, b) | PhysEvent::ContactEnd(a, b) => Some((a, b)),
            _ => None,
        }) {
            if *b < *a {
                std::mem::swap(a, b);
            }
        }
        self.events.sort_unstable();
        self.events.dedup();
        if self.faults.rebuild_moving() {
            // The W2 control of test_phys_step_alloc: a fresh list every step.
            self.moving = self.bodies().filter(|b| self.moving.contains(b)).collect();
        }
        for i in 0..self.moving.len() {
            let id = self.moving[i];
            let s = self.be_mut().state(id)?;
            if let Some(Some(b)) = self.bodies.get_mut(id.0 as usize) {
                b.prev = b.curr;
                b.curr = Pose {
                    at: s.position.local,
                    rot: s.rotation,
                };
            }
        }
        self.steps += 1;
        self.resolve_pending();
        Ok(())
    }

    /// Turn `elapsed` seconds into fixed steps (at most `max_catch_up`; a longer gap drops the
    /// excess) and set the interpolation factor from the remainder. Returns the steps run.
    pub fn advance(&mut self, elapsed: f64) -> Result<u32, PhysError> {
        if !elapsed.is_finite() || elapsed < 0.0 {
            return Err(PhysError::invalid("elapsed time must be finite and >= 0"));
        }
        let dt = self.settings.dt;
        self.accumulator += elapsed;
        let mut n = 0;
        while self.accumulator >= dt && n < self.settings.max_catch_up {
            self.step()?;
            self.accumulator -= dt;
            n += 1;
        }
        if self.accumulator >= dt {
            self.accumulator = self.accumulator.rem_euclid(dt);
        }
        self.alpha = self.accumulator / dt;
        Ok(n)
    }

    /// The interpolation factor [`Self::advance`] left (1 before any advance: the last step's
    /// pose).
    #[must_use]
    pub fn alpha(&self) -> f64 {
        self.alpha
    }

    /// Set the interpolation factor (a host with its own fixed-step clock).
    pub fn set_alpha(&mut self, alpha: f64) {
        self.alpha = if alpha.is_finite() {
            alpha.clamp(0.0, 1.0)
        } else {
            1.0
        };
    }

    /// Where to draw a body now: its pose blended between the last two steps by
    /// [`Self::alpha`] (a static body's pose as it is).
    pub fn render_pose(&self, id: BodyId) -> Result<(FramePos, DQuat), PhysError> {
        let b = self.body(id)?;
        let t = if self.faults.no_interpolation() {
            1.0
        } else {
            self.alpha
        };
        let at = b.prev.at + (b.curr.at - b.prev.at) * t;
        Ok((
            FramePos::new(self.frame, at),
            nlerp(b.prev.rot, b.curr.rot, t),
        ))
    }

    /// Bodies whose zone pass changed a velocity so far (a probe).
    #[must_use]
    pub fn zone_touches(&self) -> u64 {
        self.zone_touches
    }

    /// Apply the zones' effects for `dt` seconds (half a step: see [`Self::step`]).
    fn apply_zones(&mut self, dt: f64) -> Result<(), PhysError> {
        if self.faults.ignore_zones() || self.zones.iter().all(Option::is_none) {
            return Ok(());
        }
        let world_g = self.settings.gravity;
        for i in 0..self.dynamic.len() {
            let id = self.dynamic[i];
            let Some(b) = slot(&self.bodies, id.0) else {
                continue;
            };
            let (layers, gscale) = (b.layers, b.gravity_scale);
            if self.be_mut().is_sleeping(id)? {
                continue;
            }
            let s = self.be_mut().state(id)?;
            let p = s.position.local;
            let mut gravity: Option<(i32, u32, DVec3)> = None;
            let mut damping: Option<(i32, u32, f64, f64)> = None;
            for (zi, z) in self.zones.iter().enumerate() {
                let Some(z) = z else { continue };
                if z.layers & layers == 0 && layers != 0 {
                    continue;
                }
                let local = z.rotation.inverse_rotate(p - z.position.local);
                let inside = match z.shape {
                    ZoneShape::Sphere { radius } => local.length_squared() <= radius * radius,
                    ZoneShape::Box { half_extents: h } => {
                        local.x.abs() <= h.x && local.y.abs() <= h.y && local.z.abs() <= h.z
                    }
                };
                if !inside {
                    continue;
                }
                let zi = zi as u32;
                let better = |cur: Option<(i32, u32)>| {
                    cur.is_none_or(|(p, i)| z.priority > p || (z.priority == p && zi < i))
                };
                match z.effect {
                    ZoneEffect::Gravity { acceleration } => {
                        if better(gravity.map(|g| (g.0, g.1))) {
                            gravity = Some((z.priority, zi, acceleration));
                        }
                    }
                    ZoneEffect::PointGravity { strength } => {
                        if better(gravity.map(|g| (g.0, g.1))) {
                            let to = z.position.local - p;
                            let g = to.try_normalize().map_or(DVec3::ZERO, |d| d * strength);
                            gravity = Some((z.priority, zi, g));
                        }
                    }
                    ZoneEffect::Damping { linear, angular } => {
                        if better(damping.map(|d| (d.0, d.1))) {
                            damping = Some((z.priority, zi, linear, angular));
                        }
                    }
                }
            }
            if gravity.is_none() && damping.is_none() {
                continue;
            }
            let mut v = s.linear_velocity;
            let mut w = s.angular_velocity;
            if let Some((_, _, g)) = gravity {
                // The backend adds the world's gravity this step; replace it with the zone's.
                v += (g - world_g) * (gscale * dt);
            }
            if let Some((_, _, lin, ang)) = damping {
                v = v / (1.0 + lin * dt);
                w = w / (1.0 + ang * dt);
            }
            self.be_mut().set_velocity(id, v, w)?;
            self.zone_touches += 1;
        }
        Ok(())
    }

    // ---- queries -----------------------------------------------------------------------------

    fn check_dir(d: DVec3, max: f64) -> Result<DVec3, PhysError> {
        if !max.is_finite() || max < 0.0 {
            return Err(PhysError::invalid(
                "a query's max distance must be finite and >= 0",
            ));
        }
        d.try_normalize()
            .filter(|u| u.is_finite())
            .ok_or_else(|| PhysError::invalid("a query's direction is zero or not finite"))
    }

    fn checked_query(&self, q: &Query) -> Result<Query, PhysError> {
        Ok(match q {
            Query::Ray(r) => {
                self.check_pos(r.start, "a ray's start")?;
                Query::Ray(Ray {
                    direction: Self::check_dir(r.direction, r.max_distance)?,
                    ..*r
                })
            }
            Query::Shape(c) => {
                self.check_pos(c.start, "a shape cast's start")?;
                c.shape.check()?;
                Query::Shape(ShapeCast {
                    direction: Self::check_dir(c.direction, c.max_distance)?,
                    rotation: unit(c.rotation, "a shape cast's rotation")?,
                    ..c.clone()
                })
            }
            Query::Overlap(o) => {
                self.check_pos(o.position, "an overlap's position")?;
                o.shape.check()?;
                Query::Overlap(Overlap {
                    rotation: unit(o.rotation, "an overlap's rotation")?,
                    ..o.clone()
                })
            }
        })
    }

    fn run_query(
        backend: &dyn PhysicsBackend,
        q: &Query,
        filter: &QueryFilter,
    ) -> Result<QueryResult, PhysError> {
        Ok(match q {
            Query::Ray(r) => QueryResult::Hit(backend.cast_ray(r, filter)?),
            Query::Shape(c) => QueryResult::Hit(backend.cast_shape(c, filter)?),
            Query::Overlap(o) => {
                let mut out = Vec::new();
                backend.overlap(o, filter, &mut out)?;
                out.sort_unstable();
                out.dedup();
                QueryResult::Overlaps(out)
            }
        })
    }

    /// The first collider along a ray.
    pub fn raycast(&self, ray: &Ray, filter: &QueryFilter) -> Result<Option<Hit>, PhysError> {
        self.check_pos(ray.start, "a ray's start")?;
        let ray = Ray {
            direction: Self::check_dir(ray.direction, ray.max_distance)?,
            ..*ray
        };
        self.synced()?.cast_ray(&ray, filter)
    }

    /// The first collider a swept shape touches.
    pub fn shape_cast(
        &self,
        cast: &ShapeCast,
        filter: &QueryFilter,
    ) -> Result<Option<Hit>, PhysError> {
        match self.checked_query(&Query::Shape(cast.clone()))? {
            Query::Shape(c) => self.synced()?.cast_shape(&c, filter),
            _ => Err(PhysError::Backend("unreachable query kind".into())),
        }
    }

    /// Every collider touching a shape at rest, in id order.
    pub fn overlap(&self, q: &Overlap, filter: &QueryFilter) -> Result<Vec<ColliderId>, PhysError> {
        let q = self.checked_query(&Query::Overlap(q.clone()))?;
        match Self::run_query(self.synced()?.as_ref(), &q, filter)? {
            QueryResult::Overlaps(v) => Ok(v),
            QueryResult::Hit(_) => Err(PhysError::Backend("unreachable query kind".into())),
        }
    }

    /// Answer a batch of queries: `out[i]` answers `queries[i]`. A large batch is spread over
    /// the machine's threads (the backend is read-only while it runs); the answers are the
    /// same as one query at a time, in the same order.
    pub fn query_batch(
        &self,
        queries: &[Query],
        filter: &QueryFilter,
        out: &mut Vec<Result<QueryResult, PhysError>>,
    ) {
        out.clear();
        let guard = match self.synced() {
            Ok(g) => g,
            Err(e) => {
                out.extend(queries.iter().map(|_| Err(e.clone())));
                return;
            }
        };
        let backend: &dyn PhysicsBackend = guard.as_ref();
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .min(queries.len() / BATCH_PER_THREAD)
            .max(1);
        let one = |q: &Query| {
            self.checked_query(q)
                .and_then(|q| Self::run_query(backend, &q, filter))
        };
        if threads == 1 {
            out.extend(queries.iter().map(one));
            return;
        }
        let chunk = queries.len().div_ceil(threads);
        let parts: Vec<Vec<Result<QueryResult, PhysError>>> = std::thread::scope(|s| {
            let handles: Vec<_> = queries
                .chunks(chunk)
                .map(|c| s.spawn(move || c.iter().map(one).collect::<Vec<_>>()))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| {
                        vec![Err(PhysError::Backend("a query thread panicked".into()))]
                    })
                })
                .collect()
        });
        for p in parts {
            out.extend(p);
        }
        if out.len() != queries.len() {
            out.resize_with(queries.len(), || {
                Err(PhysError::Backend("a query thread panicked".into()))
            });
        }
    }

    /// Queue a query to be answered at the next step boundary, against the state that step
    /// produced (UE's async traces): the caller never waits on the solver, and the answer is
    /// deterministic (it depends on the step, not on timing).
    pub fn submit(&mut self, q: Query, filter: QueryFilter) -> QueryTicket {
        let t = QueryTicket(self.next_ticket);
        self.next_ticket += 1;
        self.pending.push((t, q, filter));
        t
    }

    /// Take an async query's answer, once it is ready (after the next step).
    pub fn poll(&mut self, t: QueryTicket) -> Option<Result<QueryResult, PhysError>> {
        let i = self.ready.iter().position(|(k, _)| *k == t)?;
        Some(self.ready.swap_remove(i).1)
    }

    /// Async queries still waiting for a step.
    #[must_use]
    pub fn pending_queries(&self) -> usize {
        self.pending.len()
    }

    fn resolve_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut pending = std::mem::take(&mut self.pending);
        let mut checked = std::mem::take(&mut self.checked);
        checked.extend(
            pending
                .drain(..)
                .map(|(t, q, f)| (t, self.checked_query(&q), f)),
        );
        let backend = self
            .backend
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for (t, q, f) in checked.drain(..) {
            let r = q.and_then(|q| Self::run_query(backend.as_ref(), &q, &f));
            self.ready.push((t, r));
        }
        self.checked = checked;
        // Keep the buffer's capacity for the next frame's queries.
        self.pending = pending;
        self.pending.clear();
    }

    // ---- determinism -------------------------------------------------------------------------

    /// BLAKE3 over the whole simulation state — the step count and, per body in id order, its
    /// kind, pose and velocities to the bit — as 64 hex digits.
    pub fn state_hash(&self) -> Result<String, PhysError> {
        let mut h = blake3::Hasher::new();
        h.update(b"forge-phys state v1");
        h.update(&self.steps.to_le_bytes());
        h.update(&self.frame.0.to_le_bytes());
        for (i, b) in self.bodies.iter().enumerate() {
            let Some(b) = b else { continue };
            let s = self.be().state(BodyId(i as u32))?;
            h.update(&(i as u64).to_le_bytes());
            h.update(&[b.kind as u8]);
            for x in s
                .position
                .local
                .to_bits()
                .into_iter()
                .chain([s.rotation.x, s.rotation.y, s.rotation.z, s.rotation.w].map(f64::to_bits))
                .chain(s.linear_velocity.to_bits())
                .chain(s.angular_velocity.to_bits())
            {
                h.update(&x.to_le_bytes());
            }
        }
        Ok(h.finalize().to_hex().to_string())
    }
}

fn check_joint_kind(k: &crate::types::JointKind) -> Result<(), PhysError> {
    use crate::types::{AxisMotion, JointKind as K};
    let range = |r: Option<(f64, f64)>| {
        r.is_none_or(|(lo, hi)| lo.is_finite() && hi.is_finite() && lo <= hi)
    };
    let ok = match *k {
        K::Fixed => true,
        K::Hinge { limits } | K::Slider { limits } => range(limits),
        K::Spring {
            rest_length,
            stiffness,
            damping,
        } => {
            rest_length.is_finite()
                && rest_length >= 0.0
                && stiffness.is_finite()
                && stiffness > 0.0
                && damping.is_finite()
                && damping >= 0.0
        }
        K::ConeTwist { swing, twist } => swing.is_finite() && swing >= 0.0 && range(Some(twist)),
        K::SixDof { axes } => axes.iter().all(|a| match *a {
            AxisMotion::Limited { min, max } => range(Some((min, max))),
            _ => true,
        }),
    };
    if ok {
        Ok(())
    } else {
        Err(PhysError::invalid(format!(
            "a joint's limits must be finite with min <= max, a spring's stiffness > 0: {k:?}"
        )))
    }
}
