//! The files hold **references and override records, never copies** (the store rule).
//!
//! In a running project an instance is materialised: every node of its scene is mirrored,
//! so the viewport, the play core and an automation session's query see it like any entity. The
//! project files keep only what cannot be derived:
//!
//! * an **instance root**: its own record with [`INSTANCE`] (the scene's stable id), its
//!   name, the properties it overrides (`scene.cleared.<path>` for one it clears), and
//!   `scene.keys` — `uid:id` for each mirror not stored, so every entity loads back with
//!   the id it had (a team's merge matches entities by id; a few bytes per node instead
//!   of the node);
//! * a mirrored node only when it carries something of its own — an **override record**:
//!   `scene.of` (its base node's uid in the instanced scene), the overridden properties,
//!   `scene.renamed` if its name is an override, and its own uid if it has a stable one
//!   that differs from the derived one. It is listed under its instance root and keeps its
//!   id, so authored children under it and entity references to it still resolve;
//! * everything authored, as it is.
//!
//! [`compact`] writes that form from a project's entities; [`expand`] rebuilds the mirrors
//! from it, scenes before the scenes that instance them. Both are pure, total and
//! deterministic (ordered maps, ids allocated in one fixed order), so the same project
//! always writes the same bytes, and the form is **lossless** for any composed project:
//! `expand(compact(e)) == e`, ids included.

use std::collections::{BTreeMap, BTreeSet};

use forge_cmd::Value;

use crate::SceneFaults;
use crate::model::{
    BASE, CLEARED, ID, INSTANCE, KEYS, LIBRARY, OF, PropertyOverride, RENAMED, UID, diff_content,
    is_meta, mix_uid,
};

/// One entity as a project file holds it (`forge-project`'s `EntityDoc`, field for field).
#[derive(Clone, Debug, PartialEq)]
pub struct FlatEntity {
    /// Its id in the file (references use it).
    pub id: u64,
    pub name: String,
    pub parent: Option<u64>,
    pub properties: BTreeMap<String, Value>,
}

/// An indexed document.
struct Doc {
    ents: BTreeMap<u64, FlatEntity>,
    kids: BTreeMap<u64, BTreeSet<u64>>,
}

impl Doc {
    fn new(v: Vec<FlatEntity>) -> Self {
        let mut d = Self {
            ents: BTreeMap::new(),
            kids: BTreeMap::new(),
        };
        for e in v {
            d.ents.insert(e.id, e);
        }
        let links: Vec<(u64, u64)> = d
            .ents
            .values()
            .filter_map(|e| e.parent.map(|p| (p, e.id)))
            .collect();
        for (p, c) in links {
            d.kids.entry(p).or_default().insert(c);
        }
        d
    }

    fn get(&self, id: u64) -> Option<&FlatEntity> {
        self.ents.get(&id)
    }

    fn prop(&self, id: u64, path: &str) -> Option<&Value> {
        self.get(id).and_then(|e| e.properties.get(path))
    }

    fn uid(&self, id: u64) -> Option<u64> {
        match self.prop(id, UID) {
            Some(Value::Int(i)) => u64::try_from(*i).ok(),
            _ => None,
        }
    }

    fn base(&self, id: u64) -> Option<u64> {
        match self.prop(id, BASE) {
            Some(Value::Entity(k)) => Some(k.0),
            _ => None,
        }
    }

    fn text(&self, id: u64, path: &str) -> Option<String> {
        match self.prop(id, path) {
            Some(Value::Text(t)) => Some(t.clone()),
            _ => None,
        }
    }

    fn kids(&self, id: u64) -> Vec<u64> {
        self.kids
            .get(&id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    fn set_parent(&mut self, id: u64, parent: Option<u64>) {
        let old = self.get(id).and_then(|e| e.parent);
        if let Some(o) = old
            && let Some(s) = self.kids.get_mut(&o)
        {
            s.remove(&id);
        }
        if let Some(e) = self.ents.get_mut(&id) {
            e.parent = parent;
        }
        if let Some(p) = parent {
            self.kids.entry(p).or_default().insert(id);
        }
    }

    fn insert(&mut self, e: FlatEntity) {
        if let Some(p) = e.parent {
            self.kids.entry(p).or_default().insert(e.id);
        }
        self.ents.insert(e.id, e);
    }

    /// Ancestors-or-self (bounded).
    fn ancestry(&self, id: u64) -> Vec<u64> {
        let mut out = Vec::new();
        let mut at = Some(id);
        while let Some(k) = at {
            if out.len() > self.ents.len() || !self.ents.contains_key(&k) {
                break;
            }
            out.push(k);
            at = self.get(k).and_then(|e| e.parent);
        }
        out
    }

    fn instance_root_of(&self, id: u64) -> Option<u64> {
        self.ancestry(id)
            .into_iter()
            .find(|k| self.prop(*k, INSTANCE).is_some())
    }

    fn is_mirror(&self, id: u64) -> bool {
        self.base(id).is_some() && self.prop(id, INSTANCE).is_none()
    }

    /// Pre-order, children in id order.
    fn subtree(&self, id: u64) -> Vec<u64> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(k) = stack.pop() {
            if out.len() > self.ents.len() {
                break;
            }
            out.push(k);
            let mut ks = self.kids(k);
            ks.reverse();
            stack.extend(ks);
        }
        out
    }

    fn content(&self, id: u64) -> BTreeMap<String, Value> {
        self.get(id)
            .map(|e| {
                e.properties
                    .iter()
                    .filter(|(p, _)| !is_meta(p))
                    .map(|(p, v)| (p.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The placement of instance root `root` against its scene root (identity without one).
    fn placement(&self, root: u64) -> crate::place::Placement {
        let own = self.content(root);
        let scene = match self.base(root).filter(|b| self.ents.contains_key(b)) {
            Some(b) => self.content(b),
            None => own.clone(),
        };
        crate::place::Placement::between(&own, &scene)
    }

    fn into_vec(self) -> Vec<FlatEntity> {
        self.ents.into_values().collect()
    }
}

fn cleared_key(path: &str) -> String {
    format!("{CLEARED}.{path}")
}

/// Write the overrides of `diff` into `props` (values set, cleared ones marked).
fn put_overrides(props: &mut BTreeMap<String, Value>, diff: Vec<PropertyOverride>) {
    for o in diff {
        match o.value {
            Some(v) => {
                props.insert(o.path, v);
            }
            None => {
                props.insert(cleared_key(&o.path), Value::Bool(true));
            }
        }
    }
}

/// The paths `scene.cleared.<path>` marks in `props`, removing the marks.
fn take_cleared(props: &mut BTreeMap<String, Value>) -> BTreeSet<String> {
    let pre = format!("{CLEARED}.");
    let keys: Vec<String> = props
        .keys()
        .filter(|k| k.starts_with(&pre))
        .cloned()
        .collect();
    keys.into_iter()
        .map(|k| {
            props.remove(&k);
            k[pre.len()..].to_string()
        })
        .collect()
}

/// The stored form of a project's entities (see the module docs). Entities keep their
/// ids; the result is in id order.
#[must_use]
pub fn compact(entities: Vec<FlatEntity>) -> Vec<FlatEntity> {
    compact_with(entities, &SceneFaults::default())
}

/// [`compact`] with fault switches (the store guard's control keeps every copy).
#[must_use]
pub fn compact_with(entities: Vec<FlatEntity>, faults: &SceneFaults) -> Vec<FlatEntity> {
    if faults.store_copies() {
        return entities;
    }
    let doc = Doc::new(entities);
    // Entities something names by a reference other than a mirror's own link: they keep a
    // record so the reference still resolves after a load.
    let mut referenced: BTreeSet<u64> = BTreeSet::new();
    for e in doc.ents.values() {
        for (p, v) in &e.properties {
            if let Value::Entity(k) = v
                && p != BASE
            {
                referenced.insert(k.0);
            }
        }
    }
    let mut out: Vec<FlatEntity> = Vec::with_capacity(doc.ents.len());
    let mut dropped: BTreeMap<u64, Vec<(u64, u64)>> = BTreeMap::new();
    for e in doc.ents.values() {
        if e.properties.contains_key(INSTANCE) {
            // An instance root: its overrides against the scene root it mirrors.
            let mut rec = e.clone();
            if let Some(b) = doc.base(e.id).filter(|b| doc.get(*b).is_some()) {
                let diff = diff_content(&doc.content(e.id), &doc.content(b));
                rec.properties.retain(|p, _| is_meta(p) && p != BASE);
                put_overrides(&mut rec.properties, diff);
            } else {
                rec.properties.remove(BASE);
            }
            out.push(rec);
            continue;
        }
        if !doc.is_mirror(e.id) {
            out.push(e.clone());
            continue;
        }
        let root = doc.instance_root_of(e.id);
        let base = doc.base(e.id).filter(|b| doc.get(*b).is_some());
        let (Some(root), Some(base), Some(bu)) = (root, base, base.and_then(|b| doc.uid(b))) else {
            // Cannot be derived again (a broken link): keep it whole.
            out.push(e.clone());
            continue;
        };
        // Against what it holds with no override: its base's content, placed by its instance.
        let base_content = doc.content(base);
        let expected = doc.placement(root).apply(&base_content);
        let diff = diff_content(&doc.content(e.id), &expected);
        let base_name = doc.get(base).map(|b| b.name.as_str()).unwrap_or_default();
        let renamed = e.name != base_name;
        let derived_uid = doc.uid(root).map(|ru| mix_uid(ru, bu));
        let own_uid = doc.uid(e.id).filter(|u| Some(*u) != derived_uid);
        let local_kids = doc.kids(e.id).into_iter().any(|k| !doc.is_mirror(k));
        if diff.is_empty()
            && !renamed
            && own_uid.is_none()
            && !local_kids
            && !referenced.contains(&e.id)
        {
            // Derived entirely from its scene: only its id is kept, on the instance root.
            dropped.entry(root).or_default().push((bu, e.id));
            continue;
        }
        let mut props = BTreeMap::new();
        props.insert(OF.to_string(), Value::Int(bu as i64));
        if renamed {
            props.insert(RENAMED.to_string(), Value::Bool(true));
        }
        if let Some(u) = own_uid {
            props.insert(UID.to_string(), Value::Int(u as i64));
        }
        put_overrides(&mut props, diff);
        out.push(FlatEntity {
            id: e.id,
            name: e.name.clone(),
            parent: Some(root),
            properties: props,
        });
    }
    for rec in &mut out {
        if let Some(mut ks) = dropped.remove(&rec.id) {
            ks.sort_unstable();
            rec.properties
                .insert(KEYS.to_string(), Value::Text(encode_keys(&ks)));
        }
    }
    out
}

/// `uid:id,uid:id` — the ids of an instance's unstored mirrors, by the uid of the node each
/// mirrors, so a load gives every entity back its id (a team's merge matches entities by it).
fn encode_keys(ks: &[(u64, u64)]) -> String {
    ks.iter()
        .map(|(u, i)| format!("{u}:{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn decode_keys(s: &str) -> BTreeMap<u64, u64> {
    s.split(',')
        .filter_map(|p| {
            let (u, i) = p.split_once(':')?;
            Some((u.trim().parse().ok()?, i.trim().parse().ok()?))
        })
        .collect()
}

/// Rebuild the mirrors of a stored document (see the module docs). Returns the entities
/// in id order (records keep their ids, new mirrors get ids after every stored one) and a
/// warning per thing that could not be composed: an instance of a scene the document does
/// not have, scenes that instance each other (both left unexpanded, never a loop), an
/// override record whose base node is gone (kept as a plain child of the instance). Nothing
/// is ever dropped: what could not be composed stays in the document, where the editor
/// shows it (an instance whose scene is missing says so in the inspector).
#[must_use]
pub fn expand(entities: Vec<FlatEntity>) -> (Vec<FlatEntity>, Vec<String>) {
    let mut doc = Doc::new(entities);
    let mut warnings = Vec::new();
    // Fresh ids go above every id the file uses, the kept mirror ids included.
    let kept_max = doc
        .ents
        .values()
        .filter_map(|e| match e.properties.get(KEYS) {
            Some(Value::Text(k)) => decode_keys(k).into_values().max(),
            _ => None,
        })
        .max();
    let mut next_id = doc
        .ents
        .keys()
        .next_back()
        .copied()
        .max(kept_max)
        .map_or(0, |k| k + 1);
    // The scenes: children of the library carrying an id.
    let libs: Vec<u64> = doc
        .ents
        .values()
        .filter(|e| e.parent.is_none() && e.properties.get(LIBRARY) == Some(&Value::Bool(true)))
        .map(|e| e.id)
        .collect();
    let mut scenes: BTreeMap<String, u64> = BTreeMap::new();
    for l in &libs {
        for k in doc.kids(*l) {
            if let Some(id) = doc.text(k, ID) {
                scenes.entry(id).or_insert(k);
            }
        }
    }
    // Dependencies, and a topological order (Kahn); what is left is in a loop.
    let deps: BTreeMap<String, BTreeSet<String>> = scenes
        .iter()
        .map(|(id, root)| {
            let d = doc
                .subtree(*root)
                .into_iter()
                .filter_map(|k| doc.text(k, INSTANCE))
                .filter(|s| scenes.contains_key(s))
                .collect();
            (id.clone(), d)
        })
        .collect();
    let mut order: Vec<String> = Vec::new();
    let mut done: BTreeSet<String> = BTreeSet::new();
    loop {
        let ready: Vec<String> = deps
            .iter()
            .filter(|(id, d)| !done.contains(*id) && d.iter().all(|x| done.contains(x)))
            .map(|(id, _)| id.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        for id in ready {
            done.insert(id.clone());
            order.push(id);
        }
    }
    let looped: BTreeSet<String> = scenes
        .keys()
        .filter(|s| !done.contains(*s))
        .cloned()
        .collect();
    if !looped.is_empty() {
        warnings.push(format!(
            "scenes that instance each other are left unexpanded: {}",
            looped.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    // Scenes first, in dependency order; then everything outside the library.
    let mut groups: Vec<Vec<u64>> = order
        .iter()
        .filter_map(|id| scenes.get(id).map(|r| vec![*r]))
        .collect();
    let world: Vec<u64> = doc
        .ents
        .values()
        .filter(|e| e.parent.is_none() && !libs.contains(&e.id))
        .map(|e| e.id)
        .collect();
    groups.push(world);
    for group in groups {
        let roots: Vec<u64> = group
            .iter()
            .flat_map(|r| doc.subtree(*r))
            .filter(|k| doc.prop(*k, INSTANCE).is_some())
            .collect();
        for i in roots {
            expand_instance(&mut doc, i, &scenes, &looped, &mut next_id, &mut warnings);
        }
    }
    (doc.into_vec(), warnings)
}

/// Mirror node `n` (alone) under `parent` for instance `ru`'s expansion: from its override
/// record if the instance has one, else new, with the id the file kept for it if any.
struct Expansion<'a> {
    ru: Option<u64>,
    placement: crate::place::Placement,
    records: BTreeMap<u64, u64>,
    keys: BTreeMap<u64, u64>,
    next_id: &'a mut u64,
}

fn expand_instance(
    doc: &mut Doc,
    i: u64,
    scenes: &BTreeMap<String, u64>,
    looped: &BTreeSet<String>,
    next_id: &mut u64,
    warnings: &mut Vec<String>,
) {
    let Some(sid) = doc.text(i, INSTANCE) else {
        return;
    };
    let name = doc.get(i).map(|e| e.name.clone()).unwrap_or_default();
    let Some(root) = scenes.get(&sid).copied() else {
        warnings.push(format!(
            "{name:?} instances the scene {sid:?}, which the project does not have: kept as it is"
        ));
        return;
    };
    if looped.contains(&sid) {
        return;
    }
    // The root: the scene root's content under the instance's own overrides.
    let root_content = doc.content(root);
    let mut keys = BTreeMap::new();
    if let Some(e) = doc.ents.get_mut(&i) {
        let cleared = take_cleared(&mut e.properties);
        if let Some(Value::Text(k)) = e.properties.remove(KEYS) {
            keys = decode_keys(&k);
        }
        for (p, v) in root_content {
            if !e.properties.contains_key(&p) && !cleared.contains(&p) {
                e.properties.insert(p, v);
            }
        }
        e.properties
            .insert(BASE.to_string(), Value::Entity(forge_cmd::EntityKey(root)));
    }
    // Its override records, by the uid of the node each one overrides.
    let mut records: BTreeMap<u64, u64> = BTreeMap::new();
    for k in doc.kids(i) {
        if let Some(Value::Int(u)) = doc.prop(k, OF)
            && let Ok(u) = u64::try_from(*u)
            && records.insert(u, k).is_some()
        {
            warnings.push(format!(
                "{name:?} has two override records for one node: the first is used"
            ));
        }
    }
    let mut x = Expansion {
        ru: doc.uid(i),
        placement: doc.placement(i),
        records,
        keys,
        next_id,
    };
    // Pre-order over the scene, iteratively (no stack depth), parents mirrored first.
    let mut stack: Vec<(u64, u64)> = doc.kids(root).into_iter().rev().map(|c| (c, i)).collect();
    while let Some((n, parent)) = stack.pop() {
        let m = mirror_node(doc, n, parent, &mut x);
        for c in doc.kids(n).into_iter().rev() {
            stack.push((c, m));
        }
    }
    for (_, rec) in std::mem::take(&mut x.records) {
        let rname = doc.get(rec).map(|e| e.name.clone()).unwrap_or_default();
        // Kept as a plain child of the instance, overrides and all, so nothing is lost: it
        // shows in the hierarchy, and merges back if the node returns to the scene.
        warnings.push(format!(
            "{name:?}: the override record for {rname:?} names a node its scene no longer has; it is kept as a child of the instance"
        ));
        let _ = rec;
    }
}

fn mirror_node(doc: &mut Doc, n: u64, parent: u64, x: &mut Expansion<'_>) -> u64 {
    let nu = doc.uid(n);
    let content = x.placement.apply(&doc.content(n)).into_owned();
    let base_name = doc.get(n).map(|e| e.name.clone()).unwrap_or_default();
    let derived_uid = match (x.ru, nu) {
        (Some(r), Some(u)) => Some(mix_uid(r, u)),
        _ => None,
    };
    match nu.and_then(|u| x.records.remove(&u)) {
        Some(rec) => {
            if let Some(e) = doc.ents.get_mut(&rec) {
                e.properties.remove(OF);
                let renamed = e.properties.remove(RENAMED).is_some();
                let cleared = take_cleared(&mut e.properties);
                if !renamed {
                    e.name = base_name;
                }
                for (p, v) in content {
                    if !e.properties.contains_key(&p) && !cleared.contains(&p) {
                        e.properties.insert(p, v);
                    }
                }
                e.properties
                    .insert(BASE.to_string(), Value::Entity(forge_cmd::EntityKey(n)));
                if !e.properties.contains_key(UID)
                    && let Some(u) = derived_uid
                {
                    e.properties.insert(UID.to_string(), Value::Int(u as i64));
                }
            }
            doc.set_parent(rec, Some(parent));
            rec
        }
        None => {
            let kept = nu
                .and_then(|u| x.keys.remove(&u))
                .filter(|id| !doc.ents.contains_key(id));
            let id = kept.unwrap_or_else(|| {
                let id = *x.next_id;
                *x.next_id += 1;
                id
            });
            let mut properties = content;
            properties.insert(BASE.to_string(), Value::Entity(forge_cmd::EntityKey(n)));
            if let Some(u) = derived_uid {
                properties.insert(UID.to_string(), Value::Int(u as i64));
            }
            doc.insert(FlatEntity {
                id,
                name: base_name,
                parent: Some(parent),
                properties,
            });
            id
        }
    }
}
