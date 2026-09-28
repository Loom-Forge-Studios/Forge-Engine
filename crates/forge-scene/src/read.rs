//! [`SceneRead`] — the one read interface scene composition works over.
//!
//! The same rules read a committed [`Project`], a command's [`DiffBuilder`] (the project as
//! the command so far leaves it) and an editor client's mirror (implemented in
//! `forge-editor`), so what the inspector shows as overridden is exactly what the deriver
//! treats as overridden.

use forge_cmd::{DiffBuilder, EntityKey, Project, Value};

/// Read access to an entity hierarchy with reflect-path properties.
pub trait SceneRead {
    /// Does `e` exist?
    fn exists(&self, e: EntityKey) -> bool;
    /// `e`'s name.
    fn name(&self, e: EntityKey) -> Option<String>;
    /// `e`'s parent (`None`: a root, or `e` does not exist).
    fn parent(&self, e: EntityKey) -> Option<EntityKey>;
    /// `e`'s children, in key order.
    fn children(&self, e: EntityKey) -> Vec<EntityKey>;
    /// One property.
    fn prop(&self, e: EntityKey, path: &str) -> Option<Value>;
    /// Every property, in path order.
    fn props(&self, e: EntityKey) -> Vec<(String, Value)>;
    /// The root entities, in key order.
    fn roots(&self) -> Vec<EntityKey>;
    /// Every entity whose `path` holds `Value::Entity(target)`, in key order.
    fn referrers(&self, target: EntityKey, path: &str) -> Vec<EntityKey>;
}

impl SceneRead for Project {
    fn exists(&self, e: EntityKey) -> bool {
        self.contains(e)
    }
    fn name(&self, e: EntityKey) -> Option<String> {
        self.entity(e).map(|d| d.name().to_string())
    }
    fn parent(&self, e: EntityKey) -> Option<EntityKey> {
        self.entity(e).and_then(forge_cmd::EntityData::parent)
    }
    fn children(&self, e: EntityKey) -> Vec<EntityKey> {
        self.entity(e)
            .map(|d| d.children().collect())
            .unwrap_or_default()
    }
    fn prop(&self, e: EntityKey, path: &str) -> Option<Value> {
        self.entity(e).and_then(|d| d.property(path)).cloned()
    }
    fn props(&self, e: EntityKey) -> Vec<(String, Value)> {
        self.entity(e)
            .map(|d| {
                d.properties()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn roots(&self) -> Vec<EntityKey> {
        Project::roots(self).collect()
    }
    fn referrers(&self, target: EntityKey, path: &str) -> Vec<EntityKey> {
        let mut v: Vec<EntityKey> = Project::referrers(self, target)
            .filter(|(_, p)| *p == path)
            .map(|(e, _)| e)
            .collect();
        v.dedup();
        v
    }
}

impl SceneRead for DiffBuilder<'_> {
    fn exists(&self, e: EntityKey) -> bool {
        DiffBuilder::exists(self, e)
    }
    fn name(&self, e: EntityKey) -> Option<String> {
        DiffBuilder::name(self, e).ok()
    }
    fn parent(&self, e: EntityKey) -> Option<EntityKey> {
        DiffBuilder::parent(self, e).ok().flatten()
    }
    fn children(&self, e: EntityKey) -> Vec<EntityKey> {
        DiffBuilder::children(self, e).unwrap_or_default()
    }
    fn prop(&self, e: EntityKey, path: &str) -> Option<Value> {
        self.property(e, path).ok().flatten()
    }
    fn props(&self, e: EntityKey) -> Vec<(String, Value)> {
        self.property_paths(e)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| {
                let v = self.property(e, &p).ok().flatten()?;
                Some((p, v))
            })
            .collect()
    }
    fn roots(&self) -> Vec<EntityKey> {
        DiffBuilder::roots(self)
    }
    fn referrers(&self, target: EntityKey, path: &str) -> Vec<EntityKey> {
        DiffBuilder::referrers(self, target, path)
    }
}
