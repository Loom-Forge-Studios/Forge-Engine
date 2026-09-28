//! Scene composition in the editor (Ch.28 §28, DoD M5-9, WP-U20): the `forge.scene.*`
//! commands and the deriver on the core bus, and the composition reads the panels make over
//! the mirror.
//!
//! * The core puts `forge_scene` on its bus: the scene commands and the deriver, so an edit
//!   of a scene reaches its instances in the same diff whoever sends it. The loads that
//!   rebuild a whole project from its files ([`WHOLE_DOCUMENT_LOADS`]: open, pull, a team
//!   sync) are left alone: what they bring is already composed.
//! * The mirror implements [`SceneRead`], so the inspector and the hierarchy judge
//!   "inherited" and "overridden" with the very functions the deriver uses.
//! * Every change the panels make here is a command ([`instance_command`],
//!   [`revert_command`], [`make_local_command`], …): undoable, provenance-tagged, and the
//!   same command an automation session sends (I7).

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_scene::model::{self, OverrideRecord, Role};
use forge_scene::plan;
use forge_scene::{SceneRead, place};

use crate::mirror::ProjectMirror;

pub use forge_scene::model::{LIBRARY_NAME as SCENE_FOLDER, MakeLocalLoss, Role as SceneRole};

/// The `Invoke` targets the deriver leaves alone: the loads that rebuild a whole project
/// from its files (open, pull, a team sync), whose entities are already composed. The core
/// installs composition on its bus with them (only `forge_editor::core` holds the bus, I7).
pub const WHOLE_DOCUMENT_LOADS: [&str; 2] = [
    forge_project::format::LOAD_CMD,
    forge_project::merge::SYNC_CMD,
];

impl SceneRead for ProjectMirror {
    fn exists(&self, e: EntityKey) -> bool {
        self.entity(e).is_some()
    }
    fn name(&self, e: EntityKey) -> Option<String> {
        self.entity(e).map(|m| m.name.clone())
    }
    fn parent(&self, e: EntityKey) -> Option<EntityKey> {
        self.entity(e).and_then(|m| m.parent)
    }
    fn children(&self, e: EntityKey) -> Vec<EntityKey> {
        self.entity(e)
            .map(|m| m.children.iter().copied().collect())
            .unwrap_or_default()
    }
    fn prop(&self, e: EntityKey, path: &str) -> Option<Value> {
        self.property(e, path).cloned()
    }
    fn props(&self, e: EntityKey) -> Vec<(String, Value)> {
        self.entity(e)
            .map(|m| {
                m.properties
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn roots(&self) -> Vec<EntityKey> {
        ProjectMirror::roots(self).collect()
    }
    /// A client's mirror keeps no reference index: a scan. Only a scene root's "used by"
    /// line in the inspector asks, when that is what is selected.
    fn referrers(&self, target: EntityKey, path: &str) -> Vec<EntityKey> {
        self.entities()
            .filter(|(_, m)| m.properties.get(path) == Some(&Value::Entity(target)))
            .map(|(k, _)| *k)
            .collect()
    }
}

/// The drag payload of a scene tile in the asset browser (`scene:<id>`).
pub const SCENE_PAYLOAD: &str = "scene:";

/// A scene tile's drag payload.
#[must_use]
pub fn scene_payload(id: &str) -> String {
    format!("{SCENE_PAYLOAD}{id}")
}

/// The scene id a drag payload names, if it is a scene tile's.
#[must_use]
pub fn payload_scene(payload: &str) -> Option<&str> {
    payload.strip_prefix(SCENE_PAYLOAD)
}

fn invoke(target: &str, args: serde_json::Value) -> EditorCommand {
    EditorCommand::Invoke {
        target: target.into(),
        args: args.to_string(),
    }
}

/// Instance scene `id` under `parent` (a root without one).
#[must_use]
pub fn instance_command(id: &str, parent: Option<EntityKey>) -> EditorCommand {
    invoke(
        plan::INSTANCE_CMD,
        serde_json::json!({"scene": id, "parent": parent.map(|p| p.0)}),
    )
}

/// A new, empty scene named `name`.
#[must_use]
pub fn new_scene_command(name: &str) -> EditorCommand {
    invoke(plan::NEW, serde_json::json!({ "name": name }))
}

/// A new scene derived from scene `base`.
#[must_use]
pub fn new_inherited_command(base: &str) -> EditorCommand {
    invoke(plan::NEW_INHERITED, serde_json::json!({ "base": base }))
}

/// Revert to base: one property (`Some(path)`) or every override of the node.
#[must_use]
pub fn revert_command(entity: EntityKey, path: Option<&str>) -> EditorCommand {
    invoke(
        plan::REVERT,
        match path {
            Some(p) => serde_json::json!({"entity": entity.0, "path": p}),
            None => serde_json::json!({ "entity": entity.0 }),
        },
    )
}

/// Make Local on an instance root.
#[must_use]
pub fn make_local_command(entity: EntityKey) -> EditorCommand {
    invoke(plan::MAKE_LOCAL, serde_json::json!({ "entity": entity.0 }))
}

/// Save a branch as a scene (an instance takes its place).
#[must_use]
pub fn pack_command(entity: EntityKey) -> EditorCommand {
    invoke(plan::PACK, serde_json::json!({ "entity": entity.0 }))
}

/// A scene as the asset browser lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneEntry {
    pub id: String,
    pub name: String,
    pub root: EntityKey,
    /// Its base scene's id, for a derived scene.
    pub inherits: Option<String>,
}

/// Every scene in the project's scene library, in key order.
#[must_use]
pub fn scene_entries(m: &ProjectMirror) -> Vec<SceneEntry> {
    model::scenes(m)
        .into_iter()
        .map(|(id, root)| SceneEntry {
            name: m.entity(root).map(|e| e.name.clone()).unwrap_or_default(),
            inherits: model::instance_scene(m, root),
            id,
            root,
        })
        .collect()
}

/// How one entity takes part in composition, as the inspector shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub role: Role,
    /// The name of the scene it comes from (instance roots, inherited nodes, derived scene
    /// roots) or is (a scene root).
    pub scene: String,
    /// The root "Open base scene" opens (its base's scene).
    pub base_scene: Option<EntityKey>,
    /// Its base node, if linked.
    pub base: Option<EntityKey>,
    /// What it overrides (empty when it is not linked).
    pub overrides: OverrideRecord,
    /// Linked, but its base scene is gone (a file that names a missing scene).
    pub missing: bool,
}

impl Link {
    /// Is `path` overridden here?
    #[must_use]
    pub fn overridden(&self, path: &str) -> bool {
        self.overrides.paths().any(|p| p == path)
    }
    /// Is this a node whose properties come from a base (so rows show inherited /
    /// overridden)?
    #[must_use]
    pub fn inherits(&self) -> bool {
        self.base.is_some() && !self.missing
    }
}

/// The name of scene `id` (its id when it is gone).
fn scene_name(m: &ProjectMirror, id: &str) -> String {
    model::scene_root(m, id)
        .and_then(|k| m.entity(k))
        .map_or_else(|| id.to_string(), |e| e.name.clone())
}

/// `e`'s composition, or `None` for a plain entity outside every scene.
#[must_use]
pub fn link(m: &ProjectMirror, e: EntityKey) -> Option<Link> {
    let role = model::role(m, e);
    let base = model::base_of(m, e);
    let missing = base.is_some_and(|b| m.entity(b).is_none())
        || (matches!(role, Role::InstanceRoot { .. }) && base.is_none());
    let scene = match &role {
        Role::Plain => return None,
        Role::Library => String::new(),
        Role::SceneRoot { inherits, .. } => match inherits {
            Some(b) => scene_name(m, b),
            None => m.entity(e).map(|x| x.name.clone()).unwrap_or_default(),
        },
        Role::InstanceRoot { scene } => scene_name(m, scene),
        Role::Inherited => {
            let id = model::instance_root_of(m, e)
                .and_then(|r| model::instance_scene(m, r))
                .unwrap_or_default();
            scene_name(m, &id)
        }
    };
    Some(Link {
        overrides: model::overrides(m, e).unwrap_or_default(),
        base_scene: model::base_scene_of(m, e),
        base,
        role,
        scene,
        missing,
    })
}

/// How many instances and derived scenes use the scene rooted at `root` (a scan of the
/// mirror; asked only while a scene root is selected).
#[must_use]
pub fn users(m: &ProjectMirror, root: EntityKey) -> usize {
    model::dependents(m, root)
        .into_iter()
        .filter(|d| model::instance_scene(m, *d).is_some())
        .count()
}

/// What Make Local on `root` loses (see [`MakeLocalLoss`]).
#[must_use]
pub fn make_local_loss(m: &ProjectMirror, root: EntityKey) -> Option<MakeLocalLoss> {
    model::make_local_loss(m, root)
}

/// Is `e` in the scene library (a scene's template, not the world)?
#[must_use]
pub fn in_library(m: &ProjectMirror, e: EntityKey) -> bool {
    model::in_library(m, e)
}

/// The root of the scene `e` belongs to, if any.
#[must_use]
pub fn scene_of(m: &ProjectMirror, e: EntityKey) -> Option<EntityKey> {
    model::scene_of(m, e)
}

/// The transform paths composition places (the viewport's; checked equal by a test).
pub const PLACED_PATHS: [&str; 6] = [
    place::P_FRAME,
    place::P_LOCAL,
    place::P_YAW,
    place::P_PITCH,
    place::P_ROLL,
    place::P_SCALE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_places_the_viewports_transform_paths() {
        use crate::viewport::scene::{P_FRAME, P_LOCAL, P_PITCH, P_ROLL, P_SCALE, P_YAW};
        assert_eq!(
            PLACED_PATHS,
            [P_FRAME, P_LOCAL, P_YAW, P_PITCH, P_ROLL, P_SCALE]
        );
    }

    #[test]
    fn payloads_round_trip() {
        assert_eq!(payload_scene(&scene_payload("tree")), Some("tree"));
        assert_eq!(payload_scene("models/ship.gltf"), None);
    }
}
