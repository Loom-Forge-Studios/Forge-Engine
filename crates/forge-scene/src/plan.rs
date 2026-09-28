//! The scene commands (`forge.scene.*`), planned on a [`DiffBuilder`] like every handler, so
//! each is one undoable, provenance-tagged diff that a person, a script and an automation session
//! send the same way (I7).
//!
//! | Target | Arguments | What it does |
//! |---|---|---|
//! | [`NEW`] | `name`, `id?` | a new, empty scene in the scene library |
//! | [`NEW_INHERITED`] | `base`, `name?`, `id?` | a scene derived from `base`: it mirrors `base` and stores only what it overrides and adds |
//! | [`INSTANCE`] | `scene`, `parent?`, `name?` | an instance of `scene` under `parent` (a root without one); refused if it would make a scene contain itself |
//! | [`REVERT`] | `entity`, `path?` | Revert to base: one property, or (no `path`) every override of the node |
//! | [`MAKE_LOCAL`] | `entity` | break an instance's link: its nodes become the container's own, keeping their values; scenes nested in it stay instances |
//! | [`PACK`] | `entity`, `id?` | save a branch as a scene: it moves into the library and an instance takes its place |

use std::collections::BTreeSet;

use forge_cmd::{CmdError, CommandPolicy, DiffBuilder, EntityKey, Value};

use crate::model::{
    self, BASE, ID, INSTANCE, LIBRARY, LIBRARY_NAME, UID, base_of, content, instance_root_of,
    instance_scene, mix_uid, scene_id, scene_of, scene_root, uid,
};
use crate::{SceneError, SceneRead};

/// `forge.scene.new`.
pub const NEW: &str = "forge.scene.new";
/// `forge.scene.new_inherited`.
pub const NEW_INHERITED: &str = "forge.scene.new_inherited";
/// `forge.scene.instance`.
pub const INSTANCE_CMD: &str = "forge.scene.instance";
/// `forge.scene.revert`.
pub const REVERT: &str = "forge.scene.revert";
/// `forge.scene.make_local`.
pub const MAKE_LOCAL: &str = "forge.scene.make_local";
/// `forge.scene.pack`.
pub const PACK: &str = "forge.scene.pack";
/// Every scene command.
pub const TARGETS: &[&str] = &[NEW, NEW_INHERITED, INSTANCE_CMD, REVERT, MAKE_LOCAL, PACK];

/// Is `target` a scene command (the only commands that may write `scene.*`)?
#[must_use]
pub fn is_scene_target(target: &str) -> bool {
    TARGETS.contains(&target)
}

/// A handler's plan function.
pub type PlanFn = fn(&mut DiffBuilder<'_>, &serde_json::Value) -> Result<(), CmdError>;

/// Every scene command with its policy and planner (ordinary: undoable, any issuer).
#[must_use]
pub fn handlers() -> Vec<(&'static str, CommandPolicy, PlanFn)> {
    vec![
        (NEW, CommandPolicy::ORDINARY, plan_new as PlanFn),
        (NEW_INHERITED, CommandPolicy::ORDINARY, plan_new_inherited),
        (INSTANCE_CMD, CommandPolicy::ORDINARY, plan_instance),
        (REVERT, CommandPolicy::ORDINARY, plan_revert),
        (MAKE_LOCAL, CommandPolicy::ORDINARY, plan_make_local),
        (PACK, CommandPolicy::ORDINARY, plan_pack),
    ]
}

// ---- arguments --------------------------------------------------------------------------

fn bad(target: &str, why: impl Into<String>) -> CmdError {
    CmdError::BadArgs {
        target: target.to_string(),
        why: why.into(),
    }
}

fn str_arg(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn entity_arg(
    target: &str,
    args: &serde_json::Value,
    key: &str,
) -> Result<Option<EntityKey>, CmdError> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(|k| Some(EntityKey(k)))
            .ok_or_else(|| bad(target, format!("{key:?} must be an entity key (a number)"))),
    }
}

fn required_entity(
    target: &str,
    args: &serde_json::Value,
    key: &str,
) -> Result<EntityKey, CmdError> {
    entity_arg(target, args, key)?
        .ok_or_else(|| bad(target, format!("missing entity argument {key:?}")))
}

/// Is `id` a well-formed scene id (`[a-z0-9_]`, 1–64 characters)?
#[must_use]
pub fn id_is_well_formed(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A free scene id derived from `name` (`Pine Tree` → `pine_tree`, then `pine_tree_2`, …):
/// deterministic, so a replayed command log makes the same id.
pub fn default_id(r: &impl SceneRead, name: &str) -> String {
    let mut stem: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    stem = stem.trim_matches('_').to_string();
    while stem.contains("__") {
        stem = stem.replace("__", "_");
    }
    stem.truncate(48);
    if stem.is_empty() {
        stem = "scene".into();
    }
    let taken: BTreeSet<String> = model::scenes(r).into_iter().map(|(id, _)| id).collect();
    if !taken.contains(&stem) {
        return stem;
    }
    (2u64..)
        .map(|n| format!("{stem}_{n}"))
        .find(|c| !taken.contains(c))
        .unwrap_or(stem)
}

fn check_new_id(b: &DiffBuilder<'_>, target: &str, id: &str) -> Result<(), CmdError> {
    if !id_is_well_formed(id) || scene_root(b, id).is_some() {
        return Err(SceneError::BadId(id.to_string()).bad_args(target));
    }
    Ok(())
}

// ---- building blocks the deriver shares ---------------------------------------------------

/// The scene library root, created if the project has none yet.
pub fn ensure_library(b: &mut DiffBuilder<'_>) -> Result<EntityKey, CmdError> {
    if let Some(l) = model::library(b) {
        return Ok(l);
    }
    let l = b.spawn(LIBRARY_NAME, None)?;
    b.set_property(l, LIBRARY, Value::Bool(true))?;
    Ok(l)
}

/// Make `m` mirror `n`: `n`'s content, the [`BASE`] link, and (inside a scene) its uid.
fn link_mirror(b: &mut DiffBuilder<'_>, n: EntityKey, m: EntityKey) -> Result<(), CmdError> {
    b.set_property(m, BASE, Value::Entity(n))?;
    // Its base's content, placed by its instance (crate::place).
    for (p, v) in model::expected(b, m).unwrap_or_else(|| content(b, n)) {
        b.set_property(m, &p, v)?;
    }
    if scene_of(b, m).is_some()
        && let (Some(ru), Some(nu)) = (instance_root_of(b, m).and_then(|r| uid(b, r)), uid(b, n))
    {
        b.set_property(m, UID, Value::Int(mix_uid(ru, nu) as i64))?;
    }
    Ok(())
}

/// Mirror node `n` (alone, not its children) under `parent`; returns the mirror.
pub fn mirror_one(
    b: &mut DiffBuilder<'_>,
    n: EntityKey,
    parent: EntityKey,
) -> Result<EntityKey, CmdError> {
    // Backstop against a scene that contains itself (the cycle checks refuse it first): a
    // mirror's chain of bases crosses one scene per hop, so it is never longer than the
    // number of scenes plus the world; a longer one would grow without end. Real
    // composition is never [`MAX_CHAIN`] deep.
    let limit = MAX_CHAIN;
    let mut hops = 0usize;
    let mut at = Some(n);
    while let Some(k) = at {
        hops += 1;
        if hops > limit || k == parent {
            return Err(CmdError::Conflict {
                detail: format!(
                    "{n} would be mirrored inside itself (a scene that contains itself)"
                ),
            });
        }
        at = base_of(b, k);
    }
    let name = b.name(n)?;
    let m = b.spawn(&name, Some(parent))?;
    link_mirror(b, n, m)?;
    Ok(m)
}

/// Mirror `n` and its whole subtree under `parent`; returns the mirror of `n`. Iterative
/// over a snapshot of the subtree taken first, so it is finite and uses no stack depth
/// even if `parent` lies inside `n` (which the cycle check refuses anyway).
pub fn mirror_tree(
    b: &mut DiffBuilder<'_>,
    n: EntityKey,
    parent: EntityKey,
) -> Result<EntityKey, CmdError> {
    let nodes = model::subtree(b, n);
    let mut made: std::collections::HashMap<EntityKey, EntityKey> =
        std::collections::HashMap::with_capacity(nodes.len());
    for k in nodes {
        let under = if k == n {
            parent
        } else {
            match b.parent(k)?.and_then(|p| made.get(&p).copied()) {
                Some(p) => p,
                None => continue,
            }
        };
        let m = mirror_one(b, k, under)?;
        made.insert(k, m);
    }
    made.get(&n).copied().ok_or(CmdError::UnknownEntity(n))
}

/// Create an instance of the scene rooted at `root` (id `id`) under `parent`: the root node
/// with [`INSTANCE`], then every node of the scene mirrored under it. `scene_uid` is the
/// instance root's uid when it lands inside a scene.
fn materialize(
    b: &mut DiffBuilder<'_>,
    id: &str,
    root: EntityKey,
    parent: Option<EntityKey>,
    name: &str,
    scene_uid: Option<u64>,
    new_scene_id: Option<&str>,
) -> Result<EntityKey, CmdError> {
    let i = b.spawn(name, parent)?;
    if let Some(sid) = new_scene_id {
        b.set_property(i, ID, Value::Text(sid.to_string()))?;
    }
    if let Some(u) = scene_uid {
        b.set_property(i, UID, Value::Int(u as i64))?;
    }
    b.set_property(i, INSTANCE, Value::Text(id.to_string()))?;
    for (p, v) in content(b, root) {
        b.set_property(i, &p, v)?;
    }
    b.set_property(i, BASE, Value::Entity(root))?;
    for c in b.children(root)? {
        mirror_tree(b, c, i)?;
    }
    Ok(i)
}

/// Give every node of `e`'s subtree a uid in the scene rooted at `scene`: authored nodes
/// (and instance roots) the next free ones, mirrored nodes their mix. Pre-order, so an
/// instance root is numbered before its mirrors.
pub fn number_subtree(
    b: &mut DiffBuilder<'_>,
    e: EntityKey,
    scene: EntityKey,
    next: &mut u64,
    only_missing: bool,
) -> Result<(), CmdError> {
    if *next == 0 {
        *next = model::next_uid(b, scene);
    }
    for k in model::subtree(b, e) {
        if only_missing && uid(b, k).is_some() {
            continue;
        }
        let mirrored = base_of(b, k).is_some() && instance_scene(b, k).is_none();
        let u = if mirrored {
            let ru = instance_root_of(b, k).and_then(|r| uid(b, r));
            let nu = base_of(b, k).and_then(|n| uid(b, n));
            match (ru, nu) {
                (Some(ru), Some(nu)) => mix_uid(ru, nu),
                _ => {
                    *next += 1;
                    *next - 1
                }
            }
        } else {
            *next += 1;
            *next - 1
        };
        if uid(b, k) != Some(u) {
            b.set_property(k, UID, Value::Int(u as i64))?;
        }
    }
    Ok(())
}

// ---- the commands -------------------------------------------------------------------------

/// `forge.scene.new {name, id?}`.
pub fn plan_new(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let name = str_arg(args, "name").ok_or_else(|| bad(NEW, "missing string argument \"name\""))?;
    let id = str_arg(args, "id").unwrap_or_else(|| default_id(b, &name));
    check_new_id(b, NEW, &id)?;
    let lib = ensure_library(b)?;
    let root = b.spawn(&name, Some(lib))?;
    b.set_property(root, ID, Value::Text(id))?;
    b.set_property(root, UID, Value::Int(1))?;
    Ok(())
}

/// `forge.scene.new_inherited {base, name?, id?}`.
pub fn plan_new_inherited(
    b: &mut DiffBuilder<'_>,
    args: &serde_json::Value,
) -> Result<(), CmdError> {
    let base = str_arg(args, "base")
        .ok_or_else(|| bad(NEW_INHERITED, "missing string argument \"base\""))?;
    let root = scene_root(b, &base)
        .ok_or_else(|| SceneError::UnknownScene(base.clone()).bad_args(NEW_INHERITED))?;
    let name = match str_arg(args, "name") {
        Some(n) => n,
        None => b.name(root)?,
    };
    let id = str_arg(args, "id").unwrap_or_else(|| default_id(b, &name));
    check_new_id(b, NEW_INHERITED, &id)?;
    let lib = ensure_library(b)?;
    materialize(b, &base, root, Some(lib), &name, Some(1), Some(&id))?;
    Ok(())
}

/// `forge.scene.instance {scene, parent?, name?}`.
pub fn plan_instance(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let id = str_arg(args, "scene")
        .ok_or_else(|| bad(INSTANCE_CMD, "missing string argument \"scene\""))?;
    let root = scene_root(b, &id)
        .ok_or_else(|| SceneError::UnknownScene(id.clone()).bad_args(INSTANCE_CMD))?;
    let parent = entity_arg(INSTANCE_CMD, args, "parent")?;
    if let Some(p) = parent
        && !b.exists(p)
    {
        return Err(CmdError::UnknownEntity(p));
    }
    let lib = model::library(b);
    let into_library = parent.is_some() && parent == lib;
    let container = parent.and_then(|p| scene_of(b, p));
    if let Some(c) = container
        && let Some(cid) = scene_id(b, c)
        && let Some(chain) = model::find_cycle(b, &cid, &BTreeSet::from([id.clone()]))
    {
        return Err(SceneError::Cycle { chain }.bad_args(INSTANCE_CMD));
    }
    let name = match str_arg(args, "name") {
        Some(n) => n,
        None => b.name(root)?,
    };
    let (scene_uid, new_id) = if into_library {
        // An instance placed straight into the library is a derived scene.
        (Some(1), Some(default_id(b, &name)))
    } else {
        (container.map(|c| model::next_uid(b, c)), None)
    };
    materialize(b, &id, root, parent, &name, scene_uid, new_id.as_deref())?;
    Ok(())
}

/// `forge.scene.revert {entity, path?}`.
pub fn plan_revert(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let e = required_entity(REVERT, args, "entity")?;
    let name = b.name(e)?;
    let base = base_of(b, e)
        .filter(|k| b.exists(*k))
        .ok_or_else(|| SceneError::NotLinked(name.clone()).bad_args(REVERT))?;
    // What the node holds with no override: its base's value, placed by its instance.
    let exp = model::expected(b, e).unwrap_or_default();
    let set = |b: &mut DiffBuilder<'_>, p: &str| -> Result<(), CmdError> {
        match exp.get(p).cloned() {
            Some(v) => b.set_property(e, p, v),
            None => {
                if b.property(e, p)?.is_some() {
                    b.remove_property(e, p)
                } else {
                    Ok(())
                }
            }
        }
    };
    match str_arg(args, "path") {
        Some(p) => {
            if model::is_meta(&p) {
                return Err(SceneError::Reserved {
                    path: p,
                    by: REVERT.into(),
                }
                .bad_args(REVERT));
            }
            set(b, &p)?;
        }
        None => {
            let o = model::overrides(b, e).unwrap_or_default();
            for p in o.paths().map(str::to_string).collect::<Vec<_>>() {
                set(b, &p)?;
            }
            if o.name.is_some() {
                let n = b.name(base)?;
                b.rename(e, &n)?;
            }
        }
    }
    Ok(())
}

/// `forge.scene.make_local {entity}`: see the module docs. Nodes keep their uids, so records
/// that instances of the container key on them stay valid.
pub fn plan_make_local(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let i = required_entity(MAKE_LOCAL, args, "entity")?;
    let name = b.name(i)?;
    if instance_scene(b, i).is_none() {
        return Err(if base_of(b, i).is_some() {
            SceneError::NotInstanceRoot(name)
        } else {
            SceneError::NotLinked(name)
        }
        .bad_args(MAKE_LOCAL));
    }
    // (A derived scene's root works the same way: the scene then stands alone.)
    let members: Vec<EntityKey> = model::subtree(b, i)
        .into_iter()
        .filter(|k| instance_root_of(b, *k) == Some(i) && base_of(b, *k).is_some())
        .collect();
    for k in members {
        let Some(n) = base_of(b, k) else { continue };
        if k == i {
            b.remove_property(i, INSTANCE)?;
            b.remove_property(i, BASE)?;
        } else if let Some(nested) = instance_scene(b, n) {
            // A scene nested in it: stays an instance of that scene.
            b.set_property(k, INSTANCE, Value::Text(nested))?;
            match base_of(b, n) {
                Some(r) => b.set_property(k, BASE, Value::Entity(r))?,
                None => b.remove_property(k, BASE)?,
            }
        } else if let Some(deeper) = base_of(b, n) {
            // Inside a nested scene: now mirrors that scene's node directly.
            b.set_property(k, BASE, Value::Entity(deeper))?;
        } else {
            b.remove_property(k, BASE)?;
        }
    }
    Ok(())
}

/// `forge.scene.pack {entity, id?}`: see the module docs.
pub fn plan_pack(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let e = required_entity(PACK, args, "entity")?;
    let name = b.name(e)?;
    if model::in_library(b, e) {
        return Err(bad(
            PACK,
            format!("{name:?} is already in the scene library"),
        ));
    }
    if base_of(b, e).is_some() && instance_scene(b, e).is_none() {
        let scene = instance_root_of(b, e)
            .and_then(|r| instance_scene(b, r))
            .unwrap_or_default();
        return Err(SceneError::FromBase {
            node: name,
            scene,
            what: "pack",
        }
        .bad_args(PACK));
    }
    let id = str_arg(args, "id").unwrap_or_else(|| default_id(b, &name));
    check_new_id(b, PACK, &id)?;
    let parent = b.parent(e)?;
    let container = parent.and_then(|p| scene_of(b, p));
    let lib = ensure_library(b)?;
    b.reparent(e, Some(lib))?;
    b.set_property(e, ID, Value::Text(id.clone()))?;
    let mut next = 1;
    number_subtree(b, e, e, &mut next, false)?;
    let scene_uid = container.map(|c| model::next_uid(b, c));
    materialize(b, &id, e, parent, &name, scene_uid, None)?;
    Ok(())
}

/// The longest chain of bases a mirror may have (instance of an instance of … a scene):
/// deeper than any real composition, and short enough that a scene which somehow contains
/// itself is refused at once instead of growing (see [`mirror_one`]).
pub const MAX_CHAIN: usize = 64;
