//! The numeric drag field with units, and the spin box (§21.16, Ch.6 unit annotations).
//!
//! * Shows the value with its unit (`12.5 N·s`).
//! * **Drag** horizontally to scrub (Shift: ×0.1 fine, Ctrl: ×10 coarse); a click without
//!   movement, a double click, Enter or F2 starts typing.
//! * **Typing** accepts any compatible unit and converts on entry (`12 kN·s` → 12 000
//!   N·s); an incompatible or unknown unit is refused **with the reason shown** and read
//!   out, and the value is left unchanged. Escape cancels.
//! * Up/Down step, PageUp/PageDown step ×10, Home/End go to the limits.
//! * `NumericCommitted` is raised once per committed edit and once at the end of a drag
//!   (the editor turns a drag into one undoable command transaction, §21.18).

use accesskit::{Action, Role};

use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::units::{Unit, parse_in, parse_unit};
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::line_edit::{EditOutcome, LineEdit};
use super::{CONTROL_MIN_H, body, control_pad_x};

/// Raised when a new value is committed (typed entry, drag end, step).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct NumericCommitted {
    pub field: WidgetId,
    pub value: f64,
}

const STEPPER_W: f32 = 18.0;

/// A numeric field bound to a `Signal<f64>`.
pub struct NumericField {
    value: Signal<f64>,
    label: String,
    unit: Option<Unit>,
    min: f64,
    max: f64,
    step: f64,
    decimals: usize,
    spin: bool,
    edit: Option<LineEdit>,
    error: Option<String>,
    drag: Option<(f32, f64, bool)>,
}

impl NumericField {
    pub fn new(value: Signal<f64>, label: &str) -> Self {
        Self {
            value,
            label: label.to_string(),
            unit: None,
            min: f64::NEG_INFINITY,
            max: f64::INFINITY,
            step: 0.1,
            decimals: 3,
            spin: false,
            edit: None,
            error: None,
            drag: None,
        }
    }
    /// Annotate with a unit (`"N·s"`, `"m/s²"`). An unparsable annotation is ignored.
    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = parse_unit(unit).ok();
        self
    }
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max.max(min);
        self
    }
    pub fn step(mut self, step: f64) -> Self {
        self.step = step.abs().max(f64::EPSILON);
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        self.decimals = d.min(12);
        self
    }
    /// A spin box: visible step buttons at the right.
    pub fn spin(mut self) -> Self {
        self.spin = true;
        self
    }
    /// The refusal reason of the last entry, if it was refused.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    /// The value shown (its signal's).
    pub fn value(&self, rt: &crate::state::Runtime) -> f64 {
        self.value.get(rt)
    }
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    fn unit_suffix(&self) -> String {
        self.unit
            .as_ref()
            .filter(|u| !u.symbol.is_empty())
            .map(|u| format!(" {}", u.symbol))
            .unwrap_or_default()
    }

    fn format(&self, v: f64) -> String {
        let mut s = format!("{v:.*}", self.decimals);
        if s.contains('.') {
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.pop();
            }
        }
        if s == "-0" {
            s = "0".into();
        }
        format!("{s}{}", self.unit_suffix())
    }

    fn clamp(&self, v: f64) -> f64 {
        v.clamp(self.min, self.max)
    }

    fn commit(&mut self, cx: &mut EventCx, v: f64) {
        let v = self.clamp(v);
        self.value.set(cx.rt_mut(), v);
        let field = cx.id();
        cx.action(NumericCommitted { field, value: v });
    }

    fn begin_edit(&mut self, cx: &mut EventCx) {
        let mut e = LineEdit::new(&self.format(self.value.get(cx.rt())));
        e.select_all();
        self.edit = Some(e);
        cx.set_ime_allowed(true);
        cx.request_paint();
        cx.request_a11y();
    }

    fn end_edit(&mut self, cx: &mut EventCx, accept: bool) {
        let Some(e) = self.edit.take() else {
            return;
        };
        cx.set_ime_allowed(false);
        if accept {
            let unit = self.unit.clone().unwrap_or(Unit {
                symbol: String::new(),
                dim: crate::units::Dim::NONE,
                scale: 1.0,
            });
            match parse_in(&e.text, &unit) {
                Ok(v) => {
                    self.error = None;
                    self.commit(cx, v);
                }
                Err(err) => {
                    // Refused with a reason; keep editing so the user can fix it.
                    self.error = Some(err.to_string());
                    self.edit = Some(e);
                    cx.set_ime_allowed(true);
                }
            }
        } else {
            self.error = None;
        }
        cx.request_paint();
        cx.request_a11y();
    }

    fn scale_for(m: crate::input::Modifiers) -> f64 {
        if m.shift {
            0.1
        } else if m.ctrl {
            10.0
        } else {
            1.0
        }
    }

    fn stepper_rects(&self, r: Rect) -> (Rect, Rect) {
        let x = r.right() - STEPPER_W;
        (
            Rect::new(x, r.y, STEPPER_W, r.h * 0.5),
            Rect::new(x, r.y + r.h * 0.5, STEPPER_W, r.h * 0.5),
        )
    }
}

impl Widget for NumericField {
    fn role(&self) -> Role {
        Role::SpinButton
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::PAINT | Dirty::A11Y);
    }
    fn focusable(&self) -> bool {
        true
    }
    fn hover_sensitive(&self) -> bool {
        true
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let (_, t) = cx.text.layout("0.000 N·s", &body(cx.theme), None);
        Size::new(
            known
                .width
                .unwrap_or(t.w + 2.0 * control_pad_x(cx.theme) + 24.0),
            CONTROL_MIN_H,
        )
    }
    fn tooltip(&self, _rt: &crate::state::Runtime) -> Option<String> {
        self.error.clone()
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let v = self.value.get(cx.rt());
        if let Some(e) = self.edit.as_mut() {
            match ev {
                UiEvent::Key(k) if k.pressed => {
                    let out = e.key(k, cx.clipboard());
                    match out {
                        EditOutcome::Submit => self.end_edit(cx, true),
                        EditOutcome::Cancel => self.end_edit(cx, false),
                        EditOutcome::Ignored => return Handled::No,
                        _ => {
                            self.error = None;
                            cx.request_paint();
                        }
                    }
                    return Handled::Yes;
                }
                UiEvent::Ime(i) => {
                    e.ime(i);
                    cx.request_paint();
                    return Handled::Yes;
                }
                UiEvent::FocusLost => {
                    self.end_edit(cx, true);
                    if self.edit.is_some() {
                        // Refused on blur: keep the old value, drop the entry.
                        self.edit = None;
                    }
                    return Handled::Yes;
                }
                _ => {}
            }
        }
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let s = self.step * Self::scale_for(k.mods);
                let nv = match k.code {
                    KeyCode::Up => v + s,
                    KeyCode::Down => v - s,
                    KeyCode::PageUp => v + 10.0 * s,
                    KeyCode::PageDown => v - 10.0 * s,
                    KeyCode::Home if self.min.is_finite() => self.min,
                    KeyCode::End if self.max.is_finite() => self.max,
                    KeyCode::Enter | KeyCode::F2 => {
                        self.begin_edit(cx);
                        return Handled::Yes;
                    }
                    _ => {
                        // Typing a digit starts an entry with it.
                        if let Some(t) = &k.text
                            && !k.mods.ctrl
                            && t.chars()
                                .all(|c| c.is_ascii_digit() || c == '-' || c == '.')
                        {
                            let t = t.clone();
                            self.begin_edit(cx);
                            if let Some(e) = self.edit.as_mut() {
                                e.insert(&t);
                            }
                            return Handled::Yes;
                        }
                        return Handled::No;
                    }
                };
                self.commit(cx, nv);
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
            } => {
                if self.spin {
                    let (up, down) = self.stepper_rects(cx.rect());
                    if up.contains(*pos) {
                        self.commit(cx, v + self.step);
                        return Handled::Yes;
                    }
                    if down.contains(*pos) {
                        self.commit(cx, v - self.step);
                        return Handled::Yes;
                    }
                }
                if *clicks >= 2 {
                    self.begin_edit(cx);
                    return Handled::Yes;
                }
                self.drag = Some((pos.x, v, false));
                cx.capture_pointer();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                if let Some((x0, v0, moved)) = self.drag {
                    let dx = f64::from(pos.x - x0);
                    if moved || dx.abs() >= 3.0 {
                        let m = cx.modifiers();
                        let nv = self.clamp(v0 + dx * self.step * Self::scale_for(m));
                        self.drag = Some((x0, v0, true));
                        self.value.set(cx.rt_mut(), nv);
                    }
                    return Handled::Yes;
                }
                Handled::No
            }
            UiEvent::PointerUp { .. } => {
                if let Some((_, _, moved)) = self.drag.take() {
                    cx.release_pointer();
                    if moved {
                        let field = cx.id();
                        cx.action(NumericCommitted {
                            field,
                            value: self.value.get(cx.rt()),
                        });
                    } else {
                        self.begin_edit(cx);
                    }
                    return Handled::Yes;
                }
                Handled::No
            }
            UiEvent::A11yAction(Action::Increment) => {
                self.commit(cx, v + self.step);
                Handled::Yes
            }
            UiEvent::A11yAction(Action::Decrement) => {
                self.commit(cx, v - self.step);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let radius = cx.theme().radius.md;
        let border = if self.error.is_some() {
            ColorRole::Danger
        } else {
            ColorRole::Border
        };
        cx.panel(r, Some(ColorRole::BgSunken), Some(border), radius, None);
        let pad = control_pad_x(cx.theme());
        let right = if self.spin { STEPPER_W } else { 0.0 };
        let inner = Rect::new(r.x + pad, r.y, (r.w - 2.0 * pad - right).max(0.0), r.h);
        let style = body(cx.theme());
        let focused = cx.state().focused;
        match &self.edit {
            Some(e) => e.paint_line(cx, inner, &style, focused, true, "", ColorRole::BgSunken),
            None => {
                let s = self.format(self.value.get(cx.rt()));
                let (_, t) = cx.shape(&s, &style);
                cx.text(
                    &s,
                    &style,
                    Point::new(inner.x, r.y + (r.h - t.h) * 0.5),
                    ColorRole::FgPrimary,
                );
                // A scrub affordance: small chevrons at the edges while hovered.
                if cx.state().hovered && !self.spin {
                    let small = crate::text::TextStyle::body(cx.theme().type_scale.small);
                    let (_, c) = cx.shape("‹", &small);
                    cx.text(
                        "‹",
                        &small,
                        Point::new(r.x + 2.0, r.y + (r.h - c.h) * 0.5),
                        ColorRole::FgMuted,
                    );
                    cx.text(
                        "›",
                        &small,
                        Point::new(r.right() - c.w - 2.0, r.y + (r.h - c.h) * 0.5),
                        ColorRole::FgMuted,
                    );
                }
            }
        }
        if self.spin {
            let (up, down) = self.stepper_rects(r);
            let small = crate::text::TextStyle::body(cx.theme().type_scale.small);
            for (rect, g) in [(up, "▴"), (down, "▾")] {
                let (_, t) = cx.shape(g, &small);
                cx.text(
                    g,
                    &small,
                    Point::new(rect.x + (rect.w - t.w) * 0.5, rect.y + (rect.h - t.h) * 0.5),
                    ColorRole::FgPrimary,
                );
            }
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        let v = self.value.get(cx.rt);
        node.set_label(self.label.as_str());
        node.set_numeric_value(v);
        if self.min.is_finite() {
            node.set_min_numeric_value(self.min);
        }
        if self.max.is_finite() {
            node.set_max_numeric_value(self.max);
        }
        node.set_numeric_value_step(self.step);
        node.set_value(self.format(v));
        if let Some(e) = &self.error {
            node.set_description(e.as_str());
            node.set_invalid(accesskit::Invalid::True);
        }
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
        node.add_action(Action::SetValue);
    }
}
