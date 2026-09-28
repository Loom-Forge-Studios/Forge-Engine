//! The composition model: which entities are scenes, instances and inherited nodes, and
//! what an instance overrides. Pure reads over [`SceneRead`].
//!
//! **Representation (Ch.28 §28.2).** Scene definitions live in the project, under one
//! *scene library* root, so every edit of a scene is an ordinary command (undo, provenance,
//! automation sessions and collaboration for free). A scene is a subtree whose root is a child of
//! the library and carries [`ID`]. An *instance* is an authored node carrying [`INSTANCE`] (the
//! scene's id) whose subtree **mirrors** the scene: every mirrored node carries [`BASE`], the
//! entity it mirrors. A *derived* scene is a scene whose own root is an instance of its base:
//! nesting and inheritance are one mechanism (Godot's model). Nodes a scene or an instance adds
//! itself are *authored*; inside a scene every node carries a [`UID`] unique in that scene, the
//! stable id override records key on.
//!
//! **Overrides are diffs.** A mirrored node overrides exactly the properties whose value
//! differs from its base node's (a property the base has and the node lacks is *cleared*),
//! and its name if it differs (an instance root's name is always its own). Nothing else
//! records an override, so there is no bookkeeping to drift: an edit on an instance *is*
//! the override, and Revert to base is setting the value back.

use std::collections::{BTreeMap, BTreeSet};

use forge_cmd::{EntityKey, Value};
use serde::{Deserialize, Serialize};

use crate::SceneRead;

/// Every composition property lives under this prefix.
pub const PREFIX: &str = "scene";
/// On the scene library root: `Bool(true)`.
pub const LIBRARY: &str = "scene.library";
/// On a scene's root: its stable id (`Text`).
pub const ID: &str = "scene.id";
/// On every node inside a scene: its id in that scene (`Int`), what override records key on.
pub const UID: &str = "scene.uid";
/// On an instance's root: the id of the scene it instances (`Text`).
pub const INSTANCE: &str = "scene.instance";
/// On every mirrored node and every instance root: the entity it mirrors (`Entity`).
pub const BASE: &str = "scene.base";
/// Files only: an override record's base node, by its [`UID`] in the instanced scene.
pub const OF: &str = "scene.of";
/// Files only: the record's name is an override.
pub const RENAMED: &str = "scene.renamed";
/// Files only: `scene.cleared.<path>` — the instance clears the base's `<path>`.
pub const CLEARED: &str = "scene.cleared";
/// Files only, on an instance root: `uid:id,…` — the ids of its mirrors the file does not
/// store, so a load gives every entity back its id.
pub const KEYS: &str = "scene.keys";
/// The scene library root's name (project data, not a UI string: every client and every
/// locale writes the same bytes).
pub const LIBRARY_NAME: &str = "Scenes";

/// Longest walk up a hierarchy before a (corrupt) parent loop is assumed.
const MAX_DEPTH: usize = 4096;
/// Authored uids stay below this; mixed (mirrored) uids set it.
const MIXED_BIT: u64 = 1 << 62;

/// Is `path` composition bookkeeping (`scene` or `scene.…`)?
#[must_use]
pub fn is_meta(path: &str) -> bool {
    path.strip_prefix(PREFIX)
        .is_some_and(|r| r.is_empty() || r.starts_with('.'))
}

/// The uid of the mirror of a node with uid `base` under an instance root with uid `root`:
/// a fixed integer mix (SplitMix64 finaliser), identical on every platform, in its own range
/// so it never meets an authored uid.
#[must_use]
pub fn mix_uid(root: u64, base: u64) -> u64 {
    let mut z = root
        .rotate_left(29)
        .wrapping_add(base)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z & (MIXED_BIT - 1)) | MIXED_BIT
}

fn text(v: Option<Value>) -> Option<String> {
    match v {
        Some(Value::Text(t)) => Some(t),
        _ => None,
    }
}

fn entity(v: Option<Value>) -> Option<EntityKey> {
    match v {
        Some(Value::Entity(k)) => Some(k),
        _ => None,
    }
}

/// `e`'s [`UID`], if it has one.
pub fn uid(r: &impl SceneRead, e: EntityKey) -> Option<u64> {
    match r.prop(e, UID) {
        Some(Value::Int(i)) => u64::try_from(i).ok(),
        _ => None,
    }
}

/// The node `e` mirrors ([`BASE`]).
pub fn base_of(r: &impl SceneRead, e: EntityKey) -> Option<EntityKey> {
    entity(r.prop(e, BASE))
}

/// The scene id `e` instances, if `e` is an instance root ([`INSTANCE`]).
pub fn instance_scene(r: &impl SceneRead, e: EntityKey) -> Option<String> {
    text(r.prop(e, INSTANCE))
}

/// The scene id of a scene root ([`ID`]).
pub fn scene_id(r: &impl SceneRead, e: EntityKey) -> Option<String> {
    text(r.prop(e, ID))
}

/// The scene library root, if the project has one.
pub fn library(r: &impl SceneRead) -> Option<EntityKey> {
    r.roots()
        .into_iter()
        .find(|k| r.prop(*k, LIBRARY) == Some(Value::Bool(true)))
}

/// Every scene: `(id, root)`, in key order of the roots.
pub fn scenes(r: &impl SceneRead) -> Vec<(String, EntityKey)> {
    let Some(lib) = library(r) else {
        return Vec::new();
    };
    r.children(lib)
        .into_iter()
        .filter_map(|k| scene_id(r, k).map(|id| (id, k)))
        .collect()
}

/// The root of the scene with id `id`.
pub fn scene_root(r: &impl SceneRead, id: &str) -> Option<EntityKey> {
    scenes(r).into_iter().find(|(s, _)| s == id).map(|(_, k)| k)
}

/// Ancestors of `e`, `e` first (bounded: a corrupt loop ends the walk).
pub fn ancestry(r: &impl SceneRead, e: EntityKey) -> Vec<EntityKey> {
    let mut out = Vec::new();
    let mut at = Some(e);
    while let Some(k) = at {
        if out.len() > MAX_DEPTH || !r.exists(k) {
            break;
        }
        out.push(k);
        at = r.parent(k);
    }
    out
}

/// Is `e` inside the scene library (the library root included)?
pub fn in_library(r: &impl SceneRead, e: EntityKey) -> bool {
    ancestry(r, e)
        .last()
        .is_some_and(|root| r.prop(*root, LIBRARY) == Some(Value::Bool(true)))
}

/// The root of the scene `e` belongs to (`e` itself for a scene root), if `e` is inside a
/// scene.
pub fn scene_of(r: &impl SceneRead, e: EntityKey) -> Option<EntityKey> {
    let chain = ancestry(r, e);
    let lib = *chain.last()?;
    if r.prop(lib, LIBRARY) != Some(Value::Bool(true)) || chain.len() < 2 {
        return None;
    }
    let root = chain[chain.len() - 2];
    scene_id(r, root).map(|_| root)
}

/// The instance root `e` belongs to: the nearest ancestor-or-self carrying [`INSTANCE`].
pub fn instance_root_of(r: &impl SceneRead, e: EntityKey) -> Option<EntityKey> {
    ancestry(r, e)
        .into_iter()
        .find(|k| r.prop(*k, INSTANCE).is_some())
}

/// The instances and derived scenes of scene `root`, and every mirror of a node (the
/// entities whose [`BASE`] is `n`).
pub fn dependents(r: &impl SceneRead, n: EntityKey) -> Vec<EntityKey> {
    r.referrers(n, BASE)
}

/// `e`'s properties that are not bookkeeping, by path.
pub fn content(r: &impl SceneRead, e: EntityKey) -> BTreeMap<String, Value> {
    r.props(e)
        .into_iter()
        .filter(|(p, _)| !is_meta(p))
        .collect()
}

/// A subtree, `e` first, pre-order with children in key order.
pub fn subtree(r: &impl SceneRead, e: EntityKey) -> Vec<EntityKey> {
    let mut out = Vec::new();
    let mut stack = vec![e];
    while let Some(k) = stack.pop() {
        out.push(k);
        let mut kids = r.children(k);
        kids.reverse();
        stack.extend(kids);
    }
    out
}

/// The scene ids scene `root` instances anywhere inside it (its own base, for a derived
/// scene, included).
pub fn scene_deps(r: &impl SceneRead, root: EntityKey) -> BTreeSet<String> {
    subtree(r, root)
        .into_iter()
        .filter_map(|k| instance_scene(r, k))
        .collect()
}

/// If putting instances of the scenes `placed` inside the scene `container` would make a
/// scene contain itself, the loop as scene names (`[container, …, container]`).
pub fn find_cycle(
    r: &impl SceneRead,
    container: &str,
    placed: &BTreeSet<String>,
) -> Option<Vec<String>> {
    let roots: BTreeMap<String, EntityKey> = scenes(r).into_iter().collect();
    let name = |id: &str| {
        roots
            .get(id)
            .and_then(|k| r.name(*k))
            .unwrap_or_else(|| id.to_string())
    };
    // Depth-first from each placed scene looking for `container`, remembering the path.
    for start in placed {
        let mut stack: Vec<(String, Vec<String>)> = vec![(start.clone(), vec![start.clone()])];
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some((id, path)) = stack.pop() {
            if id == container {
                let mut chain = vec![name(container)];
                chain.extend(path.iter().map(|s| name(s)));
                return Some(chain);
            }
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(root) = roots.get(&id) {
                for d in scene_deps(r, *root) {
                    let mut p = path.clone();
                    p.push(d.clone());
                    stack.push((d, p));
                }
            }
        }
    }
    None
}

/// The next free authored uid in the scene rooted at `scene_root`.
pub fn next_uid(r: &impl SceneRead, scene_root: EntityKey) -> u64 {
    subtree(r, scene_root)
        .into_iter()
        .filter_map(|k| uid(r, k))
        .filter(|u| *u < MIXED_BIT)
        .max()
        .map_or(1, |m| m + 1)
}

/// One overridden property: its reflect path and the instance's value (`None`: the
/// instance clears what the base has). A reflected diff (Ch.6): the inspector, the session
/// schema and the files all read it the same way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, bevy_reflect::Reflect)]
pub struct PropertyOverride {
    /// The reflect path (`transform.position`).
    pub path: String,
    /// The instance's value; `None` when it clears the base's.
    pub value: Option<Value>,
}

/// What one linked node overrides of its base node: the reflected property diff, and its
/// name when that differs too.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, bevy_reflect::Reflect)]
pub struct OverrideRecord {
    /// The overridden properties, in path order.
    pub properties: Vec<PropertyOverride>,
    /// The node's own name, when it overrides its base's (never for an instance root, whose
    /// name is always its own).
    pub name: Option<String>,
}

impl OverrideRecord {
    /// Nothing overridden?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.properties.is_empty() && self.name.is_none()
    }

    /// The overridden paths.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.properties.iter().map(|p| p.path.as_str())
    }
}

/// The diff of `node`'s content against `base`'s (both maps without bookkeeping).
#[must_use]
pub fn diff_content(
    node: &BTreeMap<String, Value>,
    base: &BTreeMap<String, Value>,
) -> Vec<PropertyOverride> {
    let mut out = Vec::new();
    for (p, v) in node {
        if base.get(p) != Some(v) {
            out.push(PropertyOverride {
                path: p.clone(),
                value: Some(v.clone()),
            });
        }
    }
    for p in base.keys() {
        if !node.contains_key(p) {
            out.push(PropertyOverride {
                path: p.clone(),
                value: None,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The placement of instance root `root`: its content against its scene root's
/// ([`crate::place`]); an identity when the scene root is gone.
pub fn placement_of(r: &impl SceneRead, root: EntityKey) -> crate::place::Placement {
    let own = content(r, root);
    match base_of(r, root).filter(|b| r.exists(*b)) {
        Some(b) => crate::place::Placement::between(&own, &content(r, b)),
        None => crate::place::Placement::between(&own, &own),
    }
}

/// What linked node `e` holds when it overrides nothing: its base's content, placed by its
/// instance ([`crate::place`]); an instance root's is its scene root's content as it is.
/// `None` if `e` is not linked.
pub fn expected(r: &impl SceneRead, e: EntityKey) -> Option<BTreeMap<String, Value>> {
    let base = base_of(r, e).filter(|b| r.exists(*b))?;
    let bc = content(r, base);
    if instance_scene(r, e).is_some() {
        return Some(bc);
    }
    Some(match instance_root_of(r, e) {
        Some(root) => placement_of(r, root).apply(&bc).into_owned(),
        None => bc,
    })
}

/// What linked node `e` overrides of its base (`None` if `e` is not linked).
pub fn overrides(r: &impl SceneRead, e: EntityKey) -> Option<OverrideRecord> {
    let base = base_of(r, e)?;
    let exp = expected(r, e)?;
    let properties = diff_content(&content(r, e), &exp);
    let name = if instance_scene(r, e).is_some() {
        None
    } else {
        let (n, b) = (r.name(e)?, r.name(base)?);
        (n != b).then_some(n)
    };
    Some(OverrideRecord { properties, name })
}

/// How a node takes part in composition (the inspector's and the hierarchy's view of it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    /// Not composed: an ordinary entity.
    Plain,
    /// The scene library root.
    Library,
    /// A scene's root; `inherits` names its base scene for a derived scene.
    SceneRoot {
        /// Its id.
        id: String,
        /// Its base scene's id, for a derived scene.
        inherits: Option<String>,
    },
    /// An instance's root: `scene` is the scene it instances.
    InstanceRoot {
        /// The instanced scene's id.
        scene: String,
    },
    /// A node mirrored from a base scene.
    Inherited,
}

/// `e`'s role.
pub fn role(r: &impl SceneRead, e: EntityKey) -> Role {
    if r.prop(e, LIBRARY) == Some(Value::Bool(true)) {
        return Role::Library;
    }
    if let Some(id) = scene_id(r, e)
        && r.parent(e).is_some_and(|p| r.prop(p, LIBRARY).is_some())
    {
        return Role::SceneRoot {
            id,
            inherits: instance_scene(r, e),
        };
    }
    if let Some(scene) = instance_scene(r, e) {
        return Role::InstanceRoot { scene };
    }
    if base_of(r, e).is_some() {
        return Role::Inherited;
    }
    Role::Plain
}

/// The scene "Open base scene" opens for `e`: the root of the scene its base node is in.
pub fn base_scene_of(r: &impl SceneRead, e: EntityKey) -> Option<EntityKey> {
    scene_of(r, base_of(r, e)?)
}

/// What Make Local on instance root `root` loses, for the confirmation that names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakeLocalLoss {
    /// The scene's name.
    pub scene: String,
    /// Nodes that stop following it.
    pub nodes: usize,
    /// Of those, how many had overrides (they keep their values, as plain values).
    pub overridden: usize,
    /// Scenes nested inside it that stay linked (their instances stay instances).
    pub nested: usize,
}

/// See [`MakeLocalLoss`]; `None` unless `root` is an instance root.
pub fn make_local_loss(r: &impl SceneRead, root: EntityKey) -> Option<MakeLocalLoss> {
    let id = instance_scene(r, root)?;
    let scene = scene_root(r, &id)
        .and_then(|k| r.name(k))
        .unwrap_or_else(|| id.clone());
    let mut nodes = 0;
    let mut overridden = 0;
    let mut nested = 0;
    for k in subtree(r, root) {
        if instance_root_of(r, k) != Some(root) || base_of(r, k).is_none() {
            continue;
        }
        nodes += 1;
        if overrides(r, k).is_some_and(|o| !o.is_empty()) {
            overridden += 1;
        }
        if k != root && base_of(r, k).is_some_and(|b| instance_scene(r, b).is_some()) {
            nested += 1;
        }
    }
    Some(MakeLocalLoss {
        scene,
        nodes,
        overridden,
        nested,
    })
}
