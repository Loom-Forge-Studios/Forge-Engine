//! Gamepads through gilrs (Ch.28 §28.9; MIT OR Apache-2.0): Windows.Gaming.Input on
//! Windows, evdev with udev hot-plug on Linux. gilrs' default filters (its own deadzone and
//! jitter filter) are off: forge-input applies the authored deadzones and curves to raw
//! values. Force feedback plays the runtime's [`crate::MotorCommand`]s as a strong and a weak
//! effect.
//!
//! A pad keeps its identity across a reconnect through its UUID (the runtime's stable key),
//! so a pad whose battery died goes back to its player when it comes back.

use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Replay, Ticks};
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs, GilrsBuilder};

use crate::control::{Device, control_index};
use crate::error::InputError;
use crate::glyph::PadFamily;
use crate::runtime::{DeviceDesc, DeviceId, InputRuntime, RawEvent};

struct Pad {
    gilrs: GamepadId,
    device: DeviceId,
    /// Left x, left y, right x, right y (the 2D sticks are sent whole).
    sticks: [f32; 4],
    /// Up, down, left, right.
    dpad: [bool; 4],
    ff: bool,
    effect: Option<gilrs::ff::Effect>,
}

/// The gilrs backend (see the module docs).
pub struct GilrsBackend {
    gilrs: Option<Gilrs>,
    error: Option<String>,
    pads: Vec<Pad>,
}

impl Default for GilrsBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// The runtime's name for a gilrs button (`None`: not part of the standard pad).
pub fn button_name(b: Button) -> Option<&'static str> {
    Some(match b {
        Button::South => "South",
        Button::East => "East",
        Button::North => "North",
        Button::West => "West",
        Button::LeftTrigger => "LeftShoulder",
        Button::RightTrigger => "RightShoulder",
        Button::LeftTrigger2 => "LeftTrigger",
        Button::RightTrigger2 => "RightTrigger",
        Button::Select => "Select",
        Button::Start => "Start",
        Button::Mode => "Guide",
        Button::LeftThumb => "LeftStickPress",
        Button::RightThumb => "RightStickPress",
        Button::DPadUp => "DPadUp",
        Button::DPadDown => "DPadDown",
        Button::DPadLeft => "DPadLeft",
        Button::DPadRight => "DPadRight",
        _ => return None,
    })
}

fn uuid_key(u: [u8; 16]) -> String {
    u.iter().map(|b| format!("{b:02x}")).collect()
}

impl GilrsBackend {
    /// Start gilrs. A platform it cannot read leaves the backend empty with the reason
    /// ([`GilrsBackend::status`]); the game runs on without gamepads.
    pub fn new() -> Self {
        match GilrsBuilder::new()
            .with_default_filters(false)
            .set_update_state(false)
            .build()
        {
            Ok(g) => Self {
                gilrs: Some(g),
                error: None,
                pads: Vec::new(),
            },
            Err(gilrs::Error::NotImplemented(g)) => Self {
                gilrs: Some(g),
                error: Some("gilrs does not support this platform".into()),
                pads: Vec::new(),
            },
            Err(e) => Self {
                gilrs: None,
                error: Some(e.to_string()),
                pads: Vec::new(),
            },
        }
    }

    /// The number of pads connected, or why gamepads cannot be read.
    pub fn status(&self) -> Result<usize, InputError> {
        match &self.error {
            Some(e) => Err(InputError::Backend(e.clone())),
            None => Ok(self.pads.len()),
        }
    }

    fn connect(&mut self, rt: &mut InputRuntime, id: GamepadId) {
        if self.pads.iter().any(|p| p.gilrs == id) {
            return;
        }
        let Some(g) = &self.gilrs else { return };
        let Some(pad) = g.connected_gamepad(id) else {
            return;
        };
        let name = pad.name().to_string();
        let family = PadFamily::from_ids(pad.vendor_id(), &name);
        let ff = pad.is_ff_supported();
        let device = rt.connect(DeviceDesc {
            class: Device::Gamepad,
            name,
            family,
            stable_key: format!("gilrs:{}", uuid_key(pad.uuid())),
            is_virtual: false,
        });
        self.pads.push(Pad {
            gilrs: id,
            device,
            sticks: [0.0; 4],
            dpad: [false; 4],
            ff,
            effect: None,
        });
    }

    /// Read every gilrs event since the last poll into the runtime (call once a frame,
    /// before [`InputRuntime::update`]).
    pub fn poll(&mut self, rt: &mut InputRuntime) {
        if self.gilrs.is_none() {
            return;
        }
        if self.pads.is_empty() {
            // Pads already connected when gilrs started report no Connected event.
            let ids: Vec<GamepadId> = self
                .gilrs
                .as_ref()
                .map(|g| g.gamepads().map(|(id, _)| id).collect())
                .unwrap_or_default();
            for id in ids {
                self.connect(rt, id);
            }
        }
        while let Some(ev) = self.gilrs.as_mut().and_then(Gilrs::next_event) {
            match ev.event {
                EventType::Connected => self.connect(rt, ev.id),
                EventType::Disconnected => {
                    if let Some(k) = self.pads.iter().position(|p| p.gilrs == ev.id) {
                        let p = self.pads.remove(k);
                        rt.disconnect(p.device);
                    }
                }
                EventType::ButtonChanged(b, v, _) => {
                    let Some(p) = self.pads.iter_mut().find(|p| p.gilrs == ev.id) else {
                        continue;
                    };
                    let dev = p.device;
                    if let Some(name) = button_name(b) {
                        send(rt, dev, name, [v, 0.0]);
                    }
                    let dir = match b {
                        Button::DPadUp => Some(0),
                        Button::DPadDown => Some(1),
                        Button::DPadLeft => Some(2),
                        Button::DPadRight => Some(3),
                        _ => None,
                    };
                    if let Some(d) = dir {
                        p.dpad[d] = v >= 0.5;
                        let x = f32::from(u8::from(p.dpad[3])) - f32::from(u8::from(p.dpad[2]));
                        let y = f32::from(u8::from(p.dpad[0])) - f32::from(u8::from(p.dpad[1]));
                        send(rt, dev, "DPad", [x, y]);
                    }
                }
                EventType::AxisChanged(a, v, _) => {
                    let Some(p) = self.pads.iter_mut().find(|p| p.gilrs == ev.id) else {
                        continue;
                    };
                    let dev = p.device;
                    match a {
                        Axis::LeftStickX | Axis::LeftStickY => {
                            let k = usize::from(a == Axis::LeftStickY);
                            p.sticks[k] = v;
                            send(rt, dev, ["LeftStickX", "LeftStickY"][k], [v, 0.0]);
                            send(rt, dev, "LeftStick", [p.sticks[0], p.sticks[1]]);
                        }
                        Axis::RightStickX | Axis::RightStickY => {
                            let k = usize::from(a == Axis::RightStickY);
                            p.sticks[2 + k] = v;
                            send(rt, dev, ["RightStickX", "RightStickY"][k], [v, 0.0]);
                            send(rt, dev, "RightStick", [p.sticks[2], p.sticks[3]]);
                        }
                        // Some drivers report the analogue triggers as Z axes (-1..1 or 0..1).
                        Axis::LeftZ => send(rt, dev, "LeftTrigger", [v.max(0.0), 0.0]),
                        Axis::RightZ => send(rt, dev, "RightTrigger", [v.max(0.0), 0.0]),
                        Axis::DPadX | Axis::DPadY => {
                            let (x, y) = if a == Axis::DPadX { (v, 0.0) } else { (0.0, v) };
                            send(rt, dev, "DPad", [x, y]);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Play the runtime's changed motor levels (call after [`InputRuntime::update`]).
    pub fn apply_haptics(&mut self, rt: &InputRuntime) {
        let Some(g) = self.gilrs.as_mut() else { return };
        for c in rt.motor_commands() {
            let Some(p) = self.pads.iter_mut().find(|p| p.device == c.device) else {
                continue;
            };
            if !p.ff {
                continue;
            }
            // Dropping the old effect stops it.
            p.effect = None;
            if c.low <= 0.0 && c.high <= 0.0 {
                continue;
            }
            let mag = |x: f32| (x.clamp(0.0, 1.0) * f32::from(u16::MAX)) as u16;
            let sched = Replay {
                play_for: Ticks::from_ms(60_000),
                ..Replay::default()
            };
            let effect = EffectBuilder::new()
                .add_effect(BaseEffect {
                    kind: BaseEffectType::Strong {
                        magnitude: mag(c.low),
                    },
                    scheduling: sched,
                    ..BaseEffect::default()
                })
                .add_effect(BaseEffect {
                    kind: BaseEffectType::Weak {
                        magnitude: mag(c.high),
                    },
                    scheduling: sched,
                    ..BaseEffect::default()
                })
                .gamepads(&[p.gilrs])
                .finish(g);
            if let Ok(e) = effect
                && e.play().is_ok()
            {
                p.effect = Some(e);
            }
        }
    }
}

fn send(rt: &mut InputRuntime, device: DeviceId, name: &str, value: [f32; 2]) {
    if let Some(control) = control_index(Device::Gamepad, name) {
        rt.push(RawEvent {
            device,
            control,
            value,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_standard_button_maps_to_a_pad_control() {
        for b in [
            Button::South,
            Button::East,
            Button::North,
            Button::West,
            Button::LeftTrigger,
            Button::RightTrigger,
            Button::LeftTrigger2,
            Button::RightTrigger2,
            Button::Select,
            Button::Start,
            Button::Mode,
            Button::LeftThumb,
            Button::RightThumb,
            Button::DPadUp,
            Button::DPadDown,
            Button::DPadLeft,
            Button::DPadRight,
        ] {
            let n = button_name(b).unwrap_or_else(|| panic!("{b:?}"));
            assert!(control_index(Device::Gamepad, n).is_some(), "{n}");
        }
        assert_eq!(uuid_key([0xab; 16]).len(), 32);
    }
}
