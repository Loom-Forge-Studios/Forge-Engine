//! The control vocabulary (Ch.28 §28.10): device classes, their controls and the value shape
//! each control produces, and the authored [`Binding`] text form. The editor's input-map
//! panel (M2-68) and the runtime read the **same** types, so a binding authored in the editor
//! is exactly the binding the game evaluates.
//!
//! A control is addressed by class, not by instance (`Gamepad/South` is the south face button
//! of whichever gamepad the player owns, like Unity's `<Gamepad>/buttonSouth`). Every class
//! has a fixed control table; the runtime addresses a control by its index in that table
//! ([`control_index`]), so evaluation never compares strings.

use std::fmt;

/// What an action produces.
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

/// A device class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Device {
    Keyboard,
    Mouse,
    Gamepad,
    /// A touch screen: finger state and the recognised gestures (§28.15).
    Touch,
}

impl Device {
    pub const ALL: [Device; 4] = [
        Device::Keyboard,
        Device::Mouse,
        Device::Gamepad,
        Device::Touch,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Device::Keyboard => "Keyboard",
            Device::Mouse => "Mouse",
            Device::Gamepad => "Gamepad",
            Device::Touch => "Touch",
        }
    }
    pub fn parse(s: &str) -> Option<Device> {
        Device::ALL.into_iter().find(|d| d.name() == s)
    }
    /// Dense index (0..4), for per-class tables.
    pub fn index(self) -> usize {
        match self {
            Device::Keyboard => 0,
            Device::Mouse => 1,
            Device::Gamepad => 2,
            Device::Touch => 3,
        }
    }
}

/// The value shape a control produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shape {
    Button,
    Axis1D,
    Axis2D,
}

/// How a control's value behaves over a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// Holds its last value (keys, buttons, sticks, triggers).
    State,
    /// A per-frame delta: events add up within a frame and it reads zero the next frame
    /// (mouse motion, the wheel, finger motion, pinch, twist, gyro).
    Delta,
    /// A position (cursor, finger): state, but never a rebinding or join candidate.
    Position,
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

/// A reference to a control by path (its shape comes from the control table).
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
    pub fn new(device: Device, name: &str) -> ControlRef {
        ControlRef {
            device,
            name: name.to_string(),
        }
    }

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

    /// Its index in the class's control table, if the class has it.
    pub fn index(&self) -> Option<u16> {
        control_index(self.device, &self.name)
    }

    /// Its shape, if the class has it.
    pub fn shape(&self) -> Option<Shape> {
        self.index().map(|i| control_shape(self.device, i))
    }
}

/// One authored binding.
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

    /// [`Binding::fits`] against the built-in control tables.
    pub fn fits_tables(&self, kind: ActionKind) -> Result<(), String> {
        self.fits(kind, ControlRef::shape)
    }
}

// ---- the control tables -------------------------------------------------------------------

/// The keyboard, by physical key (US names; the glyph shows the layout's label, §28.17).
pub const KEYS: &[&str] = &[
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
    "LeftMeta",
    "RightMeta",
    "CapsLock",
    "Minus",
    "Equal",
    "BracketLeft",
    "BracketRight",
    "Backslash",
    "Semicolon",
    "Quote",
    "Backquote",
    "Comma",
    "Period",
    "Slash",
    "Numpad0",
    "Numpad1",
    "Numpad2",
    "Numpad3",
    "Numpad4",
    "Numpad5",
    "Numpad6",
    "Numpad7",
    "Numpad8",
    "Numpad9",
    "NumpadAdd",
    "NumpadSubtract",
    "NumpadMultiply",
    "NumpadDivide",
    "NumpadDecimal",
    "NumpadEnter",
    "ContextMenu",
];

/// The standard gamepad (the W3C / SDL "standard mapping" by position).
pub const PAD: &[(&str, Shape)] = &[
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
    ("Guide", Shape::Button),
    ("Touchpad", Shape::Button),
    // Degrees turned this frame (yaw, pitch), after calibration and the gyro space (§28.16).
    ("Gyro", Shape::Axis2D),
];

pub const MOUSE: &[(&str, Shape)] = &[
    ("Left", Shape::Button),
    ("Right", Shape::Button),
    ("Middle", Shape::Button),
    ("Back", Shape::Button),
    ("Forward", Shape::Button),
    ("Delta", Shape::Axis2D),
    ("Wheel", Shape::Axis1D),
    ("WheelX", Shape::Axis1D),
    ("Position", Shape::Axis2D),
];

pub const TOUCH: &[(&str, Shape)] = &[
    // Any finger down.
    ("Primary", Shape::Button),
    // The first finger (window px) and its motion this frame.
    ("Position", Shape::Axis2D),
    ("Delta", Shape::Axis2D),
    // Gestures (one frame each).
    ("Tap", Shape::Button),
    ("DoubleTap", Shape::Button),
    ("LongPress", Shape::Button),
    ("SwipeLeft", Shape::Button),
    ("SwipeRight", Shape::Button),
    ("SwipeUp", Shape::Button),
    ("SwipeDown", Shape::Button),
    // Two fingers: the spread's log-ratio, the twist (radians, counter-clockwise) and the
    // centroid's motion this frame.
    ("Pinch", Shape::Axis1D),
    ("Rotate", Shape::Axis1D),
    ("TwoFingerPan", Shape::Axis2D),
];

/// How many controls a class has.
pub fn control_count(d: Device) -> usize {
    match d {
        Device::Keyboard => KEYS.len(),
        Device::Mouse => MOUSE.len(),
        Device::Gamepad => PAD.len(),
        Device::Touch => TOUCH.len(),
    }
}

/// A control's name by index (`""` out of range).
pub fn control_name(d: Device, i: u16) -> &'static str {
    let i = usize::from(i);
    match d {
        Device::Keyboard => KEYS.get(i).copied().unwrap_or(""),
        Device::Mouse => MOUSE.get(i).map_or("", |c| c.0),
        Device::Gamepad => PAD.get(i).map_or("", |c| c.0),
        Device::Touch => TOUCH.get(i).map_or("", |c| c.0),
    }
}

/// A control's shape by index (`Button` out of range).
pub fn control_shape(d: Device, i: u16) -> Shape {
    let i = usize::from(i);
    match d {
        Device::Keyboard => Shape::Button,
        Device::Mouse => MOUSE.get(i).map_or(Shape::Button, |c| c.1),
        Device::Gamepad => PAD.get(i).map_or(Shape::Button, |c| c.1),
        Device::Touch => TOUCH.get(i).map_or(Shape::Button, |c| c.1),
    }
}

/// A control's frame behaviour.
pub fn control_behaviour(d: Device, i: u16) -> Behaviour {
    match (d, control_name(d, i)) {
        (Device::Mouse, "Delta" | "Wheel" | "WheelX")
        | (Device::Touch, "Delta" | "Pinch" | "Rotate" | "TwoFingerPan")
        | (Device::Gamepad, "Gyro") => Behaviour::Delta,
        (Device::Mouse | Device::Touch, "Position") => Behaviour::Position,
        _ => Behaviour::State,
    }
}

/// A control's index by name.
pub fn control_index(d: Device, name: &str) -> Option<u16> {
    let i = match d {
        Device::Keyboard => KEYS.iter().position(|k| *k == name),
        Device::Mouse => MOUSE.iter().position(|c| c.0 == name),
        Device::Gamepad => PAD.iter().position(|c| c.0 == name),
        Device::Touch => TOUCH.iter().position(|c| c.0 == name),
    }?;
    u16::try_from(i).ok()
}

/// Every control of a class.
pub fn controls_of(d: Device) -> Vec<Control> {
    (0..control_count(d))
        .filter_map(|i| u16::try_from(i).ok())
        .map(|i| Control {
            device: d,
            name: control_name(d, i).to_string(),
            shape: control_shape(d, i),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_index_both_ways() {
        for d in Device::ALL {
            for c in controls_of(d) {
                let r = ControlRef::parse(&c.path()).unwrap_or_else(|e| panic!("{e}"));
                assert_eq!(r.shape(), Some(c.shape), "{}", c.path());
                let i = r.index().unwrap_or(u16::MAX);
                assert_eq!(control_name(d, i), c.name);
            }
        }
        assert_eq!(
            control_behaviour(
                Device::Mouse,
                control_index(Device::Mouse, "Delta").unwrap_or(0)
            ),
            Behaviour::Delta
        );
    }

    #[test]
    fn bindings_round_trip() {
        for s in [
            "Keyboard/Space",
            "Gamepad/LeftStick",
            "Touch/DoubleTap",
            "Composite1D(Keyboard/A, Keyboard/D)",
            "Composite2D(Keyboard/W, Keyboard/S, Keyboard/A, Keyboard/D)",
        ] {
            let b = Binding::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"));
            assert_eq!(b.to_string(), s);
        }
        assert!(Binding::parse("Composite1D(Keyboard/A)").is_err());
        assert!(Binding::parse("Wheel/Up").is_err());
    }
}
