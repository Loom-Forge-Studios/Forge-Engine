//! **Three-way merge on the reflect tree** (Ch.33 §33.2 "semantic merge", Ch.37 §37.4) and
//! the **key-preserving patch** a team sandbox is rebased with (WP-U10, ADR 0039).
//!
//! A project is a tree of entities with reflect-path properties, plus settings. A merge of
//! `mine` and `theirs` over their common `base` happens **per property of that tree**, never
//! per line of a file: "A moved the light, B changed its colour" merges cleanly because the
//! two changes touch different paths. The rule for every subject (a setting, an entity's
//! name, its parent, one property, its existence) is the classic one:
//!
//! | mine vs base | theirs vs base | result |
//! |---|---|---|
//! | unchanged | changed | theirs |
//! | changed | unchanged | mine |
//! | changed | changed, to the same value | that value |
//! | changed | changed, differently | **a [`Conflict`]**, surfaced with all three values |
//!
//! "Changed" is exactly a **failed precondition** (Ch.37 §37.4): my edit assumed the base
//! value, and theirs replaced it. A conflict is never swallowed: the merge lists it, the
//! result is not applied until each has a [`Side`] chosen, and the precondition layer of the
//! patch ([`plan_patch`]) re-checks every value it replaces when it applies.
//!
//! **Identity is the entity key.** Every sandbox is rebuilt from the baseline with the
//! baseline's keys (`DiffBuilder::spawn_at`), so an id in `mine` and in `theirs` is the same
//! entity. Two sandboxes can still allocate the same fresh key for two different new
//! entities; the merge then **re-keys mine** (above every id either side uses, references
//! remapped) instead of calling it a conflict — both entities survive.
//!
//! Merges that would loop the hierarchy (I put A under B, they put B under A) turn the
//! parent changes involved into conflicts; an entity whose parent the other side deleted is
//! an orphan conflict (keep it, as a root, or delete it too).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use forge_cmd::{CmdError, DiffBuilder, EntityKey, Value};
use serde::{Deserialize, Serialize};

use crate::format::{EntityDoc, ProjectDoc};

/// The `Invoke` target that applies a patch (performed by the editor core: a pull, a rebase,
/// joining a team; never sent by a client).
pub const SYNC_CMD: &str = "forge.collab.sync";

/// What a conflict is about. Its [`Subject::key`] names it across recomputations, so a choice
/// made in the panel survives an unrelated edit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Subject {
    /// A project setting.
    Setting(String),
    /// An entity's name.
    Name(u64),
    /// An entity's parent.
    Parent(u64),
    /// One property of an entity (its reflect path).
    Property(u64, String),
    /// One side deleted the entity, the other changed it.
    Exists(u64),
    /// The other side deleted the entity's parent.
    Orphan(u64),
}

impl Subject {
    /// The stable key (`setting:editor.grid`, `e12:transform.position`, `e12:name`).
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Setting(k) => format!("setting:{k}"),
            Self::Name(e) => format!("e{e}:name"),
            Self::Parent(e) => format!("e{e}:parent"),
            Self::Property(e, p) => format!("e{e}:{p}"),
            Self::Exists(e) => format!("e{e}:exists"),
            Self::Orphan(e) => format!("e{e}:orphan"),
        }
    }

    /// The entity it concerns, if any.
    #[must_use]
    pub fn entity(&self) -> Option<u64> {
        match self {
            Self::Setting(_) => None,
            Self::Name(e)
            | Self::Parent(e)
            | Self::Property(e, _)
            | Self::Exists(e)
            | Self::Orphan(e) => Some(*e),
        }
    }
}

/// Which side a conflict takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// My sandbox's value.
    Mine,
    /// The baseline's (the teammate's) value.
    Theirs,
}

impl Side {
    /// `mine` / `theirs`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mine => "mine",
            Self::Theirs => "theirs",
        }
    }
    /// Parse [`Side::name`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mine" => Some(Self::Mine),
            "theirs" => Some(Self::Theirs),
            _ => None,
        }
    }
}

/// One conflict: the subject and its three values (`None`: absent — unset, removed, a root,
/// deleted). For `Exists` and `Orphan` the values are short descriptions of each side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub subject: Subject,
    /// What a person reads: `Lamp › light.colour`, `setting editor.grid_size`.
    pub label: String,
    pub base: Option<Value>,
    pub mine: Option<Value>,
    pub theirs: Option<Value>,
    /// The side chosen for it (`None`: not resolved yet).
    pub choice: Option<Side>,
}

/// A merge's outcome (see the module docs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merge {
    /// The merged document. Unresolved conflicts take theirs here; it is applied only when
    /// [`Merge::is_clean`].
    pub doc: ProjectDoc,
    /// Every conflict, resolved or not, in subject order.
    pub conflicts: Vec<Conflict>,
    /// Entities of mine re-keyed because theirs added a different entity with the same key:
    /// `(old, new)`.
    pub rekeyed: Vec<(u64, u64)>,
    /// The chosen sides would loop the hierarchy (names the entities); nothing is applied.
    pub hierarchy_loop: Option<String>,
}

impl Merge {
    /// Conflicts with no side chosen.
    #[must_use]
    pub fn unresolved(&self) -> usize {
        self.conflicts.iter().filter(|c| c.choice.is_none()).count()
    }
    /// Every conflict resolved and no loop: the merged document may be applied.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.unresolved() == 0 && self.hierarchy_loop.is_none()
    }
}

type Index<'a> = BTreeMap<u64, &'a EntityDoc>;

fn index(d: &ProjectDoc) -> Index<'_> {
    d.entities.iter().map(|e| (e.id, e)).collect()
}

/// Same name, parent and properties.
fn same_entity(a: &EntityDoc, b: &EntityDoc) -> bool {
    a.name == b.name && a.parent == b.parent && a.properties == b.properties
}

fn remap_value(v: &Value, map: &BTreeMap<u64, u64>) -> Value {
    match v {
        Value::Entity(k) => map
            .get(&k.0)
            .map_or_else(|| v.clone(), |n| Value::Entity(EntityKey(*n))),
        other => other.clone(),
    }
}

/// `doc` with its entity ids (and every reference to them) renamed by `map`.
fn remap_doc(doc: &ProjectDoc, map: &BTreeMap<u64, u64>) -> ProjectDoc {
    let id = |i: u64| map.get(&i).copied().unwrap_or(i);
    ProjectDoc {
        settings: doc
            .settings
            .iter()
            .map(|(k, v)| (k.clone(), remap_value(v, map)))
            .collect(),
        entities: doc
            .entities
            .iter()
            .map(|e| EntityDoc {
                id: id(e.id),
                name: e.name.clone(),
                parent: e.parent.map(id),
                properties: e
                    .properties
                    .iter()
                    .map(|(k, v)| (k.clone(), remap_value(v, map)))
                    .collect(),
            })
            .collect(),
    }
}

fn parent_value(p: Option<u64>) -> Option<Value> {
    p.map(|k| Value::Entity(EntityKey(k)))
}

fn text(s: &str) -> Option<Value> {
    Some(Value::Text(s.to_string()))
}

/// The outcome of one three-way pick: the value, or a conflict (the value then follows the
/// choice, theirs when there is none).
enum Pick<T> {
    Clean(T),
    Conflict,
}

fn pick<T: PartialEq + Clone>(b: &T, m: &T, t: &T) -> Pick<T> {
    if m == t || m == b {
        Pick::Clean(t.clone())
    } else if t == b {
        Pick::Clean(m.clone())
    } else {
        Pick::Conflict
    }
}

struct Merger<'a> {
    b: Index<'a>,
    m: Index<'a>,
    t: Index<'a>,
    choices: &'a BTreeMap<String, Side>,
    /// Parent subjects forced into conflicts (they would loop the hierarchy).
    force_parent: BTreeSet<u64>,
    conflicts: Vec<Conflict>,
}

impl Merger<'_> {
    fn name_of(&self, id: u64) -> String {
        self.m
            .get(&id)
            .or_else(|| self.t.get(&id))
            .or_else(|| self.b.get(&id))
            .map_or_else(|| format!("e{id}"), |e| e.name.clone())
    }

    /// Record a conflict; which side the result takes.
    fn conflict(
        &mut self,
        subject: Subject,
        label: String,
        base: Option<Value>,
        mine: Option<Value>,
        theirs: Option<Value>,
    ) -> Side {
        let choice = self.choices.get(&subject.key()).copied();
        self.conflicts.push(Conflict {
            subject,
            label,
            base,
            mine,
            theirs,
            choice,
        });
        choice.unwrap_or(Side::Theirs)
    }

    fn entity(&mut self, id: u64) -> Option<EntityDoc> {
        let (b, m, t) = (
            self.b.get(&id).copied(),
            self.m.get(&id).copied(),
            self.t.get(&id).copied(),
        );
        match (b, m, t) {
            (Some(be), Some(me), Some(te)) => Some(self.three(be, me, te)),
            (Some(be), Some(me), None) => {
                if same_entity(be, me) {
                    return None;
                }
                let label = format!("{}: deleted by the baseline, changed here", me.name);
                match self.conflict(
                    Subject::Exists(id),
                    label,
                    text("exists"),
                    text("kept, with my changes"),
                    None,
                ) {
                    Side::Mine => Some(me.clone()),
                    Side::Theirs => None,
                }
            }
            (Some(be), None, Some(te)) => {
                if same_entity(be, te) {
                    return None;
                }
                let label = format!("{}: deleted here, changed in the baseline", te.name);
                match self.conflict(
                    Subject::Exists(id),
                    label,
                    text("exists"),
                    None,
                    text("kept, with their changes"),
                ) {
                    Side::Mine => None,
                    Side::Theirs => Some(te.clone()),
                }
            }
            (Some(_), None, None) | (None, None, None) => None,
            (None, Some(me), None) => Some(me.clone()),
            (None, None, Some(te)) => Some(te.clone()),
            // Added on both sides with the same content (differing ones were re-keyed).
            (None, Some(me), Some(_)) => Some(me.clone()),
        }
    }

    fn three(&mut self, be: &EntityDoc, me: &EntityDoc, te: &EntityDoc) -> EntityDoc {
        let id = be.id;
        let name = match pick(&be.name, &me.name, &te.name) {
            Pick::Clean(n) => n,
            Pick::Conflict => {
                let label = format!("{} \u{203a} name", be.name);
                match self.conflict(
                    Subject::Name(id),
                    label,
                    text(&be.name),
                    text(&me.name),
                    text(&te.name),
                ) {
                    Side::Mine => me.name.clone(),
                    Side::Theirs => te.name.clone(),
                }
            }
        };
        let forced = self.force_parent.contains(&id) && me.parent != te.parent;
        let parent = match pick(&be.parent, &me.parent, &te.parent) {
            Pick::Clean(p) if !forced => p,
            _ => {
                let label = format!("{} \u{203a} parent", name);
                match self.conflict(
                    Subject::Parent(id),
                    label,
                    parent_value(be.parent),
                    parent_value(me.parent),
                    parent_value(te.parent),
                ) {
                    Side::Mine => me.parent,
                    Side::Theirs => te.parent,
                }
            }
        };
        let paths: BTreeSet<&String> = be
            .properties
            .keys()
            .chain(me.properties.keys())
            .chain(te.properties.keys())
            .collect();
        let mut properties = BTreeMap::new();
        for path in paths {
            let (bv, mv, tv) = (
                be.properties.get(path).cloned(),
                me.properties.get(path).cloned(),
                te.properties.get(path).cloned(),
            );
            let v = match pick(&bv, &mv, &tv) {
                Pick::Clean(v) => v,
                Pick::Conflict => {
                    let label = format!("{name} \u{203a} {path}");
                    match self.conflict(
                        Subject::Property(id, path.clone()),
                        label,
                        bv,
                        mv.clone(),
                        tv.clone(),
                    ) {
                        Side::Mine => mv,
                        Side::Theirs => tv,
                    }
                }
            };
            if let Some(v) = v {
                properties.insert(path.clone(), v);
            }
        }
        EntityDoc {
            id,
            name,
            parent,
            properties,
        }
    }
}

/// The ids of an entity loop in `doc`'s hierarchy, if there is one.
fn find_loop(entities: &BTreeMap<u64, EntityDoc>) -> Option<Vec<u64>> {
    let mut done: BTreeSet<u64> = BTreeSet::new();
    for start in entities.keys() {
        let mut path: Vec<u64> = Vec::new();
        let mut on_path: BTreeSet<u64> = BTreeSet::new();
        let mut cur = Some(*start);
        while let Some(c) = cur {
            if done.contains(&c) {
                break;
            }
            if !on_path.insert(c) {
                let from = path.iter().position(|x| *x == c).unwrap_or(0);
                return Some(path[from..].to_vec());
            }
            path.push(c);
            cur = entities.get(&c).and_then(|e| e.parent);
        }
        done.extend(path);
    }
    None
}

/// Merge `mine` and `theirs` over `base` (see the module docs), taking `choices` (by
/// [`Subject::key`]) for conflicts.
#[must_use]
pub fn merge3(
    base: &ProjectDoc,
    mine: &ProjectDoc,
    theirs: &ProjectDoc,
    choices: &BTreeMap<String, Side>,
) -> Merge {
    // Re-key mine's entities that collide with a different entity theirs added.
    let (bi, ti) = (index(base), index(theirs));
    let mut next = base
        .entities
        .iter()
        .chain(&mine.entities)
        .chain(&theirs.entities)
        .map(|e| e.id + 1)
        .max()
        .unwrap_or(0);
    let mut map: BTreeMap<u64, u64> = BTreeMap::new();
    for e in &mine.entities {
        if !bi.contains_key(&e.id) && ti.get(&e.id).is_some_and(|te| !same_entity(te, e)) {
            map.insert(e.id, next);
            next += 1;
        }
    }
    let mine: Cow<'_, ProjectDoc> = if map.is_empty() {
        Cow::Borrowed(mine)
    } else {
        Cow::Owned(remap_doc(mine, &map))
    };
    let mut force = BTreeSet::new();
    // Two passes at most: the second turns the parent changes of a loop into conflicts.
    for _ in 0..2 {
        let mut mg = Merger {
            b: bi.clone(),
            m: index(&mine),
            t: ti.clone(),
            choices,
            force_parent: force.clone(),
            conflicts: Vec::new(),
        };
        let mut settings = BTreeMap::new();
        let keys: BTreeSet<&String> = base
            .settings
            .keys()
            .chain(mine.settings.keys())
            .chain(theirs.settings.keys())
            .collect();
        for k in keys {
            let (bv, mv, tv) = (
                base.settings.get(k).cloned(),
                mine.settings.get(k).cloned(),
                theirs.settings.get(k).cloned(),
            );
            let v = match pick(&bv, &mv, &tv) {
                Pick::Clean(v) => v,
                Pick::Conflict => match mg.conflict(
                    Subject::Setting(k.clone()),
                    format!("setting {k}"),
                    bv,
                    mv.clone(),
                    tv.clone(),
                ) {
                    Side::Mine => mv,
                    Side::Theirs => tv,
                },
            };
            if let Some(v) = v {
                settings.insert(k.clone(), v);
            }
        }
        let ids: BTreeSet<u64> =
            mg.b.keys()
                .chain(mg.m.keys())
                .chain(mg.t.keys())
                .copied()
                .collect();
        let mut merged: BTreeMap<u64, EntityDoc> = BTreeMap::new();
        for id in ids {
            if let Some(e) = mg.entity(id) {
                merged.insert(id, e);
            }
        }
        // Orphans: a parent the other side deleted.
        let orphans: Vec<(u64, u64)> = merged
            .values()
            .filter_map(|e| {
                e.parent
                    .filter(|p| !merged.contains_key(p))
                    .map(|p| (e.id, p))
            })
            .collect();
        let mut drop_subtrees = Vec::new();
        for (id, gone) in orphans {
            let name = mg.name_of(id);
            let parent = mg.name_of(gone);
            let mine_has = mg.m.contains_key(&gone);
            let label = format!(
                "{name}: its parent \u{201c}{parent}\u{201d} was deleted {}",
                if mine_has { "in the baseline" } else { "here" }
            );
            match mg.conflict(
                Subject::Orphan(id),
                label,
                text("under its parent"),
                text("kept, as a root"),
                None,
            ) {
                Side::Mine => {
                    if let Some(e) = merged.get_mut(&id) {
                        e.parent = None;
                    }
                }
                Side::Theirs => drop_subtrees.push(id),
            }
        }
        for root in drop_subtrees {
            let mut stack = vec![root];
            while let Some(k) = stack.pop() {
                merged.remove(&k);
                stack.extend(
                    merged
                        .values()
                        .filter(|e| e.parent == Some(k))
                        .map(|e| e.id),
                );
            }
        }
        let lp = find_loop(&merged);
        let names = |l: &[u64]| {
            l.iter()
                .map(|k| format!("\u{201c}{}\u{201d}", mg.name_of(*k)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match lp {
            Some(l) if force.is_empty() => {
                // Turn the parent changes in the loop into conflicts and merge again.
                force = l.into_iter().collect();
                continue;
            }
            lp => {
                let hierarchy_loop = lp.map(|l| {
                    format!(
                        "the chosen parents put {} inside each other: choose the same side for their parents",
                        names(&l)
                    )
                });
                let mut conflicts = mg.conflicts;
                conflicts.sort_by(|a, b| a.subject.cmp(&b.subject));
                return Merge {
                    doc: ProjectDoc {
                        settings,
                        entities: merged.into_values().collect(),
                    },
                    conflicts,
                    rekeyed: map.into_iter().collect(),
                    hierarchy_loop,
                };
            }
        }
    }
    // Unreachable: the second pass always returns.
    Merge::default()
}

// ---- the key-preserving patch ------------------------------------------------------------

/// One step of a patch, carrying the value it replaces: its **precondition**. Applying it
/// to a project that no longer holds `before` is a conflict (`CMD-0008`), never an overwrite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PatchOp {
    /// Set (`Some`) or clear (`None`) a setting that holds `before`.
    Setting {
        key: String,
        before: Option<Value>,
        after: Option<Value>,
    },
    /// Create entity `id` (it must not exist) under `parent`.
    Create {
        id: u64,
        name: String,
        parent: Option<u64>,
    },
    /// Move entity `id` from `before` to `after`.
    Reparent {
        id: u64,
        before: Option<u64>,
        after: Option<u64>,
    },
    /// Rename entity `id` from `before`.
    Rename {
        id: u64,
        before: String,
        after: String,
    },
    /// Set or remove a property that holds `before`.
    Property {
        id: u64,
        path: String,
        before: Option<Value>,
        after: Option<Value>,
    },
    /// Remove entity `id` and what is left under it.
    Remove { id: u64 },
}

/// Depth of `id` in `ix` (roots 0; a loop stops counting).
fn depth(ix: &Index<'_>, id: u64) -> usize {
    let mut d = 0;
    let mut cur = ix.get(&id).and_then(|e| e.parent);
    while let Some(p) = cur {
        d += 1;
        if d > ix.len() {
            break;
        }
        cur = ix.get(&p).and_then(|e| e.parent);
    }
    d
}

/// The steps that turn `from` into `to`, keeping every key (see [`PatchOp`]): settings,
/// creations parents first, moves (through the root, so no intermediate state loops),
/// renames and properties, then removals.
#[must_use]
pub fn patch(from: &ProjectDoc, to: &ProjectDoc) -> Vec<PatchOp> {
    let mut out = Vec::new();
    let keys: BTreeSet<&String> = from.settings.keys().chain(to.settings.keys()).collect();
    for k in keys {
        let (b, a) = (from.settings.get(k), to.settings.get(k));
        if b != a {
            out.push(PatchOp::Setting {
                key: k.clone(),
                before: b.cloned(),
                after: a.cloned(),
            });
        }
    }
    let (fi, ti) = (index(from), index(to));
    let mut created: Vec<&EntityDoc> = to
        .entities
        .iter()
        .filter(|e| !fi.contains_key(&e.id))
        .collect();
    created.sort_by_key(|e| (depth(&ti, e.id), e.id));
    for e in &created {
        out.push(PatchOp::Create {
            id: e.id,
            name: e.name.clone(),
            parent: e.parent,
        });
    }
    let mut movers: Vec<(&EntityDoc, &EntityDoc)> = to
        .entities
        .iter()
        .filter_map(|t| fi.get(&t.id).map(|f| (*f, t)))
        .filter(|(f, t)| f.parent != t.parent)
        .collect();
    movers.sort_by_key(|(_, t)| (depth(&ti, t.id), t.id));
    for (f, _) in &movers {
        if f.parent.is_some() {
            out.push(PatchOp::Reparent {
                id: f.id,
                before: f.parent,
                after: None,
            });
        }
    }
    for (_, t) in &movers {
        if t.parent.is_some() {
            out.push(PatchOp::Reparent {
                id: t.id,
                before: None,
                after: t.parent,
            });
        }
    }
    for t in &to.entities {
        let f = fi.get(&t.id).copied();
        if let Some(f) = f
            && f.name != t.name
        {
            out.push(PatchOp::Rename {
                id: t.id,
                before: f.name.clone(),
                after: t.name.clone(),
            });
        }
        let empty = BTreeMap::new();
        let fp = f.map_or(&empty, |f| &f.properties);
        let paths: BTreeSet<&String> = fp.keys().chain(t.properties.keys()).collect();
        for p in paths {
            let (b, a) = (fp.get(p), t.properties.get(p));
            if b != a {
                out.push(PatchOp::Property {
                    id: t.id,
                    path: p.clone(),
                    before: b.cloned(),
                    after: a.cloned(),
                });
            }
        }
    }
    let removed: BTreeSet<u64> = from
        .entities
        .iter()
        .filter(|e| !ti.contains_key(&e.id))
        .map(|e| e.id)
        .collect();
    for id in &removed {
        let parent_removed = fi
            .get(id)
            .and_then(|e| e.parent)
            .is_some_and(|p| removed.contains(&p));
        if !parent_removed {
            out.push(PatchOp::Remove { id: *id });
        }
    }
    out
}

fn precondition(what: String) -> CmdError {
    CmdError::Conflict {
        detail: format!("precondition failed: {what}"),
    }
}

fn bad(why: impl Into<String>) -> CmdError {
    CmdError::BadArgs {
        target: SYNC_CMD.into(),
        why: why.into(),
    }
}

/// The arguments of [`SYNC_CMD`]: the patch and what it brings in (for the history label).
#[must_use]
pub fn sync_args(ops: &[PatchOp], rev: Option<&str>, label: &str) -> serde_json::Value {
    serde_json::json!({ "ops": ops, "rev": rev, "label": label })
}

/// Plan [`SYNC_CMD`]: apply each step after checking its precondition against the project as
/// the steps before it left it (see [`PatchOp`]).
pub fn plan_patch(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let ops: Vec<PatchOp> = serde_json::from_value(
        args.get("ops")
            .cloned()
            .ok_or_else(|| bad("\"ops\" is missing"))?,
    )
    .map_err(|e| bad(format!("\"ops\": {e}")))?;
    let key = EntityKey;
    for op in ops {
        match op {
            PatchOp::Setting {
                key: k,
                before,
                after,
            } => {
                let now = b.setting(&k);
                if now != before {
                    return Err(precondition(format!(
                        "setting {k} is {now:?}, the patch expected {before:?}"
                    )));
                }
                b.set_setting(&k, after)?;
            }
            PatchOp::Create { id, name, parent } => {
                b.spawn_at(key(id), &name, parent.map(key))?;
            }
            PatchOp::Reparent { id, before, after } => {
                let now = b.parent(key(id))?;
                if now != before.map(key) {
                    return Err(precondition(format!(
                        "e{id} is under {now:?}, the patch expected {before:?}"
                    )));
                }
                b.reparent(key(id), after.map(key))?;
            }
            PatchOp::Rename { id, before, after } => {
                let now = b.name(key(id))?;
                if now != before {
                    return Err(precondition(format!(
                        "e{id} is named {now:?}, the patch expected {before:?}"
                    )));
                }
                b.rename(key(id), &after)?;
            }
            PatchOp::Property {
                id,
                path,
                before,
                after,
            } => {
                let now = b.property(key(id), &path)?;
                if now != before {
                    return Err(precondition(format!(
                        "e{id} {path} is {now:?}, the patch expected {before:?}"
                    )));
                }
                match after {
                    Some(v) => b.set_property(key(id), &path, v)?,
                    None if now.is_some() => b.remove_property(key(id), &path)?,
                    None => {}
                }
            }
            PatchOp::Remove { id } => {
                if !b.exists(key(id)) {
                    return Err(precondition(format!("e{id} is already gone")));
                }
                b.despawn(key(id))?;
            }
        }
    }
    Ok(())
}

// ---- content paths (scoping and review rules) ---------------------------------------------

/// Where an entity sits, as a content path: `scene/<root>/<child>/…` by name (a `/` in a
/// name reads as `_`).
fn entity_path(ix: &Index<'_>, id: u64) -> String {
    let mut names = Vec::new();
    let mut cur = Some(id);
    let mut guard = 0;
    while let Some(c) = cur {
        guard += 1;
        if guard > ix.len() + 1 {
            break;
        }
        match ix.get(&c) {
            Some(e) => {
                names.push(e.name.replace('/', "_"));
                cur = e.parent;
            }
            None => break,
        }
    }
    names.reverse();
    format!("scene/{}", names.join("/"))
}

/// The content paths a change from `base` to `mine` touches (Ch.37 §37.6 per-path scoping,
/// O-16 per-path review rules): `settings/<key with / for .>` for a setting, `scene/…` for an
/// entity (where it is in `mine`, or was in `base` when removed). Sorted, deduplicated.
#[must_use]
pub fn changed_paths(base: &ProjectDoc, mine: &ProjectDoc) -> Vec<String> {
    let mut out = BTreeSet::new();
    let keys: BTreeSet<&String> = base.settings.keys().chain(mine.settings.keys()).collect();
    for k in keys {
        if base.settings.get(k) != mine.settings.get(k) {
            out.insert(format!("settings/{}", k.replace('.', "/")));
        }
    }
    let (bi, mi) = (index(base), index(mine));
    let ids: BTreeSet<u64> = bi.keys().chain(mi.keys()).copied().collect();
    for id in ids {
        match (bi.get(&id), mi.get(&id)) {
            (Some(b), Some(m)) if same_entity(b, m) => {}
            (Some(b), Some(m)) => {
                out.insert(entity_path(&mi, id));
                if b.parent != m.parent || b.name != m.name {
                    out.insert(entity_path(&bi, id));
                }
            }
            (None, Some(_)) => {
                out.insert(entity_path(&mi, id));
            }
            (Some(_), None) => {
                out.insert(entity_path(&bi, id));
            }
            (None, None) => {}
        }
    }
    out.into_iter().collect()
}

/// Does `path` match the glob `pattern`? Segments split on `/`; `**` matches any number of
/// segments (none included), `*` any run of characters within one segment.
#[must_use]
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.trim().trim_matches('/').split('/').collect();
    let s: Vec<&str> = path.trim_matches('/').split('/').collect();
    fn seg(p: &str, s: &str) -> bool {
        match p.split_once('*') {
            None => p == s,
            Some((head, rest)) => {
                let Some(tail) = s.strip_prefix(head) else {
                    return false;
                };
                (0..=tail.len())
                    .filter(|i| tail.is_char_boundary(*i))
                    .any(|i| seg(rest, &tail[i..]))
            }
        }
    }
    fn go(p: &[&str], s: &[&str]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(&"**") => (0..=s.len()).any(|i| go(&p[1..], &s[i..])),
            Some(h) => s.first().is_some_and(|x| seg(h, x)) && go(&p[1..], &s[1..]),
        }
    }
    go(&p, &s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(id: u64, name: &str, parent: Option<u64>, props: &[(&str, Value)]) -> EntityDoc {
        EntityDoc {
            id,
            name: name.into(),
            parent,
            properties: props
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
        }
    }

    fn doc(entities: Vec<EntityDoc>, settings: &[(&str, Value)]) -> ProjectDoc {
        ProjectDoc {
            settings: settings
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
            entities,
        }
    }

    fn f(x: f64) -> Value {
        Value::Float(x)
    }

    fn none() -> BTreeMap<String, Side> {
        BTreeMap::new()
    }

    #[test]
    fn different_properties_merge_cleanly_and_the_same_property_conflicts() {
        let base = doc(
            vec![ent(0, "Lamp", None, &[("pos", f(0.0)), ("colour", f(1.0))])],
            &[("grid", f(1.0))],
        );
        // I moved the lamp; they changed its colour: clean.
        let mine = doc(
            vec![ent(0, "Lamp", None, &[("pos", f(5.0)), ("colour", f(1.0))])],
            &[("grid", f(1.0))],
        );
        let theirs = doc(
            vec![ent(0, "Lamp", None, &[("pos", f(0.0)), ("colour", f(2.0))])],
            &[("grid", f(2.0))],
        );
        let m = merge3(&base, &mine, &theirs, &none());
        assert!(m.is_clean(), "{:?}", m.conflicts);
        let e = &m.doc.entities[0];
        assert_eq!(e.properties.get("pos"), Some(&f(5.0)));
        assert_eq!(e.properties.get("colour"), Some(&f(2.0)));
        assert_eq!(m.doc.settings.get("grid"), Some(&f(2.0)));
        // Both moved it: a conflict with all three values, never a silent pick.
        let theirs = doc(
            vec![ent(0, "Lamp", None, &[("pos", f(9.0)), ("colour", f(1.0))])],
            &[("grid", f(1.0))],
        );
        let m = merge3(&base, &mine, &theirs, &none());
        assert_eq!(m.unresolved(), 1);
        assert!(!m.is_clean());
        let c = &m.conflicts[0];
        assert_eq!(c.subject, Subject::Property(0, "pos".into()));
        assert_eq!(
            (c.base.clone(), c.mine.clone(), c.theirs.clone()),
            (Some(f(0.0)), Some(f(5.0)), Some(f(9.0)))
        );
        // Choosing a side resolves it.
        let choices: BTreeMap<String, Side> = [(c.subject.key(), Side::Mine)].into_iter().collect();
        let m = merge3(&base, &mine, &theirs, &choices);
        assert!(m.is_clean());
        assert_eq!(m.doc.entities[0].properties.get("pos"), Some(&f(5.0)));
    }

    #[test]
    fn same_new_key_on_both_sides_rekeys_mine_and_keeps_both() {
        let base = doc(vec![ent(0, "Root", None, &[])], &[]);
        let mine = doc(
            vec![
                ent(0, "Root", None, &[("target", Value::Entity(EntityKey(1)))]),
                ent(1, "My tree", Some(0), &[]),
                ent(2, "My leaf", Some(1), &[]),
            ],
            &[],
        );
        let theirs = doc(
            vec![
                ent(0, "Root", None, &[]),
                ent(1, "Their rock", Some(0), &[]),
            ],
            &[],
        );
        let m = merge3(&base, &mine, &theirs, &none());
        assert!(m.is_clean(), "{:?}", m.conflicts);
        assert_eq!(m.rekeyed, vec![(1, 3)]);
        let names: BTreeMap<u64, (String, Option<u64>)> = m
            .doc
            .entities
            .iter()
            .map(|e| (e.id, (e.name.clone(), e.parent)))
            .collect();
        assert_eq!(names[&1], ("Their rock".into(), Some(0)));
        assert_eq!(names[&3], ("My tree".into(), Some(0)));
        assert_eq!(names[&2], ("My leaf".into(), Some(3)), "children follow");
        assert_eq!(
            m.doc.entities[0].properties.get("target"),
            Some(&Value::Entity(EntityKey(3))),
            "references follow"
        );
    }

    #[test]
    fn delete_versus_edit_and_orphans_are_conflicts() {
        let base = doc(
            vec![ent(0, "Level", None, &[]), ent(1, "Crate", Some(0), &[])],
            &[],
        );
        // They deleted the level (and its crate); I edited the crate and added a child.
        let mine = doc(
            vec![
                ent(0, "Level", None, &[]),
                ent(1, "Crate", Some(0), &[("mass", f(3.0))]),
                ent(5, "Label", Some(1), &[]),
            ],
            &[],
        );
        let theirs = doc(vec![], &[]);
        let m = merge3(&base, &mine, &theirs, &none());
        let subjects: Vec<Subject> = m.conflicts.iter().map(|c| c.subject.clone()).collect();
        assert!(subjects.contains(&Subject::Exists(1)), "{subjects:?}");
        // Unresolved: theirs (deleted), so the label is an orphan too.
        assert!(subjects.contains(&Subject::Orphan(5)), "{subjects:?}");
        // Keep my crate: then only the level is gone and the crate is an orphan to decide.
        let choices: BTreeMap<String, Side> = [
            (Subject::Exists(1).key(), Side::Mine),
            (Subject::Orphan(1).key(), Side::Mine),
        ]
        .into_iter()
        .collect();
        let m = merge3(&base, &mine, &theirs, &choices);
        assert!(m.is_clean(), "{:?}", m.conflicts);
        let ids: Vec<(u64, Option<u64>)> =
            m.doc.entities.iter().map(|e| (e.id, e.parent)).collect();
        assert_eq!(ids, vec![(1, None), (5, Some(1))]);
    }

    #[test]
    fn a_hierarchy_loop_becomes_parent_conflicts() {
        let base = doc(vec![ent(0, "A", None, &[]), ent(1, "B", None, &[])], &[]);
        let mine = doc(vec![ent(0, "A", Some(1), &[]), ent(1, "B", None, &[])], &[]);
        let theirs = doc(vec![ent(0, "A", None, &[]), ent(1, "B", Some(0), &[])], &[]);
        let m = merge3(&base, &mine, &theirs, &none());
        let parents: Vec<&Subject> = m
            .conflicts
            .iter()
            .map(|c| &c.subject)
            .filter(|s| matches!(s, Subject::Parent(_)))
            .collect();
        assert_eq!(parents.len(), 2, "{:?}", m.conflicts);
        // Mixed sides loop again: reported, not applied.
        let mixed: BTreeMap<String, Side> = [
            (Subject::Parent(0).key(), Side::Mine),
            (Subject::Parent(1).key(), Side::Theirs),
        ]
        .into_iter()
        .collect();
        let m = merge3(&base, &mine, &theirs, &mixed);
        assert!(m.hierarchy_loop.is_some());
        assert!(!m.is_clean());
        let same: BTreeMap<String, Side> = [
            (Subject::Parent(0).key(), Side::Mine),
            (Subject::Parent(1).key(), Side::Mine),
        ]
        .into_iter()
        .collect();
        assert!(merge3(&base, &mine, &theirs, &same).is_clean());
    }

    #[test]
    fn globs_match_by_segment() {
        assert!(glob_match("scene/Levels/**", "scene/Levels/One/Tree"));
        assert!(glob_match("scene/Levels/**", "scene/Levels"));
        assert!(!glob_match("scene/Levels/**", "scene/LevelsX/One"));
        assert!(glob_match("settings/editor/*", "settings/editor/grid_size"));
        assert!(!glob_match("settings/editor/*", "settings/editor/a/b"));
        assert!(glob_match("scene/*/Tree", "scene/Forest/Tree"));
        assert!(glob_match("scene/Tree*", "scene/Tree 2"));
        assert!(glob_match("**", "anything/at/all"));
    }

    #[test]
    fn changed_paths_name_settings_and_entities() {
        let base = doc(
            vec![ent(0, "Levels", None, &[]), ent(1, "One", Some(0), &[])],
            &[("editor.grid", f(1.0))],
        );
        let mine = doc(
            vec![
                ent(0, "Levels", None, &[]),
                ent(1, "One", Some(0), &[("x", f(1.0))]),
            ],
            &[("editor.grid", f(2.0))],
        );
        assert_eq!(
            changed_paths(&base, &mine),
            vec![
                "scene/Levels/One".to_string(),
                "settings/editor/grid".into()
            ]
        );
    }
}

#[cfg(test)]
mod patch_tests {
    use super::*;
    use forge_cmd::{Bus, CommandPolicy, CommandSink, EditorCommand, Issuer};

    fn bus() -> Bus {
        let mut b = Bus::new();
        b.register_handler(SYNC_CMD, CommandPolicy::ORDINARY, plan_patch)
            .unwrap_or_else(|e| panic!("{e}"));
        b
    }

    fn apply(b: &mut Bus, ops: &[PatchOp]) -> Result<(), String> {
        let e = b.envelope(
            Issuer::Test,
            EditorCommand::Invoke {
                target: SYNC_CMD.into(),
                args: sync_args(ops, None, "test").to_string(),
            },
        );
        b.apply(e).map(drop).map_err(|r| r.error.to_string())
    }

    fn sorted(mut d: ProjectDoc) -> ProjectDoc {
        d.entities.sort_by_key(|e| e.id);
        d
    }

    fn ent(id: u64, name: &str, parent: Option<u64>, props: &[(&str, Value)]) -> EntityDoc {
        EntityDoc {
            id,
            name: name.into(),
            parent,
            properties: props
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn a_patch_keeps_keys_and_reaches_the_target_exactly() {
        let from = ProjectDoc {
            settings: [("a".to_string(), Value::Int(1))].into_iter().collect(),
            entities: vec![
                ent(3, "World", None, &[]),
                ent(7, "Tree", Some(3), &[("h", Value::Float(2.0))]),
                ent(9, "Rock", Some(7), &[]),
                ent(12, "Old", Some(3), &[("x", Value::Int(1))]),
                ent(13, "Old child", Some(12), &[]),
            ],
        };
        let to = ProjectDoc {
            settings: [("b".to_string(), Value::Bool(true))].into_iter().collect(),
            entities: vec![
                ent(3, "World", None, &[("sun", Value::Entity(EntityKey(20)))]),
                ent(7, "Tall tree", Some(20), &[("h", Value::Float(5.0))]),
                // The rock moves out from under the tree, the tree under a new group.
                ent(9, "Rock", None, &[]),
                ent(20, "Group", Some(3), &[]),
                ent(21, "Leaf", Some(7), &[]),
            ],
        };
        let mut b = bus();
        apply(&mut b, &patch(&ProjectDoc::default(), &from)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            sorted(ProjectDoc::from_project(b.project())),
            sorted(from.clone())
        );
        apply(&mut b, &patch(&from, &to)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(sorted(ProjectDoc::from_project(b.project())), sorted(to));
    }

    #[test]
    fn a_stale_patch_fails_its_precondition_and_applies_nothing() {
        let from = ProjectDoc {
            settings: Default::default(),
            entities: vec![ent(0, "Lamp", None, &[("c", Value::Float(1.0))])],
        };
        let mut to = from.clone();
        to.entities[0]
            .properties
            .insert("c".into(), Value::Float(2.0));
        let mut b = bus();
        apply(&mut b, &patch(&ProjectDoc::default(), &from)).unwrap_or_else(|e| panic!("{e}"));
        let ops = patch(&from, &to);
        // Someone changed the lamp after the patch was computed.
        let e = b.envelope(
            Issuer::Test,
            EditorCommand::SetProperty {
                entity: EntityKey(0),
                path: "c".into(),
                value: Value::Float(7.0),
            },
        );
        b.apply(e).unwrap_or_else(|r| panic!("{r:?}"));
        let err = apply(&mut b, &ops).expect_err("the precondition fails");
        assert!(
            err.contains("CMD-0008") && err.contains("precondition"),
            "{err}"
        );
        assert_eq!(
            b.project()
                .entity(EntityKey(0))
                .and_then(|e| e.property("c")),
            Some(&Value::Float(7.0)),
            "nothing of the patch applied"
        );
    }
}
