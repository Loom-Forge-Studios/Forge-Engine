//! The kinematic character controller (M4-3): an upright capsule moved by the game, not by
//! forces, that collides and slides — the controller every engine ships under its
//! character movement (UE's CharacterMovementComponent, Unity's CharacterController,
//! Godot's `move_and_slide`).
//!
//! [`Character::move_and_slide`] takes the displacement the game wants this step (its own
//! walking, jumping and gravity: movement modes are WP-61, on top of this) and moves the
//! capsule as far as it can: it sweeps the capsule, stops a skin short of what it hits, and
//! slides the rest along the surface — up to four times, so it runs along a corner. A surface
//! no steeper than `max_slope` is ground (the character stands on it and walking up it is
//! walking); a steeper one is a wall (it is slid along horizontally, never climbed); one
//! facing down is a ceiling. A wall no taller than `step_height` is stepped up onto. While
//! grounded, the capsule snaps down onto ground within `snap` (walking down a slope or off a
//! small ledge keeps it on the ground instead of skipping into the air).
//!
//! Everything is shape casts against the physics world, so it is the same on every backend.
//! The capsule is also a kinematic body in that world, driven to where the controller put
//! it by the next step: dynamic bodies are pushed by it and queries see it.

use forge_frames::{DQuat, DVec3, FramePos};

use crate::types::{
    AxisLocks, BodyDesc, BodyId, BodyKind, ColliderDesc, Hit, Layers, QueryFilter, Shape, ShapeCast,
};
use crate::{PhysError, PhysicsWorld};

/// A character's shape and how it moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterDesc {
    /// The capsule: its radius and the half height of its straight part (total height
    /// `2 (half_height + radius)`), upright along y.
    pub radius: f64,
    pub half_height: f64,
    /// The steepest ground it stands on and walks up, radians from horizontal.
    pub max_slope: f64,
    /// The tallest step it walks up onto.
    pub step_height: f64,
    /// How far down it stays glued to the ground while walking.
    pub snap: f64,
    /// The gap it keeps from surfaces (a sweep that ends exactly on a surface would start
    /// the next one touching it).
    pub skin: f64,
    /// What it collides with: colliders with a membership in this mask.
    pub collides_with: u32,
    /// Its own collider's layers (what collides with it).
    pub layers: Layers,
}

impl Default for CharacterDesc {
    /// A 1.8 m person: radius 0.3, 45-degree slopes, 0.35 m steps.
    fn default() -> Self {
        Self {
            radius: 0.3,
            half_height: 0.6,
            max_slope: std::f64::consts::FRAC_PI_4,
            step_height: 0.35,
            snap: 0.3,
            skin: 0.01,
            collides_with: u32::MAX,
            layers: Layers::default(),
        }
    }
}

/// What one move did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveResult {
    /// The displacement actually made.
    pub moved: DVec3,
    /// Standing on ground after the move.
    pub grounded: bool,
    /// The ground's normal, when grounded.
    pub ground_normal: DVec3,
    /// A wall stopped part of the move.
    pub hit_wall: bool,
    /// A ceiling stopped part of the move.
    pub hit_ceiling: bool,
    /// The move stepped up onto a step.
    pub stepped: bool,
    /// The bodies it touched, with the surface normal (the first few, in move order). Its
    /// kinematic body pushes free bodies it walks into; a game that wants more (a shove, a
    /// door that opens) acts on these.
    pub touched: [Option<(BodyId, DVec3)>; ITERATIONS],
}

impl MoveResult {
    fn touch(&mut self, h: &Hit) {
        if self.touched.iter().flatten().any(|(b, _)| *b == h.body) {
            return;
        }
        if let Some(slot) = self.touched.iter_mut().find(|t| t.is_none()) {
            *slot = Some((h.body, h.normal));
        }
    }
}

/// A kinematic character (see the module docs).
#[derive(Clone, Debug)]
pub struct Character {
    desc: CharacterDesc,
    body: BodyId,
    /// The capsule's centre.
    at: FramePos,
    grounded: bool,
    ground_normal: DVec3,
}

/// How many sweeps one move may make (the fourth slides along a corner's second wall).
pub const ITERATIONS: usize = 4;
/// Below this a remaining move is done (1 micrometre).
const EPS: f64 = 1e-6;

impl Character {
    /// A character with its capsule centred at `at`, and its kinematic body in `world`.
    pub fn new(
        world: &mut PhysicsWorld,
        desc: CharacterDesc,
        at: FramePos,
    ) -> Result<Character, PhysError> {
        let ok = [
            desc.radius,
            desc.half_height,
            desc.max_slope,
            desc.step_height,
            desc.snap,
            desc.skin,
        ]
        .iter()
        .all(|x| x.is_finite() && *x >= 0.0)
            && desc.radius > 0.0
            && desc.half_height > 0.0
            && desc.skin > 0.0
            && desc.max_slope < std::f64::consts::FRAC_PI_2;
        if !ok {
            return Err(PhysError::invalid(format!(
                "a character needs a positive radius, half height and skin, a slope under 90 degrees and no negative sizes: {desc:?}"
            )));
        }
        let body = world.add_body(&BodyDesc {
            locks: AxisLocks::ROTATION,
            ..BodyDesc::new(BodyKind::Kinematic, at)
        })?;
        world.add_collider(
            body,
            &ColliderDesc {
                layers: desc.layers,
                ..ColliderDesc::new(Self::capsule(&desc))
            },
        )?;
        Ok(Character {
            desc,
            body,
            at,
            grounded: false,
            ground_normal: DVec3::Y,
        })
    }

    fn capsule(d: &CharacterDesc) -> Shape {
        Shape::Capsule {
            half_height: d.half_height,
            radius: d.radius,
        }
    }

    /// Its body in the physics world.
    #[must_use]
    pub fn body(&self) -> BodyId {
        self.body
    }

    /// The capsule's centre.
    #[must_use]
    pub fn position(&self) -> FramePos {
        self.at
    }

    #[must_use]
    pub fn is_grounded(&self) -> bool {
        self.grounded
    }

    #[must_use]
    pub fn desc(&self) -> &CharacterDesc {
        &self.desc
    }

    /// Teleport it (its body follows at the next step).
    pub fn set_position(
        &mut self,
        world: &mut PhysicsWorld,
        at: FramePos,
    ) -> Result<(), PhysError> {
        world.set_kinematic_target(self.body, at, DQuat::IDENTITY)?;
        self.at = at;
        self.grounded = false;
        Ok(())
    }

    fn filter(&self) -> QueryFilter {
        QueryFilter {
            layers: self.desc.collides_with,
            include_sensors: false,
            exclude_body: Some(self.body),
        }
    }

    fn walkable(&self, n: DVec3) -> bool {
        n.y >= forge_num::det::cos(self.desc.max_slope) - 1e-9
    }

    /// The ground under a downward sweep's hit: its normal if it is walkable. The capsule's
    /// round bottom touching an edge reports the edge's slanted normal; short rays straight
    /// down just past the contact — ahead in the direction of travel (stepping up onto an
    /// edge), then behind it (rolling off one) — find the face actually stood on.
    fn ground_under(
        &self,
        world: &PhysicsWorld,
        h: &Hit,
        travel: DVec3,
    ) -> Result<Option<DVec3>, PhysError> {
        if self.walkable(h.normal) {
            return Ok(Some(h.normal));
        }
        let ahead = DVec3::new(travel.x, 0.0, travel.z)
            .try_normalize()
            .map_or(DVec3::ZERO, |u| u * (2.0 * self.desc.skin));
        let probe = 4.0 * self.desc.skin;
        for offset in [ahead, -ahead] {
            if offset == DVec3::ZERO {
                continue;
            }
            let from = h.point.local + offset + DVec3::new(0.0, probe, 0.0);
            let under = world.raycast(
                &crate::types::Ray {
                    start: FramePos::new(self.at.frame, from),
                    direction: DVec3::new(0.0, -1.0, 0.0),
                    max_distance: 2.0 * probe,
                },
                &self.filter(),
            )?;
            if let Some(n) = under.map(|u| u.normal).filter(|n| self.walkable(*n)) {
                return Ok(Some(n));
            }
        }
        Ok(None)
    }

    /// Sweep the capsule from `from` by `d`: the first hit within `|d|` plus the skin.
    fn sweep(
        &self,
        world: &PhysicsWorld,
        from: DVec3,
        d: DVec3,
    ) -> Result<Option<(DVec3, Hit)>, PhysError> {
        let len = d.length();
        if len < EPS {
            return Ok(None);
        }
        let dir = d / len;
        let hit = world.shape_cast(
            &ShapeCast {
                shape: Self::capsule(&self.desc),
                start: FramePos::new(self.at.frame, from),
                rotation: DQuat::IDENTITY,
                direction: dir,
                max_distance: len + self.desc.skin,
            },
            &self.filter(),
        )?;
        Ok(hit.map(|h| (dir, h)))
    }

    /// Move `from` by `d`, colliding and sliding. Returns where it ends and what it hit.
    fn slide(
        &self,
        world: &PhysicsWorld,
        from: DVec3,
        d: DVec3,
        r: &mut MoveResult,
        grounded: bool,
    ) -> Result<(DVec3, Option<Hit>), PhysError> {
        let mut at = from;
        let mut rest = d;
        let mut wall = None;
        for _ in 0..ITERATIONS {
            if rest.length() < EPS {
                break;
            }
            let Some((dir, hit)) = self.sweep(world, at, rest)? else {
                at += rest;
                break;
            };
            let travel = (hit.distance - self.desc.skin).max(0.0);
            at += dir * travel;
            rest -= dir * travel;
            r.touch(&hit);
            let mut n = hit.normal;
            if self.walkable(n) {
                r.grounded = true;
                r.ground_normal = n;
            } else if n.y < -0.5 {
                r.hit_ceiling = true;
            } else {
                r.hit_wall = true;
                wall = Some(hit);
                if grounded || rest.y <= 0.0 {
                    // A wall is slid along horizontally: never climbed.
                    n = DVec3::new(n.x, 0.0, n.z).try_normalize().unwrap_or(n);
                }
            }
            let into = rest.dot(n);
            if into < 0.0 {
                rest -= n * into;
            }
        }
        Ok((at, wall))
    }

    /// Move by `desired` this step, colliding and sliding (see the module docs); the body
    /// follows at the next physics step.
    pub fn move_and_slide(
        &mut self,
        world: &mut PhysicsWorld,
        desired: DVec3,
    ) -> Result<MoveResult, PhysError> {
        if !desired.is_finite() {
            return Err(PhysError::invalid("a character's move is not finite"));
        }
        let start = self.at.local;
        let mut r = MoveResult {
            moved: DVec3::ZERO,
            grounded: false,
            ground_normal: DVec3::Y,
            hit_wall: false,
            hit_ceiling: false,
            stepped: false,
            touched: [None; ITERATIONS],
        };
        let was_grounded = self.grounded;
        // Walking on ground moves along the ground: the horizontal part of the move is
        // turned onto the ground plane, so a slope is walked up, not into.
        let mut d = desired;
        if was_grounded && d.y <= 0.0 {
            let n = self.ground_normal;
            let h = DVec3::new(d.x, 0.0, d.z);
            let along = h - n * h.dot(n);
            let hl = h.length();
            d = along.try_normalize().map_or(DVec3::ZERO, |u| u * hl) + DVec3::new(0.0, d.y, 0.0);
        }
        let (mut at, wall) = self.slide(world, start, d, &mut r, was_grounded)?;
        // A step: go up, over, and back down onto walkable ground.
        if let Some(w) = wall
            && self.desc.step_height > 0.0
            && (was_grounded || r.grounded)
        {
            let horiz = DVec3::new(desired.x, 0.0, desired.z);
            let done = DVec3::new(at.x - start.x, 0.0, at.z - start.z);
            let left = horiz - done;
            // Any wall below a ceiling: a step's face, or its edge under the round bottom.
            if left.length() > EPS && w.normal.y > -0.5 {
                let mut probe = r;
                let up = DVec3::new(0.0, self.desc.step_height, 0.0);
                let (top, _) = self.slide(world, at, up, &mut probe, false)?;
                let (over, blocked) = self.slide(world, top, left, &mut probe, false)?;
                let down = DVec3::new(0.0, -(top.y - at.y) - self.desc.skin, 0.0);
                let landing = self.sweep(world, over, down)?;
                let went = DVec3::new(over.x - at.x, 0.0, over.z - at.z).length();
                if let Some((dir, h)) = landing
                    && went > EPS
                    && (blocked.is_none() || went > 0.5 * left.length())
                    && let Some(n) = self.ground_under(world, &h, left)?
                {
                    at = over + dir * (h.distance - self.desc.skin).max(0.0);
                    r.stepped = true;
                    r.grounded = true;
                    r.ground_normal = n;
                }
            }
        }
        // Stay on the ground: snap down onto walkable ground within `snap` while walking.
        if was_grounded && !r.grounded && desired.y <= 0.0 && self.desc.snap > 0.0 {
            let down = DVec3::new(0.0, -self.desc.snap, 0.0);
            if let Some((dir, h)) = self.sweep(world, at, down)?
                && let Some(n) = self.ground_under(world, &h, desired)?
            {
                at += dir * (h.distance - self.desc.skin).max(0.0);
                r.grounded = true;
                r.ground_normal = n;
            }
        }
        // Standing: is there ground just below (within twice the skin)?
        if !r.grounded && desired.y <= 0.0 {
            let down = DVec3::new(0.0, -2.0 * self.desc.skin, 0.0);
            if let Some((_, h)) = self.sweep(world, at, down)?
                && let Some(n) = self.ground_under(world, &h, desired)?
            {
                r.grounded = true;
                r.ground_normal = n;
            }
        }
        self.at = FramePos::new(self.at.frame, at);
        self.grounded = r.grounded;
        self.ground_normal = r.ground_normal;
        r.moved = at - start;
        world.set_kinematic_target(self.body, self.at, DQuat::IDENTITY)?;
        Ok(r)
    }
}
