//! Keyboard, mouse and touch from winit (Ch.28 §28.9).
//!
//! * Keys bind by **physical** position (`KeyW` is `Keyboard/W` on every layout, so WASD
//!   stays under the left hand on AZERTY); the label a prompt shows is the layout's, learnt
//!   from each key event's text ([`crate::KeyLabels`]).
//! * Mouse motion comes from the raw device motion (`DeviceEvent::MouseMotion`: unaccelerated,
//!   not clamped at the window edge — what aiming wants); until the platform delivers one, the
//!   cursor's movement stands in.
//! * Losing window focus releases every key and button (no key stuck down after Alt+Tab).
//! * Touch goes to the touch device's gesture recogniser.

use winit::event::{
    DeviceEvent, ElementState, MouseButton, MouseScrollDelta, TouchPhase as WPhase, WindowEvent,
};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::control::Device;
use crate::glyph::KeyLabels;
use crate::runtime::{DeviceDesc, DeviceId, InputRuntime};
use crate::touch::{TouchInput, TouchPhase};

/// Pixels of a pixel-precise scroll that count as one wheel line.
pub const PIXELS_PER_LINE: f32 = 40.0;

/// The runtime's name for a physical key (`None`: a key the table does not have).
pub fn key_name(code: KeyCode) -> Option<&'static str> {
    use KeyCode as K;
    Some(match code {
        K::KeyA => "A",
        K::KeyB => "B",
        K::KeyC => "C",
        K::KeyD => "D",
        K::KeyE => "E",
        K::KeyF => "F",
        K::KeyG => "G",
        K::KeyH => "H",
        K::KeyI => "I",
        K::KeyJ => "J",
        K::KeyK => "K",
        K::KeyL => "L",
        K::KeyM => "M",
        K::KeyN => "N",
        K::KeyO => "O",
        K::KeyP => "P",
        K::KeyQ => "Q",
        K::KeyR => "R",
        K::KeyS => "S",
        K::KeyT => "T",
        K::KeyU => "U",
        K::KeyV => "V",
        K::KeyW => "W",
        K::KeyX => "X",
        K::KeyY => "Y",
        K::KeyZ => "Z",
        K::Digit0 => "Digit0",
        K::Digit1 => "Digit1",
        K::Digit2 => "Digit2",
        K::Digit3 => "Digit3",
        K::Digit4 => "Digit4",
        K::Digit5 => "Digit5",
        K::Digit6 => "Digit6",
        K::Digit7 => "Digit7",
        K::Digit8 => "Digit8",
        K::Digit9 => "Digit9",
        K::Space => "Space",
        K::Enter => "Enter",
        K::Escape => "Escape",
        K::Tab => "Tab",
        K::Backspace => "Backspace",
        K::Delete => "Delete",
        K::Insert => "Insert",
        K::Home => "Home",
        K::End => "End",
        K::PageUp => "PageUp",
        K::PageDown => "PageDown",
        K::ArrowUp => "Up",
        K::ArrowDown => "Down",
        K::ArrowLeft => "Left",
        K::ArrowRight => "Right",
        K::ShiftLeft => "LeftShift",
        K::ShiftRight => "RightShift",
        K::ControlLeft => "LeftCtrl",
        K::ControlRight => "RightCtrl",
        K::AltLeft => "LeftAlt",
        K::AltRight => "RightAlt",
        K::SuperLeft => "LeftMeta",
        K::SuperRight => "RightMeta",
        K::F1 => "F1",
        K::F2 => "F2",
        K::F3 => "F3",
        K::F4 => "F4",
        K::F5 => "F5",
        K::F6 => "F6",
        K::F7 => "F7",
        K::F8 => "F8",
        K::F9 => "F9",
        K::F10 => "F10",
        K::F11 => "F11",
        K::F12 => "F12",
        K::CapsLock => "CapsLock",
        K::Minus => "Minus",
        K::Equal => "Equal",
        K::BracketLeft => "BracketLeft",
        K::BracketRight => "BracketRight",
        K::Backslash => "Backslash",
        K::Semicolon => "Semicolon",
        K::Quote => "Quote",
        K::Backquote => "Backquote",
        K::Comma => "Comma",
        K::Period => "Period",
        K::Slash => "Slash",
        K::Numpad0 => "Numpad0",
        K::Numpad1 => "Numpad1",
        K::Numpad2 => "Numpad2",
        K::Numpad3 => "Numpad3",
        K::Numpad4 => "Numpad4",
        K::Numpad5 => "Numpad5",
        K::Numpad6 => "Numpad6",
        K::Numpad7 => "Numpad7",
        K::Numpad8 => "Numpad8",
        K::Numpad9 => "Numpad9",
        K::NumpadAdd => "NumpadAdd",
        K::NumpadSubtract => "NumpadSubtract",
        K::NumpadMultiply => "NumpadMultiply",
        K::NumpadDivide => "NumpadDivide",
        K::NumpadDecimal => "NumpadDecimal",
        K::NumpadEnter => "NumpadEnter",
        K::ContextMenu => "ContextMenu",
        _ => return None,
    })
}

/// The runtime's name for a mouse button.
pub fn mouse_name(b: MouseButton) -> Option<&'static str> {
    Some(match b {
        MouseButton::Left => "Left",
        MouseButton::Right => "Right",
        MouseButton::Middle => "Middle",
        MouseButton::Back => "Back",
        MouseButton::Forward => "Forward",
        MouseButton::Other(_) => return None,
    })
}

/// A window's keyboard, mouse and touch screen (see the module docs).
pub struct WinitInput {
    pub keyboard: DeviceId,
    pub mouse: DeviceId,
    pub touch: DeviceId,
    labels: KeyLabels,
    cursor: Option<[f32; 2]>,
    raw_motion: bool,
}

impl WinitInput {
    /// Connect the window's keyboard, mouse and touch screen.
    pub fn new(rt: &mut InputRuntime) -> Self {
        Self {
            keyboard: rt.connect(DeviceDesc::new(
                Device::Keyboard,
                "Keyboard",
                "winit:keyboard",
            )),
            mouse: rt.connect(DeviceDesc::new(Device::Mouse, "Mouse", "winit:mouse")),
            touch: rt.connect(DeviceDesc::new(
                Device::Touch,
                "Touch screen",
                "winit:touch",
            )),
            labels: KeyLabels::default(),
            cursor: None,
            raw_motion: false,
        }
    }

    /// The layout's key labels learnt so far (for glyphs).
    pub fn key_labels(&self) -> &KeyLabels {
        &self.labels
    }

    /// A key changed (the testable core of `KeyboardInput`).
    pub fn key(
        &mut self,
        rt: &mut InputRuntime,
        key: PhysicalKey,
        pressed: bool,
        text: Option<&str>,
    ) {
        let PhysicalKey::Code(code) = key else { return };
        let Some(name) = key_name(code) else { return };
        if let Some(t) = text {
            self.labels.learn(name, t);
        }
        rt.send(self.keyboard, name, [if pressed { 1.0 } else { 0.0 }, 0.0]);
    }

    pub fn mouse_button(&mut self, rt: &mut InputRuntime, b: MouseButton, pressed: bool) {
        if let Some(name) = mouse_name(b) {
            rt.send(self.mouse, name, [if pressed { 1.0 } else { 0.0 }, 0.0]);
        }
    }

    pub fn cursor_moved(&mut self, rt: &mut InputRuntime, x: f32, y: f32) {
        if let Some(c) = self.cursor
            && !self.raw_motion
        {
            rt.send(self.mouse, "Delta", [x - c[0], -(y - c[1])]);
        }
        self.cursor = Some([x, y]);
        rt.send(self.mouse, "Position", [x, y]);
    }

    /// Raw motion (y up, like the stick).
    pub fn raw_motion(&mut self, rt: &mut InputRuntime, dx: f32, dy: f32) {
        self.raw_motion = true;
        rt.send(self.mouse, "Delta", [dx, -dy]);
    }

    pub fn wheel(&mut self, rt: &mut InputRuntime, lines_x: f32, lines_y: f32) {
        if lines_y != 0.0 {
            rt.send(self.mouse, "Wheel", [lines_y, 0.0]);
        }
        if lines_x != 0.0 {
            rt.send(self.mouse, "WheelX", [lines_x, 0.0]);
        }
    }

    pub fn focus(&mut self, rt: &mut InputRuntime, focused: bool) {
        if !focused {
            rt.release_all(self.keyboard);
            rt.release_all(self.mouse);
            self.cursor = None;
        }
    }

    /// Feed one window event.
    pub fn window_event(&mut self, rt: &mut InputRuntime, ev: &WindowEvent) {
        match ev {
            WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return;
                }
                self.key(
                    rt,
                    event.physical_key,
                    event.state == ElementState::Pressed,
                    event.text.as_deref(),
                );
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.mouse_button(rt, *button, *state == ElementState::Pressed);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_moved(rt, position.x as f32, position.y as f32);
            }
            WindowEvent::MouseWheel { delta, .. } => match delta {
                MouseScrollDelta::LineDelta(x, y) => self.wheel(rt, *x, *y),
                MouseScrollDelta::PixelDelta(p) => self.wheel(
                    rt,
                    p.x as f32 / PIXELS_PER_LINE,
                    p.y as f32 / PIXELS_PER_LINE,
                ),
            },
            WindowEvent::Touch(t) => {
                let phase = match t.phase {
                    WPhase::Started => TouchPhase::Started,
                    WPhase::Moved => TouchPhase::Moved,
                    WPhase::Ended => TouchPhase::Ended,
                    WPhase::Cancelled => TouchPhase::Canceled,
                };
                rt.touch(
                    self.touch,
                    TouchInput::new(t.id, phase, t.location.x as f32, t.location.y as f32),
                );
            }
            WindowEvent::Focused(f) => self.focus(rt, *f),
            _ => {}
        }
    }

    /// Feed one device event (raw mouse motion).
    pub fn device_event(&mut self, rt: &mut InputRuntime, ev: &DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = ev {
            self.raw_motion(rt, delta.0 as f32, delta.1 as f32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::control_index;

    #[test]
    fn every_mapped_key_is_in_the_table() {
        for code in [
            KeyCode::KeyA,
            KeyCode::KeyZ,
            KeyCode::Digit5,
            KeyCode::ArrowLeft,
            KeyCode::SuperLeft,
            KeyCode::NumpadEnter,
            KeyCode::Backquote,
            KeyCode::F12,
        ] {
            let n = key_name(code).unwrap_or_else(|| panic!("{code:?}"));
            assert!(control_index(Device::Keyboard, n).is_some(), "{n}");
        }
    }
}
