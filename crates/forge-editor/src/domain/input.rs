//! The game's input action map (Ch.28, Ch.27; Ch.21 §21.21 "Input action map", DoD M2-68):
//! named action maps (contexts such as "Gameplay" or "Menus"), each with actions of a kind
//! (a button, a 1D axis, a 2D axis), their **default bindings** — what a player gets
//! before rebinding in the shipped game — and each action's modifiers and triggers. This is
//! project state and **distinct from the editor keymap** (`forge_editor::keymap`, user
//! config): nothing here touches the keymap, and editor chords never become game bindings.
//!
//! The vocabulary (devices, controls, bindings, modifiers, triggers) is `forge-input`'s: the
//! runtime the shipped game runs (Ch.28 §28.9–§28.18, DoD M7-12) reads exactly what this
//! panel writes ([`forge_input::InputMapDef::from_settings`] over the same keys).
//!
//! Project settings (see [`super`] and `forge_input::map`):
//!
//! | Key | Value |
//! |---|---|
//! | `input.map.<m>.name` | text |
//! | `input.map.<m>.priority` | integer text (higher contexts consume their controls first) |
//! | `input.map.<m>.action.<a>.name` | text |
//! | `input.map.<m>.action.<a>.kind` | text: `Button`, `Axis1D` or `Axis2D` |
//! | `input.map.<m>.action.<a>.bind.<n>` | text: a [`Binding`] (`Keyboard/Space`, `Gamepad/LeftStick`, `Composite2D(Keyboard/W, Keyboard/S, Keyboard/A, Keyboard/D)`) |
//! | `input.map.<m>.action.<a>.bindmod.<n>` | text: the binding's [`Modifier`] chain |
//! | `input.map.<m>.action.<a>.modifiers` | text: the action's [`Modifier`] chain |
//! | `input.map.<m>.action.<a>.triggers` | text: the action's [`Trigger`] chain |
//!
//! A binding must fit its action ([`Binding::fits`]): a button takes a button control, a 1D
//! axis a 1D control or a negative/positive pair of buttons, a 2D axis a stick or four
//! buttons. The same control bound to two actions of one map is a conflict, shown with
//! both action names ([`ActionMap::conflicts`]); the panel refuses to create one.
//!
//! **Backends.** [`DeviceInput`] is the real input layer: a `forge-input` runtime reading
//! gamepads through gilrs (started the first time a capture or the input debugger asks, so an
//! editor that never binds a pad starts no gamepad thread) plus any virtual devices a test
//! connects. [`MemoryInput`] is the labelled in-memory stand-in (D-4) tests inject into.

use std::cell::{Cell, RefCell, RefMut};
use std::collections::BTreeMap;
use std::fmt;

use forge_cmd::{EditorCommand, Value};
pub use forge_input::control::{ActionKind, Binding, Control, ControlRef, Device, Shape};
use forge_input::control::{control_name, controls_of};
pub use forge_input::{DebugSnapshot, InputMapDef, InputRuntime, Modifier, Trigger};
use forge_ui::{KeyCode, input::PointerButton};

use super::{BackendInfo, objects, set, sub_objects, text};
use crate::mirror::ProjectMirror;

pub const MAP: &str = "input.map";

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub id: String,
    pub name: String,
    pub kind: ActionKind,
    /// Bindings by slot index (the key's `<n>`).
    pub bindings: BTreeMap<u32, Binding>,
    /// Each binding's modifiers, by slot.
    pub bindmods: BTreeMap<u32, Vec<Modifier>>,
    pub modifiers: Vec<Modifier>,
    pub triggers: Vec<Trigger>,
}

impl Action {
    /// The next free binding slot.
    pub fn next_slot(&self) -> u32 {
        self.bindings.keys().next_back().map_or(0, |n| n + 1)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionMap {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub actions: BTreeMap<String, Action>,
}

/// A control bound to two actions of one map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub control: ControlRef,
    pub first: String,
    pub second: String,
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is bound to both \u{201c}{}\u{201d} and \u{201c}{}\u{201d}",
            self.control, self.first, self.second
        )
    }
}

impl ActionMap {
    /// Every control bound to more than one action, with both actions' names.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut by: BTreeMap<&ControlRef, Vec<&str>> = BTreeMap::new();
        for a in self.actions.values() {
            for b in a.bindings.values() {
                for c in b.controls() {
                    let v = by.entry(c).or_default();
                    if !v.contains(&a.name.as_str()) {
                        v.push(&a.name);
                    }
                }
            }
        }
        by.into_iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|(c, v)| Conflict {
                control: c.clone(),
                first: v[0].to_string(),
                second: v[1].to_string(),
            })
            .collect()
    }

    /// The conflict `binding` would create on `action`, if any.
    pub fn conflict_with(&self, action: &str, binding: &Binding) -> Option<Conflict> {
        let me = self.actions.get(action).map_or(action, |a| a.name.as_str());
        for a in self.actions.values() {
            if a.id == action {
                continue;
            }
            for b in a.bindings.values() {
                for c in b.controls() {
                    if binding.controls().contains(&c) {
                        return Some(Conflict {
                            control: c.clone(),
                            first: a.name.clone(),
                            second: me.to_string(),
                        });
                    }
                }
            }
        }
        None
    }
}

/// Every action map, read from the mirror.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputDoc {
    pub maps: BTreeMap<String, ActionMap>,
    pub problems: Vec<String>,
}

impl InputDoc {
    pub fn read(m: &ProjectMirror) -> InputDoc {
        let mut d = InputDoc::default();
        let p = &mut d.problems;
        for (id, f) in objects(m, MAP) {
            let at = format!("{MAP}.{id}");
            let mut actions = BTreeMap::new();
            for (aid, af) in sub_objects(&f, "action") {
                let aat = format!("{at}.action.{aid}");
                let kind_s = text(&af, "kind", "Button", p, &aat);
                let kind = ActionKind::parse(&kind_s).unwrap_or_else(|| {
                    p.push(format!("{aat}.kind: unknown kind {kind_s:?} (Button used)"));
                    ActionKind::Button
                });
                let mut bindings = BTreeMap::new();
                let mut bindmods = BTreeMap::new();
                for (k, v) in &af {
                    if let Some(n) = k.strip_prefix("bind.") {
                        let parsed = match (n.parse::<u32>(), v) {
                            (Ok(n), Value::Text(t)) => Binding::parse(t).map(|b| (n, b)),
                            (Err(_), _) => Err("a binding slot must be a number".into()),
                            (_, other) => Err(format!("expected text, found {}", other.kind())),
                        };
                        match parsed {
                            Ok((n, b)) => {
                                bindings.insert(n, b);
                            }
                            Err(e) => p.push(format!("{aat}.{k}: {e}")),
                        }
                    } else if let Some(n) = k.strip_prefix("bindmod.") {
                        let parsed = match (n.parse::<u32>(), v) {
                            (Ok(n), Value::Text(t)) => Modifier::parse_chain(t).map(|m| (n, m)),
                            (Err(_), _) => Err("a binding slot must be a number".into()),
                            (_, other) => Err(format!("expected text, found {}", other.kind())),
                        };
                        match parsed {
                            Ok((n, m)) => {
                                bindmods.insert(n, m);
                            }
                            Err(e) => p.push(format!("{aat}.{k}: {e}")),
                        }
                    }
                }
                let modifiers = Modifier::parse_chain(&text(&af, "modifiers", "", p, &aat))
                    .unwrap_or_else(|e| {
                        p.push(format!("{aat}.modifiers: {e}"));
                        Vec::new()
                    });
                let triggers = Trigger::parse_chain(&text(&af, "triggers", "", p, &aat))
                    .unwrap_or_else(|e| {
                        p.push(format!("{aat}.triggers: {e}"));
                        Vec::new()
                    });
                actions.insert(
                    aid.to_string(),
                    Action {
                        id: aid.to_string(),
                        name: text(&af, "name", aid, p, &aat),
                        kind,
                        bindings,
                        bindmods,
                        modifiers,
                        triggers,
                    },
                );
            }
            let priority = text(&f, "priority", "0", p, &at)
                .trim()
                .parse::<i32>()
                .unwrap_or_else(|_| {
                    p.push(format!("{at}.priority: not an integer (0 used)"));
                    0
                });
            d.maps.insert(
                id.to_string(),
                ActionMap {
                    id: id.to_string(),
                    name: text(&f, "name", id, p, &at),
                    priority,
                    actions,
                },
            );
        }
        d
    }
}

/// The project's input settings as `(key, text)` pairs — what the game's runtime reads
/// ([`InputMapDef::from_settings`]).
pub fn runtime_settings(m: &ProjectMirror) -> Vec<(String, String)> {
    m.settings_under(&format!("{MAP}."))
        .filter_map(|(k, v)| {
            let t = match v {
                Value::Text(t) => t.clone(),
                Value::Int(i) => i.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Float(f) => f.to_string(),
                _ => return None,
            };
            Some((k.to_string(), t))
        })
        .collect()
}

/// What the input-map editor and the input debugger ask of the input layer (Ch.28, Ch.27).
/// [`DeviceInput`] implements it over `forge-input`; [`MemoryInput`] is the labelled
/// in-memory stand-in (D-4).
pub trait InputActions {
    fn backend(&self) -> BackendInfo;
    /// The device classes the layer offers.
    fn devices(&self) -> Vec<Device>;
    /// The controls of a device class.
    fn controls(&self, device: Device) -> Vec<Control>;
    /// The control an editor key press stands for ("press to bind").
    fn control_for_key(&self, code: KeyCode) -> Option<Control>;
    /// The control an editor mouse button stands for.
    fn control_for_pointer(&self, button: PointerButton) -> Option<Control>;
    /// A control actuated on a device the editor window does not see (a gamepad button),
    /// once; `None` when nothing was pressed.
    fn poll_capture(&self) -> Option<Control>;
    /// The live runtime with the project's map loaded (`settings`: [`runtime_settings`]),
    /// for the input debugger; `None` when this layer has no runtime. With `poll`, the
    /// devices are read and one update runs first (the debugger's Refresh and Live); without
    /// it the runtime is only shown, so opening the debugger reads no device.
    fn debug_snapshot(&self, _settings: &[(String, String)], _poll: bool) -> Option<DebugSnapshot> {
        None
    }

    /// A control's shape, if the device has it.
    fn shape(&self, c: &ControlRef) -> Option<Shape> {
        self.controls(c.device)
            .into_iter()
            .find(|k| k.name == c.name)
            .map(|k| k.shape)
    }
}

fn key_control(name: &str) -> Control {
    Control {
        device: Device::Keyboard,
        name: name.to_string(),
        shape: Shape::Button,
    }
}

/// The game control an editor key stands for (both backends).
pub fn control_for_key(code: KeyCode) -> Option<Control> {
    let name = match code {
        KeyCode::Char(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        KeyCode::Char(c) if c.is_ascii_digit() => format!("Digit{c}"),
        KeyCode::Char(' ') | KeyCode::Space => "Space".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Escape => "Escape".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Insert => "Insert".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::F1 => "F1".into(),
        KeyCode::F2 => "F2".into(),
        KeyCode::F3 => "F3".into(),
        KeyCode::F4 => "F4".into(),
        KeyCode::F5 => "F5".into(),
        KeyCode::F6 => "F6".into(),
        KeyCode::F7 => "F7".into(),
        KeyCode::F8 => "F8".into(),
        KeyCode::F9 => "F9".into(),
        KeyCode::F10 => "F10".into(),
        KeyCode::F11 => "F11".into(),
        KeyCode::F12 => "F12".into(),
        _ => return None,
    };
    Some(key_control(&name))
}

/// The game control an editor mouse button stands for (both backends).
pub fn control_for_pointer(button: PointerButton) -> Option<Control> {
    let name = match button {
        PointerButton::Primary => "Left",
        PointerButton::Secondary => "Right",
        PointerButton::Middle => "Middle",
    };
    Some(Control {
        device: Device::Mouse,
        name: name.into(),
        shape: Shape::Button,
    })
}

/// The in-memory input layer (D-4): the standard keyboard, mouse, gamepad and touch
/// controls. Key and mouse captures map the editor window's events; nothing reads a device,
/// so [`MemoryInput::inject`] stands in for a pad press in tests.
#[derive(Debug, Default)]
pub struct MemoryInput {
    injected: RefCell<Option<Control>>,
}

impl MemoryInput {
    pub fn new() -> Self {
        Self::default()
    }
    /// Stand in for a control actuated on a device (the next `poll_capture` returns it).
    pub fn inject(&self, c: Control) {
        *self.injected.borrow_mut() = Some(c);
    }
}

impl InputActions for MemoryInput {
    fn backend(&self) -> BackendInfo {
        BackendInfo {
            name: "in-memory input".into(),
            in_memory: true,
            note: "In-memory input layer (D-4): the standard keyboard, mouse, gamepad and touch controls; it reads no device.".into(),
        }
    }
    fn devices(&self) -> Vec<Device> {
        Device::ALL.to_vec()
    }
    fn controls(&self, device: Device) -> Vec<Control> {
        controls_of(device)
    }
    fn control_for_key(&self, code: KeyCode) -> Option<Control> {
        control_for_key(code)
    }
    fn control_for_pointer(&self, button: PointerButton) -> Option<Control> {
        control_for_pointer(button)
    }
    fn poll_capture(&self) -> Option<Control> {
        self.injected.borrow_mut().take()
    }
}

forge_trace::control_switches! {
    /// W2 fault switches of the device backend (`test_input_backend`'s positive control).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct DeviceInputFaults {
        /// A capture reads the runtime without pumping the devices (a gamepad press is never
        /// seen).
        pub no_pump: bool,
    }
}

/// The settings a runtime map was compiled from, and what did not parse.
type Loaded = (Vec<(String, String)>, Vec<String>);

/// The real input layer (see the module docs): a `forge-input` runtime, gamepads through
/// gilrs, and the virtual devices a test connects.
pub struct DeviceInput {
    rt: RefCell<InputRuntime>,
    pads: RefCell<Option<forge_input::backend::gilrs::GilrsBackend>>,
    /// Whether real gamepads are read (off for a test's virtual devices only).
    real: bool,
    /// The settings the runtime's map was compiled from.
    loaded: RefCell<Option<Loaded>>,
    last: Cell<Option<std::time::Instant>>,
    faults: DeviceInputFaults,
}

impl Default for DeviceInput {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceInput {
    /// Real gamepads (gilrs starts on the first capture or debugger refresh).
    pub fn new() -> Self {
        Self::build(true)
    }
    /// No real device: only what a test connects through [`DeviceInput::runtime`].
    pub fn virtual_only() -> Self {
        Self::build(false)
    }
    fn build(real: bool) -> Self {
        Self {
            rt: RefCell::new(InputRuntime::new()),
            pads: RefCell::new(None),
            real,
            loaded: RefCell::new(None),
            last: Cell::new(None),
            faults: DeviceInputFaults::default(),
        }
    }
    pub fn with_faults(mut self, f: DeviceInputFaults) -> Self {
        self.faults = f;
        self
    }
    /// The runtime (connect virtual devices, push events).
    pub fn runtime(&self) -> RefMut<'_, InputRuntime> {
        self.rt.borrow_mut()
    }

    /// Read the devices and run one update.
    fn pump(&self) {
        if self.faults.no_pump() {
            return;
        }
        let mut rt = self.rt.borrow_mut();
        if self.real {
            let mut pads = self.pads.borrow_mut();
            let pads = pads.get_or_insert_with(forge_input::backend::gilrs::GilrsBackend::new);
            pads.poll(&mut rt);
        }
        let now = std::time::Instant::now();
        let dt = self.last.get().map_or(1.0 / 60.0, |t| {
            now.duration_since(t).as_secs_f32().min(0.25)
        });
        self.last.set(Some(now));
        rt.update(dt);
    }
}

impl InputActions for DeviceInput {
    fn backend(&self) -> BackendInfo {
        let note = match self.pads.borrow().as_ref().map(|p| p.status()) {
            _ if !self.real => {
                "forge-input runtime with virtual devices only (no real device is read).".into()
            }
            None => "forge-input: gamepads are read through gilrs from the first capture or input-debugger refresh; keyboard and mouse come from the window.".into(),
            Some(Ok(n)) => format!("forge-input: {n} gamepad(s) connected (gilrs); keyboard and mouse come from the window."),
            Some(Err(e)) => format!("forge-input: gamepads cannot be read here ({e}); keyboard and mouse come from the window."),
        };
        BackendInfo {
            name: "forge-input".into(),
            in_memory: false,
            note,
        }
    }
    fn devices(&self) -> Vec<Device> {
        Device::ALL.to_vec()
    }
    fn controls(&self, device: Device) -> Vec<Control> {
        controls_of(device)
    }
    fn control_for_key(&self, code: KeyCode) -> Option<Control> {
        control_for_key(code)
    }
    fn control_for_pointer(&self, button: PointerButton) -> Option<Control> {
        control_for_pointer(button)
    }
    fn poll_capture(&self) -> Option<Control> {
        self.pump();
        let rt = self.rt.borrow();
        rt.actuated().iter().find_map(|&(dev, ctl)| {
            let class = rt.device(dev)?.class;
            // Keys and mouse buttons come through the editor window's own events.
            if matches!(class, Device::Keyboard | Device::Mouse) {
                return None;
            }
            let name = control_name(class, ctl);
            Some(Control {
                device: class,
                name: name.to_string(),
                shape: forge_input::control::control_shape(class, ctl),
            })
        })
    }
    fn debug_snapshot(&self, settings: &[(String, String)], poll: bool) -> Option<DebugSnapshot> {
        let changed = self
            .loaded
            .borrow()
            .as_ref()
            .is_none_or(|(s, _)| s.as_slice() != settings);
        if changed {
            let (def, problems) =
                InputMapDef::from_settings(settings.iter().map(|(k, v)| (k.as_str(), v.as_str())));
            let mut rt = self.rt.borrow_mut();
            rt.set_map(&def);
            rt.set_event_log(64);
            *self.loaded.borrow_mut() = Some((settings.to_vec(), problems));
        }
        if poll {
            self.pump();
        }
        let mut s = self.rt.borrow().debug_snapshot();
        if let Some((_, p)) = self.loaded.borrow().as_ref() {
            s.problems.splice(0..0, p.iter().cloned());
        }
        Some(s)
    }
}

// ---- edits --------------------------------------------------------------------------------

pub fn map_key(map: &str, field: &str) -> String {
    format!("{MAP}.{map}.{field}")
}

pub fn action_key(map: &str, action: &str, field: &str) -> String {
    format!("{MAP}.{map}.action.{action}.{field}")
}

/// A new action map.
pub fn new_map(doc: &InputDoc, name: &str) -> (String, Vec<EditorCommand>) {
    let id = super::unique_id(&super::ident_from(name), |s| doc.maps.contains_key(s));
    (
        id.clone(),
        vec![set(map_key(&id, "name"), Value::Text(name.into()))],
    )
}

/// A new action in `map`.
pub fn new_action(map: &ActionMap, name: &str, kind: ActionKind) -> (String, Vec<EditorCommand>) {
    let id = super::unique_id(&super::ident_from(name), |s| map.actions.contains_key(s));
    let cmds = vec![
        set(action_key(&map.id, &id, "name"), Value::Text(name.into())),
        set(
            action_key(&map.id, &id, "kind"),
            Value::Text(kind.name().into()),
        ),
    ];
    (id, cmds)
}

/// Add `binding` to an action, refusing one that does not fit or that another action of
/// the map already uses (the reason names both actions).
pub fn add_binding(
    map: &ActionMap,
    action: &str,
    binding: &Binding,
    backend: &dyn InputActions,
) -> Result<EditorCommand, String> {
    let a = map
        .actions
        .get(action)
        .ok_or_else(|| format!("no action {action:?} in {:?}", map.name))?;
    binding.fits(a.kind, |c| backend.shape(c))?;
    if a.bindings.values().any(|b| b == binding) {
        return Err(format!("\u{201c}{}\u{201d} already has {binding}", a.name));
    }
    if let Some(c) = map.conflict_with(action, binding) {
        return Err(c.to_string());
    }
    Ok(set(
        action_key(&map.id, action, &format!("bind.{}", a.next_slot())),
        Value::Text(binding.to_string()),
    ))
}

/// Set (or, with empty text, clear) an action's trigger chain. The text is parsed with the
/// runtime's parser, and every action a chord or combo names must exist in the project.
pub fn set_triggers(
    doc: &InputDoc,
    map: &ActionMap,
    action: &str,
    text: &str,
) -> Result<EditorCommand, String> {
    let a = map
        .actions
        .get(action)
        .ok_or_else(|| format!("no action {action:?} in {:?}", map.name))?;
    let key = action_key(&map.id, action, "triggers");
    if text.trim().is_empty() {
        return Ok(super::clear(key));
    }
    let t = Trigger::parse_chain(text)?;
    for trig in &t {
        for dep in trig.depends_on() {
            let (m, x) = dep.split_once('/').unwrap_or((map.id.as_str(), dep));
            if !doc.maps.get(m).is_some_and(|mp| mp.actions.contains_key(x)) {
                return Err(format!(
                    "{trig} names {dep}, which is not an action of this project"
                ));
            }
            if m == map.id && x == a.id {
                return Err(format!("{trig} names the action itself"));
            }
        }
    }
    Ok(set(key, Value::Text(Trigger::chain_text(&t))))
}

/// Set (or clear) an action's modifier chain (applied after its bindings combine).
pub fn set_modifiers(map: &ActionMap, action: &str, text: &str) -> Result<EditorCommand, String> {
    if !map.actions.contains_key(action) {
        return Err(format!("no action {action:?} in {:?}", map.name));
    }
    let key = action_key(&map.id, action, "modifiers");
    if text.trim().is_empty() {
        return Ok(super::clear(key));
    }
    let m = Modifier::parse_chain(text)?;
    Ok(set(key, Value::Text(Modifier::chain_text(&m))))
}

/// Set (or clear) one binding's modifier chain.
pub fn set_binding_modifiers(
    map: &ActionMap,
    action: &str,
    slot: u32,
    text: &str,
) -> Result<EditorCommand, String> {
    let a = map
        .actions
        .get(action)
        .ok_or_else(|| format!("no action {action:?} in {:?}", map.name))?;
    if !a.bindings.contains_key(&slot) {
        return Err(format!("{} has no binding in slot {slot}", a.name));
    }
    let key = action_key(&map.id, action, &format!("bindmod.{slot}"));
    if text.trim().is_empty() {
        return Ok(super::clear(key));
    }
    let m = Modifier::parse_chain(text)?;
    Ok(set(key, Value::Text(Modifier::chain_text(&m))))
}

/// Set a map's priority (higher maps consume their controls first).
pub fn set_priority(map: &ActionMap, priority: i32) -> EditorCommand {
    set(
        map_key(&map.id, "priority"),
        Value::Text(priority.to_string()),
    )
}

/// A binding row's label: the binding and its modifiers.
pub fn binding_label(b: &Binding, mods: Option<&Vec<Modifier>>) -> String {
    match mods {
        Some(m) if !m.is_empty() => format!("{b} | {}", Modifier::chain_text(m)),
        _ => b.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cref(s: &str) -> ControlRef {
        ControlRef::parse(s).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn bindings_round_trip_and_fit_their_kind() {
        let be = MemoryInput::new();
        for s in [
            "Keyboard/Space",
            "Gamepad/LeftStick",
            "Composite1D(Keyboard/A, Keyboard/D)",
            "Composite2D(Keyboard/W, Keyboard/S, Keyboard/A, Keyboard/D)",
        ] {
            let b = Binding::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"));
            assert_eq!(b.to_string(), s);
        }
        let shape = |c: &ControlRef| be.shape(c);
        assert!(
            Binding::parse("Keyboard/Space").map(|b| b.fits(ActionKind::Button, shape).is_ok())
                == Ok(true)
        );
        let stick = Binding::Control(cref("Gamepad/LeftStick"));
        assert!(stick.fits(ActionKind::Button, shape).is_err());
        assert!(stick.fits(ActionKind::Axis2D, shape).is_ok());
        assert!(
            Binding::Control(cref("Keyboard/Nope"))
                .fits(ActionKind::Button, shape)
                .is_err()
        );
        assert!(Binding::parse("Composite1D(Keyboard/A)").is_err());
        assert!(Binding::parse("Wheel/Up").is_err());
    }

    #[test]
    fn conflicts_name_both_actions() {
        let mut map = ActionMap {
            id: "gameplay".into(),
            name: "Gameplay".into(),
            priority: 0,
            actions: BTreeMap::new(),
        };
        for (id, name) in [("jump", "Jump"), ("fire", "Fire")] {
            map.actions.insert(
                id.into(),
                Action {
                    id: id.into(),
                    name: name.into(),
                    kind: ActionKind::Button,
                    bindings: BTreeMap::new(),
                    bindmods: BTreeMap::new(),
                    modifiers: Vec::new(),
                    triggers: Vec::new(),
                },
            );
        }
        if let Some(a) = map.actions.get_mut("jump") {
            a.bindings
                .insert(0, Binding::Control(cref("Keyboard/Space")));
        }
        let be = MemoryInput::new();
        let e = add_binding(&map, "fire", &Binding::Control(cref("Keyboard/Space")), &be)
            .err()
            .unwrap_or_default();
        assert!(e.contains("Jump") && e.contains("Fire"), "{e}");
        assert!(add_binding(&map, "fire", &Binding::Control(cref("Mouse/Left")), &be).is_ok());
        if let Some(a) = map.actions.get_mut("fire") {
            a.bindings
                .insert(0, Binding::Control(cref("Keyboard/Space")));
        }
        let c = map.conflicts();
        assert_eq!(c.len(), 1);
        assert_eq!(
            (c[0].first.as_str(), c[0].second.as_str()),
            ("Fire", "Jump")
        );
    }

    #[test]
    fn key_capture_maps_editor_keys_to_game_controls() {
        let be = MemoryInput::new();
        assert_eq!(
            be.control_for_key(KeyCode::Char('w')).map(|c| c.path()),
            Some("Keyboard/W".into())
        );
        assert_eq!(
            be.control_for_key(KeyCode::Space).map(|c| c.path()),
            Some("Keyboard/Space".into())
        );
        assert!(be.control_for_key(KeyCode::Other).is_none());
        for d in be.devices() {
            for c in be.controls(d) {
                assert_eq!(be.shape(&cref(&c.path())), Some(c.shape), "{}", c.path());
            }
        }
    }
}
