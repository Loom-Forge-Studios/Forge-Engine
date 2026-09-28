//! Preset promotion (Ch.31 §31.4) — the rules, shared by the dialogs (which show what would
//! be lost) and the command's planner (which refuses a lossy change unless it says
//! `"accept_loss": true`).
//!
//! | From → To | Cost |
//! |---|---|
//! | 2D → 3D | additive: the 2D content keeps working as a 2D layer (`render.layers_2d`) |
//! | 3D → 2D | narrowing: 3D-only data (meshes, lights, and what a plugin's rule adds) is removed and depth and tilt are flattened — lossy when any exists, and the UI says so |
//! | a family's base preset → a plugin's preset, and back | what the plugin's [`PromotionRule`] says (a settings change when it says nothing) |
//! | across families | the arrows through the families' base presets, their losses together |
//!
//! **A preset is named by its directory** (`project.preset`: `2d`, `3d`, or the directory of
//! a preset a plugin adds, `crate::presets::preset_dir_of_key`). A plugin's preset belongs
//! to a family (2D or 3D); promoting to or from it goes through that family's base preset,
//! and the plugin's [`PromotionRule`] (the `PromotionRule` extension point,
//! [`PromotionRulePoint`]) says what each step costs and changes. A rule may also add to
//! what every narrowing to 2D removes, and to what the rules track per entity
//! ([`Tracking`]).
//!
//! **A preset sets defaults (I15).** Promotion swaps the old preset's default settings for
//! the new one's only where the project still has the old default — a value the user set
//! is theirs and stays. Nothing here gates a panel or an API.
//!
//! Lossiness is computed from the project, not from the arrow: a narrowing with nothing to
//! lose says so ("nothing in this project is lost") and needs no confirmation; one with
//! something to lose names it — how many, and which — and needs one.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use forge_cmd::{CmdError, DiffBuilder, EditorCommand, EntityKey, Project, Value};

use super::{PRESET_SETTING, PROMOTE_CMD, preset_defaults};
use crate::mirror::{MirrorChange, ProjectMirror};
use crate::presets::Family;
use crate::viewport::scene::{P_FRAME, P_LOCAL, P_PITCH, P_ROLL};

/// Set when 2D content lives on as a 2D layer of a 3D project.
pub const LAYERS_2D_SETTING: &str = "render.layers_2d";
/// Property prefixes only a 3D world draws or simulates (a rule adds its own,
/// [`Tracking::only3d`]).
pub const ONLY_3D: &[&str] = &["mesh.", "light."];
/// Entities named in a loss line, at most.
pub const NAMES_SHOWN: usize = 5;

impl Family {
    /// The base preset's directory.
    pub fn dir(self) -> &'static str {
        crate::presets::family_base_dir(self)
    }
    pub fn label(self) -> &'static str {
        match self {
            Family::TwoD => "2D",
            Family::ThreeD => "3D",
        }
    }
    pub fn from_dir(d: &str) -> Option<Family> {
        Family::ALL.into_iter().find(|f| f.dir() == d)
    }
    pub const ALL: [Family; 2] = [Family::TwoD, Family::ThreeD];
}

// ---- what the rules see ------------------------------------------------------------------

/// What a plugin's rules add to what every [`View`] keeps (see [`PromotionRule::tracking`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tracking {
    /// Property prefixes only a 3D world draws (`body.`): a narrowing to 2D removes them,
    /// like [`ONLY_3D`]'s.
    pub only3d: Vec<String>,
    /// Properties whose values the facts keep ([`Facts::tracked`]).
    pub tracked: Vec<String>,
    /// Property prefixes whose properties the command's view keeps whole
    /// ([`Facts::records`]); a panel's [`MirrorView`] does not (it plans to show).
    pub records: Vec<String>,
    /// Settings the rules read besides the presets' defaults.
    pub settings: Vec<String>,
}

impl Tracking {
    /// `self` with `other`'s entries added.
    pub fn merge(&mut self, other: &Tracking) {
        for (mine, theirs) in [
            (&mut self.only3d, &other.only3d),
            (&mut self.tracked, &other.tracked),
            (&mut self.records, &other.records),
            (&mut self.settings, &other.settings),
        ] {
            for t in theirs {
                if !mine.contains(t) {
                    mine.push(t.clone());
                }
            }
        }
    }
    fn is_only3d(&self, p: &str) -> bool {
        ONLY_3D.iter().any(|x| p.starts_with(x)) || self.only3d.iter().any(|x| p.starts_with(x))
    }
    fn is_record(&self, p: &str) -> bool {
        self.records.iter().any(|x| p.starts_with(x.as_str()))
    }
    /// Whether a property can change an entity's [`Facts`].
    fn reads(&self, p: &str) -> bool {
        matches!(p, P_FRAME | P_LOCAL | P_PITCH | P_ROLL)
            || self.is_only3d(p)
            || self.tracked.iter().any(|t| t == p)
    }
}

/// What one entity holds that a promotion cares about.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub key: EntityKey,
    pub name: String,
    /// `transform.position.frame` (0: the world frame).
    pub frame: i64,
    /// The local offset (`transform.position.local`), read only to flatten depth; not a
    /// position API (I1: positions cross public signatures as `FramePos`).
    pub(crate) local: Option<[f64; 3]>,
    pub pitch: f64,
    pub roll: f64,
    /// Its properties under 3D-only prefixes ([`ONLY_3D`] and the rules').
    pub only3d: BTreeSet<String>,
    /// The values of the properties the rules track ([`Tracking::tracked`]).
    pub tracked: BTreeMap<String, Value>,
    /// Its properties under the rules' record prefixes ([`Tracking::records`]). Only the
    /// command's view of the project ([`View::of_project_in`]) keeps them.
    pub records: Vec<(String, Value)>,
}

/// Whether `set` holds a string starting with `prefix`.
pub fn has_prefix(set: &BTreeSet<String>, prefix: &str) -> bool {
    set.range::<str, _>((
        std::ops::Bound::Included(prefix),
        std::ops::Bound::Unbounded,
    ))
    .next()
    .is_some_and(|q| q.starts_with(prefix))
}

impl Facts {
    fn interesting(&self) -> bool {
        self.frame != 0
            || self.depth()
            || self.pitch != 0.0
            || self.roll != 0.0
            || !self.only3d.is_empty()
            || !self.tracked.is_empty()
    }
    pub(crate) fn depth(&self) -> bool {
        self.local.is_some_and(|l| l[2] != 0.0)
    }
    /// Whether it has a property under `prefix` among its 3D-only ones.
    #[must_use]
    pub fn has_only3d(&self, prefix: &str) -> bool {
        has_prefix(&self.only3d, prefix)
    }
    /// A tracked integer (0 when absent or not an integer).
    #[must_use]
    pub fn tracked_int(&self, key: &str) -> i64 {
        match self.tracked.get(key) {
            Some(Value::Int(n)) => *n,
            _ => 0,
        }
    }
    /// What a dialog shows of this entity besides its name: the flags the loss rules test,
    /// which 3D-only kinds it has and its tracked values — not the exact offsets or
    /// property names, which only the applied edits use.
    fn shown(&self) -> ([bool; 4], Vec<&str>, &BTreeMap<String, Value>) {
        let mut kinds: Vec<&str> = self
            .only3d
            .iter()
            .filter_map(|p| p.split('.').next())
            .collect();
        kinds.dedup();
        (
            [
                self.frame != 0,
                self.depth(),
                self.pitch != 0.0,
                self.roll != 0.0,
            ],
            kinds,
            &self.tracked,
        )
    }
    /// Take one property's new value (`None`: removed) into these facts.
    fn take(&mut self, p: &str, v: Option<&Value>, t: &Tracking) {
        match p {
            P_FRAME => {
                self.frame = match v {
                    Some(Value::Int(i)) => *i,
                    _ => 0,
                }
            }
            P_LOCAL => {
                self.local = match v {
                    Some(Value::Vec3(l)) => Some(*l),
                    _ => None,
                }
            }
            P_PITCH | P_ROLL => {
                let x = match v {
                    Some(Value::Float(x)) => *x,
                    _ => 0.0,
                };
                if p == P_PITCH {
                    self.pitch = x;
                } else {
                    self.roll = x;
                }
            }
            _ => {}
        }
        if t.tracked.iter().any(|k| k == p) {
            match v {
                Some(v) => {
                    self.tracked.insert(p.to_string(), v.clone());
                }
                None => {
                    self.tracked.remove(p);
                }
            }
        }
        if t.is_only3d(p) {
            if v.is_some() {
                if !self.only3d.contains(p) {
                    self.only3d.insert(p.to_string());
                }
            } else {
                self.only3d.remove(p);
            }
        }
    }
}

fn fact<'a>(
    key: EntityKey,
    name: &str,
    props: impl Iterator<Item = (&'a str, &'a Value)>,
    with_records: bool,
    t: &Tracking,
) -> Option<Facts> {
    let mut f = Facts {
        key,
        name: name.to_string(),
        frame: 0,
        local: None,
        pitch: 0.0,
        roll: 0.0,
        only3d: BTreeSet::new(),
        tracked: BTreeMap::new(),
        records: Vec::new(),
    };
    for (p, v) in props {
        if with_records && t.is_record(p) {
            f.records.push((p.to_string(), v.clone()));
        }
        if t.reads(p) {
            f.take(p, Some(v), t);
        }
    }
    f.interesting().then_some(f)
}

/// The project as the rules see it: the settings they read ([`rule_keys`] and the rules'
/// own) and the entities that matter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// Only the settings the rules read: a promotion reads nothing else, so a view never
    /// copies the rest (a tile map's chunks, say).
    pub settings: BTreeMap<String, Value>,
    pub entities: BTreeMap<EntityKey, Facts>,
}

/// The settings the rules read besides the presets' defaults.
const RULE_SETTINGS: [&str; 2] = [PRESET_SETTING, LAYERS_2D_SETTING];

type Defaults = BTreeMap<&'static str, BTreeMap<String, Value>>;

fn builtin_all() -> &'static Defaults {
    static ALL: std::sync::OnceLock<Defaults> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        Family::ALL
            .iter()
            .map(|f| (f.dir(), preset_defaults(f.dir()).unwrap_or_default()))
            .collect()
    })
}

/// Every setting the promotion rules read with the built-in presets: the preset, the
/// 2D-layer flag and every key a built-in preset gives a default.
pub fn rule_keys() -> &'static BTreeSet<String> {
    static KEYS: std::sync::OnceLock<BTreeSet<String>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        RULE_SETTINGS
            .iter()
            .map(|k| (*k).to_string())
            .chain(builtin_all().values().flat_map(|d| d.keys().cloned()))
            .collect()
    })
}

/// The preset directory a `project.preset` value names (none: `3d`, the default).
pub fn preset_dir_of(preset: Option<&Value>) -> &str {
    match preset {
        Some(Value::Text(t)) if !t.is_empty() => t,
        _ => Family::ThreeD.dir(),
    }
}

/// The base family a `project.preset` value names without any rule at hand: 2D for `2d`,
/// else 3D (a plugin's preset of the 2D family says so through its rule:
/// [`PromotionRules::family_of_dir`]).
pub fn family_of(preset: Option<&Value>) -> Family {
    match preset_dir_of(preset) {
        "2d" => Family::TwoD,
        _ => Family::ThreeD,
    }
}

impl View {
    /// The command's view with no rules: every entity's facts, and the rule settings read
    /// by exact key.
    pub fn of_project(p: &Project) -> View {
        Self::of_project_in(p, std::iter::empty(), &Tracking::default())
    }
    /// [`View::of_project`] also reading `extra` settings: the keys a project's own presets
    /// give defaults that no built-in preset does (WP-16), so a user's value for one of them
    /// is seen — and kept — like any other.
    pub fn of_project_with<'a>(p: &Project, extra: impl IntoIterator<Item = &'a String>) -> View {
        Self::of_project_in(p, extra, &Tracking::default())
    }
    /// The command's view with the rules' tracking: every entity's facts with their records,
    /// the rule settings, the rules' settings and `extra`.
    pub fn of_project_in<'a>(
        p: &Project,
        extra: impl IntoIterator<Item = &'a String>,
        t: &Tracking,
    ) -> View {
        let mut v = View {
            settings: rule_keys()
                .iter()
                .chain(t.settings.iter())
                .filter_map(|k| p.setting(k).map(|v| (k.clone(), v.clone())))
                .collect(),
            entities: p
                .entities()
                .filter_map(|(k, e)| fact(k, e.name(), e.properties(), true, t).map(|f| (k, f)))
                .collect(),
        };
        for k in extra {
            if !v.settings.contains_key(k)
                && let Some(val) = p.setting(k)
            {
                v.settings.insert(k.clone(), val.clone());
            }
        }
        v
    }
    /// The project's preset directory (`project.preset`; a project without one is 3D).
    pub fn preset_dir(&self) -> &str {
        preset_dir_of(self.settings.get(PRESET_SETTING))
    }
    /// The project's base family without rules ([`family_of`]).
    pub fn family(&self) -> Family {
        family_of(self.settings.get(PRESET_SETTING))
    }
    /// The sum of a tracked integer over every entity.
    #[must_use]
    pub fn sum_tracked(&self, key: &str) -> i64 {
        self.entities.values().map(|f| f.tracked_int(key)).sum()
    }
}

/// What bringing a [`MirrorView`] up to date did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Refresh {
    /// Something a dialog shows may have changed: plan again.
    pub changed: bool,
    /// Settings plus entity properties read: the work done (a panel's probe adds it).
    pub work: usize,
}

/// A panel's [`View`] of the mirror, kept up to date **incrementally**: it reads the rule
/// settings by exact key and applies the mirror's change logs, so a change to a setting
/// the rules do not read (a painted tile chunk) costs nothing, and a changed property
/// costs that property. It walks the whole mirror only the first time and after the
/// mirror dropped changes it had not seen (a resync, or a reader too far behind).
///
/// It plans to *show* (the losses and the cost): it keeps no records, and the command plans
/// again from the project when it applies.
#[derive(Clone, Debug, Default)]
pub struct MirrorView {
    view: View,
    tracking: Tracking,
    seq: Option<u64>,
    setting_seq: Option<u64>,
}

impl MirrorView {
    /// A view that also keeps what `tracking` names (the rules' [`PromotionRules::tracking`]).
    #[must_use]
    pub fn tracking(tracking: Tracking) -> Self {
        Self {
            tracking,
            ..Self::default()
        }
    }
    pub fn view(&self) -> &View {
        &self.view
    }
    fn reads_setting(&self, k: &str) -> bool {
        rule_keys().contains(k) || self.tracking.settings.iter().any(|s| s == k)
    }
    /// Bring the view up to date with `m`.
    pub fn update(&mut self, m: &ProjectMirror) -> Refresh {
        let mut r = Refresh::default();
        let sseq = m.setting_change_seq();
        if self.setting_seq != Some(sseq) {
            match self.setting_seq.and_then(|s| m.setting_changes_since(s)) {
                Some(keys) => {
                    for k in keys {
                        if self.reads_setting(k) {
                            self.read_setting(m, k, &mut r);
                        }
                    }
                }
                None => {
                    self.view.settings.clear();
                    let keys: Vec<String> = rule_keys()
                        .iter()
                        .chain(self.tracking.settings.iter())
                        .cloned()
                        .collect();
                    for k in keys {
                        self.read_setting(m, &k, &mut r);
                    }
                    r.changed = true;
                }
            }
            self.setting_seq = Some(sseq);
        }
        let seq = m.change_seq();
        if self.seq != Some(seq) {
            match self.seq.and_then(|s| m.changes_since(s)) {
                Some(changes) => {
                    for c in changes {
                        self.apply(m, c, &mut r);
                    }
                }
                None => {
                    self.view.entities.clear();
                    for (k, _) in m.entities() {
                        self.read_entity(m, *k, &mut r);
                    }
                    r.changed = true;
                }
            }
            self.seq = Some(seq);
        }
        r
    }
    fn read_setting(&mut self, m: &ProjectMirror, k: &str, r: &mut Refresh) {
        r.work += 1;
        let now = m.setting(k);
        if self.view.settings.get(k) != now {
            match now {
                Some(v) => self.view.settings.insert(k.to_string(), v.clone()),
                None => self.view.settings.remove(k),
            };
            r.changed = true;
        }
    }
    fn place(&mut self, k: EntityKey, f: Option<Facts>, r: &mut Refresh) {
        let before = self.view.entities.get(&k);
        if before.map(|b| (&b.name, b.shown())) != f.as_ref().map(|a| (&a.name, a.shown())) {
            r.changed = true;
        }
        match f {
            Some(f) => {
                self.view.entities.insert(k, f);
            }
            None => {
                self.view.entities.remove(&k);
            }
        }
    }
    fn read_entity(&mut self, m: &ProjectMirror, k: EntityKey, r: &mut Refresh) {
        let f = m.entity(k).and_then(|e| {
            r.work += e.properties.len().max(1);
            fact(
                k,
                &e.name,
                e.properties.iter().map(|(p, v)| (p.as_str(), v)),
                false,
                &self.tracking,
            )
        });
        self.place(k, f, r);
    }
    fn apply(&mut self, m: &ProjectMirror, c: &MirrorChange, r: &mut Refresh) {
        match c {
            MirrorChange::Created(k) => self.read_entity(m, *k, r),
            MirrorChange::Removed { entity, .. } => {
                r.work += 1;
                self.place(*entity, None, r);
            }
            MirrorChange::Renamed(k) => {
                r.work += 1;
                if let (Some(f), Some(e)) = (self.view.entities.get_mut(k), m.entity(*k))
                    && f.name != e.name
                {
                    f.name.clone_from(&e.name);
                    r.changed = true;
                }
            }
            MirrorChange::Reparented { .. } => {}
            MirrorChange::Property(k, p) => {
                if !self.tracking.reads(p) {
                    return;
                }
                r.work += 1;
                let v = m.property(*k, p);
                let Some(f) = self.view.entities.get_mut(k) else {
                    // Not interesting until now: this property may make it so.
                    if v.is_some() {
                        self.read_entity(m, *k, r);
                    }
                    return;
                };
                let before = f.shown();
                let before = (before.0, before.1.join("."), before.2.clone());
                f.take(p, v, &self.tracking);
                let after = f.shown();
                if (after.0, after.1.join("."), after.2.clone()) != before {
                    r.changed = true;
                }
                if !f.interesting() {
                    r.changed = true;
                    self.view.entities.remove(k);
                }
            }
        }
    }
}

// ---- what a promotion costs ----------------------------------------------------------------

/// One thing a change would lose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loss {
    /// What happens to several entities, in words ("lose their 3D-only data (mesh,
    /// light)"), or the whole loss when [`Loss::count`] is 0.
    pub what: String,
    /// What happens to one entity, in words ("loses its 3D-only data (mesh)"), so the
    /// warning a user reads stays grammatical for a count of 1.
    pub what_one: String,
    /// How many entities (0: a project-wide loss).
    pub count: usize,
    /// Some of their names.
    pub names: Vec<String>,
}

impl Loss {
    /// "3 entities lose their 3D-only data (mesh, light): Ground, Sun, Crate", or
    /// "1 entity loses its 3D-only data (mesh): Crate".
    pub fn line(&self) -> String {
        if self.count == 0 {
            return self.what.clone();
        }
        let names = self.names.join(", ");
        let more = if self.count > self.names.len() {
            forge_ui::trf!(", and {n} more", n = self.count - self.names.len())
        } else {
            String::new()
        };
        if self.count == 1 {
            forge_ui::trf!(
                "1 entity {what}: {names}{more}",
                what = self.what_one,
                names,
                more
            )
        } else {
            forge_ui::trf!(
                "{n} entities {what}: {names}{more}",
                n = self.count,
                what = self.what,
                names,
                more
            )
        }
    }
    /// A project-wide loss (no entity count), `what` in words.
    #[must_use]
    pub fn project(what: &str) -> Loss {
        Loss {
            what: what.to_string(),
            what_one: what.to_string(),
            count: 0,
            names: Vec::new(),
        }
    }
}

/// A loss to the entities in `list`, worded for one entity (`one`) and for several
/// (`many`).
pub fn loss(one: &str, many: &str, list: &[&Facts]) -> Loss {
    Loss {
        what: many.to_string(),
        what_one: one.to_string(),
        count: list.len(),
        names: list
            .iter()
            .take(NAMES_SHOWN)
            .map(|f| format!("\u{201c}{}\u{201d}", f.name))
            .collect(),
    }
}

/// How an arrow costs (Ch.31 §31.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cost {
    NonDestructive,
    Additive,
    Narrowing,
}

/// Edits to one entity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityEdit {
    pub remove: BTreeSet<String>,
    pub set: BTreeMap<String, Value>,
}

/// A promotion, planned (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct Promotion {
    /// The preset directories promoted from and to.
    pub from: String,
    pub to: String,
    /// Their labels ("3D", a plugin preset's label).
    pub from_label: String,
    pub to_label: String,
    /// The arrows walked (one, or several through the families' base presets).
    pub steps: Vec<(String, String)>,
    /// The costliest step's cost.
    pub cost: Cost,
    /// The Ch.31 §31.4 rows, in words.
    pub describe: Vec<String>,
    pub losses: Vec<Loss>,
    /// Settings to write (`None`: clear).
    pub settings: BTreeMap<String, Option<Value>>,
    pub edits: BTreeMap<EntityKey, EntityEdit>,
}

impl Promotion {
    pub fn lossy(&self) -> bool {
        !self.losses.is_empty()
    }
    /// The confirmation text: what is lost, one line each.
    pub fn loss_text(&self) -> String {
        self.losses
            .iter()
            .map(Loss::line)
            .collect::<Vec<_>>()
            .join("; ")
    }
}

// ---- plugin rules --------------------------------------------------------------------------

/// A preset a plugin's rule adds (see [`PromotionRule::preset`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulePreset {
    /// Its directory (the `project.preset` value; the plugin registers the preset itself
    /// under `forge.preset.<dir>`).
    pub dir: String,
    /// Shown in the preset switcher.
    pub label: String,
    /// The family it belongs to: promotions to and from it pass through the family's base
    /// preset.
    pub family: Family,
    /// The cost row of entering it from the base preset.
    pub enter: (Cost, String),
    /// The cost row of leaving it for the base preset.
    pub leave: (Cost, String),
}

/// One promotion step as a rule changes it: the facts and settings as the plan has them so
/// far, and the promotion being planned.
pub struct StepCx<'a> {
    pub facts: &'a mut BTreeMap<EntityKey, Facts>,
    pub settings: &'a mut BTreeMap<String, Value>,
    pub out: &'a mut Promotion,
}

impl StepCx<'_> {
    /// Write setting `k` (`None`: clear) as part of the promotion.
    pub fn set(&mut self, k: &str, val: Option<Value>) {
        match &val {
            Some(x) => self.settings.insert(k.to_string(), x.clone()),
            None => self.settings.remove(k),
        };
        self.out.settings.insert(k.to_string(), val);
    }
    /// The edits to entity `k`.
    pub fn edit(&mut self, k: EntityKey) -> &mut EntityEdit {
        self.out.edits.entry(k).or_default()
    }
}

/// What a plugin adds to promotion (the `PromotionRule` extension point): a preset of its
/// own and its arrows ([`PromotionRule::preset`], [`PromotionRule::enter`],
/// [`PromotionRule::leave`]), and what its content loses in a narrowing to 2D
/// ([`PromotionRule::tracking`], [`PromotionRule::to_2d`]).
pub trait PromotionRule: Send + Sync {
    /// What the rule tracks (none by default).
    fn tracking(&self) -> Tracking {
        Tracking::default()
    }
    /// The preset this rule adds, if any.
    fn preset(&self) -> Option<RulePreset> {
        None
    }
    /// Entering the rule's preset from its family's base preset (after its defaults are
    /// applied). Nothing more by default.
    fn enter(&self, _cx: &mut StepCx<'_>) {}
    /// Leaving the rule's preset for its family's base preset. Nothing more by default.
    fn leave(&self, _cx: &mut StepCx<'_>) {}
    /// A narrowing 3D → 2D (whatever preset asked for it): what else is lost or changed,
    /// beyond the 3D-only properties the rule tracks (which the narrowing removes).
    fn to_2d(&self, _cx: &mut StepCx<'_>) {}
}

/// The `PromotionRule` extension point (`forge.editor.promotion_rule`).
pub struct PromotionRulePoint;

impl forge_plugin::ExtensionPoint for PromotionRulePoint {
    type Item = Arc<dyn PromotionRule>;
    const ID: &'static str = "forge.editor.promotion_rule";
    const NAME: &'static str = "PromotionRule";
}

/// The promotion rules an editor has (its `PromotionRule` registry, in order), with their
/// tracking merged.
#[derive(Clone, Default)]
pub struct PromotionRules {
    rules: Vec<Arc<dyn PromotionRule>>,
    presets: Vec<(RulePreset, usize)>,
    tracking: Tracking,
}

impl std::fmt::Debug for PromotionRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromotionRules")
            .field("rules", &self.rules.len())
            .field(
                "presets",
                &self.presets.iter().map(|(p, _)| &p.dir).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl PromotionRules {
    /// The rules in `rules`, in order.
    #[must_use]
    pub fn new(rules: Vec<Arc<dyn PromotionRule>>) -> Self {
        let mut tracking = Tracking::default();
        let mut presets = Vec::new();
        for (i, r) in rules.iter().enumerate() {
            tracking.merge(&r.tracking());
            if let Some(p) = r.preset()
                && Family::from_dir(&p.dir).is_none()
                && !presets
                    .iter()
                    .any(|(q, _): &(RulePreset, usize)| q.dir == p.dir)
            {
                presets.push((p, i));
            }
        }
        Self {
            rules,
            presets,
            tracking,
        }
    }
    /// The rules of a `PromotionRule` registry.
    #[must_use]
    pub fn from_registry(reg: &forge_plugin::Registry<PromotionRulePoint>) -> Self {
        Self::new(reg.iter().map(|(_, r)| Arc::clone(r)).collect())
    }
    /// What every rule tracks, merged.
    #[must_use]
    pub fn tracking(&self) -> &Tracking {
        &self.tracking
    }
    /// The presets the rules add, in order.
    pub fn presets(&self) -> impl Iterator<Item = &RulePreset> {
        self.presets.iter().map(|(p, _)| p)
    }
    fn preset(&self, dir: &str) -> Option<(&RulePreset, &dyn PromotionRule)> {
        self.presets
            .iter()
            .find(|(p, _)| p.dir == dir)
            .and_then(|(p, i)| self.rules.get(*i).map(|r| (p, r.as_ref())))
    }
    /// The family of the preset in `dir`: a base preset's, a rule preset's, else 3D.
    #[must_use]
    pub fn family_of_dir(&self, dir: &str) -> Family {
        Family::from_dir(dir)
            .or_else(|| self.preset(dir).map(|(p, _)| p.family))
            .unwrap_or(Family::ThreeD)
    }
    /// The label of the preset in `dir` (a directory no rule knows: the directory itself).
    #[must_use]
    pub fn label_of(&self, dir: &str) -> String {
        match Family::from_dir(dir) {
            Some(f) => f.label().to_string(),
            None => self
                .preset(dir)
                .map_or_else(|| dir.to_string(), |(p, _)| p.label.clone()),
        }
    }
    /// Whether promotion can reach `dir`: a base preset or a rule's.
    #[must_use]
    pub fn knows(&self, dir: &str) -> bool {
        Family::from_dir(dir).is_some() || self.preset(dir).is_some()
    }
    /// Every preset directory promotion can reach: the base ones, then the rules'.
    #[must_use]
    pub fn dirs(&self) -> Vec<String> {
        Family::ALL
            .iter()
            .map(|f| f.dir().to_string())
            .chain(self.presets().map(|p| p.dir.clone()))
            .collect()
    }
}

fn steps(from: &str, to: &str, rules: &PromotionRules) -> Vec<(String, String)> {
    let (ff, ft) = (rules.family_of_dir(from), rules.family_of_dir(to));
    let mut path: Vec<&str> = vec![from, ff.dir()];
    if ff != ft {
        path.push(ft.dir());
    }
    path.push(to);
    path.dedup();
    if from == to {
        return Vec::new();
    }
    path.windows(2)
        .map(|w| (w[0].to_string(), w[1].to_string()))
        .collect()
}

fn describe(step: (&str, &str), rules: &PromotionRules) -> (Cost, String) {
    match (Family::from_dir(step.0), Family::from_dir(step.1)) {
        (Some(Family::TwoD), Some(Family::ThreeD)) => (
            Cost::Additive,
            forge_ui::tr!(
                "2D \u{2192} 3D: additive. The 2D content keeps working as a 2D layer in the 3D project."
            )
            .to_string(),
        ),
        (Some(Family::ThreeD), Some(Family::TwoD)) => (
            Cost::Narrowing,
            forge_ui::tr!(
                "3D \u{2192} 2D: narrowing. 3D-only data is removed and depth is flattened onto the 2D plane."
            )
            .to_string(),
        ),
        (Some(_), None) => rules.preset(step.1).map_or_else(
            || settings_change(step, rules),
            |(p, _)| p.enter.clone(),
        ),
        _ => rules.preset(step.0).map_or_else(
            || settings_change(step, rules),
            |(p, _)| p.leave.clone(),
        ),
    }
}

fn settings_change(step: (&str, &str), rules: &PromotionRules) -> (Cost, String) {
    (
        Cost::NonDestructive,
        forge_ui::trf!(
            "{from} \u{2192} {to}: a settings change. Non-destructive.",
            from = rules.label_of(step.0),
            to = rules.label_of(step.1)
        ),
    )
}

/// Plan promoting the project seen as `v` to the preset in `to` (`presets`: a preset's
/// default settings, by directory — keys a [`View`] holds, [`rule_keys`];
/// [`builtin_defaults`] is the source), with no plugin rules.
pub fn plan_promotion(
    v: &View,
    to: Family,
    presets: &dyn Fn(&str) -> BTreeMap<String, Value>,
) -> Promotion {
    plan_promotion_to(v, to.dir(), presets, &PromotionRules::default())
}

/// [`plan_promotion`] to any preset directory, with `rules`.
pub fn plan_promotion_to(
    v: &View,
    to: &str,
    presets: &dyn Fn(&str) -> BTreeMap<String, Value>,
    rules: &PromotionRules,
) -> Promotion {
    let from = v.preset_dir().to_string();
    let mut settings: BTreeMap<String, Value> = v.settings.clone();
    let mut facts: BTreeMap<EntityKey, Facts> = v.entities.clone();
    let mut out = Promotion {
        from_label: rules.label_of(&from),
        to_label: rules.label_of(to),
        steps: steps(&from, to, rules),
        from,
        to: to.to_string(),
        cost: Cost::NonDestructive,
        describe: Vec::new(),
        losses: Vec::new(),
        settings: BTreeMap::new(),
        edits: BTreeMap::new(),
    };
    for (a, b) in out.steps.clone() {
        let (cost, text) = describe((&a, &b), rules);
        if cost == Cost::Narrowing || (cost == Cost::Additive && out.cost == Cost::NonDestructive) {
            out.cost = cost;
        }
        out.describe.push(text);
        let mut cx = StepCx {
            facts: &mut facts,
            settings: &mut settings,
            out: &mut out,
        };
        // The preset's defaults: swapped only where the project still has the old default.
        let old = presets(&a);
        let new = presets(&b);
        let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
        for k in keys {
            if cx.settings.get(k.as_str()) == old.get(k.as_str()) {
                let want = new.get(k.as_str()).cloned();
                if cx.settings.get(k.as_str()) != want.as_ref() {
                    cx.set(k, want);
                }
            }
        }
        match (Family::from_dir(&a), Family::from_dir(&b)) {
            (Some(Family::TwoD), Some(Family::ThreeD)) => {
                cx.set(LAYERS_2D_SETTING, Some(Value::Bool(true)));
            }
            (Some(Family::ThreeD), Some(Family::TwoD)) => {
                narrow_to_2d(&mut cx);
                for r in &rules.rules {
                    r.to_2d(&mut cx);
                }
            }
            (Some(_), None) => {
                if let Some((_, r)) = rules.preset(&b) {
                    r.enter(&mut cx);
                }
            }
            (None, _) => {
                if let Some((_, r)) = rules.preset(&a) {
                    r.leave(&mut cx);
                }
            }
            _ => {}
        }
        cx.set(PRESET_SETTING, Some(Value::Text(b.clone())));
    }
    out.edits
        .retain(|_, e| !e.remove.is_empty() || !e.set.is_empty());
    // Only what differs from the project is written.
    out.settings
        .retain(|k, val| v.settings.get(k.as_str()) != val.as_ref());
    out
}

/// 3D → 2D: every 3D-only property goes, and depth and tilt are flattened.
fn narrow_to_2d(cx: &mut StepCx<'_>) {
    let only: Vec<&Facts> = cx.facts.values().filter(|f| !f.only3d.is_empty()).collect();
    if !only.is_empty() {
        let kinds: BTreeSet<&str> = only
            .iter()
            .flat_map(|f| f.only3d.iter())
            .filter_map(|p| p.split('.').next())
            .collect();
        let kinds = kinds.into_iter().collect::<Vec<_>>().join(", ");
        let l = loss(
            &forge_ui::trf!(
                "loses its 3D-only data ({kinds}), which the 2D render path cannot draw",
                kinds
            ),
            &forge_ui::trf!(
                "lose their 3D-only data ({kinds}), which the 2D render path cannot draw",
                kinds
            ),
            &only,
        );
        cx.out.losses.push(l);
    }
    let flat: Vec<&Facts> = cx
        .facts
        .values()
        .filter(|f| f.depth() || f.pitch != 0.0 || f.roll != 0.0)
        .collect();
    if !flat.is_empty() {
        let l = loss(
            forge_ui::tr!("is flattened onto the 2D plane: its depth and tilt are removed"),
            forge_ui::tr!("are flattened onto the 2D plane: their depth and tilt are removed"),
            &flat,
        );
        cx.out.losses.push(l);
    }
    let keys: Vec<EntityKey> = cx.facts.keys().copied().collect();
    for k in keys {
        let Some(f) = cx.facts.get_mut(&k) else {
            continue;
        };
        let e = cx.out.edits.entry(k).or_default();
        for p in std::mem::take(&mut f.only3d) {
            e.remove.insert(p);
        }
        if let Some(l) = f.local.filter(|l| l[2] != 0.0) {
            e.set.insert(P_LOCAL.into(), Value::Vec3([l[0], l[1], 0.0]));
            f.local = Some([l[0], l[1], 0.0]);
        }
        if f.pitch != 0.0 {
            e.remove.insert(P_PITCH.into());
            f.pitch = 0.0;
        }
        if f.roll != 0.0 {
            e.remove.insert(P_ROLL.into());
            f.roll = 0.0;
        }
        f.tracked.clear();
    }
    cx.set(LAYERS_2D_SETTING, None);
}

/// The built-in presets' defaults (the planner's source; the dialog uses the same), parsed
/// once per process.
pub fn builtin_defaults(dir: &str) -> BTreeMap<String, Value> {
    builtin_all().get(dir).cloned().unwrap_or_default()
}

/// The promote command to a base preset.
pub fn promote_command(to: Family, accept_loss: bool) -> EditorCommand {
    promote_command_to(to.dir(), accept_loss)
}

/// The promote command to the preset in `dir` (a base preset's or a plugin's).
pub fn promote_command_to(dir: &str, accept_loss: bool) -> EditorCommand {
    EditorCommand::Invoke {
        target: PROMOTE_CMD.into(),
        args: serde_json::json!({ "to": dir, "accept_loss": accept_loss }).to_string(),
    }
}

fn bad(target: &str, why: impl Into<String>) -> CmdError {
    CmdError::BadArgs {
        target: target.to_string(),
        why: why.into(),
    }
}

/// Read a command's `"accept_loss"` (absent: false).
pub fn accept(args: &serde_json::Value, target: &str) -> Result<bool, CmdError> {
    match args.get("accept_loss") {
        None => Ok(false),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| bad(target, "\"accept_loss\" must be true or false")),
    }
}

/// Plan `forge.project.promote {"to": "2d" | "3d", "accept_loss": bool}` with the built-in
/// presets' defaults and no plugin rules.
pub fn plan_promote(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    plan_promote_with(
        b,
        args,
        &builtin_defaults,
        &BTreeSet::new(),
        &PromotionRules::default(),
    )
}

/// Each preset's default settings by directory, shared between the editor core (which
/// replaces them when a project with its own presets loads, WP-16) and its promote
/// command's planner ([`promote_handler`]).
pub type PresetDefaults =
    std::sync::Arc<std::sync::RwLock<BTreeMap<String, BTreeMap<String, Value>>>>;

/// The promotion rules the core's promote planner uses (the plugins' `PromotionRule`
/// registry, attached with the plugin commands).
pub type SharedRules = std::sync::Arc<std::sync::RwLock<PromotionRules>>;

/// The built-in presets' defaults, as a fresh shared table.
#[must_use]
pub fn builtin_preset_defaults() -> PresetDefaults {
    std::sync::Arc::new(std::sync::RwLock::new(
        Family::ALL
            .iter()
            .map(|f| (f.dir().to_string(), builtin_defaults(f.dir())))
            .collect(),
    ))
}

/// The promote command's planner over `table` — the presets the open project works with
/// (its own copies in place of the editor's, WP-16) — and `rules`. The core registers it.
pub fn promote_handler(
    table: PresetDefaults,
    rules: SharedRules,
) -> impl Fn(&mut DiffBuilder<'_>, &serde_json::Value) -> Result<(), CmdError> + Send + Sync + 'static
{
    move |b, args| {
        let t = table
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let r = rules
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let extra: BTreeSet<String> = t.values().flat_map(|d| d.keys().cloned()).collect();
        plan_promote_with(
            b,
            args,
            &|dir| t.get(dir).cloned().unwrap_or_default(),
            &extra,
            &r,
        )
    }
}

/// [`plan_promote`] with `defaults` (a preset's default settings by directory), also
/// reading the settings `extra` names, with `rules`.
pub fn plan_promote_with(
    b: &mut DiffBuilder<'_>,
    args: &serde_json::Value,
    defaults: &dyn Fn(&str) -> BTreeMap<String, Value>,
    extra: &BTreeSet<String>,
    rules: &PromotionRules,
) -> Result<(), CmdError> {
    let to = args
        .get("to")
        .and_then(serde_json::Value::as_str)
        .filter(|d| rules.knows(d))
        .ok_or_else(|| {
            bad(
                PROMOTE_CMD,
                format!("\"to\" must be one of {:?}", rules.dirs()),
            )
        })?
        .to_string();
    let accept_loss = accept(args, PROMOTE_CMD)?;
    let v = View::of_project_in(b.project(), extra, rules.tracking());
    if v.preset_dir() == to {
        return Err(bad(
            PROMOTE_CMD,
            format!(
                "the project already uses the {} preset",
                rules.label_of(&to)
            ),
        ));
    }
    let p = plan_promotion_to(&v, &to, defaults, rules);
    if p.lossy() && !accept_loss {
        return Err(bad(
            PROMOTE_CMD,
            format!(
                "{} \u{2192} {} is lossy for this project: {}. Resend with \"accept_loss\": true to confirm.",
                p.from_label,
                p.to_label,
                p.loss_text()
            ),
        ));
    }
    apply(b, &p.settings, &p.edits)
}

fn apply(
    b: &mut DiffBuilder<'_>,
    settings: &BTreeMap<String, Option<Value>>,
    edits: &BTreeMap<EntityKey, EntityEdit>,
) -> Result<(), CmdError> {
    for (k, v) in settings {
        b.set_setting(k, v.clone())?;
    }
    for (k, e) in edits {
        for p in &e.remove {
            b.remove_property(*k, p)?;
        }
        for (p, v) in &e.set {
            b.set_property(*k, p, v.clone())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Template, template_doc};
    use forge_cmd::{Bus, CommandSink, Issuer};

    fn bus_with(t: Template) -> Bus {
        bus_of(t, None)
    }

    /// A bus over template `t`; `promote`: its promote planner (else the built-in one).
    fn bus_of(t: Template, promote: Option<(PresetDefaults, SharedRules)>) -> Bus {
        let mut b = Bus::new();
        for (target, policy, plan) in crate::project::handlers() {
            if target == PROMOTE_CMD
                && let Some((table, rules)) = promote.clone()
            {
                b.register_handler(target, policy, promote_handler(table, rules))
                    .unwrap_or_else(|e| panic!("{e}"));
                continue;
            }
            b.register_handler(target, policy, plan)
                .unwrap_or_else(|e| panic!("{e}"));
        }
        let doc = template_doc(t, "Demo").unwrap_or_else(|e| panic!("{e}"));
        let cmd = doc.load_command().unwrap_or_else(|e| panic!("{e}"));
        let e = b.envelope(Issuer::Test, cmd);
        b.apply(e).unwrap_or_else(|r| panic!("{r:?}"));
        b
    }

    fn send(b: &mut Bus, cmd: EditorCommand) -> Result<(), String> {
        let e = b.envelope(Issuer::Test, cmd);
        b.apply(e).map(drop).map_err(|r| r.error.to_string())
    }

    /// A rule adding a 3D preset `deep` (deeper defaults), whose `deep.*` properties are
    /// 3D-only and whose leaving drops every `deep.probe` (lossy with any).
    struct Deep;

    impl PromotionRule for Deep {
        fn tracking(&self) -> Tracking {
            Tracking {
                only3d: vec!["deep.".into()],
                tracked: vec!["deep.count".into()],
                records: vec![],
                settings: vec!["deep.on".into()],
            }
        }
        fn preset(&self) -> Option<RulePreset> {
            Some(RulePreset {
                dir: "deep".into(),
                label: "Deep".into(),
                family: Family::ThreeD,
                enter: (Cost::NonDestructive, "3D \u{2192} Deep: settings.".into()),
                leave: (Cost::Narrowing, "Deep \u{2192} 3D: probes go.".into()),
            })
        }
        fn leave(&self, cx: &mut StepCx<'_>) {
            let probes: Vec<&Facts> = cx
                .facts
                .values()
                .filter(|f| f.only3d.contains("deep.probe"))
                .collect();
            if !probes.is_empty() {
                let l = loss("loses its probe", "lose their probes", &probes);
                cx.out.losses.push(l);
            }
            let keys: Vec<EntityKey> = probes.iter().map(|f| f.key).collect();
            for k in keys {
                cx.edit(k).remove.insert("deep.probe".into());
            }
        }
        fn to_2d(&self, cx: &mut StepCx<'_>) {
            if cx.settings.get("deep.on") == Some(&Value::Bool(true)) {
                cx.out.losses.push(Loss::project("Deep mode is turned off"));
                cx.set("deep.on", Some(Value::Bool(false)));
            }
        }
    }

    fn deep_defaults(dir: &str) -> BTreeMap<String, Value> {
        let mut d = builtin_defaults(if dir == "deep" { "3d" } else { dir });
        if dir == "deep" {
            d.insert("frames.depth".into(), Value::Int(8));
        }
        d
    }

    fn deep_bus(t: Template) -> Bus {
        let rules: SharedRules =
            Arc::new(std::sync::RwLock::new(PromotionRules::new(vec![Arc::new(
                Deep,
            )])));
        let table = builtin_preset_defaults();
        table
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert("deep".into(), deep_defaults("deep"));
        bus_of(t, Some((table, rules)))
    }

    #[test]
    fn two_d_to_three_d_and_back_restores_the_project_exactly() {
        // The 2D template has no 3D-only data.
        let mut b = bus_with(Template::TwoD);
        let h0 = b.project().state_hash();
        send(&mut b, promote_command(Family::ThreeD, false)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            b.project().setting(LAYERS_2D_SETTING),
            Some(&Value::Bool(true))
        );
        send(&mut b, promote_command(Family::TwoD, false)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(b.project().state_hash(), h0);
    }

    /// A plugin's preset is reached through its family's base preset, with its rule's cost
    /// rows, its losses and its narrowing to 2D.
    #[test]
    fn a_rule_preset_is_reached_through_its_family_and_names_its_losses() {
        let rules = PromotionRules::new(vec![Arc::new(Deep)]);
        let mut v = View {
            settings: builtin_defaults("2d"),
            ..View::default()
        };
        v.settings
            .insert(PRESET_SETTING.into(), Value::Text("2d".into()));
        let p = plan_promotion_to(&v, "deep", &deep_defaults, &rules);
        assert_eq!(
            p.steps,
            vec![
                ("2d".to_string(), "3d".to_string()),
                ("3d".to_string(), "deep".to_string())
            ]
        );
        assert_eq!(p.to_label, "Deep");
        assert_eq!(p.settings.get("frames.depth"), Some(&Some(Value::Int(8))));
        assert!(!p.lossy());
        // Leaving with a probe on an entity: lossy, named; to 2D also turns Deep mode off.
        let mut b = bus_with(Template::ThreeD);
        send(
            &mut b,
            EditorCommand::Spawn {
                name: "Probe".into(),
                parent: None,
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let probe = b
            .project()
            .entities()
            .find(|(_, e)| e.name() == "Probe")
            .map_or(EntityKey(0), |(k, _)| k);
        for (k, v) in [
            (PRESET_SETTING, Value::Text("deep".into())),
            ("deep.on", Value::Bool(true)),
        ] {
            send(
                &mut b,
                EditorCommand::SetSetting {
                    key: k.into(),
                    value: Some(v),
                },
            )
            .unwrap_or_else(|e| panic!("{e}"));
        }
        send(
            &mut b,
            EditorCommand::SetProperty {
                entity: probe,
                path: "deep.probe".into(),
                value: Value::Bool(true),
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let v = View::of_project_in(b.project(), std::iter::empty(), rules.tracking());
        let p = plan_promotion_to(&v, "2d", &deep_defaults, &rules);
        assert_eq!(p.steps.len(), 2);
        let text = p.loss_text();
        assert!(
            text.contains("1 entity loses its probe: \u{201c}Probe\u{201d}"),
            "{text}"
        );
        assert!(text.contains("Deep mode is turned off"), "{text}");
        assert_eq!(p.settings.get("deep.on"), Some(&Some(Value::Bool(false))));
    }

    #[test]
    fn the_core_planner_takes_its_rules_and_refuses_an_unknown_preset() {
        let mut b = deep_bus(Template::ThreeD);
        let h0 = b.project().state_hash();
        send(&mut b, promote_command_to("deep", false)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(b.project().setting("frames.depth"), Some(&Value::Int(8)));
        send(&mut b, promote_command_to("3d", false)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(b.project().state_hash(), h0, "a round trip loses nothing");
        let e = send(&mut b, promote_command_to("nowhere", false))
            .err()
            .unwrap_or_default();
        assert!(e.contains("\"to\" must be one of"), "{e}");
    }

    #[test]
    fn a_user_setting_survives_a_promotion() {
        let mut b = deep_bus(Template::ThreeD);
        send(
            &mut b,
            EditorCommand::SetSetting {
                key: "camera.projection".into(),
                value: Some(Value::Text("orthographic".into())),
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        send(&mut b, promote_command_to("deep", false)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            b.project().setting("camera.projection"),
            Some(&Value::Text("orthographic".into()))
        );
    }

    #[test]
    fn lossy_arrows_are_refused_without_acceptance_and_name_the_loss() {
        // 3D -> 2D with meshes and lights.
        let mut b = bus_with(Template::ThreeD);
        let h = b.project().state_hash();
        let e = send(&mut b, promote_command(Family::TwoD, false))
            .err()
            .unwrap_or_default();
        assert!(
            e.contains("lossy") && e.contains("\u{201c}Ground\u{201d}"),
            "{e}"
        );
        assert_eq!(b.project().state_hash(), h, "refused: nothing changed");
        send(&mut b, promote_command(Family::TwoD, true)).unwrap_or_else(|e| panic!("{e}"));
        let v = View::of_project(b.project());
        assert!(v.entities.values().all(|f| f.only3d.is_empty()));
    }

    /// A loss line agrees with its count: "1 entity is", "2 entities are".
    #[test]
    fn a_loss_line_is_worded_for_its_count() {
        let pitch = Value::Float(0.5);
        let f = |n: &str| {
            fact(
                EntityKey(0),
                n,
                std::iter::once((P_PITCH, &pitch)),
                false,
                &Tracking::default(),
            )
            .unwrap_or_else(|| panic!("a tilted entity is interesting"))
        };
        let (a, b) = (f("A"), f("B"));
        let one = loss("is flattened", "are flattened", &[&a]);
        assert_eq!(one.line(), "1 entity is flattened: \u{201c}A\u{201d}");
        let two = loss("is flattened", "are flattened", &[&a, &b]);
        assert_eq!(
            two.line(),
            "2 entities are flattened: \u{201c}A\u{201d}, \u{201c}B\u{201d}"
        );
    }

    /// A panel's `MirrorView` applies the mirror's change logs: after every kind of change
    /// it equals a view rebuilt from the whole mirror, a setting the rules do not read costs
    /// nothing, and a moved mesh costs one property and changes nothing a dialog shows. It
    /// follows what a rule tracks the same way.
    #[test]
    fn a_mirror_view_follows_changes_and_matches_a_rebuild() {
        use crate::client::BusClient;
        use crate::core::EditorCore;
        let core = EditorCore::new();
        let mut c = EditorCore::connect(&core, Issuer::Test);
        let mut m = ProjectMirror::new();
        let (project, next) = c.snapshot();
        m.resync(&project, next, c.history());
        let tracking = Deep.tracking();
        let mut mv = MirrorView::tracking(tracking.clone());
        let step = |mv: &mut MirrorView,
                    c: &mut crate::core::LocalBus,
                    m: &mut ProjectMirror,
                    cmds: Vec<EditorCommand>|
         -> Refresh {
            for cmd in cmds {
                c.apply(cmd, None);
            }
            let p = c.pump();
            if p.gap.is_some() {
                let (project, next) = c.snapshot();
                m.resync(&project, next, c.history());
            }
            for ev in &p.events {
                m.apply(ev, |t| c.txn_info(t));
            }
            let r = mv.update(m);
            let mut fresh = MirrorView::tracking(tracking.clone());
            fresh.update(m);
            assert_eq!(mv.view(), fresh.view(), "incremental == rebuilt");
            r
        };
        let spawn = |n: &str| EditorCommand::Spawn {
            name: n.into(),
            parent: None,
        };
        let prop = |e: u64, p: &str, v: Value| EditorCommand::SetProperty {
            entity: EntityKey(e),
            path: p.into(),
            value: v,
        };
        let r = step(
            &mut mv,
            &mut c,
            &mut m,
            vec![
                spawn("Crate"),
                spawn("Probe"),
                spawn("Counter"),
                spawn("Sprite"),
                prop(0, "mesh.source", Value::Text("crate.glb".into())),
                prop(1, "deep.probe", Value::Bool(true)),
                prop(2, "deep.count", Value::Int(0)),
                prop(3, P_LOCAL, Value::Vec3([1.0, 2.0, 0.0])),
            ],
        );
        assert!(r.changed);
        assert_eq!(mv.view().entities.len(), 3, "the sprite is not interesting");
        // A setting the rules do not read: nothing read, nothing changed.
        let r = step(
            &mut mv,
            &mut c,
            &mut m,
            vec![EditorCommand::SetSetting {
                key: "d2.tilemap.m.layer.0.chunk.c0_0".into(),
                value: Some(Value::Text("x".into())),
            }],
        );
        assert_eq!(r, Refresh::default());
        // Moving the crate sideways: one property read, nothing a dialog shows.
        let r = step(
            &mut mv,
            &mut c,
            &mut m,
            vec![prop(0, P_LOCAL, Value::Vec3([3.0, 0.0, 0.0]))],
        );
        assert_eq!((r.changed, r.work), (false, 1));
        // Every change a dialog shows, one per step, on an entity the view already holds,
        // must say `changed` — or the dialog keeps showing the old plan.
        let one = |mv: &mut MirrorView,
                   c: &mut crate::core::LocalBus,
                   m: &mut ProjectMirror,
                   cmd: EditorCommand,
                   what: &str| {
            let r = step(mv, c, m, vec![cmd]);
            assert!(
                r.changed,
                "{what} is shown, so it must mark the view changed"
            );
        };
        // A flag the rules test: depth on the crate, then taken away.
        one(
            &mut mv,
            &mut c,
            &mut m,
            prop(0, P_LOCAL, Value::Vec3([3.0, 0.0, 2.0])),
            "depth",
        );
        assert!(mv.view().entities[&EntityKey(0)].depth());
        one(
            &mut mv,
            &mut c,
            &mut m,
            prop(0, P_LOCAL, Value::Vec3([3.0, 0.0, 0.0])),
            "depth removed",
        );
        one(
            &mut mv,
            &mut c,
            &mut m,
            prop(0, P_PITCH, Value::Float(0.5)),
            "tilt",
        );
        one(
            &mut mv,
            &mut c,
            &mut m,
            prop(0, P_PITCH, Value::Float(0.0)),
            "tilt removed",
        );
        // A new 3D-only kind on the crate (it has a mesh; now a light too), then gone.
        one(
            &mut mv,
            &mut c,
            &mut m,
            prop(0, "light.intensity", Value::Float(2.0)),
            "a new 3D-only kind",
        );
        // A second property of a kind it already has: nothing a dialog shows.
        let r = step(
            &mut mv,
            &mut c,
            &mut m,
            vec![prop(0, "light.range", Value::Float(9.0))],
        );
        assert_eq!((r.changed, r.work), (false, 1));
        let r = step(
            &mut mv,
            &mut c,
            &mut m,
            vec![EditorCommand::RemoveProperty {
                entity: EntityKey(0),
                path: "light.intensity".into(),
            }],
        );
        assert_eq!((r.changed, r.work), (false, 1), "a light property is left");
        one(
            &mut mv,
            &mut c,
            &mut m,
            EditorCommand::RemoveProperty {
                entity: EntityKey(0),
                path: "light.range".into(),
            },
            "the last light property removed (the kind is gone)",
        );
        // A tracked value.
        one(
            &mut mv,
            &mut c,
            &mut m,
            prop(2, "deep.count", Value::Int(3)),
            "a tracked value",
        );
        assert_eq!(mv.view().sum_tracked("deep.count"), 3);
        // A rename.
        one(
            &mut mv,
            &mut c,
            &mut m,
            EditorCommand::Rename {
                entity: EntityKey(1),
                name: "Sonde".into(),
            },
            "a name",
        );
        // Depth on the sprite makes it interesting.
        one(
            &mut mv,
            &mut c,
            &mut m,
            prop(3, P_LOCAL, Value::Vec3([1.0, 2.0, 5.0])),
            "an entity becoming interesting",
        );
        // The preset, and a rule's setting.
        one(
            &mut mv,
            &mut c,
            &mut m,
            EditorCommand::SetSetting {
                key: PRESET_SETTING.into(),
                value: Some(Value::Text("deep".into())),
            },
            "the preset",
        );
        assert_eq!(mv.view().preset_dir(), "deep");
        one(
            &mut mv,
            &mut c,
            &mut m,
            EditorCommand::SetSetting {
                key: "deep.on".into(),
                value: Some(Value::Bool(true)),
            },
            "a rule's setting",
        );
        // Losing the only 3D-only property; a despawn.
        let r = step(
            &mut mv,
            &mut c,
            &mut m,
            vec![
                EditorCommand::RemoveProperty {
                    entity: EntityKey(0),
                    path: "mesh.source".into(),
                },
                EditorCommand::Despawn {
                    entity: EntityKey(2),
                },
            ],
        );
        assert!(r.changed);
        assert!(!mv.view().entities.contains_key(&EntityKey(0)));
        assert_eq!(mv.view().sum_tracked("deep.count"), 0);
        // A resync: the view notices it missed changes and rebuilds.
        let (project, next) = c.snapshot();
        m.resync(&project, next, c.history());
        let r = mv.update(&m);
        assert!(r.changed && r.work > 0, "{r:?}");
    }
}
