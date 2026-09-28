//! The multiline editor (§21.16): wrapped text with selection, field-local undo, IME,
//! clipboard, an optional line-number gutter, and its own vertical scrolling that keeps
//! the caret in view. Tab leaves the editor (keyboard users are never trapped) unless
//! `tab_inserts` is set, in which case Ctrl+Tab leaves.

use accesskit::Role;

use crate::damage::Dirty;
use crate::geom::{Corners, Point, Rect, Size};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::render::Primitive;
use crate::state::Signal;
use crate::style::{ColorRole, PairKind};
use crate::text::TextStyle;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::line_edit::{EditOutcome, LineEdit};
use super::{body, control_pad_x, small};

/// A multiline text editor bound to a `Signal<String>`.
pub struct MultilineEditor {
    model: Signal<String>,
    label: String,
    edit: LineEdit,
    line_numbers: bool,
    tab_inserts: bool,
    mono: bool,
    scroll_y: f32,
    selecting: bool,
    /// Preferred caret x for vertical movement (keeps the column across short lines).
    goal_x: Option<f32>,
}

impl MultilineEditor {
    pub fn new(model: Signal<String>, label: &str) -> Self {
        Self {
            model,
            label: label.to_string(),
            edit: {
                let mut e = LineEdit::default();
                e.multiline = true;
                e
            },
            line_numbers: false,
            tab_inserts: false,
            mono: false,
            scroll_y: 0.0,
            selecting: false,
            goal_x: None,
        }
    }
    pub fn line_numbers(mut self) -> Self {
        self.line_numbers = true;
        self
    }
    /// Tab inserts four spaces (code); Ctrl+Tab then moves focus.
    pub fn tab_inserts(mut self) -> Self {
        self.tab_inserts = true;
        self
    }
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    /// The text it edits.
    pub fn model(&self) -> Signal<String> {
        self.model
    }
    pub fn cursor(&self) -> usize {
        self.edit.cursor
    }
    pub fn selection(&self) -> (usize, usize) {
        self.edit.selection()
    }
    pub fn scroll_y(&self) -> f32 {
        self.scroll_y
    }

    fn style(&self, theme: &crate::style::Theme) -> TextStyle {
        let mut s = if self.mono {
            TextStyle {
                mono: true,
                ..TextStyle::body(theme.type_scale.mono)
            }
        } else {
            body(theme)
        };
        s.wrap = true;
        s
    }

    fn gutter(&self, theme: &crate::style::Theme) -> f32 {
        if self.line_numbers {
            theme.type_scale.small * 2.6 + 8.0
        } else {
            0.0
        }
    }

    fn inner(&self, r: Rect, theme: &crate::style::Theme) -> Rect {
        let pad = control_pad_x(theme) * 0.5;
        let g = self.gutter(theme);
        Rect::new(
            r.x + pad + g,
            r.y + pad,
            (r.w - 2.0 * pad - g).max(1.0),
            (r.h - 2.0 * pad).max(1.0),
        )
    }

    fn sync_from_model(&mut self, cx: &mut EventCx) {
        let m = self.model.get(cx.rt());
        if m != self.edit.text {
            let c = self.edit.cursor.min(m.len());
            self.edit.set_text(&m);
            self.edit.set_cursor(c, false);
        }
    }

    fn write_model(&self, cx: &mut EventCx) {
        let t = self.edit.text.clone();
        self.model.set(cx.rt_mut(), t);
    }

    /// Shape the display text at the inner width; returns the run and inner rect.
    fn shaped(&self, cx: &mut EventCx) -> (crate::text::GlyphRunId, Rect) {
        let theme = cx.theme().clone();
        let inner = self.inner(cx.rect(), &theme);
        let (disp, _) = self.edit.display();
        let (run, _) = cx.text().layout(&disp, &self.style(&theme), Some(inner.w));
        (run, inner)
    }

    fn vertical(&mut self, cx: &mut EventCx, lines: f32, extend: bool) {
        let (run, _) = self.shaped(cx);
        let (x, top, h) = cx.text().caret_pos(run, self.edit.cursor);
        let gx = *self.goal_x.get_or_insert(x);
        let y = top + h * 0.5 + lines * h;
        let i = if y < 0.0 {
            0
        } else {
            cx.text().hit_point(run, gx, y)
        };
        self.edit.set_cursor(i, extend);
    }

    fn keep_caret_visible(&mut self, cx: &mut EventCx) {
        let (run, inner) = self.shaped(cx);
        let (x, top, h) = cx.text().caret_pos(run, self.edit.display_caret());
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if top + h > self.scroll_y + inner.h {
            self.scroll_y = top + h - inner.h;
        }
        let r = cx.rect();
        let _ = r;
        cx.set_ime_caret(Rect::new(
            inner.x + x,
            inner.y + top - self.scroll_y,
            1.0,
            h,
        ));
    }

    fn max_scroll(&self, cx: &mut EventCx) -> f32 {
        let (run, inner) = self.shaped(cx);
        let total = cx.text().run_size(run).map_or(0.0, |s| s.h);
        (total - inner.h).max(0.0)
    }
}

impl Widget for MultilineEditor {
    fn role(&self) -> Role {
        Role::MultilineTextInput
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
        let lh = cx.theme.type_scale.body * 1.35;
        Size::new(
            known.width.unwrap_or(320.0),
            known.height.unwrap_or(lh * 6.0 + control_pad_x(cx.theme)),
        )
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::BindingChanged | UiEvent::VisibilityChanged(true) => {
                self.sync_from_model(cx);
                Handled::No
            }
            UiEvent::FocusGained { .. } => {
                self.sync_from_model(cx);
                self.keep_caret_visible(cx);
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::FocusLost => {
                self.edit.preedit = None;
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => {
                self.sync_from_model(cx);
                if self.edit.preedit.is_some() {
                    return Handled::Yes;
                }
                let m = k.mods;
                match k.code {
                    KeyCode::Tab if self.tab_inserts && !m.ctrl && !m.shift => {
                        self.edit.insert("    ");
                        self.write_model(cx);
                    }
                    KeyCode::Tab => return Handled::No,
                    KeyCode::Up => self.vertical(cx, -1.0, m.shift),
                    KeyCode::Down => self.vertical(cx, 1.0, m.shift),
                    KeyCode::PageUp | KeyCode::PageDown => {
                        let (_, inner) = self.shaped(cx);
                        let lh = cx.theme().type_scale.body * 1.35;
                        let lines = (inner.h / lh).floor().max(1.0);
                        let dir = if k.code == KeyCode::PageUp { -1.0 } else { 1.0 };
                        self.vertical(cx, dir * lines, m.shift);
                    }
                    _ => {
                        self.goal_x = None;
                        match self.edit.key(k, cx.clipboard()) {
                            EditOutcome::Changed => self.write_model(cx),
                            EditOutcome::Ignored => return Handled::No,
                            // Escape leaves the editor's text alone and lets the key
                            // bubble (a dialog closes, say).
                            EditOutcome::Cancel => return Handled::No,
                            EditOutcome::Moved | EditOutcome::Submit => {}
                        }
                    }
                }
                if !matches!(
                    k.code,
                    KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
                ) {
                    self.goal_x = None;
                }
                self.keep_caret_visible(cx);
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::Ime(i) => {
                if self.edit.ime(i) == EditOutcome::Changed {
                    self.write_model(cx);
                }
                self.keep_caret_visible(cx);
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
            } => {
                self.sync_from_model(cx);
                let (run, inner) = self.shaped(cx);
                let i = cx
                    .text()
                    .hit_point(run, pos.x - inner.x, pos.y - inner.y + self.scroll_y);
                let extend = cx.modifiers().shift;
                self.edit.set_cursor(i, extend);
                if *clicks == 2 {
                    // Select the word under the pointer.
                    let t = &self.edit.text;
                    let start = t[..i]
                        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                        .map_or(0, |p| p + t[p..].chars().next().map_or(1, char::len_utf8));
                    let end = t[i..]
                        .find(|c: char| !c.is_alphanumeric() && c != '_')
                        .map_or(t.len(), |p| i + p);
                    self.edit.set_cursor(start, false);
                    self.edit.set_cursor(end, true);
                } else if *clicks >= 3 {
                    let t = &self.edit.text;
                    let start = t[..i].rfind('\n').map_or(0, |p| p + 1);
                    let end = t[i..].find('\n').map_or(t.len(), |p| i + p);
                    self.edit.set_cursor(start, false);
                    self.edit.set_cursor(end, true);
                }
                self.selecting = true;
                self.goal_x = None;
                cx.capture_pointer();
                self.keep_caret_visible(cx);
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if self.selecting => {
                let (run, inner) = self.shaped(cx);
                let i = cx
                    .text()
                    .hit_point(run, pos.x - inner.x, pos.y - inner.y + self.scroll_y);
                self.edit.set_cursor(i, true);
                self.keep_caret_visible(cx);
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PointerUp { .. } if self.selecting => {
                self.selecting = false;
                cx.release_pointer();
                Handled::Yes
            }
            UiEvent::Wheel { dy, .. } => {
                let max = self.max_scroll(cx);
                let ny = (self.scroll_y - dy).clamp(0.0, max);
                if (ny - self.scroll_y).abs() > f32::EPSILON {
                    self.scroll_y = ny;
                    cx.request_paint();
                    return Handled::Yes;
                }
                Handled::No
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
        let theme = cx.theme().clone();
        let inner = self.inner(r, &theme);
        let style = self.style(&theme);
        // The model is the truth for what is drawn when not focused (another widget or
        // the bus may have changed it).
        let focused = cx.state().focused;
        let model = self.model.get(cx.rt());
        let mut edit = self.edit.clone();
        if edit.text != model {
            edit.set_text(&model);
        }
        let (disp, pre_at) = edit.display();
        let (run, _) = cx.text.layout(&disp, &style, Some(inner.w));
        let origin = Point::new(inner.x, inner.y - self.scroll_y);
        let clip = Rect::new(r.x + 1.0, inner.y, r.w - 2.0, inner.h);
        cx.primitive(Primitive::PushClip {
            rect: clip,
            radii: Corners::ZERO,
        });
        // Selection: one rect per visual line it spans.
        let (a, b) = edit.selection();
        if focused && a != b && edit.preedit.is_none() {
            let sel = theme.color(ColorRole::Selection);
            cx.pair(ColorRole::FgPrimary, ColorRole::Selection, PairKind::Text);
            let (xa, ta, h) = cx.text_system().caret_pos(run, a);
            let (xb, tb, _) = cx.text_system().caret_pos(run, b);
            let mut rects = Vec::new();
            if (ta - tb).abs() < 0.5 {
                rects.push(Rect::new(xa, ta, xb - xa, h));
            } else {
                rects.push(Rect::new(xa, ta, inner.w - xa, h));
                let mut y = ta + h;
                while y + 0.5 < tb {
                    rects.push(Rect::new(0.0, y, inner.w, h));
                    y += h;
                }
                rects.push(Rect::new(0.0, tb, xb, h));
            }
            for sr in rects {
                cx.primitive(Primitive::Quad {
                    rect: sr.translate(origin.x, origin.y),
                    radii: Corners::ZERO,
                    fill: sel,
                    border: None,
                    shadow: None,
                });
            }
        }
        cx.run(run, origin, ColorRole::FgPrimary);
        if let Some((p, _)) = &edit.preedit {
            let (x0, t0, h) = cx.text_system().caret_pos(run, pre_at);
            let (x1, _, _) = cx.text_system().caret_pos(run, pre_at + p.len());
            cx.mark(
                Rect::new(
                    origin.x + x0,
                    origin.y + t0 + h - 2.0,
                    (x1 - x0).max(1.0),
                    1.0,
                ),
                ColorRole::FgPrimary,
                ColorRole::BgSunken,
                0.0,
            );
        }
        if focused {
            let (x, top, h) = cx.text_system().caret_pos(run, edit.display_caret());
            cx.mark(
                Rect::new(origin.x + x, origin.y + top, 1.5, h),
                ColorRole::FgPrimary,
                ColorRole::BgSunken,
                0.0,
            );
        }
        cx.primitive(Primitive::PopClip);
        if self.line_numbers {
            let st = small(&theme);
            let mut start = 0usize;
            let g = self.gutter(&theme);
            cx.primitive(Primitive::PushClip {
                rect: Rect::new(r.x, inner.y, g, inner.h),
                radii: Corners::ZERO,
            });
            for (n, line) in edit.text.split('\n').enumerate() {
                let (_, top, h) = cx.text_system().caret_pos(run, start);
                let y = origin.y + top;
                if y + h >= inner.y && y <= inner.bottom() {
                    let label = (n + 1).to_string();
                    let (_, t) = cx.shape(&label, &st);
                    cx.text(
                        &label,
                        &st,
                        Point::new(r.x + g - t.w - 6.0, y + (h - t.h) * 0.5),
                        ColorRole::FgMuted,
                    );
                }
                start += line.len() + 1;
            }
            cx.primitive(Primitive::PopClip);
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_value(self.model.get(cx.rt));
    }
}
