//! The action map (Ch.28 §28.10): contexts (the editor's "action maps", such as Gameplay or
//! Menus), their actions, default bindings, modifiers and triggers — authored as project
//! settings by the input-map panel (M2-68) and compiled here into flat tables the runtime
//! evaluates without a string compare or an allocation.
//!
//! Project settings (every value is text):
//!
//! | Key | Value |
//! |---|---|
//! | `input.map.<m>.name` | display name |
//! | `input.map.<m>.priority` | integer; higher contexts are evaluated first and consume their controls (default 0) |
//! | `input.map.<m>.consume` | `true`/`false`: whether an actuated action hides its controls from lower contexts (default `true`) |
//! | `input.map.<m>.action.<a>.name` / `.kind` | display name; `Button`, `Axis1D` or `Axis2D` |
//! | `input.map.<m>.action.<a>.bind.<n>` | a [`Binding`] |
//! | `input.map.<m>.action.<a>.bindmod.<n>` | the binding's [`Modifier`] chain (`Deadzone(Radial, 0.15, 0.95) \| Curve(Power, 2)`) |
//! | `input.map.<m>.action.<a>.modifiers` | the action's modifier chain (after the bindings combine) |
//! | `input.map.<m>.action.<a>.triggers` | the action's [`Trigger`] chain (`Hold(0.5) \| Chord(gameplay/aim)`) |
//!
//! An action is addressed as `<m>/<a>` ([`ActionHandle`] once compiled).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::control::{ActionKind, Binding, Device, Shape};
use crate::modifier::Modifier;
use crate::trigger::{CTrigger, Trigger};

/// The settings prefix.
pub const MAP_PREFIX: &str = "input.map";

/// A binding with its modifiers.
#[derive(Clone, Debug, PartialEq)]
pub struct BindingDef {
    pub binding: Binding,
    pub modifiers: Vec<Modifier>,
}

impl BindingDef {
    pub fn new(binding: Binding) -> Self {
        Self {
            binding,
            modifiers: Vec::new(),
        }
    }
    pub fn with(mut self, m: Modifier) -> Self {
        self.modifiers.push(m);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionDef {
    pub id: String,
    pub name: String,
    pub kind: ActionKind,
    /// By slot.
    pub bindings: BTreeMap<u32, BindingDef>,
    pub modifiers: Vec<Modifier>,
    pub triggers: Vec<Trigger>,
}

impl ActionDef {
    pub fn new(id: &str, kind: ActionKind) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            kind,
            bindings: BTreeMap::new(),
            modifiers: Vec::new(),
            triggers: Vec::new(),
        }
    }
    /// Add a binding in the next free slot.
    pub fn bind(mut self, b: BindingDef) -> Self {
        let slot = self.bindings.keys().next_back().map_or(0, |n| n + 1);
        self.bindings.insert(slot, b);
        self
    }
    pub fn modifier(mut self, m: Modifier) -> Self {
        self.modifiers.push(m);
        self
    }
    pub fn trigger(mut self, t: Trigger) -> Self {
        self.triggers.push(t);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextDef {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub consume: bool,
    pub actions: BTreeMap<String, ActionDef>,
}

impl ContextDef {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            priority: 0,
            consume: true,
            actions: BTreeMap::new(),
        }
    }
    pub fn priority(mut self, p: i32) -> Self {
        self.priority = p;
        self
    }
    pub fn action(mut self, a: ActionDef) -> Self {
        self.actions.insert(a.id.clone(), a);
        self
    }
}

/// The whole authored map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputMapDef {
    pub contexts: BTreeMap<String, ContextDef>,
}

impl InputMapDef {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn context(mut self, c: ContextDef) -> Self {
        self.contexts.insert(c.id.clone(), c);
        self
    }

    /// Read the map from project settings (`(key, text)` pairs; keys outside
    /// [`MAP_PREFIX`] are ignored). What does not parse is skipped and named in the second
    /// value, so one bad binding never loses the rest of the map.
    pub fn from_settings<'a>(
        settings: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> (InputMapDef, Vec<String>) {
        let mut d = InputMapDef::default();
        let mut problems = Vec::new();
        let mut bindmods: Vec<(String, String, u32, String, String)> = Vec::new();
        for (key, text) in settings {
            let Some(rest) = key
                .strip_prefix(MAP_PREFIX)
                .and_then(|r| r.strip_prefix('.'))
            else {
                continue;
            };
            let parts: Vec<&str> = rest.split('.').collect();
            let Some(m) = parts.first().copied().filter(|m| !m.is_empty()) else {
                continue;
            };
            let ctx = d
                .contexts
                .entry(m.to_string())
                .or_insert_with(|| ContextDef::new(m));
            match parts.as_slice() {
                [_, "name"] => ctx.name = text.to_string(),
                [_, "priority"] => match text.trim().parse::<i32>() {
                    Ok(p) => ctx.priority = p,
                    Err(_) => problems.push(format!("{key}: {text:?} is not an integer")),
                },
                [_, "consume"] => match text.trim() {
                    "true" => ctx.consume = true,
                    "false" => ctx.consume = false,
                    o => problems.push(format!("{key}: {o:?} is not true or false")),
                },
                [_, "action", a, field @ ..] => {
                    let act = ctx
                        .actions
                        .entry((*a).to_string())
                        .or_insert_with(|| ActionDef::new(a, ActionKind::Button));
                    match field {
                        ["name"] => act.name = text.to_string(),
                        ["kind"] => match ActionKind::parse(text.trim()) {
                            Some(k) => act.kind = k,
                            None => {
                                problems.push(format!("{key}: unknown kind {text:?} (Button used)"))
                            }
                        },
                        ["modifiers"] => match Modifier::parse_chain(text) {
                            Ok(v) => act.modifiers = v,
                            Err(e) => problems.push(format!("{key}: {e}")),
                        },
                        ["triggers"] => match Trigger::parse_chain(text) {
                            Ok(v) => act.triggers = v,
                            Err(e) => problems.push(format!("{key}: {e}")),
                        },
                        ["bind", n] => match (n.parse::<u32>(), Binding::parse(text)) {
                            (Ok(n), Ok(b)) => {
                                let e = act.bindings.entry(n).or_insert_with(|| BindingDef {
                                    binding: b.clone(),
                                    modifiers: Vec::new(),
                                });
                                e.binding = b;
                            }
                            (Err(_), _) => {
                                problems.push(format!("{key}: a binding slot must be a number"));
                            }
                            (_, Err(e)) => problems.push(format!("{key}: {e}")),
                        },
                        ["bindmod", n] => match n.parse::<u32>() {
                            Ok(n) => bindmods.push((
                                m.to_string(),
                                (*a).to_string(),
                                n,
                                key.to_string(),
                                text.to_string(),
                            )),
                            Err(_) => {
                                problems.push(format!("{key}: a binding slot must be a number"));
                            }
                        },
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        for (m, a, n, key, text) in bindmods {
            let slot = d
                .contexts
                .get_mut(&m)
                .and_then(|c| c.actions.get_mut(&a))
                .and_then(|a| a.bindings.get_mut(&n));
            match (slot, Modifier::parse_chain(&text)) {
                (Some(b), Ok(v)) => b.modifiers = v,
                (None, _) => problems.push(format!("{key}: no binding in slot {n}")),
                (_, Err(e)) => problems.push(format!("{key}: {e}")),
            }
        }
        (d, problems)
    }

    /// The map as project settings (the inverse of [`InputMapDef::from_settings`]).
    pub fn to_settings(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for c in self.contexts.values() {
            let k = |f: &str| format!("{MAP_PREFIX}.{}.{f}", c.id);
            out.push((k("name"), c.name.clone()));
            if c.priority != 0 {
                out.push((k("priority"), c.priority.to_string()));
            }
            if !c.consume {
                out.push((k("consume"), "false".into()));
            }
            for a in c.actions.values() {
                let k = |f: &str| format!("{MAP_PREFIX}.{}.action.{}.{f}", c.id, a.id);
                out.push((k("name"), a.name.clone()));
                out.push((k("kind"), a.kind.name().into()));
                if !a.modifiers.is_empty() {
                    out.push((k("modifiers"), Modifier::chain_text(&a.modifiers)));
                }
                if !a.triggers.is_empty() {
                    out.push((k("triggers"), Trigger::chain_text(&a.triggers)));
                }
                for (n, b) in &a.bindings {
                    out.push((k(&format!("bind.{n}")), b.binding.to_string()));
                    if !b.modifiers.is_empty() {
                        out.push((
                            k(&format!("bindmod.{n}")),
                            Modifier::chain_text(&b.modifiers),
                        ));
                    }
                }
            }
        }
        out
    }
}

// ---- compiled -----------------------------------------------------------------------------

/// A compiled action, by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionHandle(pub u32);

/// A compiled context, by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContextHandle(pub u16);

/// Where a binding reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Control(Device, u16),
    Comp1([(Device, u16); 2]),
    Comp2([(Device, u16); 4]),
}

impl Source {
    pub(crate) fn from_binding(b: &Binding) -> Result<(Source, Shape), String> {
        let r = |c: &crate::control::ControlRef| -> Result<(Device, u16), String> {
            c.index()
                .map(|i| (c.device, i))
                .ok_or_else(|| format!("{c} is not a control the input layer knows"))
        };
        Ok(match b {
            Binding::Control(c) => {
                let (d, i) = r(c)?;
                (Source::Control(d, i), crate::control::control_shape(d, i))
            }
            Binding::Composite1D(a, b) => (Source::Comp1([r(a)?, r(b)?]), Shape::Axis1D),
            Binding::Composite2D([u, d, l, x]) => {
                (Source::Comp2([r(u)?, r(d)?, r(l)?, r(x)?]), Shape::Axis2D)
            }
        })
    }

    pub(crate) fn for_each(&self, mut f: impl FnMut(Device, u16)) {
        match self {
            Source::Control(d, i) => f(*d, *i),
            Source::Comp1(a) => a.iter().for_each(|(d, i)| f(*d, *i)),
            Source::Comp2(a) => a.iter().for_each(|(d, i)| f(*d, *i)),
        }
    }
}

/// A compiled binding.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CBinding {
    pub slot: u32,
    pub source: Source,
    pub shape: Shape,
    pub modifiers: Vec<Modifier>,
}

impl CBinding {
    pub(crate) fn compile(slot: u32, b: &BindingDef, kind: ActionKind) -> Result<Self, String> {
        b.binding.fits_tables(kind)?;
        let (source, shape) = Source::from_binding(&b.binding)?;
        Ok(CBinding {
            slot,
            source,
            shape,
            modifiers: b.modifiers.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CContext {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub consume: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CAction {
    pub path: String,
    pub name: String,
    pub kind: ActionKind,
    pub ctx: u16,
    pub modifiers: Vec<Modifier>,
    pub triggers: Vec<CTrigger>,
    pub any_explicit: bool,
    /// The authored bindings by slot.
    pub defaults: BTreeMap<u32, BindingDef>,
    /// Index of the first trigger state (per player, flattened).
    pub tstate: u32,
}

/// The compiled map (see the module docs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompiledMap {
    pub(crate) contexts: Vec<CContext>,
    pub(crate) actions: Vec<CAction>,
    /// Evaluation order: dependencies (chords, combos) first, then by context priority.
    pub(crate) order: Vec<u32>,
    pub(crate) index: BTreeMap<String, u32>,
    pub(crate) trigger_states: u32,
    /// What could not be compiled (skipped, never fatal).
    pub problems: Vec<String>,
}

impl CompiledMap {
    pub fn compile(def: &InputMapDef) -> CompiledMap {
        let mut m = CompiledMap::default();
        let mut ctxs: Vec<&ContextDef> = def.contexts.values().collect();
        ctxs.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
        for (ci, c) in ctxs.iter().enumerate() {
            let ci = u16::try_from(ci).unwrap_or(u16::MAX);
            m.contexts.push(CContext {
                id: c.id.clone(),
                name: c.name.clone(),
                priority: c.priority,
                consume: c.consume,
            });
            for a in c.actions.values() {
                let path = format!("{}/{}", c.id, a.id);
                let ix = u32::try_from(m.actions.len()).unwrap_or(u32::MAX);
                m.index.insert(path.clone(), ix);
                let mut defaults = BTreeMap::new();
                for (slot, b) in &a.bindings {
                    match CBinding::compile(*slot, b, a.kind) {
                        Ok(_) => {
                            defaults.insert(*slot, b.clone());
                        }
                        Err(e) => m.problems.push(format!("{path} slot {slot}: {e}")),
                    }
                }
                m.actions.push(CAction {
                    path,
                    name: a.name.clone(),
                    kind: a.kind,
                    ctx: ci,
                    modifiers: a.modifiers.clone(),
                    triggers: Vec::new(),
                    any_explicit: false,
                    defaults,
                    tstate: 0,
                });
            }
        }
        // Triggers, now that every action has an index.
        let mut tstate = 0u32;
        let mut deps: Vec<Vec<u32>> = vec![Vec::new(); m.actions.len()];
        for (ctx, c) in ctxs.iter().enumerate() {
            for a in c.actions.values() {
                let path = format!("{}/{}", c.id, a.id);
                let Some(&ix) = m.index.get(&path) else {
                    continue;
                };
                let mut ts = Vec::new();
                for t in &a.triggers {
                    let resolve = |p: &str| -> Option<u32> {
                        // `map/action`, or a bare action id in the same context.
                        m.index
                            .get(p)
                            .copied()
                            .or_else(|| m.index.get(&format!("{}/{p}", ctxs[ctx].id)).copied())
                    };
                    let ct = match t {
                        Trigger::Down => Some(CTrigger::Down),
                        Trigger::Pressed => Some(CTrigger::Pressed),
                        Trigger::Released => Some(CTrigger::Released),
                        Trigger::Hold { time, repeat } => Some(CTrigger::Hold {
                            time: *time,
                            repeat: *repeat,
                        }),
                        Trigger::Tap { max } => Some(CTrigger::Tap { max: *max }),
                        Trigger::MultiTap { count, gap } => Some(CTrigger::MultiTap {
                            count: *count,
                            gap: *gap,
                        }),
                        Trigger::Pulse { interval } => Some(CTrigger::Pulse {
                            interval: *interval,
                        }),
                        Trigger::Chord(p) => resolve(p).map(CTrigger::Chord),
                        Trigger::Combo(steps) => steps
                            .iter()
                            .map(|(p, w)| resolve(p).map(|i| (i, *w)))
                            .collect::<Option<Vec<_>>>()
                            .map(CTrigger::Combo),
                    };
                    match ct {
                        Some(ct) => {
                            match &ct {
                                CTrigger::Chord(d) => deps[ix as usize].push(*d),
                                CTrigger::Combo(s) => {
                                    deps[ix as usize].extend(s.iter().map(|(d, _)| *d));
                                }
                                _ => {}
                            }
                            ts.push(ct);
                        }
                        None => m
                            .problems
                            .push(format!("{path}: {t} names an action the map does not have")),
                    }
                }
                let any_explicit = ts.iter().any(|t| !t.implicit());
                if !any_explicit {
                    // No explicit trigger: fire while actuated.
                    ts.push(CTrigger::Down);
                }
                if let Some(act) = m.actions.get_mut(ix as usize) {
                    act.any_explicit = true;
                    act.tstate = tstate;
                    tstate += u32::try_from(ts.len()).unwrap_or(0);
                    act.triggers = ts;
                }
            }
        }
        m.trigger_states = tstate;
        // Order: Kahn over the dependencies, the ready action of the highest-priority context
        // (lowest index) first.
        let n = m.actions.len();
        let mut indeg: Vec<usize> = deps.iter().map(Vec::len).collect();
        let mut users: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (a, ds) in deps.iter().enumerate() {
            for d in ds {
                if let Some(u) = users.get_mut(*d as usize) {
                    u.push(u32::try_from(a).unwrap_or(u32::MAX));
                }
            }
        }
        let mut done = vec![false; n];
        while m.order.len() < n {
            let next = (0..n).find(|&i| !done[i] && indeg[i] == 0);
            let i = match next {
                Some(i) => i,
                None => {
                    // A cycle: evaluate the rest in index order (chords read last frame).
                    let i = (0..n).find(|&i| !done[i]).unwrap_or(0);
                    m.problems.push(format!(
                        "{}: its chords/combos form a cycle; it reads the other actions' previous frame",
                        m.actions[i].path
                    ));
                    i
                }
            };
            done[i] = true;
            m.order.push(u32::try_from(i).unwrap_or(u32::MAX));
            for u in &users[i] {
                if let Some(d) = indeg.get_mut(*u as usize) {
                    *d = d.saturating_sub(1);
                }
            }
        }
        m
    }

    /// An action by `map/action` path.
    pub fn handle(&self, path: &str) -> Option<ActionHandle> {
        self.index.get(path).map(|&i| ActionHandle(i))
    }
    /// A context by id.
    pub fn context(&self, id: &str) -> Option<ContextHandle> {
        self.contexts
            .iter()
            .position(|c| c.id == id)
            .and_then(|i| u16::try_from(i).ok())
            .map(ContextHandle)
    }
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }
    pub fn context_count(&self) -> usize {
        self.contexts.len()
    }
    /// An action's `map/action` path.
    pub fn path(&self, h: ActionHandle) -> &str {
        self.actions
            .get(h.0 as usize)
            .map_or("", |a| a.path.as_str())
    }
    /// An action's display name.
    pub fn name(&self, h: ActionHandle) -> &str {
        self.actions
            .get(h.0 as usize)
            .map_or("", |a| a.name.as_str())
    }
    pub fn kind(&self, h: ActionHandle) -> ActionKind {
        self.actions
            .get(h.0 as usize)
            .map_or(ActionKind::Button, |a| a.kind)
    }
    /// The context an action belongs to.
    pub fn context_of(&self, h: ActionHandle) -> ContextHandle {
        ContextHandle(self.actions.get(h.0 as usize).map_or(0, |a| a.ctx))
    }
    pub fn context_id(&self, c: ContextHandle) -> &str {
        self.contexts
            .get(usize::from(c.0))
            .map_or("", |c| c.id.as_str())
    }
    /// The authored (default) bindings of an action.
    pub fn defaults(&self, h: ActionHandle) -> Option<&BTreeMap<u32, BindingDef>> {
        self.actions.get(h.0 as usize).map(|a| &a.defaults)
    }
    /// Every action handle, in authored order.
    pub fn actions(&self) -> impl Iterator<Item = ActionHandle> + '_ {
        (0..self.actions.len()).filter_map(|i| u32::try_from(i).ok().map(ActionHandle))
    }
    /// The actions of one context.
    pub fn actions_of(&self, c: ContextHandle) -> impl Iterator<Item = ActionHandle> + '_ {
        self.actions
            .iter()
            .enumerate()
            .filter(move |(_, a)| a.ctx == c.0)
            .filter_map(|(i, _)| u32::try_from(i).ok().map(ActionHandle))
    }
}

/// A binding override file's form (§28.13): per action path, slot → binding text or
/// `None` (unbound by the player).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverridesFile {
    pub version: u32,
    pub bindings: BTreeMap<String, BTreeMap<u32, Option<String>>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlRef;

    fn sample() -> InputMapDef {
        let key = |k: &str| Binding::Control(ControlRef::new(Device::Keyboard, k));
        InputMapDef::new()
            .context(
                ContextDef::new("gameplay").action(
                    ActionDef::new("jump", ActionKind::Button)
                        .bind(BindingDef::new(key("Space")))
                        .trigger(Trigger::Hold {
                            time: 0.5,
                            repeat: false,
                        }),
                ),
            )
            .context(
                ContextDef::new("menu").priority(5).action(
                    ActionDef::new("move", ActionKind::Axis2D).bind(
                        BindingDef::new(Binding::Control(ControlRef::new(
                            Device::Gamepad,
                            "LeftStick",
                        )))
                        .with(Modifier::Deadzone {
                            kind: crate::modifier::DeadzoneKind::Radial,
                            lower: 0.2,
                            upper: 1.0,
                        }),
                    ),
                ),
            )
    }

    #[test]
    fn settings_round_trip() {
        let d = sample();
        let s = d.to_settings();
        let (back, problems) =
            InputMapDef::from_settings(s.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(back, d);
    }

    #[test]
    fn compile_orders_by_priority_and_resolves() {
        let m = CompiledMap::compile(&sample());
        assert!(m.problems.is_empty(), "{:?}", m.problems);
        assert_eq!(m.context_id(ContextHandle(0)), "menu");
        assert!(m.handle("gameplay/jump").is_some());
        assert!(m.handle("gameplay/nope").is_none());
    }
}
