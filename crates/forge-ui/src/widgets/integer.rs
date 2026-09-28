//! The exact integer field (§21.16): the numeric drag field's behaviour over `i128`, so a
//! `u64` seed or hash, an `i64` tick count or a `u8` byte is shown, stepped and typed
//! without passing through `f64` (which is exact only up to 2^53).
//!
//! * Shows the value with its unit; with `decimals(d)` the value is a count of 10^-d units
//!   shown as a fixed-point decimal (a tick in microseconds shown in seconds), still exact.
//! * **Drag** horizontally to scrub by whole steps (Shift: fine, Ctrl: ×10); a click without
//!   movement, a double click, Enter or F2 starts typing; typing a digit starts an entry.
//! * **Typing** takes a whole number (or a decimal with at most `d` places), optionally
//!   followed by the field's own unit. Anything else, or a value outside the range (the
//!   type's range narrowed by the field's), is refused **with the reason shown** and read
//!   out, and the value is left unchanged. Escape cancels.
//! * Up/Down step, PageUp/PageDown step ×10, Home/End go to the limits; stepping and
//!   dragging stop at the limits.
//! * `IntegerCommitted` is raised once per committed edit and once at the end of a drag.

use accesskit::{Action, Role};

use crate::damage::Dirty;
use crate::geom::Rect;
use crate::geom::{Point, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::line_edit::{EditOutcome, LineEdit};
use super::{CONTROL_MIN_H, body, control_pad_x};

/// Raised when a new value is committed (typed entry, drag end, step).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct IntegerCommitted {
    pub field: WidgetId,
    pub value: i128,
}

/// An exact integer field bound to a `Signal<i128>`.
pub struct IntegerField {
    value: Signal<i128>,
    label: String,
    unit: String,
    min: i128,
    max: i128,
    step: i128,
    decimals: u32,
    edit: Option<LineEdit>,
    error: Option<String>,
    drag: Option<(f32, i128, bool)>,
}

impl IntegerField {
    pub fn new(value: Signal<i128>, label: &str) -> Self {
        Self {
            value,
            label: label.to_string(),
            unit: String::new(),
            min: i128::from(i64::MIN),
            max: i128::from(i64::MAX),
            step: 1,
            decimals: 0,
            edit: None,
            error: None,
            drag: None,
        }
    }
    /// The unit shown after the value (and accepted after a typed one).
    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = unit.trim().to_string();
        self
    }
    /// Inclusive limits.
    pub fn range(mut self, min: i128, max: i128) -> Self {
        self.min = min;
        self.max = max.max(min);
        self
    }
    /// The increment of a step or a pixel of drag (at least 1).
    pub fn step(mut self, step: i128) -> Self {
        self.step = step.max(1);
        self
    }
    /// Show the value as a fixed-point decimal with `d` places (the value counts 10^-d).
    pub fn decimals(mut self, d: u32) -> Self {
        self.decimals = d.min(18);
        self
    }
    /// The refusal reason of the last entry, if it was refused.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn value(&self, rt: &crate::state::Runtime) -> i128 {
        self.value.get(rt)
    }
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// The value as text, without the unit.
    pub fn format_number(&self, v: i128) -> String {
        if self.decimals == 0 {
            return v.to_string();
        }
        let p = 10u128.pow(self.decimals);
        let a = v.unsigned_abs();
        let frac = format!("{:0w$}", a % p, w = self.decimals as usize);
        let frac = frac.trim_end_matches('0');
        let sign = if v < 0 { "-" } else { "" };
        if frac.is_empty() {
            format!("{sign}{}", a / p)
        } else {
            format!("{sign}{}.{frac}", a / p)
        }
    }

    fn format(&self, v: i128) -> String {
        if self.unit.is_empty() {
            self.format_number(v)
        } else {
            format!("{} {}", self.format_number(v), self.unit)
        }
    }

    /// Parse typed text exactly (see the module docs).
    pub fn parse(&self, text: &str) -> Result<i128, String> {
        let mut t = text.trim();
        if !self.unit.is_empty()
            && let Some(rest) = t.strip_suffix(self.unit.as_str())
        {
            t = rest.trim_end();
        }
        let t: String = t.chars().filter(|c| *c != '_' && *c != ' ').collect();
        let (neg, digits) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t.strip_prefix('+').unwrap_or(&t)),
        };
        let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
        let whole = self.whole_number_hint();
        if int.is_empty() && frac.is_empty()
            || !int.chars().all(|c| c.is_ascii_digit())
            || !frac.chars().all(|c| c.is_ascii_digit())
        {
            return Err(crate::trf!(
                "\u{201c}{text}\u{201d} is not {whole}",
                text = text.trim(),
                whole
            ));
        }
        let kept = frac.trim_end_matches('0');
        if kept.len() > self.decimals as usize {
            return Err(crate::trf!(
                "{label} takes {whole}",
                label = self.label,
                whole
            ));
        }
        let too_big = || crate::trf!("\u{201c}{text}\u{201d} is out of range", text = text.trim());
        let p = 10i128.pow(self.decimals);
        let mut v: i128 = if int.is_empty() {
            0
        } else {
            int.parse::<i128>().map_err(|_| too_big())?
        };
        v = v.checked_mul(p).ok_or_else(too_big)?;
        if !kept.is_empty() {
            let f: i128 = format!("{kept:0<w$}", w = self.decimals as usize)
                .parse()
                .map_err(|_| too_big())?;
            v = v.checked_add(f).ok_or_else(too_big)?;
        }
        if neg {
            v = -v;
        }
        if v < self.min {
            return Err(crate::trf!(
                "{value} is below the minimum {min}",
                value = self.format_number(v),
                min = self.format_number(self.min)
            ));
        }
        if v > self.max {
            return Err(crate::trf!(
                "{value} is above the maximum {max}",
                value = self.format_number(v),
                max = self.format_number(self.max)
            ));
        }
        Ok(v)
    }

    fn whole_number_hint(&self) -> String {
        match self.decimals {
            0 => crate::tr!("a whole number").into(),
            d => crate::trf!("a number with at most {d} decimals", d),
        }
    }

    fn clamp(&self, v: i128) -> i128 {
        v.clamp(self.min, self.max)
    }

    fn commit(&mut self, cx: &mut EventCx, v: i128) {
        let v = self.clamp(v);
        self.value.set(cx.rt_mut(), v);
        let field = cx.id();
        cx.action(IntegerCommitted { field, value: v });
    }

    fn begin_edit(&mut self, cx: &mut EventCx) {
        let mut e = LineEdit::new(&self.format_number(self.value.get(cx.rt())));
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
            match self.parse(&e.text) {
                Ok(v) => {
                    self.error = None;
                    self.commit(cx, v);
                }
                Err(err) => {
                    // Refused with a reason; keep editing so the user can fix it.
                    self.error = Some(err);
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

    /// Step multiplier for the held modifiers (Shift: one step at a time, Ctrl: ×10).
    fn scale_for(m: crate::input::Modifiers) -> i128 {
        if m.ctrl { 10 } else { 1 }
    }
    /// Pixels of drag per step (Shift drags finer).
    fn px_per_step(m: crate::input::Modifiers) -> f64 {
        if m.shift { 10.0 } else { 1.0 }
    }
}

impl Widget for IntegerField {
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
        let (_, t) = cx.text.layout("0000000000 s", &body(cx.theme), None);
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
                    match e.key(k, cx.clipboard()) {
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
                let s = self.step.saturating_mul(Self::scale_for(k.mods));
                let nv = match k.code {
                    KeyCode::Up => v.saturating_add(s),
                    KeyCode::Down => v.saturating_sub(s),
                    KeyCode::PageUp => v.saturating_add(s.saturating_mul(10)),
                    KeyCode::PageDown => v.saturating_sub(s.saturating_mul(10)),
                    KeyCode::Home => self.min,
                    KeyCode::End => self.max,
                    KeyCode::Enter | KeyCode::F2 => {
                        self.begin_edit(cx);
                        return Handled::Yes;
                    }
                    _ => {
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
                        let steps = (dx / Self::px_per_step(m)).round() as i128;
                        let delta = steps
                            .saturating_mul(self.step)
                            .saturating_mul(Self::scale_for(m));
                        let nv = self.clamp(v0.saturating_add(delta));
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
                        cx.action(IntegerCommitted {
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
                self.commit(cx, v.saturating_add(self.step));
                Handled::Yes
            }
            UiEvent::A11yAction(Action::Decrement) => {
                self.commit(cx, v.saturating_sub(self.step));
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
        let inner = Rect::new(r.x + pad, r.y, (r.w - 2.0 * pad).max(0.0), r.h);
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
            }
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        let v = self.value.get(cx.rt);
        let scale = 10f64.powi(self.decimals as i32);
        node.set_label(self.label.as_str());
        // AccessKit's numeric value is an f64: approximate for AT; the exact value is the
        // text value below.
        node.set_numeric_value(v as f64 / scale);
        node.set_min_numeric_value(self.min as f64 / scale);
        node.set_max_numeric_value(self.max as f64 / scale);
        node.set_numeric_value_step(self.step as f64 / scale);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Runtime;

    fn field(min: i128, max: i128, d: u32) -> IntegerField {
        let mut rt = Runtime::new();
        let s = rt.signal(0i128);
        IntegerField::new(s, "Value").range(min, max).decimals(d)
    }

    #[test]
    fn parses_and_formats_exactly_over_the_whole_u64_range() {
        let f = field(0, i128::from(u64::MAX), 0);
        assert_eq!(f.parse("18446744073709551615"), Ok(i128::from(u64::MAX)));
        assert_eq!(f.parse("9007199254740993"), Ok(9_007_199_254_740_993));
        assert_eq!(f.format_number(9_007_199_254_740_993), "9007199254740993");
        assert!(
            f.parse("18446744073709551616").is_err(),
            "one past u64::MAX"
        );
        assert!(
            f.parse("-1")
                .is_err_and(|e| e.contains("below the minimum 0"))
        );
        assert!(f.parse("1.5").is_err_and(|e| e.contains("whole number")));
        assert!(f.parse("abc").is_err());
        assert_eq!(f.parse("1_000"), Ok(1000));
        assert_eq!(f.parse("7.0"), Ok(7));
    }

    #[test]
    fn a_byte_field_refuses_what_a_byte_cannot_hold() {
        let f = field(0, 255, 0);
        assert!(
            f.parse("300")
                .is_err_and(|e| e.contains("above the maximum 255"))
        );
        assert!(f.parse("-1").is_err());
        assert_eq!(f.parse("255"), Ok(255));
    }

    #[test]
    fn fixed_point_decimals_are_exact() {
        let f = field(i128::from(i64::MIN), i128::from(i64::MAX), 6).unit("s");
        // 1,780,000,000.000001 s: 16 significant digits, beyond f64 at micro precision.
        assert_eq!(f.parse("1780000000.000001 s"), Ok(1_780_000_000_000_001));
        assert_eq!(f.format_number(1_780_000_000_000_001), "1780000000.000001");
        assert_eq!(f.format_number(-1_500_000), "-1.5");
        assert_eq!(f.parse("-1.5"), Ok(-1_500_000));
        assert!(
            f.parse("0.0000001")
                .is_err_and(|e| e.contains("6 decimals"))
        );
    }
}
