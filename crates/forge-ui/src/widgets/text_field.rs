//! Single-line text field: grapheme-correct editing, selection, field-local undo,
//! clipboard, IME pre-edit/commit, and the caret-blink timeout (§21.3, §21.7, §21.9).

use std::time::Duration;

use accesskit::Role;
use unicode_segmentation::UnicodeSegmentation;

use crate::damage::{Dirty, UiTime};
use crate::geom::{Corners, Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, ImeEvent, KeyCode, PointerButton, UiEvent};
use crate::render::Primitive;
use crate::state::Signal;
use crate::style::{ColorRole, PairKind};
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::{CONTROL_MIN_H, body, control_pad_x, control_pad_y};

/// Raised on Enter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submitted {
    pub field: WidgetId,
    pub text: String,
}

const CARET_TAG: u64 = 1;
const BLINK: Duration = Duration::from_millis(530);
/// The caret blinks for 5 s after the last input, then stays solid (WCAG 2.2.2), and the
/// editor becomes truly idle (§21.3).
pub const CARET_BLINK_TIMEOUT: Duration = Duration::from_secs(5);
const UNDO_LIMIT: usize = 200;

/// A single-line text field editing a `Signal<String>` model.
///
/// Pre-edit text is drawn in place, underlined, and **never** written to the model before
/// the IME commits (`test_ui_ime_composition`).
pub struct TextField {
    model: Signal<String>,
    label: String,
    placeholder: String,
    cursor: usize,
    anchor: usize,
    preedit: Option<(String, Option<(usize, usize)>)>,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    caret_on: bool,
    blinking: bool,
    last_input: UiTime,
    /// Fault (W2 control for `ui_typing_latency_one_frame`): apply typed text one frame late.
    defer_one_frame: crate::controls::Switch,
    deferred: Vec<String>,
    /// Fault (W2 control for `test_ui_ime_composition`): write pre-edit text to the model.
    commit_preedit_early: crate::controls::Switch,
    caret_blink_enabled: bool,
    /// A password field: drawn masked, never copied or cut, value hidden from AT.
    password: bool,
}

impl TextField {
    pub fn new(model: Signal<String>, label: &str) -> Self {
        Self {
            model,
            label: label.to_string(),
            placeholder: String::new(),
            cursor: 0,
            anchor: 0,
            preedit: None,
            undo: Vec::new(),
            redo: Vec::new(),
            caret_on: true,
            blinking: false,
            last_input: Duration::ZERO,
            defer_one_frame: crate::controls::Switch::default(),
            deferred: Vec::new(),
            commit_preedit_early: crate::controls::Switch::default(),
            caret_blink_enabled: true,
            password: false,
        }
    }
    /// A password field (§21.16): masked, not copyable, its value never exposed.
    pub fn password(mut self) -> Self {
        self.password = true;
        self
    }
    pub fn placeholder(mut self, p: &str) -> Self {
        self.placeholder = p.to_string();
        self
    }
    /// Settings: turn the caret blink off entirely.
    pub fn caret_blink(mut self, on: bool) -> Self {
        self.caret_blink_enabled = on;
        self
    }
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn with_fault_defer_update(mut self) -> Self {
        self.defer_one_frame.on = true;
        self
    }
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn with_fault_commit_preedit_early(mut self) -> Self {
        self.commit_preedit_early.on = true;
        self
    }

    /// The pre-edit text currently shown, if composing.
    pub fn preedit(&self) -> Option<&str> {
        self.preedit.as_ref().map(|(s, _)| s.as_str())
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn caret_visible(&self) -> bool {
        self.caret_on
    }
    pub fn is_blinking(&self) -> bool {
        self.blinking
    }

    fn clamp(&mut self, text: &str) {
        let fix = |i: usize| {
            let mut i = i.min(text.len());
            while !text.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        self.cursor = fix(self.cursor);
        self.anchor = fix(self.anchor);
    }

    fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    fn prev_boundary(text: &str, i: usize) -> usize {
        text[..i]
            .grapheme_indices(true)
            .next_back()
            .map(|(p, _)| p)
            .unwrap_or(0)
    }
    fn next_boundary(text: &str, i: usize) -> usize {
        text[i..]
            .graphemes(true)
            .next()
            .map(|g| i + g.len())
            .unwrap_or(text.len())
    }

    fn push_undo(&mut self, text: &str) {
        self.undo.push((text.to_string(), self.cursor));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn set_text(&mut self, cx: &mut EventCx, text: String) {
        self.model.set(cx.rt_mut(), text);
    }

    /// Replace the selection with `s` (one model write).
    fn insert(&mut self, cx: &mut EventCx, s: &str) {
        let mut text = self.model.get(cx.rt());
        self.clamp(&text);
        self.push_undo(&text);
        let (a, b) = self.selection();
        text.replace_range(a..b, s);
        self.cursor = a + s.len();
        self.anchor = self.cursor;
        self.set_text(cx, text);
    }

    fn delete(&mut self, cx: &mut EventCx, forward: bool) {
        let mut text = self.model.get(cx.rt());
        self.clamp(&text);
        let (mut a, mut b) = self.selection();
        if a == b {
            if forward {
                b = Self::next_boundary(&text, b);
            } else {
                a = Self::prev_boundary(&text, a);
            }
        }
        if a == b {
            return;
        }
        self.push_undo(&text);
        text.replace_range(a..b, "");
        self.cursor = a;
        self.anchor = a;
        self.set_text(cx, text);
    }

    fn touched(&mut self, cx: &mut EventCx) {
        self.last_input = cx.now();
        self.caret_on = true;
        if self.caret_blink_enabled && cx.caret_blink() && !cx.reduced_motion() {
            cx.cancel_timer(CARET_TAG);
            cx.set_timer(BLINK, CARET_TAG);
            self.blinking = true;
        }
        self.report_caret(cx);
        cx.request_paint();
        cx.request_a11y();
    }

    /// The text as drawn: the model with the pre-edit spliced in at the cursor.
    fn display(&self, text: &str) -> (String, usize) {
        if self.password {
            let masked = "•".repeat(text.chars().count());
            let at = text[..self.cursor.min(text.len())].chars().count() * '•'.len_utf8();
            return match &self.preedit {
                Some((p, _)) => {
                    let mut s = masked;
                    s.insert_str(at, p);
                    (s, at)
                }
                None => (masked, at),
            };
        }
        match &self.preedit {
            Some((p, _)) => {
                let mut s = text.to_string();
                let at = self.cursor.min(s.len());
                s.insert_str(at, p);
                (s, at)
            }
            None => (text.to_string(), self.cursor),
        }
    }

    /// Byte index in the drawn text (masked for passwords) of text index `i`.
    fn to_disp(&self, text: &str, i: usize) -> usize {
        if self.password {
            text[..i.min(text.len())].chars().count() * '•'.len_utf8()
        } else {
            i
        }
    }

    /// Text index of a byte index in the drawn text.
    fn text_index_of(&self, text: &str, d: usize) -> usize {
        if self.password {
            text.char_indices()
                .nth(d / '•'.len_utf8())
                .map_or(text.len(), |(i, _)| i)
        } else {
            d
        }
    }

    /// The caret's index in the drawn text (pre-edit included).
    fn caret_index(&self, text: &str) -> usize {
        let base = self.to_disp(text, self.cursor);
        match &self.preedit {
            Some((p, c)) => base + c.map(|(_, e)| e).unwrap_or(p.len()),
            None => base,
        }
    }

    fn report_caret(&self, cx: &mut EventCx) {
        let text = self.model.get(cx.rt());
        let (disp, _) = self.display(&text);
        let style = body(cx.theme());
        let pad = control_pad_x(cx.theme());
        let r = cx.rect();
        let (run, size) = cx.text().layout(&disp, &style, None);
        let x = cx.text().caret_x(run, self.caret_index(&text));
        let lh = size.h;
        cx.set_ime_caret(Rect::new(r.x + pad + x, r.y + (r.h - lh) * 0.5, 1.0, lh));
    }

    fn key(&mut self, cx: &mut EventCx, k: &crate::input::KeyEvent) -> Handled {
        if self.preedit.is_some() {
            // The IME owns the keyboard while composing.
            return Handled::Yes;
        }
        let text = self.model.get(cx.rt());
        self.clamp(&text);
        let m = k.mods;
        match &k.code {
            KeyCode::Char('a') if m.ctrl => {
                self.anchor = 0;
                self.cursor = text.len();
            }
            KeyCode::Char('c') | KeyCode::Char('x') if m.ctrl => {
                let (a, b) = self.selection();
                if a != b && !self.password {
                    cx.clipboard().set_text(&text[a..b]);
                    if k.code == KeyCode::Char('x') {
                        self.delete(cx, false);
                    }
                }
            }
            KeyCode::Char('v') if m.ctrl => {
                if let Some(s) = cx.clipboard().get_text() {
                    let line: String = s.chars().filter(|c| *c != '\n' && *c != '\r').collect();
                    self.insert(cx, &line);
                }
            }
            KeyCode::Char('z') if m.ctrl && !m.shift => {
                // Field-local undo runs before the global (project) undo; with nothing to
                // undo here, the key bubbles on to the global binding.
                let Some((t, c)) = self.undo.pop() else {
                    return Handled::No;
                };
                self.redo.push((text.clone(), self.cursor));
                self.cursor = c;
                self.anchor = c;
                self.set_text(cx, t);
            }
            KeyCode::Char('y') | KeyCode::Char('z') if m.ctrl => {
                let Some((t, c)) = self.redo.pop() else {
                    return Handled::No;
                };
                self.undo.push((text.clone(), self.cursor));
                self.cursor = c;
                self.anchor = c;
                self.set_text(cx, t);
            }
            KeyCode::Backspace => self.delete(cx, false),
            KeyCode::Delete => self.delete(cx, true),
            KeyCode::Left => {
                let (a, _) = self.selection();
                self.cursor = if self.cursor != self.anchor && !m.shift {
                    a
                } else {
                    Self::prev_boundary(&text, self.cursor)
                };
                if !m.shift {
                    self.anchor = self.cursor;
                }
            }
            KeyCode::Right => {
                let (_, b) = self.selection();
                self.cursor = if self.cursor != self.anchor && !m.shift {
                    b
                } else {
                    Self::next_boundary(&text, self.cursor)
                };
                if !m.shift {
                    self.anchor = self.cursor;
                }
            }
            KeyCode::Home => {
                self.cursor = 0;
                if !m.shift {
                    self.anchor = 0;
                }
            }
            KeyCode::End => {
                self.cursor = text.len();
                if !m.shift {
                    self.anchor = self.cursor;
                }
            }
            KeyCode::Enter => {
                let id = cx.id();
                cx.action(Submitted { field: id, text });
            }
            _ => match &k.text {
                Some(t) if !m.ctrl && !m.alt && !t.chars().any(char::is_control) => {
                    if self.defer_one_frame.on() {
                        self.deferred.push(t.clone());
                        cx.request_anim_frame();
                        return Handled::Yes;
                    }
                    let t = t.clone();
                    self.insert(cx, &t);
                }
                _ => return Handled::No,
            },
        }
        self.touched(cx);
        Handled::Yes
    }

    fn ime(&mut self, cx: &mut EventCx, ev: &ImeEvent) {
        match ev {
            ImeEvent::Preedit { text, cursor } => {
                if text.is_empty() {
                    self.preedit = None;
                } else {
                    if self.commit_preedit_early.on() {
                        let t = text.clone();
                        self.insert(cx, &t);
                    }
                    self.preedit = Some((text.clone(), *cursor));
                }
            }
            ImeEvent::Commit(s) => {
                self.preedit = None;
                self.insert(cx, s);
            }
            ImeEvent::Enabled | ImeEvent::Disabled => self.preedit = None,
        }
        self.touched(cx);
    }
}

impl Widget for TextField {
    fn role(&self) -> Role {
        Role::TextInput
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.model.any()), Dirty::PAINT | Dirty::A11Y);
    }
    fn focusable(&self) -> bool {
        true
    }
    fn accepts_text(&self) -> bool {
        true
    }
    fn hover_sensitive(&self) -> bool {
        false
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
        let (_, t) = cx.text.layout("Ag", &body(cx.theme), None);
        Size::new(
            known.width.unwrap_or(200.0),
            (t.h + 2.0 * control_pad_y(cx.theme)).max(CONTROL_MIN_H),
        )
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::FocusGained { .. } => {
                self.touched(cx);
                Handled::Yes
            }
            UiEvent::FocusLost => {
                self.preedit = None;
                self.blinking = false;
                self.caret_on = true;
                cx.cancel_timer(CARET_TAG);
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => self.key(cx, k),
            UiEvent::Ime(e) => {
                self.ime(cx, e);
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let text = self.model.get(cx.rt());
                let style = body(cx.theme());
                let pad = control_pad_x(cx.theme());
                let r = cx.rect();
                let (disp, _) = self.display(&text);
                let (run, _) = cx.text().layout(&disp, &style, None);
                let d = cx.text().hit_index(run, pos.x - r.x - pad);
                let i = self.text_index_of(&text, d);
                self.cursor = i;
                self.anchor = i;
                cx.capture_pointer();
                self.touched(cx);
                Handled::Yes
            }
            UiEvent::Timer(CARET_TAG) => {
                if cx.now().saturating_sub(self.last_input) >= CARET_BLINK_TIMEOUT {
                    // Blinking is over: solid caret, no timer, nothing scheduled.
                    self.blinking = false;
                    self.caret_on = true;
                } else {
                    self.caret_on = !self.caret_on;
                    cx.set_timer(BLINK, CARET_TAG);
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::AnimFrame => {
                for t in std::mem::take(&mut self.deferred) {
                    self.insert(cx, &t);
                }
                self.touched(cx);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let radius = cx.theme().radius.md;
        cx.panel(
            r,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            radius,
            None,
        );
        let text = self.model.get(cx.rt());
        let style = body(cx.theme());
        let pad = control_pad_x(cx.theme());
        let inner = Rect::new(r.x + pad, r.y, (r.w - 2.0 * pad).max(0.0), r.h);
        cx.primitive(Primitive::PushClip {
            rect: inner,
            radii: Corners::ZERO,
        });
        let (disp, pre_at) = self.display(&text);
        let (run, size) = cx.shape(&disp, &style);
        let y = r.y + (r.h - size.h) * 0.5;
        let focused = cx.state().focused;
        if disp.is_empty() {
            if !self.placeholder.is_empty() {
                cx.text(
                    &self.placeholder,
                    &style,
                    Point::new(inner.x, y),
                    ColorRole::FgMuted,
                );
            }
        } else {
            let (a, b) = (self.cursor.min(self.anchor), self.cursor.max(self.anchor));
            if focused && a != b && self.preedit.is_none() {
                let x0 = cx.text_system().caret_x(run, self.to_disp(&text, a));
                let x1 = cx.text_system().caret_x(run, self.to_disp(&text, b));
                let sel = cx.theme().color(ColorRole::Selection);
                cx.pair(ColorRole::FgPrimary, ColorRole::Selection, PairKind::Text);
                cx.primitive(Primitive::Quad {
                    rect: Rect::new(inner.x + x0, y, x1 - x0, size.h),
                    radii: Corners::ZERO,
                    fill: sel,
                    border: None,
                    shadow: None,
                });
            }
            cx.run(run, Point::new(inner.x, y), ColorRole::FgPrimary);
            if let Some((p, _)) = &self.preedit {
                let x0 = cx.text_system().caret_x(run, pre_at);
                let x1 = cx.text_system().caret_x(run, pre_at + p.len());
                cx.mark(
                    Rect::new(inner.x + x0, y + size.h - 2.0, (x1 - x0).max(1.0), 1.0),
                    ColorRole::FgPrimary,
                    ColorRole::BgSunken,
                    0.0,
                );
            }
        }
        if focused && self.caret_on {
            let x = cx.text_system().caret_x(run, self.caret_index(&text));
            cx.mark(
                Rect::new(inner.x + x, y, 1.5, size.h),
                ColorRole::FgPrimary,
                ColorRole::BgSunken,
                0.0,
            );
        }
        cx.primitive(Primitive::PopClip);
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        if self.password {
            node.set_role(Role::PasswordInput);
        } else {
            node.set_value(self.model.get(cx.rt));
        }
        if !self.placeholder.is_empty() {
            node.set_placeholder(self.placeholder.as_str());
        }
    }
}
