//! [`SceneDeriver`] — base changes reach every instance, in the same diff.
//!
//! Installed on the bus as its [`CommandDeriver`], it runs after **every** command is
//! planned, whoever sent it and whichever command it is, and:
//!
//! 1. **Refuses** what would break composition (only for changes the command itself made;
//!    what the deriver adds is consistent by construction): writing `scene.*` outside the
//!    scene commands (`SCENE-0006`), deleting or moving a node an instance mirrors from its
//!    base (`SCENE-0003`), deleting a scene or moving it out of the library while instances
//!    use it (`SCENE-0004`), and moving an instance into a scene that it contains
//!    (`SCENE-0002`, naming the loop).
//! 2. **Propagates**, as a worklist over the diff (derived changes are processed too, so a
//!    change runs down a chain of scenes of any depth): a property change on a base node is
//!    applied to every mirror whose value still equals the base's old value — a mirror
//!    holding anything else has overridden it and keeps it; the same for names (an instance
//!    root's name is its own); a node created, removed or moved in a scene is created,
//!    removed or moved in every instance; a node created inside a scene gets its uid, and a
//!    node put straight into the library becomes a scene.
//!
//! The cost is per change, never per project: finding a base node's mirrors is the
//! project's reference index (`O(log n + k)`), and the rest walks only the changed node's
//! ancestors. A command that touches no scene pays a map lookup per change.
//!
//! Commands that rebuild the whole project from a document (`forge.project.load`, a team
//! sync) are *exempt*: the document is already composed (the files store references and
//! the load expands them, [`crate::store::expand`]).

use std::collections::{BTreeSet, HashMap};

use forge_cmd::{Change, CmdError, CommandDeriver, DiffBuilder, EditorCommand, EntityKey, Value};

use crate::model::{
    self, ID, LIBRARY, UID, base_of, dependents, instance_root_of, instance_scene, is_meta,
    mix_uid, scene_id, scene_of, uid,
};
use crate::place::{self, Content};
use crate::plan::{self, is_scene_target};
use crate::{SceneError, SceneFaults, SceneRead};

/// The most changes one command may derive: a bound, so a corrupt project (a scene that
/// somehow contains itself) is refused instead of growing without end.
pub const MAX_DERIVED: usize = 4_000_000;

/// See the module docs.
pub struct SceneDeriver {
    exempt: BTreeSet<String>,
    faults: SceneFaults,
}

impl SceneDeriver {
    /// A deriver that leaves the `Invoke` targets in `exempt` alone (the commands that load
    /// a whole, already composed document).
    #[must_use]
    pub fn new(exempt: &[&str]) -> Self {
        Self {
            exempt: exempt.iter().map(|s| (*s).to_string()).collect(),
            faults: SceneFaults::default(),
        }
    }

    /// W2 positive controls only: the same deriver with fault switches on.
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    #[must_use]
    pub fn with_faults(exempt: &[&str], faults: SceneFaults) -> Self {
        Self {
            exempt: exempt.iter().map(|s| (*s).to_string()).collect(),
            faults,
        }
    }
}

/// Contents and placements read during one fan-out: `(as it was, as it is)`.
#[derive(Default)]
struct Memo {
    contents: HashMap<EntityKey, (Content, Content)>,
    placements: HashMap<EntityKey, (place::Placement, place::Placement)>,
}

impl Memo {
    fn pair(
        &mut self,
        b: &DiffBuilder<'_>,
        last: &HashMap<EntityKey, Content>,
        k: EntityKey,
    ) -> &(Content, Content) {
        self.contents
            .entry(k)
            .or_insert_with(|| (snapshot(b, last, k), model::content(b, k)))
    }

    /// Instance root `root`'s placement against its scene root, as it was and as it is.
    fn placement(
        &mut self,
        b: &DiffBuilder<'_>,
        last: &HashMap<EntityKey, Content>,
        root: EntityKey,
    ) -> (place::Placement, place::Placement) {
        if let Some(p) = self.placements.get(&root) {
            return p.clone();
        }
        let r = self.pair(b, last, root).clone();
        let s = match base_of(b, root).filter(|k| b.exists(*k)) {
            Some(scene) => self.pair(b, last, scene).clone(),
            None => r.clone(),
        };
        let p = (
            place::Placement::between(&r.0, &s.0),
            place::Placement::between(&r.1, &s.1),
        );
        self.placements.insert(root, p.clone());
        p
    }
}

/// The content `k` had when its mirrors were last brought up to date in this command, or
/// before the command.
fn snapshot(b: &DiffBuilder<'_>, last: &HashMap<EntityKey, Content>, k: EntityKey) -> Content {
    match last.get(&k) {
        Some(c) => c.clone(),
        None if b.project().contains(k) => model::content(b.project(), k),
        None => model::content(b, k),
    }
}

/// The nodes of instance `root` (its mirrors, not nested instances' own roots' insides that
/// belong to another instance root).
fn members(b: &DiffBuilder<'_>, root: EntityKey) -> Vec<EntityKey> {
    model::subtree(b, root)
        .into_iter()
        .filter(|k| *k != root && is_mirror(b, *k) && instance_root_of(b, *k) == Some(root))
        .collect()
}

fn is_library(r: &impl SceneRead, e: EntityKey) -> bool {
    r.prop(e, LIBRARY) == Some(Value::Bool(true))
}

/// A mirrored node (not an instance root, which is the container's own).
fn is_mirror(r: &impl SceneRead, e: EntityKey) -> bool {
    base_of(r, e).is_some() && instance_scene(r, e).is_none()
}

/// The name of the scene an instance root (or anything inside one) instances.
fn scene_name_of(r: &impl SceneRead, e: EntityKey) -> String {
    let id = instance_root_of(r, e)
        .and_then(|i| instance_scene(r, i))
        .unwrap_or_default();
    model::scene_root(r, &id)
        .and_then(|k| r.name(k))
        .unwrap_or(id)
}

/// The instances and derived scenes that still use scene root `root` (excluding those this
/// very command removes).
fn users(b: &DiffBuilder<'_>, root: EntityKey) -> Vec<EntityKey> {
    dependents(b, root)
        .into_iter()
        .filter(|d| b.exists(*d))
        .collect()
}

fn in_use(b: &DiffBuilder<'_>, root: EntityKey, users: &[EntityKey]) -> CmdError {
    SceneError::InUse {
        scene: b.name(root).unwrap_or_default(),
        users: users.len(),
        names: users
            .iter()
            .take(5)
            .filter_map(|u| DiffBuilder::name(b, *u).ok())
            .collect(),
    }
    .refused()
}

impl SceneDeriver {
    /// Step 1: refuse what the command itself did that composition cannot allow.
    fn validate(
        &self,
        cmd: &EditorCommand,
        b: &DiffBuilder<'_>,
        own: usize,
    ) -> Result<(), CmdError> {
        let pre = b.project();
        let by = match cmd {
            EditorCommand::Invoke { target, .. } => target.clone(),
            other => other.variant().to_string(),
        };
        for c in &b.changes()[..own] {
            match c {
                Change::Property { entity, path, .. } if is_meta(path) => {
                    if b.exists(*entity) {
                        return Err(SceneError::Reserved {
                            path: path.to_string(),
                            by,
                        }
                        .refused());
                    }
                }
                Change::Removed { entity, .. } => {
                    if is_mirror(pre, *entity)
                        && instance_root_of(pre, *entity).is_some_and(|r| b.exists(r))
                    {
                        return Err(SceneError::FromBase {
                            node: SceneRead::name(pre, *entity).unwrap_or_default(),
                            scene: scene_name_of(pre, *entity),
                            what: "delete",
                        }
                        .refused());
                    }
                    if scene_id(pre, *entity).is_some()
                        && SceneRead::parent(pre, *entity).is_some_and(|p| is_library(pre, p))
                    {
                        let u = users(b, *entity);
                        if !u.is_empty() {
                            return Err(in_use(b, *entity, &u));
                        }
                    }
                }
                Change::Reparented {
                    entity,
                    before,
                    after,
                } => {
                    if is_mirror(pre, *entity)
                        && instance_root_of(pre, *entity).is_some_and(|r| b.exists(r))
                    {
                        return Err(SceneError::FromBase {
                            node: SceneRead::name(pre, *entity).unwrap_or_default(),
                            scene: scene_name_of(pre, *entity),
                            what: "move",
                        }
                        .refused());
                    }
                    let was_scene = scene_id(pre, *entity).is_some()
                        && before.is_some_and(|p| is_library(pre, p));
                    let still_scene = after.is_some_and(|p| is_library(b, p));
                    if was_scene && !still_scene {
                        let u = users(b, *entity);
                        if !u.is_empty() {
                            return Err(in_use(b, *entity, &u));
                        }
                    }
                    if !self.faults.skip_cycle_check()
                        && let Some(container) = scene_of(b, *entity)
                        && let Some(cid) = scene_id(b, container)
                    {
                        let placed: BTreeSet<String> = model::subtree(b, *entity)
                            .into_iter()
                            .filter_map(|k| instance_scene(b, k))
                            .collect();
                        if let Some(chain) = model::find_cycle(b, &cid, &placed) {
                            return Err(SceneError::Cycle { chain }.refused());
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Step 2: the propagation worklist (see the module docs).
    fn propagate(&self, b: &mut DiffBuilder<'_>) -> Result<(), CmdError> {
        let ignore = self.faults.ignore_overrides();
        // Next free authored uid per scene root, computed once per command.
        let mut next: HashMap<EntityKey, u64> = HashMap::new();
        // The content each entity had when its mirrors were last brought up to date in this
        // command (absent: as the project held it before the command).
        let mut last: HashMap<EntityKey, Content> = HashMap::new();
        let mut i = 0;
        let own = b.changes().len();
        while i < b.changes().len() {
            if b.changes().len() - own > MAX_DERIVED {
                return Err(CmdError::Conflict {
                    detail: format!(
                        "scene composition did not settle after {MAX_DERIVED} derived changes (a scene that contains itself?)"
                    ),
                });
            }
            let c = b.changes()[i].clone();
            i += 1;
            match c {
                Change::Property { entity, path, .. } => {
                    if is_meta(&path) || !b.exists(entity) {
                        continue;
                    }
                    self.content_changed(b, entity, &mut last, ignore)?;
                }
                Change::Renamed {
                    entity,
                    before,
                    after,
                } => {
                    if !b.exists(entity) {
                        continue;
                    }
                    for d in dependents(b, entity) {
                        if instance_scene(b, d).is_some() {
                            continue; // an instance root's name is its own
                        }
                        let cur = b.name(d)?;
                        if (cur == before || ignore) && cur != after {
                            b.rename(d, &after)?;
                        }
                    }
                }
                Change::Created { entity, parent, .. } => {
                    if !b.exists(entity) {
                        continue;
                    }
                    if let Some(p) = parent
                        && is_library(b, p)
                        && scene_id(b, entity).is_none()
                    {
                        let name = b.name(entity)?;
                        let id = plan::default_id(b, &name);
                        b.set_property(entity, ID, Value::Text(id))?;
                    }
                    if uid(b, entity).is_none()
                        && let Some(scene) = scene_of(b, entity)
                    {
                        let u = self.uid_for(b, entity, scene, &mut next);
                        b.set_property(entity, UID, Value::Int(u as i64))?;
                    }
                    if let Some(p) = parent {
                        for d in dependents(b, p) {
                            plan::mirror_one(b, entity, d)?;
                        }
                    }
                }
                Change::Removed { entity, .. } => {
                    for d in dependents(b, entity) {
                        if b.exists(d) && instance_scene(b, d).is_none() {
                            b.despawn(d)?;
                        }
                    }
                }
                Change::Reparented {
                    entity,
                    before,
                    after,
                } => {
                    if !b.exists(entity) {
                        continue;
                    }
                    self.moved(b, entity, before, after, &mut next)?;
                }
                Change::Setting { .. } => {}
            }
        }
        Ok(())
    }

    /// `n`'s content changed: bring every mirror that depends on it up to date — its own
    /// mirrors (their base changed) and, when `n` holds a pose that places instances (an
    /// instance root, or the scene root instances are placed against), every node of those
    /// instances (their placement changed). A value is updated only where the mirror still
    /// holds what it was expected to hold before; anything else is an override and stays.
    fn content_changed(
        &self,
        b: &mut DiffBuilder<'_>,
        n: EntityKey,
        last: &mut HashMap<EntityKey, Content>,
        ignore: bool,
    ) -> Result<(), CmdError> {
        // The common case — an entity nothing mirrors and that places nothing (any world
        // edit, a gizmo drag) — costs an index lookup and a property lookup, no reads.
        let mut affected: Vec<EntityKey> = dependents(b, n);
        if affected.is_empty() && instance_scene(b, n).is_none() {
            return Ok(());
        }
        let now = model::content(b, n);
        let before = snapshot(b, last, n);
        if now == before {
            return Ok(()); // already brought up to date at this state
        }
        let posed = |c: &Content| {
            [
                place::P_FRAME,
                place::P_LOCAL,
                place::P_YAW,
                place::P_PITCH,
                place::P_ROLL,
                place::P_SCALE,
            ]
            .map(|k| c.get(k).cloned())
        };
        if posed(&before) != posed(&now) {
            let mut roots: Vec<EntityKey> = affected
                .iter()
                .copied()
                .filter(|d| instance_scene(b, *d).is_some())
                .collect();
            if instance_scene(b, n).is_some() {
                roots.push(n);
            }
            for r in roots {
                affected.extend(members(b, r));
            }
        }
        // Each entity's content (before, now) and each instance's placement (before, now) is
        // read once for the whole fan-out, not once per mirror.
        let mut memo = Memo::default();
        memo.contents.insert(n, (before, now));
        for m in affected {
            self.refresh(b, m, last, ignore, &mut memo)?;
        }
        if let Some((_, now)) = memo.contents.remove(&n) {
            last.insert(n, now);
        }
        Ok(())
    }

    /// Bring mirror `m` up to date: compare what it was expected to hold (from the
    /// snapshots, with `changed` as it was `was`) with what it is expected to hold now.
    fn refresh(
        &self,
        b: &mut DiffBuilder<'_>,
        m: EntityKey,
        last: &HashMap<EntityKey, Content>,
        ignore: bool,
        memo: &mut Memo,
    ) -> Result<(), CmdError> {
        if !b.exists(m) {
            return Ok(());
        }
        let Some(base) = base_of(b, m).filter(|k| b.exists(*k)) else {
            return Ok(());
        };
        // An instance root holds its scene root's content as it is; a mirror, its base's
        // content placed by its instance.
        let placement = if instance_scene(b, m).is_some() {
            None
        } else {
            instance_root_of(b, m).map(|root| memo.placement(b, last, root))
        };
        let (was, now) = memo.pair(b, last, base);
        let (old_exp, new_exp) = match &placement {
            Some((po, pn)) => (po.apply(was), pn.apply(now)),
            None => (
                std::borrow::Cow::Borrowed(was),
                std::borrow::Cow::Borrowed(now),
            ),
        };
        let paths: BTreeSet<&String> = old_exp.keys().chain(new_exp.keys()).collect();
        for p in paths {
            let (o, n) = (old_exp.get(p), new_exp.get(p));
            if o == n {
                continue;
            }
            let cur = b.property(m, p)?;
            if cur.as_ref() == o || ignore {
                match n {
                    Some(v) if cur.as_ref() != Some(v) => b.set_property(m, p, v.clone())?,
                    None if cur.is_some() => b.remove_property(m, p)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// A uid for a node newly inside `scene`: a mirror's is its mix, anything else the next
    /// free one.
    fn uid_for(
        &self,
        b: &DiffBuilder<'_>,
        e: EntityKey,
        scene: EntityKey,
        next: &mut HashMap<EntityKey, u64>,
    ) -> u64 {
        if is_mirror(b, e)
            && let (Some(ru), Some(nu)) = (
                instance_root_of(b, e).and_then(|r| uid(b, r)),
                base_of(b, e).and_then(|n| uid(b, n)),
            )
        {
            return mix_uid(ru, nu);
        }
        let n = next
            .entry(scene)
            .or_insert_with(|| model::next_uid(b, scene));
        *n += 1;
        *n - 1
    }

    /// A node moved: into or out of the library, between scenes, or within one.
    fn moved(
        &self,
        b: &mut DiffBuilder<'_>,
        entity: EntityKey,
        before: Option<EntityKey>,
        after: Option<EntityKey>,
        next: &mut HashMap<EntityKey, u64>,
    ) -> Result<(), CmdError> {
        let to_library = after.is_some_and(|p| is_library(b, p));
        if to_library && scene_id(b, entity).is_none() {
            let name = b.name(entity)?;
            let id = plan::default_id(b, &name);
            b.set_property(entity, ID, Value::Text(id))?;
        }
        if !to_library
            && before.is_some_and(|p| is_library(b.project(), p))
            && scene_id(b, entity).is_some()
        {
            b.remove_property(entity, ID)?; // it is no longer a scene (it had no users)
        }
        // Numbered in the scene it is now in, if that is another scene than before.
        let old_scene = scene_of(b.project(), entity);
        let new_scene = scene_of(b, entity);
        if let Some(s) = new_scene
            && Some(s) != old_scene
        {
            let n = next.entry(s).or_insert(0);
            if *n == 0 {
                *n = model::next_uid(b, s);
            }
            let mut at = *n;
            plan::number_subtree(b, entity, s, &mut at, old_scene.is_none())?;
            next.insert(s, at);
        }
        if new_scene.is_none() && old_scene.is_some() {
            // Out of every scene: uids mean nothing there.
            for k in model::subtree(b, entity) {
                if uid(b, k).is_some() {
                    b.remove_property(k, UID)?;
                }
            }
        }
        // Mirrors: moved to the mirror of the new parent in the same instance, or removed
        // when the new parent has none there; instances of the new place get the subtree.
        let targets: Vec<EntityKey> = match after {
            Some(p) => dependents(b, p),
            None => Vec::new(),
        };
        let mut covered: BTreeSet<EntityKey> = BTreeSet::new();
        for d in dependents(b, entity) {
            // An instance root is its container's own node: it stays where it was put.
            if !b.exists(d) || instance_scene(b, d).is_some() {
                continue;
            }
            let root = instance_root_of(b, d);
            match targets
                .iter()
                .copied()
                .find(|t| instance_root_of(b, *t) == root)
            {
                Some(t) => {
                    covered.insert(t);
                    if SceneRead::parent(b, d) != Some(t) {
                        b.reparent(d, Some(t))?;
                    }
                }
                None => b.despawn(d)?,
            }
        }
        for t in targets {
            if !covered.contains(&t) {
                plan::mirror_tree(b, entity, t)?;
            }
        }
        Ok(())
    }
}

impl CommandDeriver for SceneDeriver {
    fn derive(&self, cmd: &EditorCommand, b: &mut DiffBuilder<'_>) -> Result<(), CmdError> {
        let target = match cmd {
            EditorCommand::Invoke { target, .. } => Some(target.as_str()),
            _ => None,
        };
        if target.is_some_and(|t| self.exempt.contains(t)) || self.faults.no_propagation() {
            return Ok(());
        }
        let own = b.changes().len();
        if !target.is_some_and(is_scene_target) {
            self.validate(cmd, b, own)?;
        }
        self.propagate(b)
    }
}
