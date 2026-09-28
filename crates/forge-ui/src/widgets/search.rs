//! Search field (fuzzy, debounced) and path field (§21.16).
//!
//! * **Search**: typing updates the query and raises `SearchChanged` once typing pauses
//!   for [`SEARCH_DEBOUNCE`] (one timer; nothing polled). Enter searches immediately;
//!   Escape clears a non-empty query, and otherwise bubbles (closing a popup, say).
//!   The consumer ranks candidates with [`crate::fuzzy::rank`].
//! * **Path**: a path entry that accepts OS file drops and an optional existence check;
//!   Enter or leaving the field raises `PathChosen`.

use std::path::PathBuf;
use std::time::Duration;

use accesskit::{Action, Role};

use crate::damage::Dirty;
use crate::dnd::{DragPayload, DropVerdict};
use crate::geom::{Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, PointerButton, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::line_edit::{EditOutcome, LineEdit};
use super::{CONTROL_MIN_H, body, control_pad_x};

/// Typing pauses this long before a search runs.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
const DEBOUNCE_TAG: u64 = 1;

/// Raised with the (debounced) query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchChanged {
    pub field: WidgetId,
    pub query: String,
}

/// A search box bound to a `Signal<String>` that holds the debounced query.
pub struct SearchField {
    query: Signal<String>,
    label: String,
    edit: LineEdit,
    pending: bool,
}

impl SearchField {
    pub fn new(query: Signal<String>, label: &str) -> Self {
        Self {
            query,
            label: label.to_string(),
            edit: LineEdit::default(),
            pending: false,
        }
    }
    /// The text as typed (the query updates after the debounce).
    pub fn text(&self) -> &str {
        &self.edit.text
    }
    fn fire(&mut self, cx: &mut EventCx) {
        self.pending = false;
        cx.cancel_timer(DEBOUNCE_TAG);
        let q = self.edit.text.clone();
        self.query.set(cx.rt_mut(), q.clone());
        let field = cx.id();
        cx.action(SearchChanged { field, query: q });
    }
    fn clear_rect(r: Rect) -> Rect {
        Rect::new(r.right() - r.h, r.y, r.h, r.h)
    }
}

impl Widget for SearchField {
    fn role(&self) -> Role {
        Role::SearchInput
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.query.any()), Dirty::A11Y);
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
        _cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::new(known.width.unwrap_or(220.0), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let out = self.edit.key(k, cx.clipboard());
                match out {
                    EditOutcome::Changed => {
                        self.pending = true;
                        cx.cancel_timer(DEBOUNCE_TAG);
                        cx.set_timer(SEARCH_DEBOUNCE, DEBOUNCE_TAG);
                        cx.request_paint();
                        Handled::Yes
                    }
                    EditOutcome::Submit => {
                        self.fire(cx);
                        Handled::Yes
                    }
                    EditOutcome::Cancel if !self.edit.text.is_empty() => {
                        self.edit.set_text("");
                        self.fire(cx);
                        cx.request_paint();
                        Handled::Yes
                    }
                    EditOutcome::Cancel | EditOutcome::Ignored => Handled::No,
                    EditOutcome::Moved => {
                        cx.request_paint();
                        Handled::Yes
                    }
                }
            }
            UiEvent::Ime(i) => {
                if self.edit.ime(i) == EditOutcome::Changed {
                    self.pending = true;
                    cx.cancel_timer(DEBOUNCE_TAG);
                    cx.set_timer(SEARCH_DEBOUNCE, DEBOUNCE_TAG);
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::Timer(DEBOUNCE_TAG) if self.pending => {
                self.fire(cx);
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let r = cx.rect();
                if !self.edit.text.is_empty() && Self::clear_rect(r).contains(*pos) {
                    self.edit.set_text("");
                    self.fire(cx);
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::FocusGained { .. } | UiEvent::FocusLost => {
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::A11yAction(Action::SetValue) => Handled::No,
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let radius = r.h * 0.5;
        cx.panel(
            r,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            radius,
            None,
        );
        let style = body(cx.theme());
        let (_, g) = cx.shape("⌕", &style);
        cx.text(
            "⌕",
            &style,
            Point::new(r.x + 8.0, r.y + (r.h - g.h) * 0.5),
            ColorRole::FgMuted,
        );
        let pad = control_pad_x(cx.theme());
        let inner = Rect::new(
            r.x + 12.0 + g.w,
            r.y,
            (r.w - 12.0 - g.w - r.h - pad * 0.5).max(0.0),
            r.h,
        );
        let focused = cx.state().focused;
        self.edit.paint_line(
            cx,
            inner,
            &style,
            focused,
            true,
            &self.label,
            ColorRole::BgSunken,
        );
        if !self.edit.text.is_empty() {
            let c = Self::clear_rect(r);
            let (_, t) = cx.shape("✕", &style);
            cx.text(
                "✕",
                &style,
                Point::new(c.x + (c.w - t.w) * 0.5, c.y + (c.h - t.h) * 0.5),
                ColorRole::FgMuted,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_value(self.edit.text.as_str());
        node.set_placeholder(self.label.as_str());
    }
}

/// Raised when a path is committed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathChosen {
    pub field: WidgetId,
    pub path: PathBuf,
}

/// A file-system path entry. Accepts OS file drops; optionally refuses paths that do not
/// exist (the reason is shown and read out).
pub struct PathField {
    label: String,
    edit: LineEdit,
    must_exist: bool,
    error: Option<String>,
    drop_hover: bool,
}

impl PathField {
    pub fn new(label: &str, initial: &str) -> Self {
        Self {
            label: label.to_string(),
            edit: LineEdit::new(initial),
            must_exist: false,
            error: None,
            drop_hover: false,
        }
    }
    pub fn must_exist(mut self) -> Self {
        self.must_exist = true;
        self
    }
    pub fn text(&self) -> &str {
        &self.edit.text
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    fn commit(&mut self, cx: &mut EventCx) {
        let p = PathBuf::from(self.edit.text.trim());
        if self.must_exist && !p.exists() {
            self.error = Some(crate::trf!("{path} does not exist", path = p.display()));
        } else {
            self.error = None;
            let field = cx.id();
            cx.action(PathChosen { field, path: p });
        }
        cx.request_paint();
        cx.request_a11y();
    }
}

impl Widget for PathField {
    fn role(&self) -> Role {
        Role::TextInput
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
        _cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::new(known.width.unwrap_or(260.0), CONTROL_MIN_H)
    }
    fn tooltip(&self, _rt: &crate::state::Runtime) -> Option<String> {
        self.error.clone()
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::Key(k) if k.pressed => match self.edit.key(k, cx.clipboard()) {
                EditOutcome::Submit => {
                    self.commit(cx);
                    Handled::Yes
                }
                EditOutcome::Ignored | EditOutcome::Cancel => Handled::No,
                _ => {
                    self.error = None;
                    cx.request_paint();
                    Handled::Yes
                }
            },
            UiEvent::Ime(i) => {
                self.edit.ime(i);
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::FocusLost => {
                self.commit(cx);
                Handled::Yes
            }
            UiEvent::DragOver(p) => {
                let ok = matches!(p, DragPayload::Files(f) if f.len() == 1);
                cx.set_drop_verdict(if ok {
                    DropVerdict::Accepted
                } else {
                    DropVerdict::Refused(crate::tr!("drop one file or folder").into())
                });
                self.drop_hover = ok;
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::Drop(DragPayload::Files(f)) => {
                if let Some(p) = f.first() {
                    self.edit.set_text(&p.display().to_string());
                    self.drop_hover = false;
                    self.commit(cx);
                }
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
        } else if self.drop_hover {
            ColorRole::FocusRing
        } else {
            ColorRole::Border
        };
        cx.panel(r, Some(ColorRole::BgSunken), Some(border), radius, None);
        let style = body(cx.theme());
        let (_, g) = cx.shape("🗀", &style);
        cx.text(
            "🗀",
            &style,
            Point::new(r.x + 6.0, r.y + (r.h - g.h) * 0.5),
            ColorRole::FgMuted,
        );
        let inner = Rect::new(r.x + 12.0 + g.w, r.y, (r.w - 18.0 - g.w).max(0.0), r.h);
        let focused = cx.state().focused;
        self.edit.paint_line(
            cx,
            inner,
            &style,
            focused,
            true,
            crate::tr!("Path"),
            ColorRole::BgSunken,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_value(self.edit.text.as_str());
        if let Some(e) = &self.error {
            node.set_description(e.as_str());
            node.set_invalid(accesskit::Invalid::True);
        }
    }
}
