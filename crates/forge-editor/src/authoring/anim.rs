//! The animation state machine, blend spaces and bone masks (Ch.19; Ch.21 §21.21
//! "Animation state machine", DoD M2-64).
//!
//! A **machine** has typed **parameters** (float, bool, trigger), **states** and
//! **transitions**. A state plays one motion, or a **blend space** of motions over one
//! parameter (1D) or two (2D); it may be filtered by a **bone mask** (per-bone weights over
//! a skeleton). A transition goes from a state (or from *any* state) to another when all its
//! **conditions** hold, optionally only after an exit time, and cross-fades over its
//! duration. Motions and skeletons come from the animation backend ([`AnimSource`]); the
//! labelled in-memory [`MemoryAnim`] stands in until `forge-anim` (D-4).
//!
//! Project settings (see [`crate::domain`]):
//!
//! | Key | Value |
//! |---|---|
//! | `anim.machine.<m>.name` / `.skeleton` / `.entry` | text (the entry is a state id) |
//! | `anim.machine.<m>.param.<p>.kind` | `Float`, `Bool` or `Trigger` |
//! | `anim.machine.<m>.param.<p>.default` | float or bool |
//! | `anim.machine.<m>.state.<s>.name` / `.motion` / `.mask` | text |
//! | `anim.machine.<m>.state.<s>.x` / `.y` | the node's place on the graph (float) |
//! | `anim.machine.<m>.state.<s>.speed` | playback rate (float, default 1) |
//! | `anim.machine.<m>.state.<s>.blend` | `None`, `1D` or `2D` |
//! | `anim.machine.<m>.state.<s>.px` / `.py` | the blend parameters |
//! | `anim.machine.<m>.state.<s>.sample.<n>.motion` / `.x` / `.y` | a blend sample |
//! | `anim.machine.<m>.transition.<t>.from` / `.to` | state ids (`from` may be `any`) |
//! | `anim.machine.<m>.transition.<t>.duration` / `.exit` | seconds / normalised time |
//! | `anim.machine.<m>.transition.<t>.cond.<n>` | `speed > 0.5`, `grounded`, `!grounded`, `jump` |
//! | `anim.mask.<k>.name` / `.skeleton` | text |
//! | `anim.mask.<k>.bone.b<i>` | the weight of bone `i` (0..=1; absent: 0) |
//!
//! Every edit is a `SetSetting` command (I7). [`validate`] lists what would not run
//! (a transition to a missing state, a condition on an unknown parameter, a blend space
//! with fewer than two samples, an unknown motion, an unreachable state, …) and
//! [`MachineSim`] runs a machine — the preview the editor shows is the evaluation a game
//! would run, deterministic `f64`.

use std::collections::{BTreeMap, BTreeSet};

use forge_cmd::{EditorCommand, Value};

use crate::domain::{BackendInfo, float, ident_from, objects, set, sub_objects, text, unique_id};
use crate::mirror::ProjectMirror;

pub const MACHINE: &str = "anim.machine";
pub const MASK: &str = "anim.mask";
/// The `from` of a transition that leaves any state.
pub const ANY: &str = "any";

// ---- the backend ---------------------------------------------------------------------------

/// One bone of a skeleton.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skeleton {
    pub name: String,
    pub bones: Vec<Bone>,
}

impl Skeleton {
    /// Bone `i` and every bone below it.
    pub fn subtree(&self, i: usize) -> Vec<usize> {
        let mut out = vec![i];
        let mut k = 0;
        while k < out.len() {
            let p = out[k];
            out.extend(
                self.bones
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.parent == Some(p))
                    .map(|(j, _)| j),
            );
            k += 1;
        }
        out
    }
    /// A bone's depth (roots are 0).
    pub fn depth(&self, i: usize) -> usize {
        let mut d = 0;
        let mut cur = self.bones.get(i).and_then(|b| b.parent);
        while let Some(p) = cur {
            d += 1;
            cur = self.bones.get(p).and_then(|b| b.parent);
            if d > self.bones.len() {
                break;
            }
        }
        d
    }
}

/// One motion (an animation clip asset).
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub name: String,
    pub skeleton: String,
    /// Seconds.
    pub length: f64,
    pub looping: bool,
}

/// What the animation system knows (Ch.19): skeletons and motions. The real backend is
/// `forge-anim`; [`MemoryAnim`] is the labelled in-memory stand-in (D-4).
pub trait AnimSource {
    fn backend(&self) -> BackendInfo;
    fn skeletons(&self) -> Vec<Skeleton>;
    fn motions(&self) -> Vec<Motion>;
    fn skeleton(&self, name: &str) -> Option<Skeleton> {
        self.skeletons().into_iter().find(|s| s.name == name)
    }
    fn motion(&self, name: &str) -> Option<Motion> {
        self.motions().into_iter().find(|m| m.name == name)
    }
}

/// A humanoid skeleton and a locomotion set of motions, in memory (D-4: `forge-anim` does
/// not exist yet; the editor says so in its footer).
#[derive(Clone, Debug)]
pub struct MemoryAnim {
    skeletons: Vec<Skeleton>,
    motions: Vec<Motion>,
}

impl Default for MemoryAnim {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryAnim {
    pub fn new() -> Self {
        let bones: &[(&str, Option<usize>)] = &[
            ("Hips", None),
            ("Spine", Some(0)),
            ("Chest", Some(1)),
            ("Neck", Some(2)),
            ("Head", Some(3)),
            ("LeftShoulder", Some(2)),
            ("LeftArm", Some(5)),
            ("LeftForearm", Some(6)),
            ("LeftHand", Some(7)),
            ("RightShoulder", Some(2)),
            ("RightArm", Some(9)),
            ("RightForearm", Some(10)),
            ("RightHand", Some(11)),
            ("LeftUpLeg", Some(0)),
            ("LeftLeg", Some(13)),
            ("LeftFoot", Some(14)),
            ("RightUpLeg", Some(0)),
            ("RightLeg", Some(16)),
            ("RightFoot", Some(17)),
        ];
        let humanoid = Skeleton {
            name: "Humanoid".into(),
            bones: bones
                .iter()
                .map(|(n, p)| Bone {
                    name: (*n).into(),
                    parent: *p,
                })
                .collect(),
        };
        let m = |n: &str, len: f64, looping: bool| Motion {
            name: n.into(),
            skeleton: "Humanoid".into(),
            length: len,
            looping,
        };
        Self {
            skeletons: vec![humanoid],
            motions: vec![
                m("Idle", 2.0, true),
                m("Walk", 1.0, true),
                m("Run", 0.7, true),
                m("StrafeLeft", 1.0, true),
                m("StrafeRight", 1.0, true),
                m("WalkBack", 1.1, true),
                m("Jump", 0.9, false),
                m("Fall", 0.6, true),
                m("Land", 0.4, false),
                m("Wave", 1.5, false),
            ],
        }
    }
}

impl AnimSource for MemoryAnim {
    fn backend(&self) -> BackendInfo {
        BackendInfo {
            name: "in-memory animation library".into(),
            in_memory: true,
            note: "Motions and skeletons are an in-memory sample set; forge-anim (Ch.19) is not built yet."
                .into(),
        }
    }
    fn skeletons(&self) -> Vec<Skeleton> {
        self.skeletons.clone()
    }
    fn motions(&self) -> Vec<Motion> {
        self.motions.clone()
    }
}

// ---- the model -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParamKind {
    Float,
    Bool,
    /// A one-shot bool: set by the game, consumed by the transition it fires.
    Trigger,
}

impl ParamKind {
    pub const ALL: [ParamKind; 3] = [ParamKind::Float, ParamKind::Bool, ParamKind::Trigger];
    pub fn name(self) -> &'static str {
        match self {
            ParamKind::Float => "Float",
            ParamKind::Bool => "Bool",
            ParamKind::Trigger => "Trigger",
        }
    }
    pub fn parse(s: &str) -> Option<ParamKind> {
        ParamKind::ALL.into_iter().find(|k| k.name() == s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub id: String,
    pub kind: ParamKind,
    /// The float default, or 0/1 for a bool.
    pub default: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendKind {
    None,
    OneD,
    TwoD,
}

impl BlendKind {
    pub fn name(self) -> &'static str {
        match self {
            BlendKind::None => "None",
            BlendKind::OneD => "1D",
            BlendKind::TwoD => "2D",
        }
    }
    pub fn parse(s: &str) -> Option<BlendKind> {
        match s {
            "None" => Some(BlendKind::None),
            "1D" => Some(BlendKind::OneD),
            "2D" => Some(BlendKind::TwoD),
            _ => None,
        }
    }
}

/// One motion of a blend space, at its point in parameter space.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub id: String,
    pub motion: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub speed: f64,
    pub mask: Option<String>,
    pub motion: Option<String>,
    pub blend: BlendKind,
    pub px: Option<String>,
    pub py: Option<String>,
    pub samples: Vec<Sample>,
}

/// A comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

/// One transition condition.
#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    /// A bool is true / a trigger is set.
    Is(String),
    /// A bool is false.
    Not(String),
    /// A float compared with a constant.
    Cmp(String, Op, f64),
}

impl Cond {
    pub fn param(&self) -> &str {
        match self {
            Cond::Is(p) | Cond::Not(p) | Cond::Cmp(p, _, _) => p,
        }
    }

    /// Parse `speed > 0.5`, `grounded`, `!grounded`, `jump`.
    pub fn parse(s: &str) -> Result<Cond, String> {
        let s = s.trim();
        for (tok, op) in [
            (">=", Op::Ge),
            ("<=", Op::Le),
            ("==", Op::Eq),
            ("!=", Op::Ne),
            (">", Op::Gt),
            ("<", Op::Lt),
        ] {
            if let Some((a, b)) = s.split_once(tok) {
                let p = a.trim();
                if !crate::domain::is_ident(p) {
                    return Err(format!("{p:?} is not a parameter name"));
                }
                let v: f64 = b
                    .trim()
                    .parse()
                    .map_err(|_| format!("{:?} is not a number", b.trim()))?;
                if !v.is_finite() {
                    return Err("a condition compares with a finite number".into());
                }
                return Ok(Cond::Cmp(p.to_string(), op, v));
            }
        }
        if let Some(p) = s.strip_prefix('!') {
            let p = p.trim();
            if crate::domain::is_ident(p) {
                return Ok(Cond::Not(p.to_string()));
            }
            return Err(format!("{p:?} is not a parameter name"));
        }
        if crate::domain::is_ident(s) {
            return Ok(Cond::Is(s.to_string()));
        }
        Err(format!(
            "{s:?} is not a condition (use `speed > 0.5`, `grounded`, `!grounded` or a trigger)"
        ))
    }
}

impl std::fmt::Display for Cond {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cond::Is(p) => write!(f, "{p}"),
            Cond::Not(p) => write!(f, "!{p}"),
            Cond::Cmp(p, op, v) => {
                let o = match op {
                    Op::Gt => ">",
                    Op::Lt => "<",
                    Op::Ge => ">=",
                    Op::Le => "<=",
                    Op::Eq => "==",
                    Op::Ne => "!=",
                };
                write!(f, "{p} {o} {v}")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub id: String,
    /// A state id, or [`ANY`].
    pub from: String,
    pub to: String,
    /// Cross-fade seconds.
    pub duration: f64,
    /// Normalised time of the source state before which the transition waits.
    pub exit: Option<f64>,
    pub conds: Vec<Cond>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub skeleton: String,
    pub entry: String,
    pub params: BTreeMap<String, Param>,
    pub states: BTreeMap<String, State>,
    pub transitions: BTreeMap<String, Transition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub id: String,
    pub name: String,
    pub skeleton: String,
    /// Bone index → weight (0..=1); absent bones weigh 0.
    pub weights: BTreeMap<usize, f64>,
}

/// Every machine and mask, read from the mirror.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimDoc {
    pub machines: BTreeMap<String, Machine>,
    pub masks: BTreeMap<String, Mask>,
    pub problems: Vec<String>,
}

fn opt_text(f: &BTreeMap<&str, &Value>, k: &str, p: &mut Vec<String>, at: &str) -> Option<String> {
    let s = text(f, k, "", p, at);
    (!s.is_empty()).then_some(s)
}

impl AnimDoc {
    pub fn read(m: &ProjectMirror) -> AnimDoc {
        let mut d = AnimDoc::default();
        let p = &mut d.problems;
        for (id, f) in objects(m, MACHINE) {
            let at = format!("{MACHINE}.{id}");
            let mut params = BTreeMap::new();
            for (pid, pf) in sub_objects(&f, "param") {
                let pat = format!("{at}.param.{pid}");
                let ks = text(&pf, "kind", "Float", p, &pat);
                let kind = ParamKind::parse(&ks).unwrap_or_else(|| {
                    p.push(format!("{pat}.kind: unknown kind {ks:?} (Float used)"));
                    ParamKind::Float
                });
                let default = match pf.get("default") {
                    Some(Value::Bool(b)) => f64::from(u8::from(*b)),
                    Some(Value::Float(x)) => *x,
                    Some(Value::Int(i)) => *i as f64,
                    None => 0.0,
                    Some(v) => {
                        p.push(format!("{pat}.default: unexpected {}", v.kind()));
                        0.0
                    }
                };
                params.insert(
                    pid.to_string(),
                    Param {
                        id: pid.to_string(),
                        kind,
                        default,
                    },
                );
            }
            let mut states = BTreeMap::new();
            for (sid, sf) in sub_objects(&f, "state") {
                let sat = format!("{at}.state.{sid}");
                let bs = text(&sf, "blend", "None", p, &sat);
                let blend = BlendKind::parse(&bs).unwrap_or_else(|| {
                    p.push(format!("{sat}.blend: unknown {bs:?} (None used)"));
                    BlendKind::None
                });
                let mut samples: Vec<Sample> = sub_objects(&sf, "sample")
                    .into_iter()
                    .map(|(nid, nf)| {
                        let nat = format!("{sat}.sample.{nid}");
                        Sample {
                            id: nid.to_string(),
                            motion: text(&nf, "motion", "", p, &nat),
                            x: float(&nf, "x", 0.0, p, &nat),
                            y: float(&nf, "y", 0.0, p, &nat),
                        }
                    })
                    .collect();
                samples.sort_by(|a, b| a.id.cmp(&b.id));
                states.insert(
                    sid.to_string(),
                    State {
                        id: sid.to_string(),
                        name: text(&sf, "name", sid, p, &sat),
                        x: float(&sf, "x", 0.0, p, &sat),
                        y: float(&sf, "y", 0.0, p, &sat),
                        speed: float(&sf, "speed", 1.0, p, &sat),
                        mask: opt_text(&sf, "mask", p, &sat),
                        motion: opt_text(&sf, "motion", p, &sat),
                        blend,
                        px: opt_text(&sf, "px", p, &sat),
                        py: opt_text(&sf, "py", p, &sat),
                        samples,
                    },
                );
            }
            let mut transitions = BTreeMap::new();
            for (tid, tf) in sub_objects(&f, "transition") {
                let tat = format!("{at}.transition.{tid}");
                let mut conds = Vec::new();
                let mut raw: Vec<(&&str, &&Value)> =
                    tf.iter().filter(|(k, _)| k.starts_with("cond.")).collect();
                raw.sort_by_key(|(k, _)| **k);
                for (k, v) in raw {
                    match v {
                        Value::Text(s) => match Cond::parse(s) {
                            Ok(c) => conds.push(c),
                            Err(e) => p.push(format!("{tat}.{k}: {e}")),
                        },
                        other => {
                            p.push(format!("{tat}.{k}: expected text, found {}", other.kind()))
                        }
                    }
                }
                let exit = match tf.get("exit") {
                    None => None,
                    Some(Value::Float(x)) => Some(*x),
                    Some(Value::Int(i)) => Some(*i as f64),
                    Some(v) => {
                        p.push(format!("{tat}.exit: expected a number, found {}", v.kind()));
                        None
                    }
                };
                transitions.insert(
                    tid.to_string(),
                    Transition {
                        id: tid.to_string(),
                        from: text(&tf, "from", "", p, &tat),
                        to: text(&tf, "to", "", p, &tat),
                        duration: float(&tf, "duration", 0.2, p, &tat).max(0.0),
                        exit,
                        conds,
                    },
                );
            }
            d.machines.insert(
                id.to_string(),
                Machine {
                    id: id.to_string(),
                    name: text(&f, "name", id, p, &at),
                    skeleton: text(&f, "skeleton", "", p, &at),
                    entry: text(&f, "entry", "", p, &at),
                    params,
                    states,
                    transitions,
                },
            );
        }
        for (id, f) in objects(m, MASK) {
            let at = format!("{MASK}.{id}");
            let mut weights = BTreeMap::new();
            for (k, v) in &f {
                let Some(b) = k.strip_prefix("bone.b") else {
                    continue;
                };
                match (b.parse::<usize>(), v) {
                    (Ok(i), Value::Float(w)) => {
                        weights.insert(i, w.clamp(0.0, 1.0));
                    }
                    (Ok(i), Value::Int(w)) => {
                        weights.insert(i, (*w as f64).clamp(0.0, 1.0));
                    }
                    _ => p.push(format!("{at}.{k}: expected a bone weight")),
                }
            }
            d.masks.insert(
                id.to_string(),
                Mask {
                    id: id.to_string(),
                    name: text(&f, "name", id, p, &at),
                    skeleton: text(&f, "skeleton", "", p, &at),
                    weights,
                },
            );
        }
        d
    }
}

// ---- blending ------------------------------------------------------------------------------

/// 1D blend weights: linear between the two samples around `p` (by position), clamped to
/// the ends. Weights sum to 1; samples at one position share their weight equally.
pub fn blend_weights_1d(xs: &[f64], p: f64) -> Vec<f64> {
    let n = xs.len();
    let mut w = vec![0.0; n];
    if n == 0 {
        return w;
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| xs[*a].total_cmp(&xs[*b]));
    let lo = xs[order[0]];
    let hi = xs[order[n - 1]];
    let share = |w: &mut Vec<f64>, at: f64, amount: f64| {
        let same: Vec<usize> = (0..n).filter(|i| xs[*i] == at).collect();
        for i in &same {
            w[*i] += amount / same.len() as f64;
        }
    };
    if p <= lo {
        share(&mut w, lo, 1.0);
        return w;
    }
    if p >= hi {
        share(&mut w, hi, 1.0);
        return w;
    }
    // The sample positions around p.
    let a = order
        .iter()
        .map(|i| xs[*i])
        .filter(|x| *x <= p)
        .fold(f64::NEG_INFINITY, f64::max);
    let b = order
        .iter()
        .map(|i| xs[*i])
        .filter(|x| *x > p)
        .fold(f64::INFINITY, f64::min);
    let u = if b > a { (p - a) / (b - a) } else { 0.0 };
    share(&mut w, a, 1.0 - u);
    share(&mut w, b, u);
    w
}

/// 2D blend weights by gradient-band interpolation (Cartesian, Johansen 2009 — what the
/// big engines call "freeform cartesian"): each sample's influence is the minimum over the
/// other samples of how far `p` is from the band between them; normalised. Exact at a
/// sample, continuous, and needs no triangulation. Coincident samples share their weight.
pub fn blend_weights_2d(pts: &[(f64, f64)], p: (f64, f64)) -> Vec<f64> {
    let n = pts.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![1.0];
    }
    let mut w = vec![0.0; n];
    for i in 0..n {
        let (xi, yi) = pts[i];
        let mut m = f64::INFINITY;
        for (j, &(xj, yj)) in pts.iter().enumerate() {
            if i == j {
                continue;
            }
            let (dx, dy) = (xj - xi, yj - yi);
            let len2 = dx * dx + dy * dy;
            if len2 <= 0.0 {
                continue;
            }
            let h = 1.0 - ((p.0 - xi) * dx + (p.1 - yi) * dy) / len2;
            m = m.min(h);
        }
        w[i] = if m.is_finite() { m.max(0.0) } else { 1.0 };
    }
    let sum: f64 = w.iter().sum();
    if sum > 0.0 {
        for x in &mut w {
            *x /= sum;
        }
    } else {
        // Outside every band (cannot happen for p inside the hull): the nearest sample.
        let near = (0..n)
            .min_by(|a, b| {
                let d = |k: usize| (pts[k].0 - p.0).powi(2) + (pts[k].1 - p.1).powi(2);
                d(*a).total_cmp(&d(*b))
            })
            .unwrap_or(0);
        w[near] = 1.0;
    }
    w
}

// ---- validation ----------------------------------------------------------------------------

/// What would not run in `m` (see the module docs), for the editor to show.
pub fn validate(m: &Machine, anim: &dyn AnimSource, masks: &BTreeMap<String, Mask>) -> Vec<String> {
    let mut out = Vec::new();
    let motions: BTreeMap<String, Motion> = anim
        .motions()
        .into_iter()
        .map(|x| (x.name.clone(), x))
        .collect();
    if m.states.is_empty() {
        out.push("the machine has no states".into());
        return out;
    }
    if !m.states.contains_key(&m.entry) {
        out.push(format!("the entry state {:?} does not exist", m.entry));
    }
    if !m.skeleton.is_empty() && anim.skeleton(&m.skeleton).is_none() {
        out.push(format!(
            "skeleton {:?} is not in the animation library",
            m.skeleton
        ));
    }
    let check_motion = |out: &mut Vec<String>, at: &str, name: &str| match motions.get(name) {
        None => out.push(format!(
            "{at}: motion {name:?} is not in the animation library"
        )),
        Some(mo) if !m.skeleton.is_empty() && mo.skeleton != m.skeleton => out.push(format!(
            "{at}: motion {name:?} is for skeleton {:?}, not {:?}",
            mo.skeleton, m.skeleton
        )),
        _ => {}
    };
    for s in m.states.values() {
        let at = format!("state {}", s.name);
        match s.blend {
            BlendKind::None => match &s.motion {
                Some(mo) => check_motion(&mut out, &at, mo),
                None => out.push(format!("{at}: no motion")),
            },
            BlendKind::OneD | BlendKind::TwoD => {
                let need_y = s.blend == BlendKind::TwoD;
                for (axis, p) in [("x", &s.px), ("y", &s.py)] {
                    if axis == "y" && !need_y {
                        continue;
                    }
                    match p.as_ref().and_then(|p| m.params.get(p)) {
                        Some(par) if par.kind == ParamKind::Float => {}
                        Some(_) => out.push(format!("{at}: the {axis} parameter must be a float")),
                        None => out.push(format!("{at}: no {axis} parameter")),
                    }
                }
                if s.samples.len() < 2 {
                    out.push(format!("{at}: a blend space needs at least two motions"));
                }
                for smp in &s.samples {
                    check_motion(&mut out, &format!("{at}, sample {}", smp.id), &smp.motion);
                }
                let mut seen: BTreeSet<(u64, u64)> = BTreeSet::new();
                for smp in &s.samples {
                    let y = if need_y { smp.y } else { 0.0 };
                    if !seen.insert((smp.x.to_bits(), y.to_bits())) {
                        out.push(format!("{at}: two samples at ({}, {y})", smp.x));
                    }
                }
            }
        }
        if let Some(mk) = &s.mask {
            match masks.get(mk) {
                None => out.push(format!("{at}: mask {mk:?} does not exist")),
                Some(mask) if !m.skeleton.is_empty() && mask.skeleton != m.skeleton => out.push(
                    format!("{at}: mask {mk:?} is for skeleton {:?}", mask.skeleton),
                ),
                _ => {}
            }
        }
        if !(s.speed.is_finite() && s.speed > 0.0) {
            out.push(format!("{at}: speed {} must be positive", s.speed));
        }
    }
    for t in m.transitions.values() {
        let from = if t.from == ANY {
            "any state".to_string()
        } else {
            m.states
                .get(&t.from)
                .map_or_else(|| t.from.clone(), |s| s.name.clone())
        };
        let at = format!("transition {from} \u{2192} {}", t.to);
        if t.from != ANY && !m.states.contains_key(&t.from) {
            out.push(format!("{at}: from a state that does not exist"));
        }
        if !m.states.contains_key(&t.to) {
            out.push(format!("{at}: to a state that does not exist"));
        }
        if t.conds.is_empty() && t.exit.is_none() {
            out.push(format!(
                "{at}: no condition and no exit time (it fires at once)"
            ));
        }
        for c in &t.conds {
            match (m.params.get(c.param()), c) {
                (None, _) => out.push(format!("{at}: unknown parameter {:?}", c.param())),
                (Some(p), Cond::Cmp(..)) if p.kind != ParamKind::Float => {
                    out.push(format!("{at}: {c} compares a {}", p.kind.name()))
                }
                (Some(p), Cond::Is(_) | Cond::Not(_)) if p.kind == ParamKind::Float => out.push(
                    format!("{at}: {c} tests a float as a bool (compare it instead)"),
                ),
                (Some(p), Cond::Not(_)) if p.kind == ParamKind::Trigger => {
                    out.push(format!("{at}: {c} negates a trigger"))
                }
                _ => {}
            }
        }
        if let Some(e) = t.exit
            && !(0.0..=1.0).contains(&e)
        {
            out.push(format!("{at}: exit time {e} is outside 0..=1"));
        }
    }
    // Reachability from the entry.
    if m.states.contains_key(&m.entry) {
        let mut seen = BTreeSet::from([m.entry.clone()]);
        let any_targets: Vec<&String> = m
            .transitions
            .values()
            .filter(|t| t.from == ANY)
            .map(|t| &t.to)
            .collect();
        for t in any_targets {
            seen.insert(t.clone());
        }
        let mut changed = true;
        while changed {
            changed = false;
            for t in m.transitions.values() {
                if seen.contains(&t.from) && seen.insert(t.to.clone()) {
                    changed = true;
                }
            }
        }
        for s in m.states.values() {
            if !seen.contains(&s.id) {
                out.push(format!("state {} is unreachable from the entry", s.name));
            }
        }
    }
    out
}

// ---- the runtime evaluation ----------------------------------------------------------------

/// A running machine (the preview; the same evaluation a game runs).
#[derive(Clone, Debug, PartialEq)]
pub struct MachineSim {
    pub state: String,
    /// Seconds in the current state.
    pub time: f64,
    /// A cross-fade in progress: `(from state, its time, elapsed, duration)`.
    pub fading: Option<(String, f64, f64, f64)>,
    pub params: BTreeMap<String, f64>,
    /// Transitions taken, in order (the preview's log).
    pub log: Vec<String>,
}

impl MachineSim {
    /// At the entry state with every parameter at its default.
    pub fn new(m: &Machine) -> Self {
        Self {
            state: m.entry.clone(),
            time: 0.0,
            fading: None,
            params: m
                .params
                .values()
                .map(|p| (p.id.clone(), p.default))
                .collect(),
            log: Vec::new(),
        }
    }

    pub fn set_param(&mut self, id: &str, v: f64) {
        if let Some(x) = self.params.get_mut(id) {
            *x = v;
        }
    }

    fn holds(&self, c: &Cond) -> bool {
        let v = |p: &str| self.params.get(p).copied().unwrap_or(0.0);
        match c {
            Cond::Is(p) => v(p) != 0.0,
            Cond::Not(p) => v(p) == 0.0,
            Cond::Cmp(p, op, k) => {
                let x = v(p);
                match op {
                    Op::Gt => x > *k,
                    Op::Lt => x < *k,
                    Op::Ge => x >= *k,
                    Op::Le => x <= *k,
                    Op::Eq => x == *k,
                    Op::Ne => x != *k,
                }
            }
        }
    }

    /// The current state's length in seconds (its motion's, or its blend's heaviest).
    fn state_length(&self, m: &Machine, anim: &dyn AnimSource, id: &str) -> f64 {
        let Some(s) = m.states.get(id) else {
            return 1.0;
        };
        let len = |name: &str| anim.motion(name).map_or(1.0, |x| x.length);
        match s.blend {
            BlendKind::None => s.motion.as_deref().map_or(1.0, len),
            _ => {
                let w = self.blend(m, id);
                w.iter().map(|(mo, wt)| len(mo) * wt).sum::<f64>().max(1e-3)
            }
        }
    }

    /// Step by `dt` seconds: advance time, finish a cross-fade, then take the first
    /// transition that holds (any-state transitions first, then the current state's, each
    /// in id order). A trigger is consumed by the transition it fires.
    pub fn step(&mut self, m: &Machine, anim: &dyn AnimSource, dt: f64) {
        let speed = m.states.get(&self.state).map_or(1.0, |s| s.speed);
        self.time += dt * speed;
        if let Some((f, ft, el, dur)) = self.fading.take() {
            let el = el + dt;
            if el < dur {
                self.fading = Some((f, ft + dt, el, dur));
            }
        }
        let norm = self.time / self.state_length(m, anim, &self.state);
        let candidates = m
            .transitions
            .values()
            .filter(|t| t.from == ANY && t.to != self.state)
            .chain(m.transitions.values().filter(|t| t.from == self.state));
        let mut fire = None;
        for t in candidates {
            if !m.states.contains_key(&t.to) {
                continue;
            }
            if let Some(e) = t.exit
                && norm < e
            {
                continue;
            }
            if t.conds.iter().all(|c| self.holds(c)) {
                fire = Some(t.clone());
                break;
            }
        }
        if let Some(t) = fire {
            for c in &t.conds {
                if let Cond::Is(p) = c
                    && m.params
                        .get(p)
                        .is_some_and(|x| x.kind == ParamKind::Trigger)
                {
                    self.set_param(p, 0.0);
                }
            }
            self.log.push(format!("{} \u{2192} {}", self.state, t.to));
            self.fading =
                (t.duration > 0.0).then(|| (self.state.clone(), self.time, 0.0, t.duration));
            self.state = t.to.clone();
            self.time = 0.0;
        }
    }

    /// The motions a state plays and their weights (summing to 1).
    pub fn blend(&self, m: &Machine, id: &str) -> Vec<(String, f64)> {
        let Some(s) = m.states.get(id) else {
            return Vec::new();
        };
        let p = |x: &Option<String>| {
            x.as_ref()
                .and_then(|k| self.params.get(k))
                .copied()
                .unwrap_or(0.0)
        };
        match s.blend {
            BlendKind::None => s.motion.iter().map(|x| (x.clone(), 1.0)).collect(),
            BlendKind::OneD => {
                let xs: Vec<f64> = s.samples.iter().map(|x| x.x).collect();
                let w = blend_weights_1d(&xs, p(&s.px));
                s.samples
                    .iter()
                    .zip(w)
                    .map(|(x, w)| (x.motion.clone(), w))
                    .collect()
            }
            BlendKind::TwoD => {
                let pts: Vec<(f64, f64)> = s.samples.iter().map(|x| (x.x, x.y)).collect();
                let w = blend_weights_2d(&pts, (p(&s.px), p(&s.py)));
                s.samples
                    .iter()
                    .zip(w)
                    .map(|(x, w)| (x.motion.clone(), w))
                    .collect()
            }
        }
    }

    /// Every motion playing now and its weight: the current state's blend, cross-faded with
    /// the state being left. Motions under 0.001 are dropped; weights sum to 1.
    pub fn weights(&self, m: &Machine) -> Vec<(String, f64)> {
        let mut acc: BTreeMap<String, f64> = BTreeMap::new();
        let (a, from) = match &self.fading {
            Some((f, _, el, dur)) if *dur > 0.0 => ((el / dur).clamp(0.0, 1.0), Some(f)),
            _ => (1.0, None),
        };
        for (mo, w) in self.blend(m, &self.state) {
            *acc.entry(mo).or_default() += w * a;
        }
        if let Some(f) = from {
            for (mo, w) in self.blend(m, f) {
                *acc.entry(mo).or_default() += w * (1.0 - a);
            }
        }
        acc.into_iter().filter(|(_, w)| *w >= 0.001).collect()
    }
}

// ---- edits ---------------------------------------------------------------------------------

fn mk(m: &str, rest: &str) -> String {
    format!("{MACHINE}.{m}.{rest}")
}

/// A new machine with one state (the entry), on `skeleton`: `(id, commands)`.
pub fn new_machine(
    doc: &AnimDoc,
    name: &str,
    skeleton: &str,
    first_motion: &str,
) -> (String, Vec<EditorCommand>) {
    let id = unique_id(&ident_from(name), |s| doc.machines.contains_key(s));
    (
        id.clone(),
        vec![
            set(mk(&id, "name"), Value::Text(name.into())),
            set(mk(&id, "skeleton"), Value::Text(skeleton.into())),
            set(mk(&id, "entry"), Value::Text("idle".into())),
            set(mk(&id, "state.idle.name"), Value::Text(first_motion.into())),
            set(
                mk(&id, "state.idle.motion"),
                Value::Text(first_motion.into()),
            ),
            set(mk(&id, "state.idle.x"), Value::Float(0.0)),
            set(mk(&id, "state.idle.y"), Value::Float(0.0)),
        ],
    )
}

/// A state playing `motion` at graph point `(x, y)`.
pub fn add_state(
    m: &Machine,
    name: &str,
    motion: &str,
    x: f64,
    y: f64,
) -> (String, Vec<EditorCommand>) {
    let id = unique_id(&ident_from(name), |s| m.states.contains_key(s) || s == ANY);
    let b = |f: &str| mk(&m.id, &format!("state.{id}.{f}"));
    (
        id.clone(),
        vec![
            set(b("name"), Value::Text(name.into())),
            set(b("motion"), Value::Text(motion.into())),
            set(b("x"), Value::Float(x)),
            set(b("y"), Value::Float(y)),
        ],
    )
}

/// A blend-space state over `px` (and `py` for 2D) with `samples` `(motion, x, y)`.
pub fn add_blend_state(
    m: &Machine,
    name: &str,
    kind: BlendKind,
    px: &str,
    py: Option<&str>,
    samples: &[(&str, f64, f64)],
    at: (f64, f64),
) -> Result<(String, Vec<EditorCommand>), String> {
    if kind == BlendKind::None {
        return Err("a blend state is 1D or 2D".into());
    }
    for (axis, p) in [("x", Some(px)), ("y", py)] {
        if axis == "y" && kind == BlendKind::OneD {
            continue;
        }
        let p = p.ok_or(format!("a 2D blend space needs a {axis} parameter"))?;
        match m.params.get(p) {
            Some(par) if par.kind == ParamKind::Float => {}
            Some(par) => {
                return Err(format!(
                    "{p} is a {}, a blend axis is a float",
                    par.kind.name()
                ));
            }
            None => return Err(format!("no parameter {p}")),
        }
    }
    let id = unique_id(&ident_from(name), |s| m.states.contains_key(s) || s == ANY);
    let b = |f: &str| mk(&m.id, &format!("state.{id}.{f}"));
    let mut cmds = vec![
        set(b("name"), Value::Text(name.into())),
        set(b("blend"), Value::Text(kind.name().into())),
        set(b("px"), Value::Text(px.into())),
        set(b("x"), Value::Float(at.0)),
        set(b("y"), Value::Float(at.1)),
    ];
    if let Some(py) = py.filter(|_| kind == BlendKind::TwoD) {
        cmds.push(set(b("py"), Value::Text(py.into())));
    }
    for (i, (mo, x, y)) in samples.iter().enumerate() {
        cmds.push(set(
            b(&format!("sample.s{i}.motion")),
            Value::Text((*mo).into()),
        ));
        cmds.push(set(b(&format!("sample.s{i}.x")), Value::Float(*x)));
        if kind == BlendKind::TwoD {
            cmds.push(set(b(&format!("sample.s{i}.y")), Value::Float(*y)));
        }
    }
    Ok((id, cmds))
}

/// Move a blend sample to `(x, y)` (a 1D space ignores `y`).
pub fn move_sample(m: &Machine, state: &State, sample: &str, x: f64, y: f64) -> Vec<EditorCommand> {
    let b = |f: &str| mk(&m.id, &format!("state.{}.sample.{sample}.{f}", state.id));
    let mut v = vec![set(b("x"), Value::Float(x))];
    if state.blend == BlendKind::TwoD {
        v.push(set(b("y"), Value::Float(y)));
    }
    v
}

/// Add a motion to a blend space at `(x, y)`.
pub fn add_sample(m: &Machine, state: &State, motion: &str, x: f64, y: f64) -> Vec<EditorCommand> {
    let id = unique_id(&format!("s{}", state.samples.len()), |s| {
        state.samples.iter().any(|x| x.id == s)
    });
    let b = |f: &str| mk(&m.id, &format!("state.{}.sample.{id}.{f}", state.id));
    let mut v = vec![
        set(b("motion"), Value::Text(motion.into())),
        set(b("x"), Value::Float(x)),
    ];
    if state.blend == BlendKind::TwoD {
        v.push(set(b("y"), Value::Float(y)));
    }
    v
}

/// A transition `from` → `to` with `conds` (each parsed and checked against the
/// parameters, so a bad condition is refused with its reason, not written).
pub fn add_transition(
    m: &Machine,
    from: &str,
    to: &str,
    conds: &[&str],
    duration: f64,
) -> Result<(String, Vec<EditorCommand>), String> {
    if from != ANY && !m.states.contains_key(from) {
        return Err(format!("no state {from}"));
    }
    if !m.states.contains_key(to) {
        return Err(format!("no state {to}"));
    }
    if from == to {
        return Err("a transition goes to another state".into());
    }
    if !(duration.is_finite() && (0.0..=10.0).contains(&duration)) {
        return Err(format!("a cross-fade is 0..=10 s, not {duration}"));
    }
    for c in conds {
        let c = Cond::parse(c)?;
        match m.params.get(c.param()) {
            None => return Err(format!("unknown parameter {:?}", c.param())),
            Some(p) if matches!(c, Cond::Cmp(..)) && p.kind != ParamKind::Float => {
                return Err(format!("{c}: {} is a {}, not a float", p.id, p.kind.name()));
            }
            Some(p) if !matches!(c, Cond::Cmp(..)) && p.kind == ParamKind::Float => {
                return Err(format!(
                    "{c}: {} is a float; compare it (`{} > 0.5`)",
                    p.id, p.id
                ));
            }
            _ => {}
        }
    }
    let id = unique_id(&format!("{from}_to_{to}"), |s| {
        m.transitions.contains_key(s)
    });
    let b = |f: &str| mk(&m.id, &format!("transition.{id}.{f}"));
    let mut v = vec![
        set(b("from"), Value::Text(from.into())),
        set(b("to"), Value::Text(to.into())),
        set(b("duration"), Value::Float(duration)),
    ];
    for (i, c) in conds.iter().enumerate() {
        v.push(set(b(&format!("cond.c{i}")), Value::Text(c.trim().into())));
    }
    Ok((id, v))
}

/// A parameter.
pub fn add_param(m: &Machine, name: &str, kind: ParamKind) -> (String, Vec<EditorCommand>) {
    let id = unique_id(&ident_from(name), |s| m.params.contains_key(s));
    let b = |f: &str| mk(&m.id, &format!("param.{id}.{f}"));
    let default = match kind {
        ParamKind::Float => Value::Float(0.0),
        _ => Value::Bool(false),
    };
    (
        id.clone(),
        vec![
            set(b("kind"), Value::Text(kind.name().into())),
            set(b("default"), default),
        ],
    )
}

/// Move states on the graph (one gesture's end).
pub fn move_states(m: &Machine, moves: &[(String, f64, f64)]) -> Vec<EditorCommand> {
    moves
        .iter()
        .filter(|(s, _, _)| m.states.contains_key(s))
        .flat_map(|(s, x, y)| {
            [
                set(mk(&m.id, &format!("state.{s}.x")), Value::Float(*x)),
                set(mk(&m.id, &format!("state.{s}.y")), Value::Float(*y)),
            ]
        })
        .collect()
}

pub fn set_entry(m: &Machine, state: &str) -> Result<Vec<EditorCommand>, String> {
    if !m.states.contains_key(state) {
        return Err(format!("no state {state}"));
    }
    Ok(vec![set(mk(&m.id, "entry"), Value::Text(state.into()))])
}

/// Remove states and every transition into or out of them (one transaction).
pub fn delete_states(mirror: &ProjectMirror, m: &Machine, states: &[String]) -> Vec<EditorCommand> {
    let mut v = Vec::new();
    for s in states {
        v.extend(crate::domain::clear_under(
            mirror,
            &mk(&m.id, &format!("state.{s}")),
        ));
    }
    for t in m.transitions.values() {
        if states.contains(&t.from) || states.contains(&t.to) {
            v.extend(crate::domain::clear_under(
                mirror,
                &mk(&m.id, &format!("transition.{}", t.id)),
            ));
        }
    }
    v
}

pub fn delete_transition(mirror: &ProjectMirror, m: &Machine, t: &str) -> Vec<EditorCommand> {
    crate::domain::clear_under(mirror, &mk(&m.id, &format!("transition.{t}")))
}

/// A new bone mask on `skeleton` including the bones `bones` (weight 1).
pub fn new_mask(
    doc: &AnimDoc,
    name: &str,
    skeleton: &str,
    bones: &[usize],
) -> (String, Vec<EditorCommand>) {
    let id = unique_id(&ident_from(name), |s| doc.masks.contains_key(s));
    let mut v = vec![
        set(format!("{MASK}.{id}.name"), Value::Text(name.into())),
        set(
            format!("{MASK}.{id}.skeleton"),
            Value::Text(skeleton.into()),
        ),
    ];
    for b in bones {
        v.push(set(format!("{MASK}.{id}.bone.b{b}"), Value::Float(1.0)));
    }
    (id, v)
}

/// Set bones' weights in a mask (a weight of 0 clears the bone).
pub fn set_mask_weights(
    mask: &Mask,
    bones: &[usize],
    w: f64,
) -> Result<Vec<EditorCommand>, String> {
    if !(0.0..=1.0).contains(&w) {
        return Err(format!("a bone weight is 0..=1, not {w}"));
    }
    Ok(bones
        .iter()
        .map(|b| {
            let k = format!("{MASK}.{}.bone.b{b}", mask.id);
            if w == 0.0 {
                crate::domain::clear(k)
            } else {
                set(k, Value::Float(w))
            }
        })
        .collect())
}

/// Assign a mask to a state (`None` clears it).
pub fn set_state_mask(m: &Machine, state: &str, mask: Option<&str>) -> Vec<EditorCommand> {
    let k = mk(&m.id, &format!("state.{state}.mask"));
    vec![match mask {
        Some(x) => set(k, Value::Text(x.into())),
        None => crate::domain::clear(k),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_d_weights_interpolate_and_clamp() {
        let xs = [0.0, 1.0, 3.0];
        assert_eq!(blend_weights_1d(&xs, 0.5), vec![0.5, 0.5, 0.0]);
        assert_eq!(blend_weights_1d(&xs, 2.0), vec![0.0, 0.5, 0.5]);
        assert_eq!(blend_weights_1d(&xs, -4.0), vec![1.0, 0.0, 0.0]);
        assert_eq!(blend_weights_1d(&xs, 9.0), vec![0.0, 0.0, 1.0]);
        assert_eq!(
            blend_weights_1d(&[2.0, 0.0], 0.5),
            vec![0.25, 0.75],
            "any order"
        );
        assert_eq!(
            blend_weights_1d(&[1.0, 1.0], 1.0),
            vec![0.5, 0.5],
            "coincident share"
        );
    }

    #[test]
    fn two_d_weights_are_exact_at_samples_and_sum_to_one() {
        let pts = [(0.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0), (0.0, -1.0)];
        for (i, p) in pts.iter().enumerate() {
            let w = blend_weights_2d(&pts, *p);
            assert!((w[i] - 1.0).abs() < 1e-12, "{i}: {w:?}");
        }
        for p in [(0.3, 0.4), (-0.7, 0.1), (0.2, -0.9), (0.5, 0.5)] {
            let w = blend_weights_2d(&pts, p);
            let s: f64 = w.iter().sum();
            assert!((s - 1.0).abs() < 1e-12, "{p:?}: {w:?}");
            assert!(w.iter().all(|x| *x >= 0.0));
        }
        // Halfway between centre and forward: those two only.
        let w = blend_weights_2d(&pts, (0.0, 0.5));
        assert!(
            (w[0] - 0.5).abs() < 1e-12 && (w[1] - 0.5).abs() < 1e-12,
            "{w:?}"
        );
    }

    #[test]
    fn conditions_parse_and_print() {
        assert_eq!(
            Cond::parse("speed > 0.5"),
            Ok(Cond::Cmp("speed".into(), Op::Gt, 0.5))
        );
        assert_eq!(Cond::parse("!grounded"), Ok(Cond::Not("grounded".into())));
        assert_eq!(Cond::parse(" jump "), Ok(Cond::Is("jump".into())));
        assert_eq!(
            Cond::parse("speed>=2").map(|c| c.to_string()),
            Ok("speed >= 2".into())
        );
        assert!(Cond::parse("speed > fast").is_err());
        assert!(Cond::parse("1x").is_err());
        assert!(Cond::parse("a b").is_err());
    }

    #[test]
    fn subtree_and_depth() {
        let a = MemoryAnim::new();
        let s = a.skeleton("Humanoid").expect("humanoid");
        let arm = s
            .bones
            .iter()
            .position(|b| b.name == "LeftShoulder")
            .expect("bone");
        let sub: Vec<&str> = s
            .subtree(arm)
            .iter()
            .map(|i| s.bones[*i].name.as_str())
            .collect();
        assert_eq!(
            sub,
            vec!["LeftShoulder", "LeftArm", "LeftForearm", "LeftHand"]
        );
        assert_eq!(s.depth(arm), 3);
    }
}
