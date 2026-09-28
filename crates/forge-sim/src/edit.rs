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

/// One entity of the edit world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditEntity {
    pub key: EntityKey,
    pub name: String,
    pub parent: Option<EntityKey>,
    pub properties: BTreeMap<String, Value>,
}

/// The edit world: entities in ascending key order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EditSnapshot {
    pub entities: Vec<EditEntity>,
}

impl EditSnapshot {
    /// From the core's project.
    #[must_use]
    pub fn from_project(p: &Project) -> Self {
        Self::from_entities(p.entities().map(|(key, e)| {
            EditEntity {
                key,
                name: e.name().to_owned(),
                parent: e.parent(),
                properties: e
                    .properties()
                    .map(|(k, v)| (k.to_owned(), v.clone()))
                    .collect(),
            }
        }))
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
        Self { entities }
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
