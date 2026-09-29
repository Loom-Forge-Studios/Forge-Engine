//! The backend-neutral data model: what a game or the editor describes, and what every
//! backend on `forge.phys.backend` builds from the same description.
//!
//! Positions are [`FramePos`] (I1): a physics world is region-local (Ch.17), so every
//! position given to it or read from it is in its one frame. Offsets inside a body (a
//! collider's offset, a joint's anchor) are in the body's own frame and are not positions.

use forge_frames::{DQuat, DVec3, FramePos};

/// A rigid body of a [`crate::PhysicsWorld`]. Ids are dense and never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BodyId(pub u32);

/// A collider (attached to one body).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColliderId(pub u32);

/// A joint between two bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JointId(pub u32);

/// A gravity or damping zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZoneId(pub u32);

/// How a body moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BodyKind {
    /// Never moves (level geometry).
    #[default]
    Static,
    /// Moved by the game (a target pose or a velocity); nothing pushes it (moving platforms).
    Kinematic,
    /// Moved by forces, gravity, contacts and joints.
    Dynamic,
}

/// Axes a dynamic body may not move or turn along (in the world frame's axes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AxisLocks {
    pub translation: [bool; 3],
    pub rotation: [bool; 3],
}

impl AxisLocks {
    /// No axis locked.
    pub const NONE: Self = Self {
        translation: [false; 3],
        rotation: [false; 3],
    };
    /// Every rotation locked: an upright character capsule.
    pub const ROTATION: Self = Self {
        translation: [false; 3],
        rotation: [true; 3],
    };
}

/// A body's starting state and settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyDesc {
    pub kind: BodyKind,
    pub position: FramePos,
    pub rotation: DQuat,
    /// m/s, in the frame's axes.
    pub linear_velocity: DVec3,
    /// rad/s, in the frame's axes.
    pub angular_velocity: DVec3,
    /// 1/s: velocity decays as `v / (1 + c dt)` per step.
    pub linear_damping: f64,
    pub angular_damping: f64,
    /// Multiplies gravity (and a gravity zone's) for this body.
    pub gravity_scale: f64,
    /// Sweep this body against moving bodies too (a bullet hitting a moving target). Every
    /// fast body is already swept against static geometry while the world's
    /// `PhysicsSettings::continuous` is on.
    pub ccd: bool,
    pub locks: AxisLocks,
    /// May fall asleep when at rest (a sleeping body costs nothing until touched).
    pub can_sleep: bool,
    /// Free for the game.
    pub user: u64,
}

impl BodyDesc {
    /// A body of `kind` at `position`, at rest, unrotated.
    #[must_use]
    pub fn new(kind: BodyKind, position: FramePos) -> Self {
        Self {
            kind,
            position,
            rotation: DQuat::IDENTITY,
            linear_velocity: DVec3::ZERO,
            angular_velocity: DVec3::ZERO,
            linear_damping: 0.0,
            angular_damping: 0.0,
            gravity_scale: 1.0,
            ccd: false,
            locks: AxisLocks::NONE,
            can_sleep: true,
            user: 0,
        }
    }

    /// A dynamic body at `position`.
    #[must_use]
    pub fn dynamic(position: FramePos) -> Self {
        Self::new(BodyKind::Dynamic, position)
    }

    /// A static body at `position`.
    #[must_use]
    pub fn fixed(position: FramePos) -> Self {
        Self::new(BodyKind::Static, position)
    }
}

/// A collider's shape, in its body's frame (after the collider's own offset and rotation).
/// Round shapes along an axis (capsule, cylinder, cone) are along the body's `y`.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Sphere {
        radius: f64,
    },
    /// A box of half extents `half_extents`.
    Cuboid {
        half_extents: DVec3,
    },
    /// The segment `(0, -half_height, 0)`-`(0, half_height, 0)` rounded by `radius`.
    Capsule {
        half_height: f64,
        radius: f64,
    },
    Cylinder {
        half_height: f64,
        radius: f64,
    },
    /// Apex at `+half_height`, base disc of `radius` at `-half_height`.
    Cone {
        half_height: f64,
        radius: f64,
    },
    /// The convex hull of `vertices` — auto-generated from a mesh's vertices (the backend
    /// computes the hull; points inside it are dropped).
    ConvexHull {
        vertices: Vec<DVec3>,
    },
    /// A concave triangle mesh (static or kinematic level geometry). Triangles wind
    /// counter-clockwise seen from outside.
    TriMesh {
        vertices: Vec<DVec3>,
        indices: Vec<[u32; 3]>,
    },
    /// A height grid of `rows` x `cols` samples, row-major (`heights[r * cols + c]`),
    /// centred on the collider: column `c` is at `x = (c / (cols - 1) - 0.5) * size.x`, row
    /// `r` at `z = (r / (rows - 1) - 0.5) * size.z`, and a sample's height is `h * size.y`.
    HeightField {
        rows: u32,
        cols: u32,
        heights: Vec<f64>,
        size: DVec3,
    },
}

impl Shape {
    /// A cuboid of full `size`.
    #[must_use]
    pub fn cuboid(size: DVec3) -> Self {
        Self::Cuboid {
            half_extents: size * 0.5,
        }
    }

    /// Validate the numbers (every backend refuses the same inputs, with the same code).
    pub fn check(&self) -> Result<(), crate::PhysError> {
        use crate::PhysError as E;
        let pos = |x: f64, what: &str| {
            if x.is_finite() && x > 0.0 {
                Ok(())
            } else {
                Err(E::invalid(format!("{what} must be finite and > 0, got {x}")))
            }
        };
        match self {
            Self::Sphere { radius } => pos(*radius, "a sphere's radius"),
            Self::Cuboid { half_extents: h } => {
                pos(h.x, "a box's half extent")?;
                pos(h.y, "a box's half extent")?;
                pos(h.z, "a box's half extent")
            }
            Self::Capsule {
                half_height,
                radius,
            }
            | Self::Cylinder {
                half_height,
                radius,
            }
            | Self::Cone {
                half_height,
                radius,
            } => {
                pos(*half_height, "a half height")?;
                pos(*radius, "a radius")
            }
            Self::ConvexHull { vertices } => {
                if vertices.len() < 4 || vertices.iter().any(|v| !v.is_finite()) {
                    return Err(E::invalid(
                        "a convex hull needs at least 4 finite vertices that are not coplanar",
                    ));
                }
                Ok(())
            }
            Self::TriMesh { vertices, indices } => {
                if indices.is_empty() || vertices.iter().any(|v| !v.is_finite()) {
                    return Err(E::invalid(
                        "a triangle mesh needs at least one triangle and finite vertices",
                    ));
                }
                let n = vertices.len() as u64;
                if indices.iter().flatten().any(|&i| u64::from(i) >= n) {
                    return Err(E::invalid(format!(
                        "a triangle names a vertex past the last ({n} vertices)"
                    )));
                }
                Ok(())
            }
            Self::HeightField {
                rows,
                cols,
                heights,
                size,
            } => {
                if *rows < 2 || *cols < 2 {
                    return Err(E::invalid("a height field needs at least 2x2 samples"));
                }
                if heights.len() as u64 != u64::from(*rows) * u64::from(*cols) {
                    return Err(E::invalid(format!(
                        "a {rows}x{cols} height field needs {} heights, got {}",
                        u64::from(*rows) * u64::from(*cols),
                        heights.len()
                    )));
                }
                if heights.iter().any(|h| !h.is_finite()) {
                    return Err(E::invalid("a height is not finite"));
                }
                pos(size.x, "a height field's size")?;
                pos(size.y, "a height field's size")?;
                pos(size.z, "a height field's size")
            }
        }
    }
}

/// How two colliders' coefficients combine into the contact's (the higher-priority rule of
/// the two wins: `Max` > `Multiply` > `Min` > `Average`, as in the major engines).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum CombineRule {
    #[default]
    Average,
    Min,
    Multiply,
    Max,
}

impl CombineRule {
    /// Combine `a` and `b` under `self`.
    #[must_use]
    pub fn apply(self, a: f64, b: f64) -> f64 {
        match self {
            Self::Average => (a + b) * 0.5,
            Self::Min => a.min(b),
            Self::Multiply => a * b,
            Self::Max => a.max(b),
        }
    }
}

/// A physics material: surface response and mass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// Coulomb friction coefficient (>= 0).
    pub friction: f64,
    /// Bounciness in [0, 1].
    pub restitution: f64,
    /// kg/m³ (> 0): a dynamic body's mass is the sum of its colliders' volume x density.
    pub density: f64,
    pub friction_combine: CombineRule,
    pub restitution_combine: CombineRule,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            friction: 0.5,
            restitution: 0.0,
            density: 1000.0,
            friction_combine: CombineRule::Average,
            restitution_combine: CombineRule::Average,
        }
    }
}

impl Material {
    fn check(&self) -> Result<(), crate::PhysError> {
        let ok = self.friction.is_finite()
            && self.friction >= 0.0
            && self.restitution.is_finite()
            && (0.0..=1.0).contains(&self.restitution)
            && self.density.is_finite()
            && self.density > 0.0;
        if ok {
            Ok(())
        } else {
            Err(crate::PhysError::invalid(format!(
                "a material needs friction >= 0, restitution in [0, 1] and density > 0: {self:?}"
            )))
        }
    }
}

/// Collision layers: `a` and `b` touch when `a.memberships & b.filters != 0` and
/// `b.memberships & a.filters != 0` (32 layers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layers {
    pub memberships: u32,
    pub filters: u32,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            memberships: 1,
            filters: u32::MAX,
        }
    }
}

impl Layers {
    /// Do colliders on these layers touch?
    #[must_use]
    pub fn interacts(self, o: Self) -> bool {
        self.memberships & o.filters != 0 && o.memberships & self.filters != 0
    }
}

/// A collider's definition.
#[derive(Clone, Debug, PartialEq)]
pub struct ColliderDesc {
    pub shape: Shape,
    /// Offset from the body origin, in the body's frame.
    pub offset: DVec3,
    /// Rotation relative to the body.
    pub rotation: DQuat,
    pub material: Material,
    pub layers: Layers,
    /// A trigger: reports enter and exit, never pushes.
    pub sensor: bool,
    /// Free for the game.
    pub user: u64,
}

impl ColliderDesc {
    /// A solid collider of `shape` with the default material, at the body origin.
    #[must_use]
    pub fn new(shape: Shape) -> Self {
        Self {
            shape,
            offset: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            material: Material::default(),
            layers: Layers::default(),
            sensor: false,
            user: 0,
        }
    }

    /// A trigger of `shape`.
    #[must_use]
    pub fn sensor(shape: Shape) -> Self {
        Self {
            sensor: true,
            ..Self::new(shape)
        }
    }

    pub(crate) fn check(&self) -> Result<(), crate::PhysError> {
        self.shape.check()?;
        self.material.check()?;
        if !self.offset.is_finite() || !self.rotation.is_finite() {
            return Err(crate::PhysError::invalid(
                "a collider's offset or rotation is not finite",
            ));
        }
        Ok(())
    }
}

/// What one axis of a [`JointKind::SixDof`] joint allows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AxisMotion {
    Locked,
    Free,
    /// Between `min` and `max` (metres for a linear axis, radians for an angular one).
    Limited { min: f64, max: f64 },
}

/// The joint set (M7-9): what the joint lets the two bodies do relative to each other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JointKind {
    /// No relative motion.
    Fixed,
    /// Rotation about the joint axis only (a door), optionally limited (radians).
    Hinge { limits: Option<(f64, f64)> },
    /// Translation along the joint axis only (a piston), optionally limited (metres).
    Slider { limits: Option<(f64, f64)> },
    /// The anchors are pulled toward `rest_length` apart by a damped spring (N/m, N·s/m);
    /// rotation is free.
    Spring {
        rest_length: f64,
        stiffness: f64,
        damping: f64,
    },
    /// A ball joint whose joint axis may swing at most `swing` radians off its rest
    /// direction and twist about itself within `twist` (a shoulder, a ragdoll limb).
    ConeTwist { swing: f64, twist: (f64, f64) },
    /// Each axis of the joint frame set on its own: linear x, y, z, then angular x, y, z.
    SixDof { axes: [AxisMotion; 6] },
}

/// A joint's definition. The joint frame sits at `anchor_a` on body A with its x axis along
/// `axis` (both in A's frame); at creation it coincides with the matching frame on B
/// (`anchor_b` in B's frame), so a joint starts at rest wherever the bodies are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointDesc {
    pub body_a: BodyId,
    pub body_b: BodyId,
    pub anchor_a: DVec3,
    pub anchor_b: DVec3,
    /// The joint axis (hinge axis, slider axis, cone axis, the 6DOF frame's x), in A's frame.
    pub axis: DVec3,
    pub kind: JointKind,
    /// Whether the two bodies' colliders still collide with each other.
    pub collide_connected: bool,
}

impl JointDesc {
    /// A joint of `kind` between `a` and `b` with anchors at the bodies' origins and the axis
    /// along A's x.
    #[must_use]
    pub fn new(body_a: BodyId, body_b: BodyId, kind: JointKind) -> Self {
        Self {
            body_a,
            body_b,
            anchor_a: DVec3::ZERO,
            anchor_b: DVec3::ZERO,
            axis: DVec3::X,
            kind,
            collide_connected: false,
        }
    }
}

/// The joint frames a backend builds from: anchor and basis on each body (the basis's x is
/// the joint axis), computed once by the world so every backend gets the same frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointFrames {
    pub anchor_a: DVec3,
    pub basis_a: DQuat,
    pub anchor_b: DVec3,
    pub basis_b: DQuat,
}

/// A zone's volume, centred on its position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoneShape {
    Sphere { radius: f64 },
    Box { half_extents: DVec3 },
}

/// What a zone does to the dynamic bodies whose origin is inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoneEffect {
    /// Gravity is `acceleration` (m/s², frame axes) in here instead of the world's.
    Gravity { acceleration: DVec3 },
    /// Gravity pulls toward the zone's centre with `strength` m/s² (a small planet).
    PointGravity { strength: f64 },
    /// Extra damping in here (1/s), on top of the body's own (water, thick air).
    Damping { linear: f64, angular: f64 },
}

/// A gravity or damping zone (Unity's and Godot's areas, UE's physics volumes). Where zones
/// of the same kind overlap, the highest `priority` wins (ties: the lowest id); gravity and
/// damping zones combine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneDesc {
    pub position: FramePos,
    pub rotation: DQuat,
    pub shape: ZoneShape,
    pub effect: ZoneEffect,
    pub priority: i32,
    /// Only bodies with a collider on one of these layers are affected.
    pub layers: u32,
}

impl ZoneDesc {
    /// A zone of `shape` at `position` doing `effect`, priority 0, every layer.
    #[must_use]
    pub fn new(position: FramePos, shape: ZoneShape, effect: ZoneEffect) -> Self {
        Self {
            position,
            rotation: DQuat::IDENTITY,
            shape,
            effect,
            priority: 0,
            layers: u32::MAX,
        }
    }
}

/// A body's dynamic state, as a backend reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyState {
    pub position: FramePos,
    pub rotation: DQuat,
    pub linear_velocity: DVec3,
    pub angular_velocity: DVec3,
}

/// What a query may hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryFilter {
    /// Colliders with a membership in this mask.
    pub layers: u32,
    /// Hit triggers too.
    pub include_sensors: bool,
    /// Never hit this body's colliders (the caster itself).
    pub exclude_body: Option<BodyId>,
}

impl Default for QueryFilter {
    fn default() -> Self {
        Self {
            layers: u32::MAX,
            include_sensors: false,
            exclude_body: None,
        }
    }
}

/// A ray: from `start` along the unit `direction`, up to `max_distance`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub start: FramePos,
    pub direction: DVec3,
    pub max_distance: f64,
}

/// A ray or shape cast's hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub collider: ColliderId,
    pub body: BodyId,
    /// Along the ray or sweep from its start.
    pub distance: f64,
    /// Where the ray or the swept shape first touches.
    pub point: FramePos,
    /// The surface normal there (unit, frame axes).
    pub normal: DVec3,
}

/// A shape swept from `start` along the unit `direction` up to `max_distance`.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeCast {
    pub shape: Shape,
    pub start: FramePos,
    pub rotation: DQuat,
    pub direction: DVec3,
    pub max_distance: f64,
}

/// A shape at rest, for an overlap query.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlap {
    pub shape: Shape,
    pub position: FramePos,
    pub rotation: DQuat,
}

/// One query of a batch or an async request.
#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    Ray(Ray),
    Shape(ShapeCast),
    Overlap(Overlap),
}

/// A query's answer.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryResult {
    /// A ray or shape cast: the first hit, if any.
    Hit(Option<Hit>),
    /// An overlap: every collider touching the shape, in id order.
    Overlaps(Vec<ColliderId>),
}

/// What happened during a step, in a canonical order (kind, then ids) so the same step gives
/// the same list on every backend and every machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PhysEvent {
    /// Two solid colliders started touching (lower id first).
    ContactBegin(ColliderId, ColliderId),
    /// Two solid colliders stopped touching.
    ContactEnd(ColliderId, ColliderId),
    /// A collider entered a trigger.
    TriggerEnter {
        trigger: ColliderId,
        other: ColliderId,
    },
    /// A collider left a trigger.
    TriggerExit {
        trigger: ColliderId,
        other: ColliderId,
    },
}
