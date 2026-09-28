use accesskit::{Action, Role};

use crate::damage::Dirty;
use crate::geom::{Rect, Size};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::CONTROL_MIN_H;

const THUMB: f32 = 14.0;

/// What a slider edit is doing, so an owner can make one continuous drag one transaction
/// and one undo entry (Ch.21 §21.18 gestures).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SliderPhase {
    /// A drag started (the value may already have moved to the pointer).
    Begin,
    /// The value moved during a drag.
    Update,
    /// The drag ended (pointer released).
    End,
    /// Esc during a drag: the value went back to where the drag began.
    Cancel,
    /// A one-step edit (arrow keys, Home/End, an assistive-technology increment).
    Step,
}

/// Raised on every slider edit with the new value (after the signal changed).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SliderEdit {
    pub slider: crate::id::WidgetId,
    pub phase: SliderPhase,
    pub value: f32,
}

/// A slider over `[min, max]` bound to a `Signal<f32>`, linear or logarithmic (`log`:
/// equal thumb travel multiplies the value, for ranges like 0.01–1000). Arrows step,
/// PageUp/Down jump by 10 steps, Home/End go to the ends; dragging captures the pointer.
pub struct Slider {
    value: Signal<f32>,
    label: String,
    min: f32,
    max: f32,
    step: f32,
    dragging: bool,
    /// The value when the current drag began (Esc restores it).
    drag_start: f32,
    log: bool,
}

impl Slider {
    pub fn new(value: Signal<f32>, label: &str, min: f32, max: f32, step: f32) -> Self {
        Self {
            value,
            label: label.to_string(),
            min,
            max: max.max(min),
            step: step.abs().max(f32::EPSILON),
            dragging: false,
            drag_start: 0.0,
            log: false,
        }
    }

    /// Logarithmic mapping (requires `min > 0`; otherwise stays linear). Arrow keys then
    /// move by 1% of the travel instead of by `step`.
    pub fn log(mut self) -> Self {
        self.log = self.min > 0.0;
        self
    }

    /// Position of `v` along the track, 0..=1.
    fn to_t(&self, v: f32) -> f32 {
        if self.max <= self.min {
            return 0.0;
        }
        if self.log {
            ((v.max(self.min).ln() - self.min.ln()) / (self.max.ln() - self.min.ln()))
                .clamp(0.0, 1.0)
        } else {
            ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        }
    }

    fn value_at_t(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if self.log {
            (self.min.ln() + t * (self.max.ln() - self.min.ln())).exp()
        } else {
            self.min + t * (self.max - self.min)
        }
    }

    fn nudge(&self, v: f32, steps: f32) -> f32 {
        if self.log {
            self.value_at_t(self.to_t(v) + 0.01 * steps)
        } else {
            v + steps * self.step
        }
    }

    fn set(&self, cx: &mut EventCx, v: f32) -> f32 {
        let snapped = if self.log {
            v
        } else {
            ((v - self.min) / self.step).round() * self.step + self.min
        };
        let v = snapped.clamp(self.min, self.max);
        self.value.set(cx.rt_mut(), v);
        v
    }

    fn edit(&self, cx: &mut EventCx, phase: SliderPhase, value: f32) {
        let slider = cx.id();
        cx.action(SliderEdit {
            slider,
            phase,
            value,
        });
    }

    fn value_at_x(&self, r: Rect, x: f32) -> f32 {
        let t = ((x - r.x - THUMB * 0.5) / (r.w - THUMB).max(1.0)).clamp(0.0, 1.0);
        self.value_at_t(t)
    }
}

impl Widget for Slider {
    fn role(&self) -> Role {
        Role::Slider
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
        Size::new(known.width.unwrap_or(180.0), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let v = self.value.get(cx.rt());
        match ev {
            UiEvent::Key(k) if k.pressed && k.code == KeyCode::Escape && self.dragging => {
                self.dragging = false;
                cx.release_pointer();
                let back = self.drag_start;
                self.value.set(cx.rt_mut(), back);
                self.edit(cx, SliderPhase::Cancel, back);
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => {
                let nv = match k.code {
                    KeyCode::Right | KeyCode::Up => self.nudge(v, 1.0),
                    KeyCode::Left | KeyCode::Down => self.nudge(v, -1.0),
                    KeyCode::PageUp => self.nudge(v, 10.0),
                    KeyCode::PageDown => self.nudge(v, -10.0),
                    KeyCode::Home => self.min,
                    KeyCode::End => self.max,
                    _ => return Handled::No,
                };
                let nv = self.set(cx, nv);
                self.edit(cx, SliderPhase::Step, nv);
                Handled::Yes
            }
            UiEvent::A11yAction(Action::Increment) => {
                let nv = self.set(cx, self.nudge(v, 1.0));
                self.edit(cx, SliderPhase::Step, nv);
                Handled::Yes
            }
            UiEvent::A11yAction(Action::Decrement) => {
                let nv = self.set(cx, self.nudge(v, -1.0));
                self.edit(cx, SliderPhase::Step, nv);
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                self.dragging = true;
                self.drag_start = v;
                cx.capture_pointer();
                let nv = self.value_at_x(cx.rect(), pos.x);
                let nv = self.set(cx, nv);
                self.edit(cx, SliderPhase::Begin, nv);
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if self.dragging => {
                let nv = self.value_at_x(cx.rect(), pos.x);
                let nv = self.set(cx, nv);
                self.edit(cx, SliderPhase::Update, nv);
                Handled::Yes
            }
            UiEvent::PointerUp { .. } if self.dragging => {
                self.dragging = false;
                cx.release_pointer();
                let nv = self.value.get(cx.rt());
                self.edit(cx, SliderPhase::End, nv);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let v = self.value.get(cx.rt());
        let t = self.to_t(v);
        let bg = cx.parent_bg();
        let cy = r.y + r.h * 0.5;
        let track = Rect::new(r.x + THUMB * 0.5, cy - 2.0, (r.w - THUMB).max(0.0), 4.0);
        cx.mark(track, ColorRole::Border, bg, 2.0);
        let filled = Rect::new(track.x, track.y, track.w * t, track.h);
        cx.mark(filled, ColorRole::Accent, bg, 2.0);
        // Hover/drag grow the thumb instead of recolouring it: the state is visible without
        // relying on colour, and the thumb keeps its contrast against the panel.
        let d = if cx.state().hovered || self.dragging {
            THUMB + 4.0
        } else {
            THUMB
        };
        let tx = r.x + THUMB * 0.5 + t * (r.w - THUMB) - d * 0.5;
        cx.mark(
            Rect::new(tx, cy - d * 0.5, d, d),
            ColorRole::Accent,
            bg,
            d * 0.5,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_numeric_value(f64::from(self.value.get(cx.rt)));
        node.set_min_numeric_value(f64::from(self.min));
        node.set_max_numeric_value(f64::from(self.max));
        node.set_numeric_value_step(f64::from(self.step));
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
        node.add_action(Action::SetValue);
    }
}
