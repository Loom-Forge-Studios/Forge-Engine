//! Platform-aware button glyphs (Ch.28 §28.17): what a prompt shows for a binding — "Ⓐ"
//! on an Xbox pad, "✕" on a PlayStation one, "B" on a Nintendo one (whose south button is
//! B), the layout's own letter for a key (an AZERTY player sees "Z" where a QWERTY player sees
//! "W"), and "Cmd" / "Win" / "Super" for the meta key on each OS.
//!
//! A glyph is a short label plus an **icon key** (`pad.ps.south`, `key.w`) a game's icon
//! atlas can map to artwork; the label is the fallback when it has none. The pad family comes
//! from the USB vendor id a backend reports ([`PadFamily::from_ids`]), with the device name
//! as a second guess.

use std::collections::BTreeMap;

use crate::control::{Binding, ControlRef, Device, control_index};

/// A gamepad family (whose glyphs to show).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PadFamily {
    Xbox,
    PlayStation,
    Nintendo,
    /// Position names, for an unknown pad.
    #[default]
    Generic,
}

impl PadFamily {
    pub const ALL: [PadFamily; 4] = [
        PadFamily::Xbox,
        PadFamily::PlayStation,
        PadFamily::Nintendo,
        PadFamily::Generic,
    ];

    /// From a USB vendor id (Microsoft 0x045e, Sony 0x054c, Nintendo 0x057e), then the name.
    pub fn from_ids(vendor: Option<u16>, name: &str) -> PadFamily {
        match vendor {
            Some(0x045e) => PadFamily::Xbox,
            Some(0x054c) => PadFamily::PlayStation,
            Some(0x057e) => PadFamily::Nintendo,
            _ => {
                let n = name.to_ascii_lowercase();
                if n.contains("xbox") || n.contains("xinput") {
                    PadFamily::Xbox
                } else if n.contains("dualshock")
                    || n.contains("dualsense")
                    || n.contains("playstation")
                    || n.contains("ps4")
                    || n.contains("ps5")
                {
                    PadFamily::PlayStation
                } else if n.contains("nintendo") || n.contains("switch") || n.contains("joy-con") {
                    PadFamily::Nintendo
                } else {
                    PadFamily::Generic
                }
            }
        }
    }

    fn key(self) -> &'static str {
        match self {
            PadFamily::Xbox => "xbox",
            PadFamily::PlayStation => "ps",
            PadFamily::Nintendo => "nintendo",
            PadFamily::Generic => "generic",
        }
    }
}

/// The OS, for keyboard modifier names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    Mac,
    Web,
}

impl Platform {
    /// The platform this build runs on.
    pub fn current() -> Platform {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(target_arch = "wasm32") {
            Platform::Web
        } else {
            Platform::Linux
        }
    }
}

/// A binding's glyph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub label: String,
    pub icon: String,
}

/// Layout-aware key labels: the label each physical key produces on the player's layout,
/// learnt from the window's key events (winit's logical key; `backend::winit`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyLabels {
    labels: BTreeMap<u16, String>,
}

impl KeyLabels {
    /// The key at `physical` (a `KEYS` name) produced `text` on this layout.
    pub fn learn(&mut self, physical: &str, text: &str) {
        let t = text.trim();
        if t.is_empty() || t.chars().count() > 3 || t.chars().any(char::is_control) {
            return;
        }
        if let Some(i) = control_index(Device::Keyboard, physical) {
            self.labels.insert(i, t.to_uppercase());
        }
    }
    pub fn get(&self, physical: &str) -> Option<&str> {
        control_index(Device::Keyboard, physical)
            .and_then(|i| self.labels.get(&i))
            .map(String::as_str)
    }
}

/// How glyphs are chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphContext {
    pub family: PadFamily,
    pub platform: Platform,
    pub keys: KeyLabels,
}

impl Default for GlyphContext {
    fn default() -> Self {
        Self {
            family: PadFamily::Generic,
            platform: Platform::current(),
            keys: KeyLabels::default(),
        }
    }
}

fn pad_label(f: PadFamily, name: &str) -> &'static str {
    use PadFamily as F;
    match (f, name) {
        (F::Xbox, "South") => "A",
        (F::Xbox, "East") => "B",
        (F::Xbox, "West") => "X",
        (F::Xbox, "North") => "Y",
        (F::Xbox, "LeftShoulder") => "LB",
        (F::Xbox, "RightShoulder") => "RB",
        (F::Xbox, "LeftTrigger") => "LT",
        (F::Xbox, "RightTrigger") => "RT",
        (F::Xbox, "Start") => "Menu",
        (F::Xbox, "Select") => "View",
        (F::Xbox, "Guide") => "Xbox",
        (F::Xbox, "LeftStickPress") => "LS",
        (F::Xbox, "RightStickPress") => "RS",
        (F::PlayStation, "South") => "\u{2715}",
        (F::PlayStation, "East") => "\u{25cb}",
        (F::PlayStation, "West") => "\u{25a1}",
        (F::PlayStation, "North") => "\u{25b3}",
        (F::PlayStation, "LeftShoulder") => "L1",
        (F::PlayStation, "RightShoulder") => "R1",
        (F::PlayStation, "LeftTrigger") => "L2",
        (F::PlayStation, "RightTrigger") => "R2",
        (F::PlayStation, "Start") => "Options",
        (F::PlayStation, "Select") => "Create",
        (F::PlayStation, "Guide") => "PS",
        (F::PlayStation, "LeftStickPress") => "L3",
        (F::PlayStation, "RightStickPress") => "R3",
        (F::PlayStation, "Touchpad") => "Touchpad",
        (F::Nintendo, "South") => "B",
        (F::Nintendo, "East") => "A",
        (F::Nintendo, "West") => "Y",
        (F::Nintendo, "North") => "X",
        (F::Nintendo, "LeftShoulder") => "L",
        (F::Nintendo, "RightShoulder") => "R",
        (F::Nintendo, "LeftTrigger") => "ZL",
        (F::Nintendo, "RightTrigger") => "ZR",
        (F::Nintendo, "Start") => "+",
        (F::Nintendo, "Select") => "\u{2212}",
        (F::Nintendo, "Guide") => "Home",
        (F::Nintendo, "LeftStickPress") => "LS",
        (F::Nintendo, "RightStickPress") => "RS",
        (_, "DPadUp") => "\u{2191}",
        (_, "DPadDown") => "\u{2193}",
        (_, "DPadLeft") => "\u{2190}",
        (_, "DPadRight") => "\u{2192}",
        (_, "DPad") => "\u{271c}",
        (_, "LeftStick" | "LeftStickX" | "LeftStickY") => "L\u{25ce}",
        (_, "RightStick" | "RightStickX" | "RightStickY") => "R\u{25ce}",
        (_, "Gyro") => "Gyro",
        _ => "",
    }
}

fn key_label(name: &str, platform: Platform) -> String {
    let meta = match platform {
        Platform::Windows => "Win",
        Platform::Mac => "Cmd",
        Platform::Linux | Platform::Web => "Super",
    };
    let alt = if platform == Platform::Mac {
        "Option"
    } else {
        "Alt"
    };
    let ctrl = if platform == Platform::Mac {
        "Control"
    } else {
        "Ctrl"
    };
    match name {
        "LeftMeta" | "RightMeta" => meta.into(),
        "LeftAlt" | "RightAlt" => alt.into(),
        "LeftCtrl" | "RightCtrl" => ctrl.into(),
        "LeftShift" | "RightShift" => "Shift".into(),
        "Up" => "\u{2191}".into(),
        "Down" => "\u{2193}".into(),
        "Left" => "\u{2190}".into(),
        "Right" => "\u{2192}".into(),
        "Escape" => "Esc".into(),
        "Backspace" => "\u{232b}".into(),
        "Minus" => "-".into(),
        "Equal" => "=".into(),
        "BracketLeft" => "[".into(),
        "BracketRight" => "]".into(),
        "Backslash" => "\\".into(),
        "Semicolon" => ";".into(),
        "Quote" => "'".into(),
        "Backquote" => "`".into(),
        "Comma" => ",".into(),
        "Period" => ".".into(),
        "Slash" => "/".into(),
        n if n.starts_with("Digit") => n.trim_start_matches("Digit").into(),
        n if n.starts_with("Numpad") => format!("Num {}", n.trim_start_matches("Numpad")),
        n => n.into(),
    }
}

impl GlyphContext {
    /// One control's glyph.
    pub fn control(&self, c: &ControlRef) -> Glyph {
        let lower = c.name.to_ascii_lowercase();
        match c.device {
            Device::Gamepad => {
                let l = pad_label(self.family, &c.name);
                Glyph {
                    label: if l.is_empty() {
                        c.name.clone()
                    } else {
                        l.to_string()
                    },
                    icon: format!("pad.{}.{lower}", self.family.key()),
                }
            }
            Device::Keyboard => Glyph {
                label: self
                    .keys
                    .get(&c.name)
                    .map_or_else(|| key_label(&c.name, self.platform), str::to_string),
                icon: format!("key.{lower}"),
            },
            Device::Mouse => Glyph {
                label: match c.name.as_str() {
                    "Left" => "LMB".into(),
                    "Right" => "RMB".into(),
                    "Middle" => "MMB".into(),
                    "Back" => "Mouse 4".into(),
                    "Forward" => "Mouse 5".into(),
                    "Wheel" | "WheelX" => "Wheel".into(),
                    "Delta" => "Mouse".into(),
                    o => o.into(),
                },
                icon: format!("mouse.{lower}"),
            },
            Device::Touch => Glyph {
                label: c.name.clone(),
                icon: format!("touch.{lower}"),
            },
        }
    }

    /// A binding's label: a composite shows its parts (`W/A/S/D`).
    pub fn binding(&self, b: &Binding) -> String {
        match b {
            Binding::Control(c) => self.control(c).label,
            Binding::Composite1D(a, b) => {
                format!("{}/{}", self.control(a).label, self.control(b).label)
            }
            Binding::Composite2D([u, l0, l, r]) => format!(
                "{}/{}/{}/{}",
                self.control(u).label,
                self.control(l).label,
                self.control(l0).label,
                self.control(r).label
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_and_layouts() {
        let south = ControlRef::new(Device::Gamepad, "South");
        let mut g = GlyphContext {
            family: PadFamily::from_ids(Some(0x054c), ""),
            ..GlyphContext::default()
        };
        assert_eq!(g.control(&south).label, "\u{2715}");
        g.family = PadFamily::from_ids(None, "Nintendo Switch Pro Controller");
        assert_eq!(g.control(&south).label, "B");
        g.family = PadFamily::Xbox;
        assert_eq!(g.control(&south).label, "A");
        let w = ControlRef::new(Device::Keyboard, "W");
        assert_eq!(g.control(&w).label, "W");
        g.keys.learn("W", "z");
        assert_eq!(g.control(&w).label, "Z");
        g.platform = Platform::Mac;
        assert_eq!(
            g.control(&ControlRef::new(Device::Keyboard, "LeftMeta"))
                .label,
            "Cmd"
        );
    }
}
