//! Navigation widgets (§21.16): breadcrumb, status bar, toolbar. The command palette is
//! in `palette.rs`; the dock tab strip is WP-U3's.
//!
//! * **Breadcrumb**: one tab stop; Left/Right move between segments, Home/End jump,
//!   Enter or a click raises [`BreadcrumbChosen`]. The last segment is the current
//!   location (announced as such). Each segment is an AccessKit link.
//! * **Status bar**: bound text items, left and right groups; a polite live region, so a
//!   changed status is announced without stealing focus. It is not a tab stop.
//! * **Toolbar**: a row container of buttons that is **one** tab stop (WAI-ARIA toolbar):
//!   Tab enters at the last-focused button, Left/Right/Home/End rove between buttons.

use accesskit::{Action, Live, Role};

use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::{CONTROL_MIN_H, body, small};

// ---- breadcrumb --------------------------------------------------------------------------

/// A breadcrumb segment was chosen.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BreadcrumbChosen {
    pub breadcrumb: WidgetId,
    pub index: usize,
}

const SEP: &str = "›";

/// A path of segments; the last is the current location.
pub struct Breadcrumb {
    segments: Signal<Vec<String>>,
    focus: usize,
    label: String,
}

impl Breadcrumb {
    pub fn new(segments: Signal<Vec<String>>, label: &str) -> Self {
        Self {
            segments,
            focus: usize::MAX,
            label: label.to_string(),
        }
    }
    pub fn focused_segment(&self) -> usize {
        self.focus
    }
    fn rects(
        &self,
        cx_text: &mut crate::text::TextSystem,
        theme: &crate::style::Theme,
        r: Rect,
        segs: &[String],
    ) -> Vec<Rect> {
        let st = body(theme);
        let sep = cx_text.layout(SEP, &st, None).1.w + 8.0;
        let mut x = r.x;
        segs.iter()
            .map(|s| {
                let w = cx_text.layout(s, &st, None).1.w + 8.0;
                let rr = Rect::new(x, r.y, w, r.h);
                x += w + sep;
                rr
            })
            .collect()
    }
    fn choose(&self, cx: &mut EventCx, i: usize) {
        let id = cx.id();
        cx.action(BreadcrumbChosen {
            breadcrumb: id,
            index: i,
        });
    }
}

impl Widget for Breadcrumb {
    fn role(&self) -> Role {
        Role::Navigation
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.segments.any()), Dirty::LAYOUT | Dirty::A11Y);
    }
    fn focusable(&self) -> bool {
        true
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        cx: &mut MeasureCx,
        _k: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let segs = self.segments.get(cx.rt);
        let st = body(cx.theme);
        let sep = cx.text.layout(SEP, &st, None).1.w + 8.0;
        let w: f32 = segs
            .iter()
            .map(|s| cx.text.layout(s, &st, None).1.w + 8.0)
            .sum::<f32>()
            + sep * segs.len().saturating_sub(1) as f32;
        Size::new(w, CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let segs = self.segments.get(cx.rt());
        let n = segs.len();
        if n == 0 {
            return Handled::No;
        }
        if self.focus >= n {
            self.focus = n - 1;
        }
        match ev {
            UiEvent::Key(k) if k.pressed => {
                match k.code {
                    KeyCode::Left => self.focus = self.focus.saturating_sub(1),
                    KeyCode::Right => self.focus = (self.focus + 1).min(n - 1),
                    KeyCode::Home => self.focus = 0,
                    KeyCode::End => self.focus = n - 1,
                    KeyCode::Enter | KeyCode::Space => self.choose(cx, self.focus),
                    _ => return Handled::No,
                }
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let theme = cx.theme().clone();
                let r = cx.rect();
                let rects = self.rects(&mut cx.text(), &theme, r, &segs);
                if let Some(i) = rects.iter().position(|q| q.contains(*pos)) {
                    self.focus = i;
                    self.choose(cx, i);
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::A11yChildAction(i, Action::Click) => {
                let i = (*i as usize).min(n - 1);
                self.focus = i;
                self.choose(cx, i);
                Handled::Yes
            }
            UiEvent::FocusGained { .. } | UiEvent::FocusLost => {
                cx.request_paint();
                Handled::No
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let segs = self.segments.get(cx.rt());
        let theme = cx.theme().clone();
        let st = body(&theme);
        let focused = cx.state().focused;
        let n = segs.len();
        let mut x = r.x;
        for (i, s) in segs.iter().enumerate() {
            let (run, t) = cx.shape(s, &st);
            let rr = Rect::new(x, r.y, t.w + 8.0, r.h);
            let last = i + 1 == n;
            if focused && i == self.focus.min(n.saturating_sub(1)) {
                cx.panel(rr, None, Some(ColorRole::FocusRing), theme.radius.sm, None);
            }
            cx.run(
                run,
                Point::new(rr.x + 4.0, r.y + (r.h - t.h) * 0.5),
                if last {
                    ColorRole::FgPrimary
                } else {
                    ColorRole::FgMuted
                },
            );
            if !last {
                // Underline marks a link without relying on colour.
                cx.mark(
                    Rect::new(rr.x + 4.0, r.y + (r.h + t.h) * 0.5 - 1.0, t.w, 1.0),
                    ColorRole::FgMuted,
                    cx.parent_bg(),
                    0.0,
                );
            }
            x = rr.right();
            if !last {
                let (srun, ss) = cx.shape(SEP, &st);
                cx.run(
                    srun,
                    Point::new(x + 4.0, r.y + (r.h - ss.h) * 0.5),
                    ColorRole::FgMuted,
                );
                x += ss.w + 8.0;
            }
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let segs = self.segments.get(cx.rt);
        let n = segs.len();
        for (i, s) in segs.iter().enumerate() {
            let mut node = accesskit::Node::new(Role::Link);
            node.set_label(s.as_str());
            node.add_action(Action::Click);
            node.set_position_in_set(i + 1);
            node.set_size_of_set(n);
            if i + 1 == n {
                node.set_aria_current(accesskit::AriaCurrent::Location);
            }
            out.push((i as u64, node));
        }
    }
    fn a11y_focus(&self, rt: &crate::state::Runtime) -> Option<u64> {
        let n = self.segments.get(rt).len();
        (n > 0).then(|| self.focus.min(n - 1) as u64)
    }
}

// ---- status bar --------------------------------------------------------------------------

/// A status bar item: bound text, in the left or right group.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct StatusItem {
    pub text: Signal<String>,
    pub right: bool,
}

/// The status bar.
pub struct StatusBar {
    items: Vec<StatusItem>,
}

impl StatusBar {
    pub fn new(items: Vec<StatusItem>) -> Self {
        Self { items }
    }
    fn text(&self, rt: &crate::state::Runtime) -> String {
        self.items
            .iter()
            .map(|i| i.text.get(rt))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

impl Widget for StatusBar {
    fn role(&self) -> Role {
        Role::Status
    }
    fn bind(&self, b: &mut Binder) {
        for i in &self.items {
            b.watch(Some(i.text.any()), Dirty::PAINT | Dirty::A11Y);
        }
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
        Size::new(known.width.unwrap_or(480.0), 24.0)
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgRaised, 0.0);
        cx.mark(
            Rect::new(r.x, r.y, r.w, 1.0),
            ColorRole::Border,
            ColorRole::BgRaised,
            0.0,
        );
        let st = small(cx.theme());
        let mut lx = r.x + 8.0;
        let mut rx = r.right() - 8.0;
        for i in &self.items {
            let t = i.text.get(cx.rt());
            if t.is_empty() {
                continue;
            }
            let (run, s) = cx.shape(&t, &st);
            let y = r.y + (r.h - s.h) * 0.5;
            if i.right {
                rx -= s.w;
                cx.run(run, Point::new(rx, y), ColorRole::FgMuted);
                rx -= 16.0;
            } else {
                cx.run(run, Point::new(lx, y), ColorRole::FgPrimary);
                lx += s.w + 16.0;
            }
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Status"));
        node.set_value(self.text(cx.rt));
        node.set_live(Live::Polite);
    }
}

// ---- toolbar -----------------------------------------------------------------------------

/// A toolbar: add buttons (any focusable widgets) as its children; it makes them one
/// tab stop with arrow-key roving.
pub struct Toolbar {
    label: String,
    last: Option<WidgetId>,
}

impl Toolbar {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            last: None,
        }
    }
}

impl Widget for Toolbar {
    fn role(&self) -> Role {
        Role::Toolbar
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let me = cx.id();
        let kids: Vec<WidgetId> = cx
            .children_of(me)
            .into_iter()
            .filter(|k| cx.is_focusable(*k))
            .collect();
        match ev {
            UiEvent::FocusGained { .. } => Handled::No,
            UiEvent::Key(k) if k.pressed && !k.mods.ctrl && !k.mods.alt => {
                let Some(cur) = cx.focused().and_then(|f| kids.iter().position(|k| *k == f)) else {
                    return Handled::No;
                };
                let n = kids.len();
                let to = match k.code {
                    KeyCode::Right | KeyCode::Down => (cur + 1) % n,
                    KeyCode::Left | KeyCode::Up => (cur + n - 1) % n,
                    KeyCode::Home => 0,
                    KeyCode::End => n - 1,
                    KeyCode::Tab => {
                        // Leave the toolbar in one step: skip its other buttons.
                        self.last = Some(kids[cur]);
                        let target = if k.mods.shift { kids[0] } else { kids[n - 1] };
                        if target != kids[cur] {
                            cx.focus(target, true);
                        }
                        return Handled::No;
                    }
                    _ => return Handled::No,
                };
                self.last = Some(kids[to]);
                cx.focus(kids[to], true);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_orientation(accesskit::Orientation::Horizontal);
    }
}
