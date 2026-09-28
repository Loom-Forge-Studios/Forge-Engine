//! A two-thumb range slider (§21.16). Each thumb is its own tab stop and its own
//! AccessKit slider (WAI-ARIA multi-thumb slider): Tab moves from the low thumb to the
//! high one, then on out of the widget; arrows move the focused thumb; the thumbs never
//! cross.

use accesskit::{Action, Role};

use crate::damage::Dirty;
use crate::geom::{Rect, Size};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::CONTROL_MIN_H;

const THUMB: f32 = 14.0;

/// A `(low, high)` range over `[min, max]`.
pub struct RangeSlider {
    value: Signal<(f32, f32)>,
    label: String,
    min: f32,
    max: f32,
    step: f32,
    /// 0: the low thumb, 1: the high thumb.
    active: usize,
    dragging: bool,
}

impl RangeSlider {
    pub fn new(value: Signal<(f32, f32)>, label: &str, min: f32, max: f32, step: f32) -> Self {
        Self {
            value,
            label: label.to_string(),
            min,
            max: max.max(min),
            step: step.abs().max(f32::EPSILON),
            active: 0,
            dragging: false,
        }
    }
    pub fn active_thumb(&self) -> usize {
        self.active
    }

    fn t(&self, v: f32) -> f32 {
        if self.max > self.min {
            ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    fn set_thumb(&self, cx: &mut EventCx, thumb: usize, v: f32) {
        let (lo, hi) = self.value.get(cx.rt());
        let v =
            (((v - self.min) / self.step).round() * self.step + self.min).clamp(self.min, self.max);
        let nv = if thumb == 0 {
            (v.min(hi), hi)
        } else {
            (lo, v.max(lo))
        };
        self.value.set(cx.rt_mut(), nv);
    }

    fn value_at_x(&self, r: Rect, x: f32) -> f32 {
        let t = ((x - r.x - THUMB * 0.5) / (r.w - THUMB).max(1.0)).clamp(0.0, 1.0);
        self.min + t * (self.max - self.min)
    }
}

impl Widget for RangeSlider {
    fn role(&self) -> Role {
        Role::Group
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::PAINT | Dirty::A11Y);
    }
    fn focusable(&self) -> bool {
        true
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        _cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::new(known.width.unwrap_or(200.0), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let (lo, hi) = self.value.get(cx.rt());
        let cur = if self.active == 0 { lo } else { hi };
        match ev {
            UiEvent::FocusGained { .. } => {
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => {
                let nv = match k.code {
                    KeyCode::Tab if !k.mods.shift && self.active == 0 => {
                        self.active = 1;
                        cx.request_paint();
                        cx.request_a11y();
                        return Handled::Yes;
                    }
                    KeyCode::Tab if k.mods.shift && self.active == 1 => {
                        self.active = 0;
                        cx.request_paint();
                        cx.request_a11y();
                        return Handled::Yes;
                    }
                    KeyCode::Right | KeyCode::Up => cur + self.step,
                    KeyCode::Left | KeyCode::Down => cur - self.step,
                    KeyCode::PageUp => cur + 10.0 * self.step,
                    KeyCode::PageDown => cur - 10.0 * self.step,
                    KeyCode::Home => self.min,
                    KeyCode::End => self.max,
                    _ => return Handled::No,
                };
                self.set_thumb(cx, self.active, nv);
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let v = self.value_at_x(cx.rect(), pos.x);
                self.active = if (v - lo).abs() <= (v - hi).abs() {
                    0
                } else {
                    1
                };
                self.dragging = true;
                cx.capture_pointer();
                self.set_thumb(cx, self.active, v);
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if self.dragging => {
                let v = self.value_at_x(cx.rect(), pos.x);
                self.set_thumb(cx, self.active, v);
                Handled::Yes
            }
            UiEvent::PointerUp { .. } if self.dragging => {
                self.dragging = false;
                cx.release_pointer();
                Handled::Yes
            }
            UiEvent::A11yChildAction(t, a) => {
                let thumb = (*t as usize).min(1);
                let v = if thumb == 0 { lo } else { hi };
                match a {
                    Action::Increment => self.set_thumb(cx, thumb, v + self.step),
                    Action::Decrement => self.set_thumb(cx, thumb, v - self.step),
                    Action::Focus => self.active = thumb,
                    _ => return Handled::No,
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let (lo, hi) = self.value.get(cx.rt());
        let bg = cx.parent_bg();
        let cy = r.y + r.h * 0.5;
        let track = Rect::new(r.x + THUMB * 0.5, cy - 2.0, (r.w - THUMB).max(0.0), 4.0);
        cx.mark(track, ColorRole::Border, bg, 2.0);
        let (t0, t1) = (self.t(lo), self.t(hi));
        cx.mark(
            Rect::new(
                track.x + track.w * t0,
                track.y,
                track.w * (t1 - t0),
                track.h,
            ),
            ColorRole::Accent,
            bg,
            2.0,
        );
        let focused = cx.state().focus_visible;
        for (i, t) in [t0, t1].into_iter().enumerate() {
            let d = if focused && i == self.active {
                THUMB + 4.0
            } else {
                THUMB
            };
            let x = r.x + THUMB * 0.5 + t * (r.w - THUMB) - d * 0.5;
            let thumb = Rect::new(x, cy - d * 0.5, d, d);
            cx.mark(thumb, ColorRole::Accent, bg, d * 0.5);
            if focused && i == self.active {
                // The focused thumb gets its own ring (the widget ring spans both).
                cx.panel(thumb.outset(3.0), None, Some(ColorRole::FocusRing), d, None);
            }
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let (lo, hi) = self.value.get(cx.rt);
        for (i, (name, v)) in [("minimum", lo), ("maximum", hi)].into_iter().enumerate() {
            let mut n = accesskit::Node::new(Role::Slider);
            n.set_label(format!("{} {name}", self.label));
            n.set_numeric_value(f64::from(v));
            n.set_min_numeric_value(f64::from(self.min));
            n.set_max_numeric_value(f64::from(self.max));
            n.set_numeric_value_step(f64::from(self.step));
            n.add_action(Action::Increment);
            n.add_action(Action::Decrement);
            n.add_action(Action::Focus);
            out.push((i as u64, n));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        Some(self.active as u64)
    }
}
