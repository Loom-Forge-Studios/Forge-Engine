//! [`Project`] — the project document the bus owns.
//!
//! Its mutators are `pub(crate)`: outside this crate there is **no way** to change a
//! `Project` except by sending a command to the [`Bus`](crate::Bus) that owns it (I7,
//! enforced by the type system first; `test_command_liveness` then enforces the rest through
//! the import graph). Two things build a `Project` outside a bus, and neither can reach a
//! bus's: deserializing its wire form (a remote client's snapshot, rebuilt through the same
//! checked changes) and a [`Replica`](crate::Replica), a client's copy that follows a bus's
//! `Applied` stream and never feeds anything back (the split editor, M2-16).

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::{Change, CmdError, EntityKey, Value};

/// One project entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityData {
    name: String,
    parent: Option<EntityKey>,
    children: BTreeSet<EntityKey>,
    /// Keyed by the interned path: this `Arc` is the one every change on the property shares.
    props: BTreeMap<Arc<str>, Value>,
}

impl EntityData {
    /// Its name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its parent (`None`: a root).
    #[must_use]
    pub fn parent(&self) -> Option<EntityKey> {
        self.parent
    }

    /// Its children, in key order.
    pub fn children(&self) -> impl ExactSizeIterator<Item = EntityKey> + '_ {
        self.children.iter().copied()
    }

    /// Its properties, in path order.
    pub fn properties(&self) -> impl ExactSizeIterator<Item = (&str, &Value)> {
        self.props.iter().map(|(k, v)| (&**k, v))
    }

    /// One property.
    #[must_use]
    pub fn property(&self, path: &str) -> Option<&Value> {
        self.props.get(path)
    }

    /// The interned path of an existing property (shared by every change on it).
    pub(crate) fn property_key(&self, path: &str) -> Option<&Arc<str>> {
        self.props.get_key_value(path).map(|(k, _)| k)
    }
}

/// The project document: entities (a hierarchy with reflect-path properties) and project
/// settings. Read freely; change only through the bus.
#[derive(Clone, Debug, Default)]
pub struct Project {
    entities: BTreeMap<EntityKey, EntityData>,
    roots: BTreeSet<EntityKey>,
    settings: BTreeMap<String, Value>,
    /// The next key to allocate. An allocator, not content: not hashed, never rolled back,
    /// so an undone spawn's key is never handed out again (a redo can restore it).
    next_key: u64,
    /// Who refers to whom: for every entity named by a `Value::Entity` property, the
    /// `(referrer, path)` pairs holding it. Derived from the properties (kept by
    /// `apply_one`, the only mutation), so it is never content: not hashed, not on the
    /// wire. Scene composition (WP-U20) finds a base node's instances through it in
    /// O(log n + k) instead of scanning the project on every edit.
    refs: BTreeMap<EntityKey, BTreeSet<(EntityKey, Arc<str>)>>,
}

impl Project {
    /// An empty project.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// One entity.
    #[must_use]
    pub fn entity(&self, key: EntityKey) -> Option<&EntityData> {
        self.entities.get(&key)
    }

    /// True if the entity exists.
    #[must_use]
    pub fn contains(&self, key: EntityKey) -> bool {
        self.entities.contains_key(&key)
    }

    /// Every entity, in key order.
    pub fn entities(&self) -> impl ExactSizeIterator<Item = (EntityKey, &EntityData)> {
        self.entities.iter().map(|(k, v)| (*k, v))
    }

    /// The root entities, in key order.
    pub fn roots(&self) -> impl ExactSizeIterator<Item = EntityKey> + '_ {
        self.roots.iter().copied()
    }

    /// Number of entities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// No entities.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// One project setting.
    #[must_use]
    pub fn setting(&self, key: &str) -> Option<&Value> {
        self.settings.get(key)
    }

    /// Every project setting, in key order.
    pub fn settings(&self) -> impl ExactSizeIterator<Item = (&str, &Value)> {
        self.settings.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The key the next spawn will get.
    #[must_use]
    pub fn next_key(&self) -> EntityKey {
        EntityKey(self.next_key)
    }

    /// Every `(entity, path)` whose property holds `Value::Entity(target)`, in key then path
    /// order. An index lookup (O(log n + k)), never a scan.
    pub fn referrers(&self, target: EntityKey) -> impl Iterator<Item = (EntityKey, &str)> + '_ {
        self.refs
            .get(&target)
            .into_iter()
            .flat_map(|s| s.iter().map(|(e, p)| (*e, &**p)))
    }

    /// Keep the reference index in step with one property change (`before` was checked).
    fn index_ref(
        &mut self,
        entity: EntityKey,
        path: &Arc<str>,
        before: Option<&Value>,
        after: Option<&Value>,
    ) {
        if let Some(Value::Entity(t)) = before
            && let Some(set) = self.refs.get_mut(t)
        {
            set.remove(&(entity, Arc::clone(path)));
            if set.is_empty() {
                self.refs.remove(t);
            }
        }
        if let Some(Value::Entity(t)) = after {
            self.refs
                .entry(*t)
                .or_default()
                .insert((entity, Arc::clone(path)));
        }
    }

    /// A hash of the project's **content** (entities, hierarchy, properties, settings; not
    /// the key allocator). Two projects with equal content hash equal in one build; the
    /// command contract and the undo fuzz compare it before and after.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.entities.len().hash(&mut h);
        for (k, e) in &self.entities {
            k.hash(&mut h);
            e.name.hash(&mut h);
            e.parent.hash(&mut h);
            e.children.len().hash(&mut h);
            for c in &e.children {
                c.hash(&mut h);
            }
            e.props.len().hash(&mut h);
            for (p, v) in &e.props {
                p.hash(&mut h);
                v.hash(&mut h);
            }
        }
        self.settings.len().hash(&mut h);
        for (k, v) in &self.settings {
            k.hash(&mut h);
            v.hash(&mut h);
        }
        h.finish()
    }

    /// Apply `changes` **atomically**: every change checks its preconditions (its `before`
    /// matches the project) before it mutates; if one fails, the ones already applied are
    /// rolled back and the project is exactly as it was. `Err(Poisoned)` only if a rollback
    /// itself fails, which the preconditions make unreachable.
    pub(crate) fn apply_all<'c>(
        &mut self,
        changes: impl IntoIterator<Item = &'c Change>,
    ) -> Result<(), CmdError> {
        let mut done: Vec<&Change> = Vec::new();
        for c in changes {
            if let Err(e) = self.apply_one(c) {
                for d in done.into_iter().rev() {
                    if self.apply_one(&d.inverse()).is_err() {
                        return Err(CmdError::Poisoned);
                    }
                }
                return Err(e);
            }
            done.push(c);
        }
        Ok(())
    }

    /// Check one change's preconditions, then apply it. Mutates nothing on `Err`.
    fn apply_one(&mut self, c: &Change) -> Result<(), CmdError> {
        let conflict = |detail: String| CmdError::Conflict { detail };
        match c {
            Change::Created {
                entity,
                name,
                parent,
            } => {
                if self.entities.contains_key(entity) {
                    return Err(conflict(format!("{entity} already exists")));
                }
                if let Some(p) = parent
                    && !self.entities.contains_key(p)
                {
                    return Err(CmdError::UnknownEntity(*p));
                }
                self.attach(*entity, *parent);
                self.entities.insert(
                    *entity,
                    EntityData {
                        name: name.clone(),
                        parent: *parent,
                        children: BTreeSet::new(),
                        props: BTreeMap::new(),
                    },
                );
                self.next_key = self.next_key.max(entity.0.saturating_add(1));
            }
            Change::Removed {
                entity,
                name,
                parent,
            } => {
                let e = self.get(*entity)?;
                if e.name != *name || e.parent != *parent {
                    return Err(conflict(format!(
                        "{entity} is no longer {name:?} under {parent:?}"
                    )));
                }
                if !e.props.is_empty() || !e.children.is_empty() {
                    return Err(conflict(format!(
                        "{entity} still has properties or children"
                    )));
                }
                self.detach(*entity, *parent);
                self.entities.remove(entity);
            }
            Change::Renamed {
                entity,
                before,
                after,
            } => {
                let e = self.get_mut(*entity)?;
                if e.name != *before {
                    return Err(conflict(format!(
                        "{entity} is named {:?}, not {before:?}",
                        e.name
                    )));
                }
                e.name.clone_from(after);
            }
            Change::Reparented {
                entity,
                before,
                after,
            } => {
                let current = self.get(*entity)?.parent;
                if current != *before {
                    return Err(conflict(format!(
                        "{entity}'s parent is {current:?}, not {before:?}"
                    )));
                }
                if let Some(p) = after {
                    if !self.entities.contains_key(p) {
                        return Err(CmdError::UnknownEntity(*p));
                    }
                    if self.is_ancestor_or_self(*entity, *p) {
                        return Err(CmdError::Cycle {
                            entity: *entity,
                            parent: *p,
                        });
                    }
                }
                self.detach(*entity, *before);
                self.attach(*entity, *after);
                self.get_mut(*entity)?.parent = *after;
            }
            Change::Property {
                entity,
                path,
                before,
                after,
            } => {
                let e = self.get(*entity)?;
                if e.props.get(&**path) != before.as_ref() {
                    return Err(conflict(format!(
                        "{entity}.{path} is {:?}, not {before:?}",
                        e.props.get(&**path)
                    )));
                }
                if matches!(before, Some(Value::Entity(_)))
                    || matches!(after, Some(Value::Entity(_)))
                {
                    self.index_ref(*entity, path, before.as_ref(), after.as_ref());
                }
                let e = self.get_mut(*entity)?;
                match after {
                    Some(v) => match e.props.get_mut(&**path) {
                        // Overwrite in place: no key clone, no tree rebalancing.
                        Some(slot) => slot.clone_from(v),
                        None => {
                            e.props.insert(Arc::clone(path), v.clone());
                        }
                    },
                    None => {
                        e.props.remove(&**path);
                    }
                }
            }
            Change::Setting { key, before, after } => {
                if self.settings.get(key) != before.as_ref() {
                    return Err(conflict(format!(
                        "setting {key} is {:?}, not {before:?}",
                        self.settings.get(key)
                    )));
                }
                match after {
                    Some(v) => {
                        self.settings.insert(key.clone(), v.clone());
                    }
                    None => {
                        self.settings.remove(key);
                    }
                }
            }
        }
        Ok(())
    }

    fn get(&self, k: EntityKey) -> Result<&EntityData, CmdError> {
        self.entities.get(&k).ok_or(CmdError::UnknownEntity(k))
    }

    fn get_mut(&mut self, k: EntityKey) -> Result<&mut EntityData, CmdError> {
        self.entities.get_mut(&k).ok_or(CmdError::UnknownEntity(k))
    }

    /// Is `a` an ancestor of `b`, or `b` itself? Bounded by the entity count, so a corrupt
    /// hierarchy cannot loop forever.
    pub(crate) fn is_ancestor_or_self(&self, a: EntityKey, b: EntityKey) -> bool {
        let mut cur = Some(b);
        let mut steps = 0usize;
        while let Some(k) = cur {
            if k == a {
                return true;
            }
            steps += 1;
            if steps > self.entities.len() {
                return true; // treat a (impossible) cycle as "would cycle": refuse
            }
            cur = self.entities.get(&k).and_then(|e| e.parent);
        }
        false
    }

    fn attach(&mut self, child: EntityKey, parent: Option<EntityKey>) {
        match parent.and_then(|p| self.entities.get_mut(&p)) {
            Some(p) => {
                p.children.insert(child);
            }
            None => {
                self.roots.insert(child);
            }
        }
    }

    fn detach(&mut self, child: EntityKey, parent: Option<EntityKey>) {
        match parent.and_then(|p| self.entities.get_mut(&p)) {
            Some(p) => {
                p.children.remove(&child);
            }
            None => {
                self.roots.remove(&child);
            }
        }
    }
}

// ---- the wire form (the split editor's snapshot, M2-16) ---------------------------------
//
// A remote client starts from a snapshot of the project and follows the `Applied` stream.
// The snapshot crosses the wire in this form: entities in key order with their properties,
// settings in key order, and the key allocator. Deserializing **rebuilds** the project by
// applying `Created`, `Property` and `Setting` changes in parent-first order through the
// same atomic `apply_all` every command goes through, after checking every name, path and
// value the way the builder does, so a malformed snapshot is refused and can never produce a
// project the bus itself could not have produced. The form is canonical (key order
// everywhere), so equal projects serialize to equal bytes: `test_split_editor_parity`
// compares those bytes.

#[derive(serde::Serialize, serde::Deserialize)]
struct EntityWire {
    key: EntityKey,
    name: String,
    parent: Option<EntityKey>,
    props: Vec<(String, Value)>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ProjectWire {
    entities: Vec<EntityWire>,
    settings: Vec<(String, Value)>,
    next_key: u64,
}

impl ProjectWire {
    fn of(p: &Project) -> Self {
        Self {
            entities: p
                .entities
                .iter()
                .map(|(k, e)| EntityWire {
                    key: *k,
                    name: e.name.clone(),
                    parent: e.parent,
                    props: e
                        .props
                        .iter()
                        .map(|(p, v)| (p.to_string(), v.clone()))
                        .collect(),
                })
                .collect(),
            settings: p
                .settings
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            next_key: p.next_key,
        }
    }

    fn build(self) -> Result<Project, CmdError> {
        let bad = |detail: String| CmdError::Conflict { detail };
        let mut parent_of: BTreeMap<EntityKey, Option<EntityKey>> = BTreeMap::new();
        for e in &self.entities {
            crate::check_name(&e.name)?;
            if parent_of.insert(e.key, e.parent).is_some() {
                return Err(bad(format!("{} is listed twice", e.key)));
            }
        }
        // Parent-first order; a missing parent or a cycle is refused.
        let mut order: Vec<EntityKey> = Vec::with_capacity(parent_of.len());
        let mut placed: BTreeSet<EntityKey> = BTreeSet::new();
        for &k in parent_of.keys() {
            let mut chain = Vec::new();
            let mut at = Some(k);
            while let Some(c) = at {
                if placed.contains(&c) {
                    break;
                }
                if chain.len() > parent_of.len() {
                    return Err(bad(format!("the hierarchy above {k} is a cycle")));
                }
                chain.push(c);
                at = match parent_of.get(&c) {
                    Some(p) => *p,
                    None => return Err(CmdError::UnknownEntity(c)),
                };
            }
            for c in chain.into_iter().rev() {
                if placed.insert(c) {
                    order.push(c);
                }
            }
        }
        let by_key: BTreeMap<EntityKey, &EntityWire> =
            self.entities.iter().map(|e| (e.key, e)).collect();
        let mut changes: Vec<Change> = Vec::new();
        for k in &order {
            let Some(e) = by_key.get(k) else { continue };
            changes.push(Change::Created {
                entity: e.key,
                name: e.name.clone(),
                parent: e.parent,
            });
        }
        for e in &self.entities {
            for (path, v) in &e.props {
                crate::check_path(path)?;
                if !v.is_finite() {
                    return Err(CmdError::NonFinite { path: path.clone() });
                }
                changes.push(Change::Property {
                    entity: e.key,
                    path: Arc::from(path.as_str()),
                    before: None,
                    after: Some(v.clone()),
                });
            }
        }
        for (key, v) in &self.settings {
            crate::check_path(key)?;
            if !v.is_finite() {
                return Err(CmdError::NonFinite { path: key.clone() });
            }
            changes.push(Change::Setting {
                key: key.clone(),
                before: None,
                after: Some(v.clone()),
            });
        }
        let mut p = Project::new();
        p.apply_all(&changes)?;
        p.next_key = p.next_key.max(self.next_key);
        Ok(p)
    }
}

impl serde::Serialize for Project {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&ProjectWire::of(self), s)
    }
}

impl<'de> serde::Deserialize<'de> for Project {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        <ProjectWire as serde::Deserialize>::deserialize(d)?
            .build()
            .map_err(<D::Error as serde::de::Error>::custom)
    }
}

impl Project {
    /// The key allocator's raw value (a replica restores it when it rolls a prediction back).
    pub(crate) fn raw_next_key(&self) -> u64 {
        self.next_key
    }

    pub(crate) fn set_raw_next_key(&mut self, k: u64) {
        self.next_key = k;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_rebuilds_the_same_project_and_refuses_a_bad_one() {
        let mut p = Project::new();
        p.apply_all(&[
            created(0, None),
            created(1, Some(0)),
            created(2, Some(1)),
            Change::Property {
                entity: EntityKey(2),
                path: "transform.position".into(),
                before: None,
                after: Some(Value::Vec3([1.0, -0.0, 3.5])),
            },
            Change::Setting {
                key: "editor.grid".into(),
                before: None,
                after: Some(Value::Float(0.25)),
            },
        ])
        .expect("ok");
        p.next_key = 9;
        let text = serde_json::to_string(&p).expect("serializes");
        let q: Project = serde_json::from_str(&text).expect("deserializes");
        assert_eq!(q.state_hash(), p.state_hash());
        assert_eq!(q.next_key(), EntityKey(9));
        assert_eq!(serde_json::to_string(&q).expect("again"), text, "canonical");
        // A cycle, a missing parent and a bad name are refused.
        let cycle = r#"{"entities":[{"key":0,"name":"a","parent":1,"props":[]},{"key":1,"name":"b","parent":0,"props":[]}],"settings":[],"next_key":2}"#;
        assert!(serde_json::from_str::<Project>(cycle).is_err());
        let orphan = r#"{"entities":[{"key":0,"name":"a","parent":5,"props":[]}],"settings":[],"next_key":1}"#;
        assert!(serde_json::from_str::<Project>(orphan).is_err());
        let bad_name = r#"{"entities":[{"key":0,"name":"","parent":null,"props":[]}],"settings":[],"next_key":1}"#;
        assert!(serde_json::from_str::<Project>(bad_name).is_err());
    }

    fn created(k: u64, parent: Option<u64>) -> Change {
        Change::Created {
            entity: EntityKey(k),
            name: format!("n{k}"),
            parent: parent.map(EntityKey),
        }
    }

    #[test]
    fn a_failing_change_rolls_back_the_whole_diff() {
        let mut p = Project::new();
        p.apply_all(&[created(0, None)]).expect("ok");
        let h = p.state_hash();
        let bad = [
            created(1, Some(0)),
            Change::Property {
                entity: EntityKey(1),
                path: "x".into(),
                before: None,
                after: Some(Value::Int(1)),
            },
            // precondition fails: `before` is wrong
            Change::Renamed {
                entity: EntityKey(0),
                before: "wrong".into(),
                after: "z".into(),
            },
        ];
        let e = p.apply_all(&bad).expect_err("must conflict");
        assert_eq!(e.code().as_str(), "CMD-0008");
        assert_eq!(p.state_hash(), h);
        assert_eq!(p.len(), 1);
        assert_eq!(p.entity(EntityKey(0)).map(|e| e.children().len()), Some(0));
    }

    #[test]
    fn removal_requires_an_empty_entity_and_restores_exactly() {
        let mut p = Project::new();
        p.apply_all(&[created(0, None), created(1, Some(0))])
            .expect("ok");
        let rm0 = Change::Removed {
            entity: EntityKey(0),
            name: "n0".into(),
            parent: None,
        };
        assert!(p.apply_all(&[rm0]).is_err(), "still has a child");
        let h = p.state_hash();
        let rm1 = Change::Removed {
            entity: EntityKey(1),
            name: "n1".into(),
            parent: Some(EntityKey(0)),
        };
        p.apply_all(std::slice::from_ref(&rm1)).expect("ok");
        p.apply_all(&[rm1.inverse()]).expect("ok");
        assert_eq!(p.state_hash(), h);
    }

    #[test]
    fn reparent_refuses_cycles() {
        let mut p = Project::new();
        p.apply_all(&[created(0, None), created(1, Some(0))])
            .expect("ok");
        let e = p
            .apply_all(&[Change::Reparented {
                entity: EntityKey(0),
                before: None,
                after: Some(EntityKey(1)),
            }])
            .expect_err("cycle");
        assert_eq!(e.code().as_str(), "CMD-0002");
    }

    #[test]
    fn the_allocator_is_not_content() {
        let mut a = Project::new();
        let b = Project::new();
        a.apply_all(&[created(0, None)]).expect("ok");
        a.apply_all(&[created(0, None).inverse()]).expect("ok");
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.next_key(), EntityKey(1), "keys are never reused");
    }
}
