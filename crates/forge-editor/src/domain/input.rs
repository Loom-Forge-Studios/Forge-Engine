//! The game's input action map (Ch.28, Ch.27; Ch.21 §21.21 "Input action map", DoD M2-68):
//! named action maps (contexts such as "Gameplay" or "Menus"), each with actions of a kind
//! (a button, a 1D axis, a 2D axis) and their **default bindings** — what a player gets
//! before rebinding in the shipped game. This is project state and **distinct from the
//! editor keymap** (`forge_editor::keymap`, user config): nothing here touches the keymap,
//! and editor chords never become game bindings.
//!
//! Project settings (see [`super`]):
//!
//! | Key | Value |
//! |---|---|
//! | `input.map.<m>.name` | text |
//! | `input.map.<m>.action.<a>.name` | text |
//! | `input.map.<m>.action.<a>.kind` | text: `Button`, `Axis1D` or `Axis2D` |
//! | `input.map.<m>.action.<a>.bind.<n>` | text: a [`Binding`] (`Keyboard/Space`, `Gamepad/LeftStick`, `Composite2D(Keyboard/W, Keyboard/S, Keyboard/A, Keyboard/D)`) |
//!
//! A binding must fit its action ([`Binding::fits`]): a button takes a button control, a 1D
//! axis a 1D control or a negative/positive pair of buttons, a 2D axis a stick or four
//! buttons. The same control bound to two actions of one map is a conflict, shown with
//! both action names ([`ActionMap::conflicts`]); the panel refuses to create one.

use std::collections::BTreeMap;
use std::fmt;

use forge_cmd::{EditorCommand, Value};
use forge_ui::{KeyCode, input::PointerButton};

use super::{BackendInfo, objects, set, sub_objects, text};
use crate::mirror::ProjectMirror;

pub const MAP: &str = "input.map";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActionKind {
    Button,
    Axis1D,
    Axis2D,
}

impl ActionKind {
    pub const ALL: [ActionKind; 3] = [ActionKind::Button, ActionKind::Axis1D, ActionKind::Axis2D];
    pub fn name(self) -> &'static str {
        match self {
            ActionKind::Button => "Button",
            ActionKind::Axis1D => "Axis1D",
            ActionKind::Axis2D => "Axis2D",
        }
    }
    pub fn parse(s: &str) -> Option<ActionKind> {
        ActionKind::ALL.into_iter().find(|k| k.name() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Device {
    Keyboard,
    Mouse,
    Gamepad,
}

impl Device {
    pub const ALL: [Device; 3] = [Device::Keyboard, Device::Mouse, Device::Gamepad];
    pub fn name(self) -> &'static str {
        match self {
            Device::Keyboard => "Keyboard",
            Device::Mouse => "Mouse",
            Device::Gamepad => "Gamepad",
        }
    }
    pub fn parse(s: &str) -> Option<Device> {
        Device::ALL.into_iter().find(|d| d.name() == s)
    }
}

/// The value shape a control produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shape {
    Button,
    Axis1D,
    Axis2D,
}

/// One physical control (`Gamepad/LeftStick`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Control {
    pub device: Device,
    pub name: String,
    pub shape: Shape,
}

impl Control {
    pub fn path(&self) -> String {
        format!("{}/{}", self.device.name(), self.name)
    }
}

/// A reference to a control by path (its shape comes from the backend's control list).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControlRef {
    pub device: Device,
    pub name: String,
}

impl fmt::Display for ControlRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.device.name(), self.name)
    }
}

impl ControlRef {
    pub fn parse(s: &str) -> Result<ControlRef, String> {
        let (d, n) = s
            .trim()
            .split_once('/')
            .ok_or_else(|| format!("{s:?} is not Device/Control"))?;
        let device = Device::parse(d.trim()).ok_or_else(|| format!("unknown device {d:?}"))?;
        let name = n.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("bad control name {n:?}"));
        }
        Ok(ControlRef {
            device,
            name: name.to_string(),
        })
    }
}

/// One default binding.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Binding {
    Control(ControlRef),
    /// Two buttons making a 1D axis: negative, positive.
    Composite1D(ControlRef, ControlRef),
    /// Four buttons making a 2D axis: up, down, left, right.
    Composite2D([ControlRef; 4]),
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Binding::Control(c) => write!(f, "{c}"),
            Binding::Composite1D(a, b) => write!(f, "Composite1D({a}, {b})"),
            Binding::Composite2D([u, d, l, r]) => write!(f, "Composite2D({u}, {d}, {l}, {r})"),
        }
    }
}

impl Binding {
    pub fn parse(s: &str) -> Result<Binding, String> {
        let s = s.trim();
        let parts = |inner: &str| -> Result<Vec<ControlRef>, String> {
            inner.split(',').map(ControlRef::parse).collect()
        };
        if let Some(inner) = s
            .strip_prefix("Composite1D(")
            .and_then(|r| r.strip_suffix(')'))
        {
            let p = parts(inner)?;
            return match <[ControlRef; 2]>::try_from(p) {
                Ok([a, b]) => Ok(Binding::Composite1D(a, b)),
                Err(p) => Err(format!("Composite1D takes 2 buttons, got {}", p.len())),
            };
        }
        if let Some(inner) = s
            .strip_prefix("Composite2D(")
            .and_then(|r| r.strip_suffix(')'))
        {
            let p = parts(inner)?;
            return match <[ControlRef; 4]>::try_from(p) {
                Ok(a) => Ok(Binding::Composite2D(a)),
                Err(p) => Err(format!("Composite2D takes 4 buttons, got {}", p.len())),
            };
        }
        ControlRef::parse(s).map(Binding::Control)
    }

    /// The controls this binding reads.
    pub fn controls(&self) -> Vec<&ControlRef> {
        match self {
            Binding::Control(c) => vec![c],
            Binding::Composite1D(a, b) => vec![a, b],
            Binding::Composite2D(a) => a.iter().collect(),
        }
    }

    /// Can this binding drive an action of `kind`? `shape` finds a control's shape (the
    /// backend's control list); an unknown control is refused.
    pub fn fits(
        &self,
        kind: ActionKind,
        shape: impl Fn(&ControlRef) -> Option<Shape>,
    ) -> Result<(), String> {
        let need = |c: &ControlRef, want: Shape| -> Result<(), String> {
            match shape(c) {
                None => Err(format!("{c} is not a control the input layer knows")),
                Some(s) if s == want => Ok(()),
                Some(s) => Err(format!("{c} is a {s:?}, not a {want:?}")),
            }
        };
        match (self, kind) {
            (Binding::Control(c), ActionKind::Button) => need(c, Shape::Button),
            (Binding::Control(c), ActionKind::Axis1D) => {
                // A button drives a 1D axis too (0 or 1: a trigger-like action).
                match shape(c) {
                    Some(Shape::Axis1D | Shape::Button) => Ok(()),
                    _ => need(c, Shape::Axis1D),
                }
            }
            (Binding::Control(c), ActionKind::Axis2D) => need(c, Shape::Axis2D),
            (Binding::Composite1D(a, b), ActionKind::Axis1D) => {
                need(a, Shape::Button)?;
                need(b, Shape::Button)
            }
            (Binding::Composite2D(a), ActionKind::Axis2D) => {
                a.iter().try_for_each(|c| need(c, Shape::Button))
            }
            (b, k) => Err(format!("{b} cannot drive a {} action", k.name())),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub id: String,
    pub name: String,
    pub kind: ActionKind,
    /// Bindings by slot index (the key's `<n>`).
    pub bindings: BTreeMap<u32, Binding>,
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
                for (k, v) in &af {
                    let Some(n) = k.strip_prefix("bind.") else {
                        continue;
                    };
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
                }
                actions.insert(
                    aid.to_string(),
                    Action {
                        id: aid.to_string(),
                        name: text(&af, "name", aid, p, &aat),
                        kind,
                        bindings,
                    },
                );
            }
            d.maps.insert(
                id.to_string(),
                ActionMap {
                    id: id.to_string(),
                    name: text(&f, "name", id, p, &at),
                    actions,
                },
            );
        }
        d
    }
}

/// What the input-map editor asks of the input layer (Ch.28, Ch.27). `forge-play`
/// (M5-8) implements it over the platform HAL; [`MemoryInput`] is the labelled in-memory
/// stand-in (D-4).
pub trait InputActions {
    fn backend(&self) -> BackendInfo;
    /// The devices present (the in-memory backend offers the three standard ones).
    fn devices(&self) -> Vec<Device>;
    /// The controls of a device.
    fn controls(&self, device: Device) -> Vec<Control>;
    /// The control an editor key press stands for ("press to bind").
    fn control_for_key(&self, code: KeyCode) -> Option<Control>;
    /// The control an editor mouse button stands for.
    fn control_for_pointer(&self, button: PointerButton) -> Option<Control>;
    /// A control actuated on a device the editor window does not see (a gamepad button),
    /// once; `None` when nothing was pressed.
    fn poll_capture(&self) -> Option<Control>;

    /// A control's shape, if the device has it.
    fn shape(&self, c: &ControlRef) -> Option<Shape> {
        self.controls(c.device)
            .into_iter()
            .find(|k| k.name == c.name)
            .map(|k| k.shape)
    }
}

const KEYS: &[&str] = &[
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "Digit0",
    "Digit1",
    "Digit2",
    "Digit3",
    "Digit4",
    "Digit5",
    "Digit6",
    "Digit7",
    "Digit8",
    "Digit9",
    "Space",
    "Enter",
    "Escape",
    "Tab",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Up",
    "Down",
    "Left",
    "Right",
    "LeftShift",
    "RightShift",
    "LeftCtrl",
    "RightCtrl",
    "LeftAlt",
    "RightAlt",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
];

const PAD: &[(&str, Shape)] = &[
    ("South", Shape::Button),
    ("East", Shape::Button),
    ("West", Shape::Button),
    ("North", Shape::Button),
    ("LeftShoulder", Shape::Button),
    ("RightShoulder", Shape::Button),
    ("LeftTrigger", Shape::Axis1D),
    ("RightTrigger", Shape::Axis1D),
    ("Select", Shape::Button),
    ("Start", Shape::Button),
    ("LeftStickPress", Shape::Button),
    ("RightStickPress", Shape::Button),
    ("DPadUp", Shape::Button),
    ("DPadDown", Shape::Button),
    ("DPadLeft", Shape::Button),
    ("DPadRight", Shape::Button),
    ("LeftStick", Shape::Axis2D),
    ("RightStick", Shape::Axis2D),
    ("LeftStickX", Shape::Axis1D),
    ("LeftStickY", Shape::Axis1D),
    ("RightStickX", Shape::Axis1D),
    ("RightStickY", Shape::Axis1D),
    ("DPad", Shape::Axis2D),
];

const MOUSE: &[(&str, Shape)] = &[
    ("Left", Shape::Button),
    ("Right", Shape::Button),
    ("Middle", Shape::Button),
    ("Back", Shape::Button),
    ("Forward", Shape::Button),
    ("Delta", Shape::Axis2D),
    ("Wheel", Shape::Axis1D),
];

/// The in-memory input layer (D-4): a standard keyboard, a mouse and a standard gamepad
/// layout. Key and mouse captures map the editor window's events; a gamepad capture needs
/// the real input layer, so [`MemoryInput::inject`] stands in for a pad press in tests.
#[derive(Debug, Default)]
pub struct MemoryInput {
    injected: std::cell::RefCell<Option<Control>>,
}

impl MemoryInput {
    pub fn new() -> Self {
        Self::default()
    }
    /// Stand in for a control actuated on a device (the next `poll_capture` returns it).
    pub fn inject(&self, c: Control) {
        *self.injected.borrow_mut() = Some(c);
    }
    fn key(name: &str) -> Control {
        Control {
            device: Device::Keyboard,
            name: name.to_string(),
            shape: Shape::Button,
        }
    }
}

impl InputActions for MemoryInput {
    fn backend(&self) -> BackendInfo {
        BackendInfo {
            name: "in-memory input".into(),
            in_memory: true,
            note: "In-memory input layer (D-4): the standard keyboard, mouse and gamepad controls; reading real gamepads needs forge-play (M5-8), not built yet.".into(),
        }
    }
    fn devices(&self) -> Vec<Device> {
        Device::ALL.to_vec()
    }
    fn controls(&self, device: Device) -> Vec<Control> {
        let mk = |list: &[(&str, Shape)]| {
            list.iter()
                .map(|(n, s)| Control {
                    device,
                    name: (*n).to_string(),
                    shape: *s,
                })
                .collect()
        };
        match device {
            Device::Keyboard => KEYS.iter().map(|k| Self::key(k)).collect(),
            Device::Mouse => mk(MOUSE),
            Device::Gamepad => mk(PAD),
        }
    }
    fn control_for_key(&self, code: KeyCode) -> Option<Control> {
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
        Some(Self::key(&name))
    }
    fn control_for_pointer(&self, button: PointerButton) -> Option<Control> {
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
    fn poll_capture(&self) -> Option<Control> {
        self.injected.borrow_mut().take()
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
