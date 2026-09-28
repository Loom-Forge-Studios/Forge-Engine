//! 2D physics (Ch.35 §35.2): **a 2D solver, not a 3D solver with a frozen axis**.
//!
//! Rigid bodies (static, kinematic, dynamic) carrying colliders (circles, boxes, convex
//! polygons, capsules, segments — all [`Hull`]s), revolute joints, sensors, collision
//! filtering, ray casts and box queries, contact begin/end events. The step is a fixed-step
//! sequential-impulse solver (Box2D lineage): sort-and-sweep broad phase, persistent contact
//! manifolds warm-started by feature id, speculative contacts (a fast body closes the gap
//! it sees this step and no further, so it does not tunnel through a thin wall), friction,
//! restitution, Baumgarte position correction.
//!
//! **Deterministic by construction** (I2, Ch.17's "stable contact ordering"): `f64` IEEE
//! arithmetic and `forge_num::det` only; bodies, colliders and joints are dense arrays
//! indexed by id (never reused); the broad phase sorts by `(min x, id)`; pairs, contacts and
//! events are kept in id order; iteration counts are fixed. Two runs, on any platform, give
//! the same bits — the determinism corpus hashes whole simulations
//! (`tests/determinism/test_cross_platform_hash.rs`, rows `2d/physics/*`).
//!
//! ADR 0047 records why this is Forge's own solver and not `avian2d`.

mod collide;

use forge_frames::{FrameId, FramePos2};
use forge_num::DVec2;

pub use collide::{Hull, MAX_VERTS, Manifold, ManifoldPoint, RayHit, collide, contains, raycast};

use crate::Error2d;
use crate::math::{Aabb2, Rot2};

/// A rigid body of a [`PhysicsWorld2d`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RigidBodyId(pub u32);

/// A collider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColliderId(pub u32);

/// A joint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JointId(pub u32);

/// How a body moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BodyKind {
    /// Never moves (terrain, tiles).
    #[default]
    Static,
    /// Moves by its velocity; nothing pushes it (moving platforms).
    Kinematic,
    /// Moves by forces, gravity and contacts.
    Dynamic,
}

/// A body's starting state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyDef {
    pub kind: BodyKind,
    pub position: FramePos2,
    /// Radians, counter-clockwise.
    pub angle: f64,
    pub linear_velocity: DVec2,
    pub angular_velocity: f64,
    pub linear_damping: f64,
    pub angular_damping: f64,
    pub gravity_scale: f64,
    /// Never rotates (a platformer character).
    pub lock_rotation: bool,
    /// Free for the game.
    pub user: u64,
}

impl BodyDef {
    /// A body of `kind` at `position`, at rest.
    #[must_use]
    pub fn new(kind: BodyKind, position: FramePos2) -> Self {
        Self {
            kind,
            position,
            angle: 0.0,
            linear_velocity: DVec2::ZERO,
            angular_velocity: 0.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            gravity_scale: 1.0,
            lock_rotation: false,
            user: 0,
        }
    }
}

/// A collider's shape, in its body's frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Circle {
        radius: f64,
    },
    /// A box of half extents `half`.
    Box {
        half: DVec2,
    },
    /// A capsule along the body's y axis: the segment `(0, -half_height)`-`(0, half_height)`
    /// rounded by `radius`.
    Capsule {
        half_height: f64,
        radius: f64,
    },
    /// A convex polygon (3 to 8 vertices) rounded by `radius`.
    Polygon {
        verts: Vec<DVec2>,
        radius: f64,
    },
    /// A one-sided-free segment (terrain edges, spline shapes' outlines).
    Segment {
        a: DVec2,
        b: DVec2,
    },
}

/// Which colliders may touch: `a` and `b` collide when `a.category & b.mask != 0` and
/// `b.category & a.mask != 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    pub category: u32,
    pub mask: u32,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            category: 1,
            mask: u32::MAX,
        }
    }
}

/// A collider's definition.
#[derive(Clone, Debug, PartialEq)]
pub struct ColliderDef {
    pub shape: Shape,
    /// Offset from the body origin, in the body's frame.
    pub offset: DVec2,
    /// Rotation relative to the body.
    pub angle: f64,
    pub density: f64,
    pub friction: f64,
    pub restitution: f64,
    /// Reports overlaps, never pushes.
    pub sensor: bool,
    pub filter: Filter,
}

impl ColliderDef {
    /// A solid collider of `shape` with density 1, friction 0.6, no bounce.
    #[must_use]
    pub fn new(shape: Shape) -> Self {
        Self {
            shape,
            offset: DVec2::ZERO,
            angle: 0.0,
            density: 1.0,
            friction: 0.6,
            restitution: 0.0,
            sensor: false,
            filter: Filter::default(),
        }
    }
}

/// Solver settings. Fixed for a world's life (changing them mid-run is allowed, and
/// deterministic, but a replay must change them at the same step).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicsSettings {
    pub gravity: DVec2,
    /// The fixed step (seconds).
    pub dt: f64,
    pub velocity_iterations: u32,
    /// Contacts are created this far apart (speculative distance, world units).
    pub speculative: f64,
    /// Allowed penetration.
    pub slop: f64,
    /// Fraction of penetration removed per step.
    pub baumgarte: f64,
    /// Relative speed below which a contact does not bounce.
    pub restitution_threshold: f64,
}

impl Default for PhysicsSettings {
    fn default() -> Self {
        Self {
            gravity: DVec2::new(0.0, -9.81),
            dt: 1.0 / 60.0,
            velocity_iterations: 8,
            speculative: 0.04,
            slop: 0.005,
            baumgarte: 0.2,
            restitution_threshold: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Body {
    kind: BodyKind,
    /// World position of the centre of mass, and the rotation.
    c: DVec2,
    rot: Rot2,
    /// The centre of mass in the body frame.
    local_c: DVec2,
    v: DVec2,
    w: f64,
    force: DVec2,
    torque: f64,
    inv_mass: f64,
    inv_i: f64,
    mass: f64,
    linear_damping: f64,
    angular_damping: f64,
    gravity_scale: f64,
    lock_rotation: bool,
    user: u64,
    colliders: Vec<u32>,
}

impl Body {
    fn origin(&self) -> DVec2 {
        self.c - self.rot.apply(self.local_c)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Collider {
    body: u32,
    /// The hull in the body frame.
    hull: Hull,
    def: ColliderDef,
    /// This step's world hull and box.
    world: Hull,
    aabb: Aabb2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PointState {
    id: u32,
    point: DVec2,
    separation: f64,
    ra: DVec2,
    rb: DVec2,
    normal_mass: f64,
    tangent_mass: f64,
    normal_impulse: f64,
    tangent_impulse: f64,
    bias: f64,
    bounce: f64,
}

/// A body as the solver iterates it (dense, copied in and out once per step).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SolverBody {
    c: DVec2,
    v: DVec2,
    w: f64,
    inv_mass: f64,
    inv_i: f64,
    dynamic: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Contact {
    /// Colliders (the pair key, `a < b`).
    a: u32,
    b: u32,
    /// Their bodies.
    ba: u32,
    bb: u32,
    normal: DVec2,
    points: [PointState; 2],
    count: usize,
    friction: f64,
    restitution: f64,
    sensor: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Joint {
    a: u32,
    b: u32,
    local_a: DVec2,
    local_b: DVec2,
    impulse: DVec2,
    // Solver scratch.
    ra: DVec2,
    rb: DVec2,
    mass: [f64; 4],
    bias: DVec2,
}

/// A contact event (colliders in id order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContactEvent {
    Begin(ColliderId, ColliderId),
    End(ColliderId, ColliderId),
}

/// What a body touches (for a game's "am I on the ground" question).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touch {
    pub other: RigidBodyId,
    /// Unit normal pointing from `other` toward this body.
    pub normal: DVec2,
    /// The deepest point's separation (negative: penetrating).
    pub separation: f64,
    /// Total normal impulse of the last step.
    pub impulse: f64,
    pub sensor: bool,
}

/// A ray query's hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub collider: ColliderId,
    pub body: RigidBodyId,
    pub point: FramePos2,
    pub normal: DVec2,
    pub distance: f64,
}

/// The 2D physics world (see the module docs). One root frame: every position given to it
/// or read from it is in that frame (Ch.35 §35.3).
#[derive(Clone, Debug)]
pub struct PhysicsWorld2d {
    frame: FrameId,
    pub settings: PhysicsSettings,
    bodies: Vec<Option<Body>>,
    colliders: Vec<Option<Collider>>,
    joints: Vec<Option<Joint>>,
    /// Contacts in pair order `(a, b)`.
    contacts: Vec<Contact>,
    /// The solver's body scratch (kept between steps: no allocation once warm).
    solver: Vec<SolverBody>,
    events: Vec<ContactEvent>,
    steps: u64,
    /// W2 fault switches (test builds only).
    pub faults: crate::Faults2d,
    /// Candidate pairs examined by the last broad phase (a perf probe).
    pub last_pairs: usize,
}

impl PartialEq for PhysicsWorld2d {
    fn eq(&self, o: &Self) -> bool {
        self.frame == o.frame
            && self.settings == o.settings
            && self.bodies == o.bodies
            && self.colliders == o.colliders
            && self.joints == o.joints
            && self.contacts == o.contacts
            && self.steps == o.steps
    }
}

fn finite(v: DVec2) -> bool {
    v.is_finite()
}

impl PhysicsWorld2d {
    /// An empty world in `frame` with default settings.
    #[must_use]
    pub fn new(frame: FrameId) -> Self {
        Self::with_settings(frame, PhysicsSettings::default())
    }

    #[must_use]
    pub fn with_settings(frame: FrameId, settings: PhysicsSettings) -> Self {
        Self {
            frame,
            settings,
            bodies: Vec::new(),
            colliders: Vec::new(),
            joints: Vec::new(),
            contacts: Vec::new(),
            solver: Vec::new(),
            events: Vec::new(),
            steps: 0,
            faults: crate::Faults2d::default(),
            last_pairs: 0,
        }
    }

    /// The frame everything is in.
    #[must_use]
    pub fn frame(&self) -> FrameId {
        self.frame
    }

    /// Steps taken.
    #[must_use]
    pub fn steps(&self) -> u64 {
        self.steps
    }

    fn check_frame(&self, p: FramePos2) -> Result<(), Error2d> {
        if p.frame != self.frame {
            return Err(Error2d::Frame {
                why: format!(
                    "position in frame {:?}; this 2D world is frame {:?}",
                    p.frame, self.frame
                ),
            });
        }
        if !finite(p.local) {
            return Err(Error2d::invalid("a position is not finite"));
        }
        Ok(())
    }

    fn body(&self, id: RigidBodyId) -> Result<&Body, Error2d> {
        self.bodies
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| Error2d::invalid(format!("no body {}", id.0)))
    }

    fn body_mut(&mut self, id: RigidBodyId) -> Result<&mut Body, Error2d> {
        self.bodies
            .get_mut(id.0 as usize)
            .and_then(Option::as_mut)
            .ok_or_else(|| Error2d::invalid(format!("no body {}", id.0)))
    }

    /// Add a body (no colliders yet: a dynamic body gets unit mass until it has one).
    pub fn add_body(&mut self, def: &BodyDef) -> Result<RigidBodyId, Error2d> {
        self.check_frame(def.position)?;
        if !finite(def.linear_velocity)
            || !def.angle.is_finite()
            || !def.angular_velocity.is_finite()
        {
            return Err(Error2d::invalid("a body's angle or velocity is not finite"));
        }
        let dynamic = def.kind == BodyKind::Dynamic;
        let b = Body {
            kind: def.kind,
            c: def.position.local,
            rot: Rot2::from_angle(def.angle),
            local_c: DVec2::ZERO,
            v: if def.kind == BodyKind::Static {
                DVec2::ZERO
            } else {
                def.linear_velocity
            },
            w: if def.kind == BodyKind::Static || def.lock_rotation {
                0.0
            } else {
                def.angular_velocity
            },
            force: DVec2::ZERO,
            torque: 0.0,
            inv_mass: if dynamic { 1.0 } else { 0.0 },
            inv_i: 0.0,
            mass: if dynamic { 1.0 } else { 0.0 },
            linear_damping: def.linear_damping.max(0.0),
            angular_damping: def.angular_damping.max(0.0),
            gravity_scale: def.gravity_scale,
            lock_rotation: def.lock_rotation,
            user: def.user,
            colliders: Vec::new(),
        };
        self.bodies.push(Some(b));
        Ok(RigidBodyId((self.bodies.len() - 1) as u32))
    }

    /// Remove a body, its colliders, joints and contacts.
    pub fn remove_body(&mut self, id: RigidBodyId) -> Result<(), Error2d> {
        let b = self.body(id)?.clone();
        for c in &b.colliders {
            self.colliders[*c as usize] = None;
        }
        self.contacts.retain(|k| k.ba != id.0 && k.bb != id.0);
        for j in &mut self.joints {
            if j.as_ref().is_some_and(|j| j.a == id.0 || j.b == id.0) {
                *j = None;
            }
        }
        self.bodies[id.0 as usize] = None;
        Ok(())
    }

    fn hull_of(def: &ColliderDef) -> Result<Hull, Error2d> {
        let rot = Rot2::from_angle(def.angle);
        let o = def.offset;
        let bad = |w: &str| Error2d::invalid(format!("collider shape: {w}"));
        let h = match &def.shape {
            Shape::Circle { radius } => {
                if radius.is_nan() || *radius <= 0.0 {
                    return Err(bad("a circle needs a positive radius"));
                }
                Hull::circle(o, *radius)
            }
            Shape::Box { half } => {
                if !(half.x > 0.0 && half.y > 0.0) {
                    return Err(bad("a box needs positive half extents"));
                }
                Hull::boxed(o, *half, rot, 0.0)
            }
            Shape::Capsule {
                half_height,
                radius,
            } => {
                if !(*half_height > 0.0 && *radius > 0.0) {
                    return Err(bad("a capsule needs a positive height and radius"));
                }
                let a = o + rot.apply(DVec2::new(0.0, -half_height));
                let b = o + rot.apply(DVec2::new(0.0, *half_height));
                Hull::segment(a, b, *radius).ok_or_else(|| bad("degenerate capsule"))?
            }
            Shape::Polygon { verts, radius } => {
                let pts: Vec<DVec2> = verts.iter().map(|v| o + rot.apply(*v)).collect();
                Hull::polygon(&pts, radius.max(0.0))
                    .ok_or_else(|| bad("a polygon must be convex with 3 to 8 vertices"))?
            }
            Shape::Segment { a, b } => Hull::segment(o + rot.apply(*a), o + rot.apply(*b), 0.0)
                .ok_or_else(|| bad("a segment needs two distinct ends"))?,
        };
        if (0..h.count).any(|i| !finite(h.verts[i])) {
            return Err(bad("not finite"));
        }
        Ok(h)
    }

    /// Attach a collider to `body` (recomputes its mass).
    pub fn add_collider(
        &mut self,
        body: RigidBodyId,
        def: ColliderDef,
    ) -> Result<ColliderId, Error2d> {
        let hull = Self::hull_of(&def)?;
        if !(def.density >= 0.0 && def.friction >= 0.0 && def.restitution >= 0.0) {
            return Err(Error2d::invalid(
                "density, friction and restitution must be >= 0",
            ));
        }
        self.body(body)?;
        let id = self.colliders.len() as u32;
        let world = hull;
        self.colliders.push(Some(Collider {
            body: body.0,
            hull,
            def,
            world,
            aabb: world.aabb(),
        }));
        self.body_mut(body)?.colliders.push(id);
        self.update_mass(body.0);
        self.sync_collider(id);
        Ok(ColliderId(id))
    }

    fn update_mass(&mut self, bi: u32) {
        let Some(Some(b)) = self.bodies.get(bi as usize) else {
            return;
        };
        if b.kind != BodyKind::Dynamic {
            return;
        }
        let origin = b.origin();
        let mut m = 0.0;
        let mut c = DVec2::ZERO;
        let mut i = 0.0;
        let mut parts = Vec::new();
        for ci in &b.colliders {
            if let Some(Some(col)) = self.colliders.get(*ci as usize) {
                if col.def.sensor {
                    continue;
                }
                let (pm, pc, pi) = col.hull.mass(col.def.density);
                parts.push((pm, pc, pi));
                m += pm;
                c += pc * pm;
            }
        }
        let Some(Some(b)) = self.bodies.get_mut(bi as usize) else {
            return;
        };
        if m > 0.0 {
            c = c / m;
            for (pm, pc, pi) in parts {
                let d = pc - c;
                i += pi + pm * d.dot(d);
            }
            b.mass = m;
            b.inv_mass = 1.0 / m;
            b.inv_i = if b.lock_rotation || i <= 0.0 {
                0.0
            } else {
                1.0 / i
            };
        } else {
            b.mass = 1.0;
            b.inv_mass = 1.0;
            b.inv_i = 0.0;
            c = DVec2::ZERO;
        }
        b.local_c = c;
        b.c = origin + b.rot.apply(c);
    }

    fn sync_collider(&mut self, ci: u32) {
        let Some(Some(col)) = self.colliders.get(ci as usize) else {
            return;
        };
        let Some(Some(b)) = self.bodies.get(col.body as usize) else {
            return;
        };
        let world = col.hull.transformed(b.rot, b.origin());
        let aabb = world.aabb();
        if let Some(Some(col)) = self.colliders.get_mut(ci as usize) {
            col.world = world;
            col.aabb = aabb;
        }
    }

    /// Pin `a` and `b` together at `anchor` (a revolute joint: they may turn about it).
    pub fn add_revolute(
        &mut self,
        a: RigidBodyId,
        b: RigidBodyId,
        anchor: FramePos2,
    ) -> Result<JointId, Error2d> {
        self.check_frame(anchor)?;
        let (ba, bb) = (self.body(a)?, self.body(b)?);
        let local_a = ba.rot.apply_inv(anchor.local - ba.origin());
        let local_b = bb.rot.apply_inv(anchor.local - bb.origin());
        self.joints.push(Some(Joint {
            a: a.0,
            b: b.0,
            local_a,
            local_b,
            impulse: DVec2::ZERO,
            ra: DVec2::ZERO,
            rb: DVec2::ZERO,
            mass: [0.0; 4],
            bias: DVec2::ZERO,
        }));
        Ok(JointId((self.joints.len() - 1) as u32))
    }

    // ---- state -----------------------------------------------------------------------

    /// The body's origin.
    pub fn position(&self, id: RigidBodyId) -> Result<FramePos2, Error2d> {
        Ok(FramePos2::new(self.frame, self.body(id)?.origin()))
    }

    /// The body's angle (radians).
    pub fn angle(&self, id: RigidBodyId) -> Result<f64, Error2d> {
        Ok(self.body(id)?.rot.angle())
    }

    /// The body's rotation.
    pub fn rotation(&self, id: RigidBodyId) -> Result<Rot2, Error2d> {
        Ok(self.body(id)?.rot)
    }

    pub fn linear_velocity(&self, id: RigidBodyId) -> Result<DVec2, Error2d> {
        Ok(self.body(id)?.v)
    }

    pub fn angular_velocity(&self, id: RigidBodyId) -> Result<f64, Error2d> {
        Ok(self.body(id)?.w)
    }

    pub fn mass(&self, id: RigidBodyId) -> Result<f64, Error2d> {
        Ok(self.body(id)?.mass)
    }

    pub fn user(&self, id: RigidBodyId) -> Result<u64, Error2d> {
        Ok(self.body(id)?.user)
    }

    pub fn set_linear_velocity(&mut self, id: RigidBodyId, v: DVec2) -> Result<(), Error2d> {
        if !finite(v) {
            return Err(Error2d::invalid("velocity is not finite"));
        }
        let b = self.body_mut(id)?;
        if b.kind != BodyKind::Static {
            b.v = v;
        }
        Ok(())
    }

    pub fn set_angular_velocity(&mut self, id: RigidBodyId, w: f64) -> Result<(), Error2d> {
        if !w.is_finite() {
            return Err(Error2d::invalid("angular velocity is not finite"));
        }
        let b = self.body_mut(id)?;
        if b.kind != BodyKind::Static && !b.lock_rotation {
            b.w = w;
        }
        Ok(())
    }

    /// Teleport a body.
    pub fn set_transform(
        &mut self,
        id: RigidBodyId,
        position: FramePos2,
        angle: f64,
    ) -> Result<(), Error2d> {
        self.check_frame(position)?;
        let b = self.body_mut(id)?;
        b.rot = Rot2::from_angle(angle);
        b.c = position.local + b.rot.apply(b.local_c);
        let cols = b.colliders.clone();
        for c in cols {
            self.sync_collider(c);
        }
        Ok(())
    }

    /// Add a force at the centre of mass for the next step.
    pub fn apply_force(&mut self, id: RigidBodyId, f: DVec2) -> Result<(), Error2d> {
        let b = self.body_mut(id)?;
        if b.kind == BodyKind::Dynamic {
            b.force += f;
        }
        Ok(())
    }

    /// An instant change of momentum at the centre of mass.
    pub fn apply_impulse(&mut self, id: RigidBodyId, j: DVec2) -> Result<(), Error2d> {
        let b = self.body_mut(id)?;
        if b.kind == BodyKind::Dynamic {
            b.v += j * b.inv_mass;
        }
        Ok(())
    }

    /// Live bodies, in id order.
    pub fn bodies(&self) -> impl Iterator<Item = RigidBodyId> + '_ {
        self.bodies
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.as_ref().map(|_| RigidBodyId(i as u32)))
    }

    /// Live colliders, in id order, with their body and world hull.
    pub fn colliders(&self) -> impl Iterator<Item = (ColliderId, RigidBodyId, &Hull)> + '_ {
        self.colliders.iter().enumerate().filter_map(|(i, c)| {
            c.as_ref()
                .map(|c| (ColliderId(i as u32), RigidBodyId(c.body), &c.world))
        })
    }

    /// Events of the last step, in order.
    #[must_use]
    pub fn events(&self) -> &[ContactEvent] {
        &self.events
    }

    /// Current contacts, in pair order.
    #[must_use]
    pub fn contact_count(&self) -> usize {
        self.contacts.iter().filter(|c| c.count > 0).count()
    }

    /// What `id` touches now (contacts within the slop or penetrating), in collider order.
    pub fn touches(&self, id: RigidBodyId) -> Vec<Touch> {
        let mut out = Vec::new();
        for c in &self.contacts {
            if c.count == 0 {
                continue;
            }
            let (ba, bb) = (c.ba, c.bb);
            let deepest = c.points[..c.count]
                .iter()
                .map(|p| p.separation)
                .fold(f64::INFINITY, f64::min);
            if deepest > self.settings.slop * 2.0 {
                continue;
            }
            let impulse: f64 = c.points[..c.count].iter().map(|p| p.normal_impulse).sum();
            if ba == id.0 {
                out.push(Touch {
                    other: RigidBodyId(bb),
                    normal: -c.normal,
                    separation: deepest,
                    impulse,
                    sensor: c.sensor,
                });
            } else if bb == id.0 {
                out.push(Touch {
                    other: RigidBodyId(ba),
                    normal: c.normal,
                    separation: deepest,
                    impulse,
                    sensor: c.sensor,
                });
            }
        }
        out
    }

    /// The nearest hit along a ray from `from` in unit direction `dir`, up to `max`, over
    /// colliders whose category meets `mask` (sensors skipped).
    pub fn raycast(
        &self,
        from: FramePos2,
        dir: DVec2,
        max: f64,
        mask: u32,
    ) -> Result<Option<Hit>, Error2d> {
        self.check_frame(from)?;
        let d = dir
            .try_normalize()
            .ok_or_else(|| Error2d::invalid("a ray needs a direction"))?;
        let o = from.local;
        let end = o + d * max;
        let bbox = Aabb2::new(o.min(end), o.max(end));
        let mut best: Option<Hit> = None;
        for (i, c) in self.colliders.iter().enumerate() {
            let Some(c) = c else { continue };
            if c.def.sensor || c.def.filter.category & mask == 0 || !c.aabb.overlaps(&bbox) {
                continue;
            }
            let lim = best.map_or(max, |b| b.distance);
            if let Some(h) = collide::raycast(&c.world, o, d, lim)
                && best.is_none_or(|b| h.t < b.distance)
            {
                best = Some(Hit {
                    collider: ColliderId(i as u32),
                    body: RigidBodyId(c.body),
                    point: FramePos2::new(self.frame, o + d * h.t),
                    normal: h.normal,
                    distance: h.t,
                });
            }
        }
        Ok(best)
    }

    /// Colliders whose boxes overlap `[min, max]` (in this world's frame), in id order.
    pub fn query_box(&self, min: FramePos2, max: FramePos2) -> Result<Vec<ColliderId>, Error2d> {
        self.check_frame(min)?;
        self.check_frame(max)?;
        let q = Aabb2::new(min.local.min(max.local), min.local.max(max.local));
        Ok(self
            .colliders
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                c.as_ref()
                    .filter(|c| c.aabb.overlaps(&q))
                    .map(|_| ColliderId(i as u32))
            })
            .collect())
    }

    /// A bit-exact hash of the whole dynamic state (for determinism checks and replays).
    #[must_use]
    pub fn state_bits(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.bodies.len() * 6);
        for b in self.bodies.iter().flatten() {
            out.extend_from_slice(&b.c.to_bits());
            out.push(b.rot.c.to_bits());
            out.push(b.rot.s.to_bits());
            out.extend_from_slice(&b.v.to_bits());
            out.push(b.w.to_bits());
        }
        out
    }

    // ---- the step ---------------------------------------------------------------------

    /// Advance one fixed step (`settings.dt`).
    pub fn step(&mut self) {
        let dt = self.settings.dt;
        if dt.is_nan() || dt <= 0.0 {
            return;
        }
        let inv_dt = 1.0 / dt;
        let g = self.settings.gravity;
        // 1. Velocities.
        for b in self.bodies.iter_mut().flatten() {
            if b.kind != BodyKind::Dynamic {
                continue;
            }
            b.v += (g * b.gravity_scale + b.force * b.inv_mass) * dt;
            b.w += dt * b.inv_i * b.torque;
            b.v = b.v * (1.0 / (1.0 + dt * b.linear_damping));
            b.w *= 1.0 / (1.0 + dt * b.angular_damping);
        }
        // 2-3. Broad and narrow phase: the new contact set, warm-started from the old.
        let pairs = self.broad_phase();
        self.last_pairs = pairs.len();
        let old = std::mem::take(&mut self.contacts);
        let mut events = Vec::new();
        for (ca, cb) in pairs {
            let (Some(a), Some(b)) = (&self.colliders[ca as usize], &self.colliders[cb as usize])
            else {
                continue;
            };
            // Speculative distance: the fixed margin plus how far the pair can close this
            // step, so a fast body meets a thin wall's contact before it could pass it.
            let closing = match (&self.bodies[a.body as usize], &self.bodies[b.body as usize]) {
                (Some(ba), Some(bb)) => (ba.v - bb.v).length() * self.settings.dt,
                _ => 0.0,
            };
            let margin = if a.def.sensor || b.def.sensor {
                0.0
            } else {
                self.settings.speculative + closing
            };
            let Some(m) = collide::collide(&a.world, &b.world, margin) else {
                continue;
            };
            let sensor = a.def.sensor || b.def.sensor;
            if sensor && m.points[..m.count].iter().all(|p| p.separation > 0.0) {
                continue;
            }
            let mut c = Contact {
                a: ca,
                b: cb,
                ba: a.body,
                bb: b.body,
                normal: m.normal,
                points: [PointState {
                    id: 0,
                    point: DVec2::ZERO,
                    separation: 0.0,
                    ra: DVec2::ZERO,
                    rb: DVec2::ZERO,
                    normal_mass: 0.0,
                    tangent_mass: 0.0,
                    normal_impulse: 0.0,
                    tangent_impulse: 0.0,
                    bias: 0.0,
                    bounce: 0.0,
                }; 2],
                count: m.count,
                friction: combine_friction(a.def.friction, b.def.friction),
                restitution: a.def.restitution.max(b.def.restitution),
                sensor,
            };
            let prev = old
                .binary_search_by_key(&(ca, cb), |o| (o.a, o.b))
                .ok()
                .map(|i| &old[i]);
            for i in 0..m.count {
                let p = m.points[i];
                c.points[i].id = p.id;
                c.points[i].point = p.point;
                c.points[i].separation = p.separation;
                if !self.faults.no_warm_start()
                    && let Some(prev) = prev
                    && let Some(q) = prev.points[..prev.count].iter().find(|q| q.id == p.id)
                {
                    c.points[i].normal_impulse = q.normal_impulse;
                    c.points[i].tangent_impulse = q.tangent_impulse;
                }
            }
            let touching = m.points[..m.count]
                .iter()
                .any(|p| p.separation <= self.settings.slop);
            let was = prev.is_some_and(|p| {
                p.points[..p.count]
                    .iter()
                    .any(|q| q.separation <= self.settings.slop)
            });
            if touching && !was {
                events.push(ContactEvent::Begin(ColliderId(ca), ColliderId(cb)));
            }
            // Pairs arrive sorted, so the contact list stays in pair order.
            self.contacts.push(c);
        }
        for p in &old {
            let was = p.points[..p.count]
                .iter()
                .any(|q| q.separation <= self.settings.slop);
            let is = self
                .contacts
                .binary_search_by_key(&(p.a, p.b), |c| (c.a, c.b))
                .ok()
                .is_some_and(|i| {
                    let c = &self.contacts[i];
                    c.points[..c.count]
                        .iter()
                        .any(|q| q.separation <= self.settings.slop)
                });
            if was && !is {
                events.push(ContactEvent::End(ColliderId(p.a), ColliderId(p.b)));
            }
        }
        events.sort();
        self.events = events;
        // 4. The solver's dense body copy, then prepare and warm start.
        let mut sb = std::mem::take(&mut self.solver);
        sb.clear();
        sb.extend(self.bodies.iter().map(|b| match b {
            Some(b) => SolverBody {
                c: b.c,
                v: b.v,
                w: b.w,
                inv_mass: b.inv_mass,
                inv_i: b.inv_i,
                dynamic: b.kind == BodyKind::Dynamic,
            },
            None => SolverBody::default(),
        }));
        prepare_contacts(&mut self.contacts, &sb, &self.settings, inv_dt, self.faults);
        warm_start(&self.contacts, &mut sb);
        self.prepare_joints(&mut sb, inv_dt);
        // 5. Iterate.
        let reverse = self.faults.reverse_contact_order() && self.steps % 2 == 1;
        // W2 fault only (a constant `false` in a production build): the contacts in a
        // randomly seeded `HashSet`'s iteration order, as a solver keyed by a `HashMap` would
        // visit them. The canonical path allocates nothing here.
        let hashed: Option<Vec<usize>> = self.faults.hash_ordered_contacts().then(|| {
            (0..self.contacts.len())
                .collect::<std::collections::HashSet<usize>>()
                .into_iter()
                .collect()
        });
        // `solver_repeats` is a W2 fault (0 in a production build): the same iterations again.
        let iterations = self
            .settings
            .velocity_iterations
            .saturating_mul(self.faults.solver_repeats().saturating_add(1));
        for _ in 0..iterations {
            self.solve_joints(&mut sb);
            solve_contacts(&mut self.contacts, &mut sb, reverse, hashed.as_deref());
        }
        for (b, s) in self.bodies.iter_mut().zip(&sb) {
            if let Some(b) = b
                && b.kind == BodyKind::Dynamic
            {
                b.v = s.v;
                b.w = s.w;
            }
        }
        self.solver = sb;
        // 6. Positions.
        for b in self.bodies.iter_mut().flatten() {
            if b.kind == BodyKind::Static {
                continue;
            }
            b.c += b.v * dt;
            if !b.lock_rotation {
                b.rot = b.rot.integrate(b.w * dt);
            }
            b.force = DVec2::ZERO;
            b.torque = 0.0;
        }
        for ci in 0..self.colliders.len() as u32 {
            let moving = self.colliders[ci as usize]
                .as_ref()
                .and_then(|c| self.bodies[c.body as usize].as_ref())
                .is_some_and(|b| b.kind != BodyKind::Static);
            if moving {
                self.sync_collider(ci);
            }
        }
        self.steps += 1;
    }

    /// Candidate collider pairs `(a < b)`, sorted: sort-and-sweep on x over boxes grown by
    /// the speculative distance and this step's motion.
    fn broad_phase(&self) -> Vec<(u32, u32)> {
        let dt = self.settings.dt;
        let spec = self.settings.speculative;
        let mut items: Vec<(Aabb2, u32, u32, BodyKind)> = Vec::with_capacity(self.colliders.len());
        for (i, c) in self.colliders.iter().enumerate() {
            let Some(c) = c else { continue };
            let Some(b) = &self.bodies[c.body as usize] else {
                continue;
            };
            let mut bb = c.aabb.inflate(spec * 0.5);
            if b.kind != BodyKind::Static {
                let d = b.v * dt;
                bb = bb.union(&Aabb2::new(bb.min + d, bb.max + d));
                // Rotation sweeps the box by up to |w dt| times its reach.
                let reach = (c.aabb.size().length()) * 0.5 * (b.w * dt).abs();
                bb = bb.inflate(reach);
            }
            items.push((bb, i as u32, c.body, b.kind));
        }
        let mut pairs = Vec::new();
        let ok = |i: &(Aabb2, u32, u32, BodyKind), j: &(Aabb2, u32, u32, BodyKind)| -> bool {
            if i.2 == j.2 {
                return false;
            }
            if i.3 != BodyKind::Dynamic && j.3 != BodyKind::Dynamic {
                return false;
            }
            let (Some(a), Some(b)) = (&self.colliders[i.1 as usize], &self.colliders[j.1 as usize])
            else {
                return false;
            };
            let (fa, fb) = (a.def.filter, b.def.filter);
            if fa.category & fb.mask == 0 || fb.category & fa.mask == 0 {
                return false;
            }
            // Bodies pinned together do not collide.
            !self
                .joints
                .iter()
                .flatten()
                .any(|jt| (jt.a == i.2 && jt.b == j.2) || (jt.a == j.2 && jt.b == i.2))
        };
        if self.faults.brute_force_pairs() {
            for x in 0..items.len() {
                for y in x + 1..items.len() {
                    if items[x].0.overlaps(&items[y].0) && ok(&items[x], &items[y]) {
                        let (a, b) = (items[x].1.min(items[y].1), items[x].1.max(items[y].1));
                        pairs.push((a, b));
                    }
                }
            }
        } else {
            items.sort_by(|a, b| a.0.min.x.total_cmp(&b.0.min.x).then(a.1.cmp(&b.1)));
            for x in 0..items.len() {
                let (bx, _, _, _) = items[x];
                for y in x + 1..items.len() {
                    if items[y].0.min.x > bx.max.x {
                        break;
                    }
                    if items[y].0.min.y <= bx.max.y
                        && bx.min.y <= items[y].0.max.y
                        && ok(&items[x], &items[y])
                    {
                        let (a, b) = (items[x].1.min(items[y].1), items[x].1.max(items[y].1));
                        pairs.push((a, b));
                    }
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        pairs
    }

    fn prepare_joints(&mut self, sb: &mut [SolverBody], inv_dt: f64) {
        for j in self.joints.iter_mut().flatten() {
            let (Some(a), Some(b)) = (
                self.bodies[j.a as usize].as_ref(),
                self.bodies[j.b as usize].as_ref(),
            ) else {
                continue;
            };
            j.ra = a.rot.apply(j.local_a - a.local_c);
            j.rb = b.rot.apply(j.local_b - b.local_c);
            let (ma, mb, ia, ib) = (a.inv_mass, b.inv_mass, a.inv_i, b.inv_i);
            let k11 = ma + mb + ia * j.ra.y * j.ra.y + ib * j.rb.y * j.rb.y;
            let k12 = -ia * j.ra.x * j.ra.y - ib * j.rb.x * j.rb.y;
            let k22 = ma + mb + ia * j.ra.x * j.ra.x + ib * j.rb.x * j.rb.x;
            let det = k11 * k22 - k12 * k12;
            j.mass = if det.abs() > 0.0 {
                let id = 1.0 / det;
                [k22 * id, -k12 * id, -k12 * id, k11 * id]
            } else {
                [0.0; 4]
            };
            let pa = a.c + j.ra;
            let pb = b.c + j.rb;
            j.bias = (pb - pa) * (0.2 * inv_dt);
            // Warm start.
            let imp = j.impulse;
            let a = &mut sb[j.a as usize];
            a.v -= imp * a.inv_mass;
            a.w -= a.inv_i * j.ra.cross(imp);
            let b = &mut sb[j.b as usize];
            b.v += imp * b.inv_mass;
            b.w += b.inv_i * j.rb.cross(imp);
        }
    }

    fn solve_joints(&mut self, sb: &mut [SolverBody]) {
        for j in self.joints.iter_mut().flatten() {
            let (a, b) = (sb[j.a as usize], sb[j.b as usize]);
            let dv = b.v + DVec2::cross_scalar(b.w, j.rb) - a.v - DVec2::cross_scalar(a.w, j.ra);
            let rhs = -(dv + j.bias);
            let imp = DVec2::new(
                j.mass[0] * rhs.x + j.mass[1] * rhs.y,
                j.mass[2] * rhs.x + j.mass[3] * rhs.y,
            );
            j.impulse += imp;
            let a = &mut sb[j.a as usize];
            a.v -= imp * a.inv_mass;
            a.w -= a.inv_i * j.ra.cross(imp);
            let b = &mut sb[j.b as usize];
            b.v += imp * b.inv_mass;
            b.w += b.inv_i * j.rb.cross(imp);
        }
    }
}

/// Contact constraint masses, speculative / Baumgarte biases and restitution targets.
fn prepare_contacts(
    contacts: &mut [Contact],
    sb: &[SolverBody],
    s: &PhysicsSettings,
    inv_dt: f64,
    faults: crate::Faults2d,
) {
    for c in contacts.iter_mut() {
        if c.sensor {
            continue;
        }
        let (a, b) = (sb[c.ba as usize], sb[c.bb as usize]);
        let n = c.normal;
        let t = n.perp() * -1.0;
        for p in &mut c.points[..c.count] {
            p.ra = p.point - a.c;
            p.rb = p.point - b.c;
            let rna = p.ra.cross(n);
            let rnb = p.rb.cross(n);
            let kn = a.inv_mass + b.inv_mass + a.inv_i * rna * rna + b.inv_i * rnb * rnb;
            p.normal_mass = if kn > 0.0 { 1.0 / kn } else { 0.0 };
            let rta = p.ra.cross(t);
            let rtb = p.rb.cross(t);
            let kt = a.inv_mass + b.inv_mass + a.inv_i * rta * rta + b.inv_i * rtb * rtb;
            p.tangent_mass = if kt > 0.0 { 1.0 / kt } else { 0.0 };
            p.bias = if p.separation > 0.0 {
                if faults.no_speculative() {
                    // The fault: ignore a gap entirely (no contact until overlap).
                    f64::INFINITY
                } else {
                    p.separation * inv_dt
                }
            } else {
                (s.baumgarte * inv_dt * (p.separation + s.slop)).min(0.0)
            };
            let dv = b.v + DVec2::cross_scalar(b.w, p.rb) - a.v - DVec2::cross_scalar(a.w, p.ra);
            let vn = dv.dot(n);
            p.bounce = if p.separation <= s.slop && vn < -s.restitution_threshold {
                -c.restitution * vn
            } else {
                0.0
            };
        }
    }
}

/// Apply last step's impulses (matched by feature id) before iterating.
fn warm_start(contacts: &[Contact], sb: &mut [SolverBody]) {
    for c in contacts {
        if c.sensor {
            continue;
        }
        let n = c.normal;
        let t = n.perp() * -1.0;
        for p in &c.points[..c.count] {
            let imp = n * p.normal_impulse + t * p.tangent_impulse;
            let a = &mut sb[c.ba as usize];
            a.v -= imp * a.inv_mass;
            a.w -= a.inv_i * p.ra.cross(imp);
            let b = &mut sb[c.bb as usize];
            b.v += imp * b.inv_mass;
            b.w += b.inv_i * p.rb.cross(imp);
        }
    }
}

/// One velocity iteration over every contact, in pair order (reversed or permuted only by
/// the W2 faults): friction bounded by the normal impulse, then the normal impulse.
fn solve_contacts(
    contacts: &mut [Contact],
    sb: &mut [SolverBody],
    reverse: bool,
    order: Option<&[usize]>,
) {
    let n_c = contacts.len();
    for k in 0..n_c {
        let i = if reverse { n_c - 1 - k } else { k };
        let c = &mut contacts[order.map_or(i, |o| o[i])];
        if c.sensor || c.count == 0 {
            continue;
        }
        let (a, b) = (sb[c.ba as usize], sb[c.bb as usize]);
        let (mut va, mut wa, ima, iia) = (a.v, a.w, a.inv_mass, a.inv_i);
        let (mut vb, mut wb, imb, iib) = (b.v, b.w, b.inv_mass, b.inv_i);
        let n = c.normal;
        let t = n.perp() * -1.0;
        for p in &mut c.points[..c.count] {
            let dv = vb + DVec2::cross_scalar(wb, p.rb) - va - DVec2::cross_scalar(wa, p.ra);
            let vt = dv.dot(t);
            let lambda = -p.tangent_mass * vt;
            let max_f = c.friction * p.normal_impulse;
            let new = (p.tangent_impulse + lambda).clamp(-max_f, max_f);
            let d = new - p.tangent_impulse;
            p.tangent_impulse = new;
            let imp = t * d;
            va -= imp * ima;
            wa -= iia * p.ra.cross(imp);
            vb += imp * imb;
            wb += iib * p.rb.cross(imp);
        }
        for p in &mut c.points[..c.count] {
            if !p.bias.is_finite() {
                continue;
            }
            let dv = vb + DVec2::cross_scalar(wb, p.rb) - va - DVec2::cross_scalar(wa, p.ra);
            let vn = dv.dot(n);
            let lambda = -p.normal_mass * (vn + p.bias - p.bounce);
            let new = (p.normal_impulse + lambda).max(0.0);
            let d = new - p.normal_impulse;
            p.normal_impulse = new;
            let imp = n * d;
            va -= imp * ima;
            wa -= iia * p.ra.cross(imp);
            vb += imp * imb;
            wb += iib * p.rb.cross(imp);
        }
        if a.dynamic {
            sb[c.ba as usize].v = va;
            sb[c.ba as usize].w = wa;
        }
        if b.dynamic {
            sb[c.bb as usize].v = vb;
            sb[c.bb as usize].w = wb;
        }
    }
}

/// Friction mixing: the geometric mean.
fn combine_friction(a: f64, b: f64) -> f64 {
    forge_num::det::sqrt(a * b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: FrameId = FrameId(0);

    fn at(x: f64, y: f64) -> FramePos2 {
        FramePos2::new(F, DVec2::new(x, y))
    }

    fn ground(w: &mut PhysicsWorld2d) -> RigidBodyId {
        let g = w
            .add_body(&BodyDef::new(BodyKind::Static, at(0.0, -0.5)))
            .expect("ground");
        w.add_collider(
            g,
            ColliderDef::new(Shape::Box {
                half: DVec2::new(50.0, 0.5),
            }),
        )
        .expect("collider");
        g
    }

    #[test]
    fn a_falling_box_comes_to_rest_on_the_ground() {
        let mut w = PhysicsWorld2d::new(F);
        ground(&mut w);
        let b = w
            .add_body(&BodyDef::new(BodyKind::Dynamic, at(0.0, 3.0)))
            .expect("b");
        w.add_collider(
            b,
            ColliderDef::new(Shape::Box {
                half: DVec2::new(0.5, 0.5),
            }),
        )
        .expect("c");
        for _ in 0..240 {
            w.step();
        }
        let p = w.position(b).expect("p").local;
        assert!((p.y - 0.5).abs() < 0.02, "rests on the ground: {p:?}");
        assert!(w.linear_velocity(b).expect("v").length() < 1e-3);
        assert!(w.touches(b).iter().any(|t| t.normal.y > 0.99));
    }

    #[test]
    fn frames_other_than_the_worlds_are_refused() {
        let mut w = PhysicsWorld2d::new(F);
        let e = w
            .add_body(&BodyDef::new(
                BodyKind::Dynamic,
                FramePos2::new(FrameId(3), DVec2::ZERO),
            ))
            .expect_err("frame");
        assert_eq!(e.code().as_str(), "TWOD-0002");
    }

    #[test]
    fn a_bouncy_ball_bounces_and_a_dead_one_does_not() {
        for (e, expect_bounce) in [(0.8, true), (0.0, false)] {
            let mut w = PhysicsWorld2d::new(F);
            ground(&mut w);
            let b = w
                .add_body(&BodyDef::new(BodyKind::Dynamic, at(0.0, 5.0)))
                .expect("b");
            let mut c = ColliderDef::new(Shape::Circle { radius: 0.5 });
            c.restitution = e;
            w.add_collider(b, c).expect("c");
            let mut max_up: f64 = 0.0;
            for _ in 0..180 {
                w.step();
                max_up = max_up.max(w.linear_velocity(b).expect("v").y);
            }
            assert_eq!(
                max_up > 3.0,
                expect_bounce,
                "restitution {e}: max upward speed {max_up}"
            );
        }
    }

    #[test]
    fn friction_holds_on_a_gentle_slope_and_not_on_ice() {
        for (mu, holds) in [(0.8, true), (0.05, false)] {
            let mut w = PhysicsWorld2d::new(F);
            let slope = 20f64.to_radians();
            let g = w
                .add_body(&BodyDef {
                    angle: slope,
                    ..BodyDef::new(BodyKind::Static, at(0.0, 0.0))
                })
                .expect("g");
            let mut gc = ColliderDef::new(Shape::Box {
                half: DVec2::new(20.0, 0.5),
            });
            gc.friction = mu;
            w.add_collider(g, gc).expect("gc");
            let r = Rot2::from_angle(slope);
            let start = r.apply(DVec2::new(0.0, 1.0));
            let b = w
                .add_body(&BodyDef {
                    angle: slope,
                    ..BodyDef::new(BodyKind::Dynamic, FramePos2::new(F, start))
                })
                .expect("b");
            let mut bc = ColliderDef::new(Shape::Box {
                half: DVec2::new(0.5, 0.5),
            });
            bc.friction = mu;
            w.add_collider(b, bc).expect("bc");
            for _ in 0..180 {
                w.step();
            }
            let moved = (w.position(b).expect("p").local - start).length();
            assert_eq!(moved < 0.05, holds, "mu {mu}: moved {moved}");
        }
    }

    #[test]
    fn revolute_pendulum_keeps_its_length() {
        let mut w = PhysicsWorld2d::new(F);
        let pivot = w
            .add_body(&BodyDef::new(BodyKind::Static, at(0.0, 10.0)))
            .expect("p");
        let bob = w
            .add_body(&BodyDef::new(BodyKind::Dynamic, at(3.0, 10.0)))
            .expect("b");
        w.add_collider(bob, ColliderDef::new(Shape::Circle { radius: 0.25 }))
            .expect("c");
        w.add_revolute(pivot, bob, at(0.0, 10.0)).expect("j");
        let mut worst: f64 = 0.0;
        for _ in 0..300 {
            w.step();
            let d = (w.position(bob).expect("p").local - DVec2::new(0.0, 10.0)).length();
            worst = worst.max((d - 3.0).abs());
        }
        assert!(worst < 0.05, "length drift {worst}");
    }

    #[test]
    fn raycasts_find_the_nearest_collider_and_masks_filter() {
        let mut w = PhysicsWorld2d::new(F);
        ground(&mut w);
        let b = w
            .add_body(&BodyDef::new(BodyKind::Static, at(0.0, 5.0)))
            .expect("b");
        let mut c = ColliderDef::new(Shape::Circle { radius: 1.0 });
        c.filter = Filter {
            category: 2,
            mask: u32::MAX,
        };
        w.add_collider(b, c).expect("c");
        let hit = w
            .raycast(at(0.0, 10.0), DVec2::new(0.0, -1.0), 100.0, u32::MAX)
            .expect("q")
            .expect("hit");
        assert_eq!(hit.body, b);
        assert!((hit.distance - 4.0).abs() < 1e-9);
        let hit = w
            .raycast(at(0.0, 10.0), DVec2::new(0.0, -1.0), 100.0, 1)
            .expect("q")
            .expect("hit");
        assert!(
            (hit.point.local.y - 0.0).abs() < 1e-9,
            "the circle is masked out: {hit:?}"
        );
    }

    #[test]
    fn sensors_report_but_do_not_push() {
        let mut w = PhysicsWorld2d::new(F);
        ground(&mut w);
        let s = w
            .add_body(&BodyDef::new(BodyKind::Static, at(0.0, 2.0)))
            .expect("s");
        let mut sc = ColliderDef::new(Shape::Box {
            half: DVec2::new(2.0, 0.5),
        });
        sc.sensor = true;
        let sensor = w.add_collider(s, sc).expect("sc");
        let b = w
            .add_body(&BodyDef::new(BodyKind::Dynamic, at(0.0, 4.0)))
            .expect("b");
        let body_col = w
            .add_collider(b, ColliderDef::new(Shape::Circle { radius: 0.3 }))
            .expect("c");
        let mut began = false;
        let mut ended = false;
        for _ in 0..120 {
            w.step();
            for e in w.events() {
                match e {
                    ContactEvent::Begin(x, y) if (*x, *y) == (sensor, body_col) => began = true,
                    ContactEvent::End(x, y) if (*x, *y) == (sensor, body_col) => ended = true,
                    _ => {}
                }
            }
        }
        assert!(began && ended, "the ball passed through the sensor");
        assert!(w.position(b).expect("p").local.y < 0.5);
    }
}
