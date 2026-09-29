//! Physics while playing (WP-60, Ch.17): the edit world's rigid bodies simulated by
//! `forge-phys`.
//!
//! An entity with a `physics.body` property (see [`crate::edit::P_BODY`]) becomes a body in
//! its frame's physics world — one region-local world per frame, in frame order, as the
//! scheduler has one region per frame — on the backend the project's `physics.backend`
//! setting names (the registry's default otherwise). Each simulation step runs the
//! scheduler's systems for everything else, then every physics world's fixed step, then
//! writes the bodies' poses back into the simulation's `Placement` and `Orientation`, so the
//! viewport, the recorder and the state hash see physics bodies like any other.
//!
//! A simulation without physics bodies builds no physics world at all: nothing here runs.

use std::collections::BTreeMap;

use forge_cmd::{EntityKey, Value};
use forge_core::EntityId;
use forge_frames::{DQuat, DVec3, FrameId, FramePos};
use forge_phys::{
    BodyDesc, BodyId, BodyKind, ColliderDesc, Layers, Material, PhysicsBackendPoint,
    PhysicsSettings, PhysicsWorld, Shape,
};
use forge_plugin::Registry;

use crate::SimError;
use crate::edit::{
    EditEntity, P_BODY, P_CCD, P_DENSITY, P_FRICTION, P_GRAVITY_SCALE, P_LAYERS, P_MASK,
    P_RESTITUTION, P_SENSOR, P_SHAPE, P_SIZE, S_BACKEND, S_GRAVITY,
};
use crate::world::{InputAction, Orientation, Placement, SIM_DT};

/// What an edit entity's `physics.*` properties ask for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spec {
    kind: BodyKind,
    collider: ColliderDesc,
    gravity_scale: f64,
    ccd: bool,
}

fn float(p: &BTreeMap<String, Value>, k: &str, d: f64) -> f64 {
    match p.get(k) {
        Some(Value::Float(v)) => *v,
        Some(Value::Int(v)) => *v as f64,
        _ => d,
    }
}

fn mask(p: &BTreeMap<String, Value>, k: &str, d: u32) -> u32 {
    match p.get(k) {
        Some(Value::Int(v)) => u32::try_from(*v).unwrap_or(d),
        _ => d,
    }
}

impl Spec {
    /// The physics of edit entity `e` (uniform scale `scale`), if it has a `physics.body`.
    pub(crate) fn of(e: &EditEntity, scale: f64) -> Result<Option<Spec>, SimError> {
        let p = &e.properties;
        let bad = |detail: String| SimError::BadEditWorld {
            entity: e.key,
            detail,
        };
        let kind = match p.get(P_BODY) {
            None => return Ok(None),
            Some(Value::Text(t)) => match t.as_str() {
                "dynamic" => BodyKind::Dynamic,
                "kinematic" => BodyKind::Kinematic,
                "static" => BodyKind::Static,
                other => {
                    return Err(bad(format!(
                        "{P_BODY} = \"{other}\": expected \"dynamic\", \"kinematic\" or \"static\""
                    )));
                }
            },
            Some(v) => return Err(bad(format!("{P_BODY} = {v:?} is not text"))),
        };
        let size = match p.get(P_SIZE) {
            Some(Value::Vec3(v)) => DVec3::new(v[0], v[1], v[2]),
            _ => DVec3::splat(1.0),
        } * scale;
        let shape = match p.get(P_SHAPE) {
            None => "box",
            Some(Value::Text(t)) => t.as_str(),
            Some(v) => return Err(bad(format!("{P_SHAPE} = {v:?} is not text"))),
        };
        let (r, hh) = (0.5 * size.x, 0.5 * size.y);
        let shape = match shape {
            "box" => Shape::Cuboid {
                half_extents: size * 0.5,
            },
            "sphere" => Shape::Sphere { radius: r },
            "capsule" => Shape::Capsule {
                half_height: hh - r,
                radius: r,
            },
            "cylinder" => Shape::Cylinder {
                half_height: hh,
                radius: r,
            },
            "cone" => Shape::Cone {
                half_height: hh,
                radius: r,
            },
            other => {
                return Err(bad(format!(
                    "{P_SHAPE} = \"{other}\": expected box, sphere, capsule, cylinder or cone"
                )));
            }
        };
        shape.check().map_err(|e| {
            bad(format!(
                "its collider: {e} (a capsule is taller than it is wide)"
            ))
        })?;
        let d = Material::default();
        let collider = ColliderDesc {
            material: Material {
                density: float(p, P_DENSITY, d.density),
                friction: float(p, P_FRICTION, d.friction),
                restitution: float(p, P_RESTITUTION, d.restitution),
                ..d
            },
            sensor: p.get(P_SENSOR) == Some(&Value::Bool(true)),
            layers: Layers {
                memberships: mask(p, P_LAYERS, Layers::default().memberships),
                filters: mask(p, P_MASK, Layers::default().filters),
            },
            ..ColliderDesc::new(shape)
        };
        Ok(Some(Spec {
            kind,
            collider,
            gravity_scale: float(p, P_GRAVITY_SCALE, 1.0),
            ccd: p.get(P_CCD) == Some(&Value::Bool(true)),
        }))
    }
}

/// One edit entity's body, as the fork hands it over.
pub(crate) struct Fork {
    pub key: EntityKey,
    pub id: EntityId,
    pub at: FramePos,
    pub rot: DQuat,
    pub spec: Spec,
    pub velocity: DVec3,
    /// Degrees per second about the frame's up axis.
    pub spin: f64,
    pub accel: DVec3,
}

#[derive(Clone, Copy, Debug)]
struct Body {
    world: usize,
    body: BodyId,
    id: EntityId,
    kind: BodyKind,
    /// A constant acceleration on top of gravity (`motion.acceleration`, `SetAcceleration`).
    accel: DVec3,
}

/// The simulation's physics (see the module docs).
pub(crate) struct SimPhysics {
    worlds: Vec<(FrameId, PhysicsWorld)>,
    bodies: BTreeMap<EntityKey, Body>,
}

fn phys(e: forge_phys::PhysError) -> SimError {
    SimError::Physics(e.to_string())
}

impl SimPhysics {
    /// The physics worlds for `bodies` (`None` when there are none), on the backend
    /// `settings` names from `backends`.
    pub(crate) fn build(
        settings: &BTreeMap<String, Value>,
        backends: &Registry<PhysicsBackendPoint>,
        mut bodies: Vec<Fork>,
    ) -> Result<Option<SimPhysics>, SimError> {
        if bodies.is_empty() {
            return Ok(None);
        }
        let backend = match settings.get(S_BACKEND) {
            Some(Value::Text(t)) => Some(t.as_str()),
            _ => None,
        };
        let gravity = match settings.get(S_GRAVITY) {
            Some(Value::Vec3(g)) => DVec3::new(g[0], g[1], g[2]),
            _ => PhysicsSettings::default().gravity,
        };
        let s = PhysicsSettings {
            gravity,
            dt: SIM_DT,
            ..PhysicsSettings::default()
        };
        // Frame order, then key order (the fork's entities come in key order).
        bodies.sort_by_key(|b| (b.at.frame, b.key));
        let mut worlds: Vec<(FrameId, PhysicsWorld)> = Vec::new();
        let mut out = BTreeMap::new();
        for b in bodies {
            if worlds.last().is_none_or(|(f, _)| *f != b.at.frame) {
                let w = PhysicsWorld::new(backends, backend, b.at.frame, s).map_err(phys)?;
                worlds.push((b.at.frame, w));
            }
            let wi = worlds.len() - 1;
            let w = &mut worlds[wi].1;
            let mut desc = BodyDesc::new(b.spec.kind, b.at);
            desc.rotation = b.rot;
            desc.gravity_scale = b.spec.gravity_scale;
            desc.ccd = b.spec.ccd;
            if b.spec.kind != BodyKind::Static {
                desc.linear_velocity = b.velocity;
                desc.angular_velocity = DVec3::new(0.0, b.spin.to_radians(), 0.0);
            }
            let body = w.add_body(&desc).map_err(phys)?;
            w.add_collider(body, &b.spec.collider).map_err(phys)?;
            out.insert(
                b.key,
                Body {
                    world: wi,
                    body,
                    id: b.id,
                    kind: b.spec.kind,
                    accel: b.accel,
                },
            );
        }
        Ok(Some(SimPhysics {
            worlds,
            bodies: out,
        }))
    }

    /// Whether `k` is a physics body, and whether it can move.
    pub(crate) fn moving(&self, k: EntityKey) -> Option<bool> {
        self.bodies.get(&k).map(|b| b.kind != BodyKind::Static)
    }

    /// Apply an input to a physics body (`false`: `k` is not one).
    pub(crate) fn input(&mut self, k: EntityKey, a: InputAction) -> Result<bool, SimError> {
        let Some(b) = self.bodies.get_mut(&k) else {
            return Ok(false);
        };
        let w = &mut self.worlds[b.world].1;
        let s = w.state(b.body).map_err(phys)?;
        let v3 = |a: [f64; 3]| DVec3::new(a[0], a[1], a[2]);
        let (lin, ang) = match a {
            InputAction::SetVelocity(v) => (v3(v), s.angular_velocity),
            InputAction::Impulse(dv) => (s.linear_velocity + v3(dv), s.angular_velocity),
            InputAction::SetAcceleration(acc) => {
                b.accel = v3(acc);
                return Ok(true);
            }
            InputAction::SetSpin(dps) => {
                (s.linear_velocity, DVec3::new(0.0, dps.to_radians(), 0.0))
            }
        };
        if b.kind != BodyKind::Static {
            w.set_velocity(b.body, lin, ang).map_err(phys)?;
        }
        Ok(true)
    }

    /// One fixed step of every physics world, then the poses written back.
    pub(crate) fn step(&mut self, world: &mut forge_core::World) -> Result<(), SimError> {
        for b in self.bodies.values() {
            if b.accel != DVec3::ZERO && b.kind == BodyKind::Dynamic {
                let w = &mut self.worlds[b.world].1;
                let s = w.state(b.body).map_err(phys)?;
                w.set_velocity(
                    b.body,
                    s.linear_velocity + b.accel * SIM_DT,
                    s.angular_velocity,
                )
                .map_err(phys)?;
            }
        }
        for (_, w) in &mut self.worlds {
            w.step().map_err(phys)?;
        }
        for b in self.bodies.values() {
            if b.kind == BodyKind::Static {
                continue;
            }
            let s = self.worlds[b.world].1.state(b.body).map_err(phys)?;
            if let Ok(mut p) = world.get_mut::<Placement>(b.id) {
                p.0 = s.position;
            }
            if let Ok(mut o) = world.get_mut::<Orientation>(b.id) {
                o.0 = s.rotation;
            }
        }
        Ok(())
    }

    /// The physics worlds' state, in frame order (the simulation's state hash covers it).
    pub(crate) fn hash_into(&self, h: &mut blake3::Hasher) -> Result<(), SimError> {
        h.update(b"physics");
        for (f, w) in &self.worlds {
            h.update(&f.0.to_le_bytes());
            h.update(w.backend_id().as_bytes());
            h.update(w.state_hash().map_err(phys)?.as_bytes());
        }
        Ok(())
    }
}
