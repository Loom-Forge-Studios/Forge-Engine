//! [`DiffBuilder`] — how every command (built-in or `Invoke` handler) describes its effect.
//!
//! A command never receives `&mut Project`. It receives a builder over `&Project`, asks it
//! for changes, and the builder records each one with its `before` read from the project
//! *as the earlier changes of the same command left it*. A handler therefore cannot bypass
//! undo, cannot forget provenance, and cannot mutate during `dry_run` — it has nothing to
//! mutate with.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::{Change, CmdError, Diff, EntityKey, Project, Value};

/// Longest entity name, in characters.
pub const MAX_NAME_CHARS: usize = 256;

/// Builds one command's [`Diff`] against a read-only [`Project`].
///
/// Reads go through an overlay of this command's own pending changes (hash maps, so a
/// despawn of a 10k-entity subtree stays linear, not quadratic).
pub struct DiffBuilder<'p> {
    project: &'p Project,
    changes: Vec<Change>,
    next_key: u64,
    exists: HashMap<EntityKey, bool>,
    names: HashMap<EntityKey, String>,
    parents: HashMap<EntityKey, Option<EntityKey>>,
    /// Properties this command has touched, per entity, keyed by the interned path.
    props: HashMap<EntityKey, HashMap<Arc<str>, Option<Value>>>,
    settings: HashMap<String, Option<Value>>,
    /// Entities that became a child of the key during this command (candidates only).
    adopted: HashMap<EntityKey, BTreeSet<EntityKey>>,
    /// Entities that became a root during this command (candidates only).
    rooted: BTreeSet<EntityKey>,
    /// `(referrer, path)` pairs this command set to `Value::Entity(key)` (candidates only;
    /// the project's own index holds the rest).
    refs_added: HashMap<EntityKey, Vec<(EntityKey, Arc<str>)>>,
}

impl<'p> DiffBuilder<'p> {
    /// A builder over `project`.
    #[must_use]
    pub fn new(project: &'p Project) -> Self {
        Self {
            project,
            changes: Vec::new(),
            next_key: project.next_key().0,
            exists: HashMap::new(),
            names: HashMap::new(),
            parents: HashMap::new(),
            props: HashMap::new(),
            settings: HashMap::new(),
            adopted: HashMap::new(),
            rooted: BTreeSet::new(),
            refs_added: HashMap::new(),
        }
    }

    /// The project as it was before this command (read-only).
    #[must_use]
    pub fn project(&self) -> &'p Project {
        self.project
    }

    /// The changes so far.
    #[must_use]
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// The finished diff.
    #[must_use]
    pub fn finish(self) -> Diff {
        Diff {
            changes: self.changes,
        }
    }

    /// Record a change and update the overlay.
    fn record(&mut self, c: Change) {
        match &c {
            Change::Created {
                entity,
                name,
                parent,
            } => {
                self.exists.insert(*entity, true);
                self.names.insert(*entity, name.clone());
                self.parents.insert(*entity, *parent);
                match parent {
                    Some(p) => {
                        self.adopted.entry(*p).or_default().insert(*entity);
                    }
                    None => {
                        self.rooted.insert(*entity);
                    }
                }
            }
            Change::Removed { entity, .. } => {
                self.exists.insert(*entity, false);
            }
            Change::Renamed { entity, after, .. } => {
                self.names.insert(*entity, after.clone());
            }
            Change::Reparented { entity, after, .. } => {
                self.parents.insert(*entity, *after);
                match after {
                    Some(p) => {
                        self.adopted.entry(*p).or_default().insert(*entity);
                    }
                    None => {
                        self.rooted.insert(*entity);
                    }
                }
            }
            Change::Property {
                entity,
                path,
                after,
                ..
            } => {
                if let Some(Value::Entity(t)) = after {
                    self.refs_added
                        .entry(*t)
                        .or_default()
                        .push((*entity, Arc::clone(path)));
                }
                self.props
                    .entry(*entity)
                    .or_default()
                    .insert(Arc::clone(path), after.clone());
            }
            Change::Setting { key, after, .. } => {
                self.settings.insert(key.clone(), after.clone());
            }
        }
        self.changes.push(c);
    }

    // ---- reads, through the pending changes --------------------------------------------

    /// Does `e` exist, counting this command's own spawns and despawns?
    #[must_use]
    pub fn exists(&self, e: EntityKey) -> bool {
        self.exists
            .get(&e)
            .copied()
            .unwrap_or_else(|| self.project.contains(e))
    }

    /// `e`'s name as this command has left it.
    pub fn name(&self, e: EntityKey) -> Result<String, CmdError> {
        self.require(e)?;
        if let Some(n) = self.names.get(&e) {
            return Ok(n.clone());
        }
        self.project
            .entity(e)
            .map(|d| d.name().to_string())
            .ok_or(CmdError::UnknownEntity(e))
    }

    /// `e`'s parent as this command has left it.
    pub fn parent(&self, e: EntityKey) -> Result<Option<EntityKey>, CmdError> {
        self.require(e)?;
        if let Some(p) = self.parents.get(&e) {
            return Ok(*p);
        }
        self.project
            .entity(e)
            .map(|d| d.parent())
            .ok_or(CmdError::UnknownEntity(e))
    }

    /// `e.path` as this command has left it.
    pub fn property(&self, e: EntityKey, path: &str) -> Result<Option<Value>, CmdError> {
        self.require(e)?;
        if let Some(v) = self.props.get(&e).and_then(|m| m.get(path)) {
            return Ok(v.clone());
        }
        Ok(self
            .project
            .entity(e)
            .and_then(|d| d.property(path))
            .cloned())
    }

    /// A setting as this command has left it.
    #[must_use]
    pub fn setting(&self, key: &str) -> Option<Value> {
        match self.settings.get(key) {
            Some(v) => v.clone(),
            None => self.project.setting(key).cloned(),
        }
    }

    /// `e`'s children as this command has left them, in key order.
    pub fn children(&self, e: EntityKey) -> Result<Vec<EntityKey>, CmdError> {
        self.require(e)?;
        let mut candidates: BTreeSet<EntityKey> = self
            .project
            .entity(e)
            .map(|d| d.children().collect())
            .unwrap_or_default();
        if let Some(extra) = self.adopted.get(&e) {
            candidates.extend(extra.iter().copied());
        }
        let mut out = Vec::new();
        for c in candidates {
            if self.exists(c) && self.parent(c)? == Some(e) {
                out.push(c);
            }
        }
        Ok(out)
    }

    /// The root entities as this command has left them, in key order.
    #[must_use]
    pub fn roots(&self) -> Vec<EntityKey> {
        let mut candidates: BTreeSet<EntityKey> = self.project.roots().collect();
        candidates.extend(self.rooted.iter().copied());
        candidates
            .into_iter()
            .filter(|c| self.exists(*c) && matches!(self.parent(*c), Ok(None)))
            .collect()
    }

    /// Every entity whose `path` holds `Value::Entity(target)` as this command has left the
    /// project, in key order: the project's reference index plus this command's own
    /// changes, each re-checked against its current value (O(log n + k), never a scan).
    #[must_use]
    pub fn referrers(&self, target: EntityKey, path: &str) -> Vec<EntityKey> {
        let mut candidates: BTreeSet<EntityKey> = self
            .project
            .referrers(target)
            .filter(|(_, p)| *p == path)
            .map(|(e, _)| e)
            .collect();
        if let Some(added) = self.refs_added.get(&target) {
            candidates.extend(added.iter().filter(|(_, p)| &**p == path).map(|(e, _)| *e));
        }
        candidates
            .into_iter()
            .filter(|e| {
                self.exists(*e)
                    && matches!(self.property(*e, path), Ok(Some(Value::Entity(k))) if k == target)
            })
            .collect()
    }

    /// `e`'s property paths as this command has left them, in path order.
    pub fn property_paths(&self, e: EntityKey) -> Result<Vec<String>, CmdError> {
        self.require(e)?;
        let mut paths: BTreeSet<String> = self
            .project
            .entity(e)
            .map(|d| d.properties().map(|(p, _)| p.to_string()).collect())
            .unwrap_or_default();
        if let Some(t) = self.props.get(&e) {
            paths.extend(t.keys().map(|k| k.to_string()));
        }
        let mut out = Vec::new();
        for p in paths {
            if self.property(e, &p)?.is_some() {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// The shared path for `e.path`: the one this command or the project already holds if
    /// the property exists, so only a brand-new property allocates its path.
    fn intern(&self, e: EntityKey, path: &str) -> Arc<str> {
        if let Some((k, _)) = self.props.get(&e).and_then(|m| m.get_key_value(path)) {
            return Arc::clone(k);
        }
        self.project
            .entity(e)
            .and_then(|d| d.property_key(path))
            .map_or_else(|| Arc::from(path), Arc::clone)
    }

    fn require(&self, e: EntityKey) -> Result<(), CmdError> {
        if self.exists(e) {
            Ok(())
        } else {
            Err(CmdError::UnknownEntity(e))
        }
    }

    // ---- writes (recorded, never applied here) -------------------------------------------

    /// Spawn an entity; returns the key it will have.
    pub fn spawn(&mut self, name: &str, parent: Option<EntityKey>) -> Result<EntityKey, CmdError> {
        check_name(name)?;
        if let Some(p) = parent {
            self.require(p)?;
        }
        let entity = EntityKey(self.next_key);
        self.next_key = self.next_key.saturating_add(1);
        self.record(Change::Created {
            entity,
            name: name.to_string(),
            parent,
        });
        Ok(entity)
    }

    /// Spawn an entity **with the key `key`** (additive, WP-U10). A team sandbox rebuilt from
    /// the baseline keeps the baseline's keys, so an entity is the same entity in every
    /// teammate's sandbox (a presence badge, a claim and a three-way merge all name it by
    /// key). Refused with `CMD-0008` when `key` exists; later spawns allocate above it.
    pub fn spawn_at(
        &mut self,
        key: EntityKey,
        name: &str,
        parent: Option<EntityKey>,
    ) -> Result<EntityKey, CmdError> {
        check_name(name)?;
        if self.exists(key) {
            return Err(CmdError::Conflict {
                detail: format!("{key} already exists"),
            });
        }
        if let Some(p) = parent {
            self.require(p)?;
        }
        self.next_key = self.next_key.max(key.0.saturating_add(1));
        self.record(Change::Created {
            entity: key,
            name: name.to_string(),
            parent,
        });
        Ok(key)
    }

    /// Despawn `e` and its whole subtree (children first, each emptied of its properties
    /// first), so every step is an elementary, invertible change.
    pub fn despawn(&mut self, e: EntityKey) -> Result<(), CmdError> {
        // Post-order without recursion (a deep hierarchy must not overflow the stack).
        let mut order = Vec::new();
        let mut stack = vec![(e, false)];
        while let Some((k, expanded)) = stack.pop() {
            if expanded {
                order.push(k);
            } else {
                stack.push((k, true));
                for c in self.children(k)?.into_iter().rev() {
                    stack.push((c, false));
                }
            }
        }
        for k in order {
            for path in self.property_paths(k)? {
                self.remove_property(k, &path)?;
            }
            let name = self.name(k)?;
            let parent = self.parent(k)?;
            self.record(Change::Removed {
                entity: k,
                name,
                parent,
            });
        }
        Ok(())
    }

    /// Rename `e`. Renaming to the current name records nothing.
    pub fn rename(&mut self, e: EntityKey, name: &str) -> Result<(), CmdError> {
        check_name(name)?;
        let before = self.name(e)?;
        if before != name {
            self.record(Change::Renamed {
                entity: e,
                before,
                after: name.to_string(),
            });
        }
        Ok(())
    }

    /// Move `e` under `parent` (`None`: make it a root). Refuses a cycle.
    pub fn reparent(&mut self, e: EntityKey, parent: Option<EntityKey>) -> Result<(), CmdError> {
        let before = self.parent(e)?;
        if let Some(p) = parent {
            self.require(p)?;
            // Walk up from `p` through the pending hierarchy; bounded, so a bad state cannot
            // loop.
            let mut cur = Some(p);
            let mut steps = 0usize;
            while let Some(k) = cur {
                if k == e || steps > self.project.len() + self.changes.len() {
                    return Err(CmdError::Cycle {
                        entity: e,
                        parent: p,
                    });
                }
                steps += 1;
                cur = self.parent(k)?;
            }
        }
        if before != parent {
            self.record(Change::Reparented {
                entity: e,
                before,
                after: parent,
            });
        }
        Ok(())
    }

    /// Set `e.path = value`. Setting the current value records nothing.
    pub fn set_property(&mut self, e: EntityKey, path: &str, value: Value) -> Result<(), CmdError> {
        check_path(path)?;
        if !value.is_finite() {
            return Err(CmdError::NonFinite {
                path: path.to_string(),
            });
        }
        let before = self.property(e, path)?;
        if before.as_ref() != Some(&value) {
            self.record(Change::Property {
                entity: e,
                path: self.intern(e, path),
                before,
                after: Some(value),
            });
        }
        Ok(())
    }

    /// Remove `e.path`. Removing an absent property records nothing.
    pub fn remove_property(&mut self, e: EntityKey, path: &str) -> Result<(), CmdError> {
        check_path(path)?;
        let before = self.property(e, path)?;
        if before.is_some() {
            self.record(Change::Property {
                entity: e,
                path: self.intern(e, path),
                before,
                after: None,
            });
        }
        Ok(())
    }

    /// Set a project setting (`None`: remove it).
    pub fn set_setting(&mut self, key: &str, value: Option<Value>) -> Result<(), CmdError> {
        check_path(key)?;
        if let Some(v) = &value
            && !v.is_finite()
        {
            return Err(CmdError::NonFinite {
                path: key.to_string(),
            });
        }
        let before = self.setting(key);
        if before != value {
            self.record(Change::Setting {
                key: key.to_string(),
                before,
                after: value,
            });
        }
        Ok(())
    }
}

/// A property path or setting key: `ident(.ident)*`, `ident = [A-Za-z_][A-Za-z0-9_]*` or a
/// decimal index, at most 512 bytes.
pub fn check_path(path: &str) -> Result<(), CmdError> {
    let ok = !path.is_empty()
        && path.len() <= 512
        && path.split('.').all(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(c) if c.is_ascii_digit() => seg.chars().all(|c| c.is_ascii_digit()),
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
                }
                _ => false,
            }
        });
    if ok {
        Ok(())
    } else {
        Err(CmdError::BadPath(path.to_string()))
    }
}

/// An entity name: 1..=256 characters, no control characters.
pub fn check_name(name: &str) -> Result<(), CmdError> {
    let n = name.chars().count();
    if (1..=MAX_NAME_CHARS).contains(&n) && !name.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(CmdError::BadName(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        for ok in ["x", "transform.position", "a_1.b2", "items.0.name", "_x"] {
            assert!(check_path(ok).is_ok(), "{ok}");
        }
        for bad in ["", ".", "a..b", "a.", "1a", "a-b", "a b", "é"] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names() {
        assert!(check_name("Player 1").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("a\nb").is_err());
        assert!(check_name(&"x".repeat(257)).is_err());
        assert!(check_name(&"é".repeat(256)).is_ok());
    }

    #[test]
    fn despawn_empties_a_subtree_bottom_up() {
        let mut p = Project::new();
        let mut b = DiffBuilder::new(&p);
        let root = b.spawn("root", None).expect("ok");
        let kid = b.spawn("kid", Some(root)).expect("ok");
        b.set_property(kid, "hp", Value::Int(3)).expect("ok");
        let d = b.finish();
        p.apply_all(&d.changes).expect("ok");

        let mut b = DiffBuilder::new(&p);
        b.despawn(root).expect("ok");
        let d = b.finish();
        // kid's property, kid, then root
        assert!(matches!(d.changes[0], Change::Property { after: None, .. }));
        assert!(matches!(d.changes[1], Change::Removed { entity, .. } if entity == kid));
        assert!(matches!(d.changes[2], Change::Removed { entity, .. } if entity == root));
        p.apply_all(&d.changes).expect("ok");
        assert!(p.is_empty());
    }

    #[test]
    fn reads_see_earlier_changes_of_the_same_command() {
        let p = Project::new();
        let mut b = DiffBuilder::new(&p);
        let a = b.spawn("a", None).expect("ok");
        let c = b.spawn("c", Some(a)).expect("ok");
        assert_eq!(b.children(a).expect("ok"), vec![c]);
        b.set_property(c, "x", Value::Int(1)).expect("ok");
        b.set_property(c, "x", Value::Int(2)).expect("ok");
        assert_eq!(b.property(c, "x").expect("ok"), Some(Value::Int(2)));
        assert!(
            b.reparent(a, Some(c)).is_err(),
            "cycle through a pending child"
        );
        b.despawn(c).expect("ok");
        assert!(!b.exists(c));
        assert!(b.children(a).expect("ok").is_empty());
    }
}
