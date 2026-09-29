//! The edit world as the play core receives it: a plain, ordered snapshot of the project's
//! entities (Ch.21 §21.21 "Play controls": Play forks the simulation from the edit world).
//!
//! The core's project ([`EditSnapshot::from_project`], what `forge --headless` forks from)
//! and the editor's mirror (`forge_editor::sim_bridge::edit_snapshot`, what the GUI forks
//! from) must produce the **same** snapshot for the same project — that is half of what
//! `test_headless_parity` checks, through [`EditSnapshot::hash`].

use std::collections::BTreeMap;

use forge_cmd::{EntityKey, Project, Value};
use serde::{Deserialize, Serialize};

/// Transform property paths (the built-in `Transform` component the inspector shows).
pub const P_FRAME: &str = "transform.position.frame";
pub const P_LOCAL: &str = "transform.position.local";
pub const P_YAW: &str = "transform.yaw";
pub const P_PITCH: &str = "transform.pitch";
pub const P_ROLL: &str = "transform.roll";
pub const P_SCALE: &str = "transform.scale";
/// Motion the simulation integrates: linear velocity (m/s, frame axes), linear acceleration
/// (m/s², frame axes), spin (degrees per second about the frame's up axis).
pub const P_VELOCITY: &str = "motion.velocity";
pub const P_ACCELERATION: &str = "motion.acceleration";
pub const P_SPIN: &str = "motion.spin_dps";

/// Physics (WP-60, Ch.17): an entity with `physics.body` (`"dynamic"`, `"kinematic"` or
/// `"static"`) is a rigid body while playing, simulated by `forge-phys` in its frame's
/// region-local physics world. Its collider is `physics.shape` (`"box"` — the default —,
/// `"sphere"`, `"capsule"`, `"cylinder"`, `"cone"`) of full size `physics.size` (default
/// 1 m each way; a sphere's diameter is x, a round shape's is x and its height y) times
/// `transform.scale`. `motion.velocity` and `motion.spin_dps` are its starting velocities,
/// `motion.acceleration` a constant acceleration on top of gravity.
pub const P_BODY: &str = "physics.body";
pub const P_SHAPE: &str = "physics.shape";
pub const P_SIZE: &str = "physics.size";
/// Material: kg/m³, Coulomb friction, bounciness.
pub const P_DENSITY: &str = "physics.density";
pub const P_FRICTION: &str = "physics.friction";
pub const P_RESTITUTION: &str = "physics.restitution";
/// A trigger (reports overlaps, never pushes).
pub const P_SENSOR: &str = "physics.sensor";
/// Sweep against moving bodies too.
pub const P_CCD: &str = "physics.ccd";
pub const P_GRAVITY_SCALE: &str = "physics.gravity_scale";
/// Collision layers: memberships and filters, 32-bit masks.
pub const P_LAYERS: &str = "physics.layers";
pub const P_MASK: &str = "physics.mask";

/// The project settings a simulation reads (`physics.*`): which backend
/// (`physics.backend`, `"avian3d"` or `"rapier3d"`; the 3D preset's default is avian3d) and
/// the gravity (`physics.gravity`, m/s² in frame axes). A simulation on avian3d hashes the
/// same on every supported platform; one on rapier3d replays bit for bit on the platform
/// that recorded it only (`forge_phys::CROSS_PLATFORM`, ADR 0066).
pub const SETTINGS_PREFIX: &str = "physics.";
pub const S_BACKEND: &str = "physics.backend";
pub const S_GRAVITY: &str = "physics.gravity";

/// One entity of the edit world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditEntity {
    pub key: EntityKey,
    pub name: String,
    pub parent: Option<EntityKey>,
    pub properties: BTreeMap<String, Value>,
}

/// The edit world: entities in ascending key order, and the project settings the
/// simulation reads (the `physics.*` ones; empty for a project that sets none, and then
/// left out of the file and the hash, so snapshots from before WP-60 read and hash as they
/// did).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EditSnapshot {
    pub entities: Vec<EditEntity>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, Value>,
}

impl EditSnapshot {
    /// From the core's project.
    #[must_use]
    pub fn from_project(p: &Project) -> Self {
        let snap = Self::from_entities(p.entities().map(|(key, e)| {
            EditEntity {
                key,
                name: e.name().to_owned(),
                parent: e.parent(),
                properties: e
                    .properties()
                    .map(|(k, v)| (k.to_owned(), v.clone()))
                    .collect(),
            }
        }));
        snap.with_settings(p.settings())
    }

    /// With the project settings the simulation reads (the `physics.*` ones of `it`).
    #[must_use]
    pub fn with_settings<'a>(mut self, it: impl IntoIterator<Item = (&'a str, &'a Value)>) -> Self {
        self.settings = it
            .into_iter()
            .filter(|(k, _)| k.starts_with(SETTINGS_PREFIX))
            .map(|(k, v)| (k.to_owned(), v.clone()))
            .collect();
        self
    }

    /// From any entity source (a client's mirror); sorted by key. The scene library
    /// (`forge_scene`, WP-U20) is left out: its scenes are templates, not part of the world
    /// that plays — their instances are.
    pub fn from_entities(it: impl IntoIterator<Item = EditEntity>) -> Self {
        let mut entities: Vec<EditEntity> = it.into_iter().collect();
        entities.sort_by_key(|e| e.key);
        let libraries: Vec<EntityKey> = entities
            .iter()
            .filter(|e| {
                e.parent.is_none()
                    && e.properties.get(forge_scene::model::LIBRARY) == Some(&Value::Bool(true))
            })
            .map(|e| e.key)
            .collect();
        if !libraries.is_empty() {
            let mut kids: BTreeMap<EntityKey, Vec<EntityKey>> = BTreeMap::new();
            for e in &entities {
                if let Some(p) = e.parent {
                    kids.entry(p).or_default().push(e.key);
                }
            }
            let mut out: std::collections::BTreeSet<EntityKey> = std::collections::BTreeSet::new();
            let mut stack = libraries;
            while let Some(k) = stack.pop() {
                if out.insert(k)
                    && let Some(c) = kids.get(&k)
                {
                    stack.extend(c.iter().copied());
                }
            }
            entities.retain(|e| !out.contains(&e.key));
        }
        Self {
            entities,
            settings: BTreeMap::new(),
        }
    }

    /// BLAKE3 over a canonical encoding (keys, names, parents, every property with floats by
    /// their bits), as 64 hex digits. Two snapshots hash equal iff they are equal.
    #[must_use]
    pub fn hash(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(b"forge-sim edit v1");
        h.update(&(self.entities.len() as u64).to_le_bytes());
        for e in &self.entities {
            h.update(&e.key.0.to_le_bytes());
            bytes(&mut h, e.name.as_bytes());
            match e.parent {
                Some(p) => {
                    h.update(&[1]);
                    h.update(&p.0.to_le_bytes());
                }
                None => {
                    h.update(&[0]);
                }
            }
            h.update(&(e.properties.len() as u64).to_le_bytes());
            for (k, v) in &e.properties {
                bytes(&mut h, k.as_bytes());
                value(&mut h, v);
            }
        }
        if !self.settings.is_empty() {
            h.update(b"settings");
            h.update(&(self.settings.len() as u64).to_le_bytes());
            for (k, v) in &self.settings {
                bytes(&mut h, k.as_bytes());
                value(&mut h, v);
            }
        }
        h.finalize().to_hex().to_string()
    }
}

fn bytes(h: &mut blake3::Hasher, b: &[u8]) {
    h.update(&(b.len() as u64).to_le_bytes());
    h.update(b);
}

fn value(h: &mut blake3::Hasher, v: &Value) {
    match v {
        Value::Bool(b) => {
            h.update(&[0, u8::from(*b)]);
        }
        Value::Int(i) => {
            h.update(&[1]);
            h.update(&i.to_le_bytes());
        }
        Value::Float(x) => {
            h.update(&[2]);
            h.update(&x.to_bits().to_le_bytes());
        }
        Value::Text(s) => {
            h.update(&[3]);
            bytes(h, s.as_bytes());
        }
        Value::Vec3(a) => {
            h.update(&[4]);
            for x in a {
                h.update(&x.to_bits().to_le_bytes());
            }
        }
        Value::Entity(k) => {
            h.update(&[5]);
            h.update(&k.0.to_le_bytes());
        } // `Value` is not `non_exhaustive` today; a new variant must be added here.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(key: u64, props: &[(&str, Value)]) -> EditEntity {
        EditEntity {
            key: EntityKey(key),
            name: format!("e{key}"),
            parent: None,
            properties: props
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn the_hash_sees_every_bit_and_not_the_input_order() {
        let a = EditSnapshot::from_entities([
            ent(2, &[(P_LOCAL, Value::Vec3([0.0, 1.0, 2.0]))]),
            ent(1, &[]),
        ]);
        let b = EditSnapshot::from_entities([
            ent(1, &[]),
            ent(2, &[(P_LOCAL, Value::Vec3([0.0, 1.0, 2.0]))]),
        ]);
        assert_eq!(a.hash(), b.hash());
        let c = EditSnapshot::from_entities([
            ent(1, &[]),
            ent(2, &[(P_LOCAL, Value::Vec3([-0.0, 1.0, 2.0]))]),
        ]);
        assert_ne!(a.hash(), c.hash(), "-0.0 is not 0.0");
        let d = EditSnapshot::from_entities([
            ent(1, &[]),
            ent(2, &[(P_LOCAL, Value::Vec3([0.0, 1.0, 2.0 + 1e-9]))]),
        ]);
        assert_ne!(a.hash(), d.hash());
    }
}
