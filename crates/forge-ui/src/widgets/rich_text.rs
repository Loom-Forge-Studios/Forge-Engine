//! Rich text (§21.16): styled spans, links and inline icons in one shaped paragraph, so
//! wrapping and bidi run across span boundaries. Links are keyboard-operable: the
//! paragraph is one tab stop, Left/Right move between its links, Enter follows one; each
//! link is an AccessKit `Link` node.

use accesskit::{Action, Role};

use crate::geom::{Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::style::ColorRole;
use crate::text::{RichSpan, TextStyle};
use crate::widget::{A11yCx, EventCx, MeasureCx, PaintCx, Widget};

use super::body;

/// How a span is styled.
#[derive(Clone, Debug, PartialEq)]
pub enum SpanKind {
    Plain,
    Strong,
    Emphasis,
    Code,
    Muted,
    /// A link to `target` (an URL, a doc anchor, an asset path).
    Link(String),
    /// An inline icon glyph with its accessible name.
    Icon(String),
}

/// One span.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub kind: SpanKind,
}

impl Span {
    pub fn plain(t: &str) -> Self {
        Self {
            text: t.into(),
            kind: SpanKind::Plain,
        }
    }
    pub fn strong(t: &str) -> Self {
        Self {
            text: t.into(),
            kind: SpanKind::Strong,
        }
    }
    pub fn em(t: &str) -> Self {
        Self {
            text: t.into(),
            kind: SpanKind::Emphasis,
        }
    }
    pub fn code(t: &str) -> Self {
        Self {
            text: t.into(),
            kind: SpanKind::Code,
        }
    }
    pub fn muted(t: &str) -> Self {
        Self {
            text: t.into(),
            kind: SpanKind::Muted,
        }
    }
    pub fn link(t: &str, target: &str) -> Self {
        Self {
            text: t.into(),
            kind: SpanKind::Link(target.into()),
        }
    }
    pub fn icon(glyph: &str, name: &str) -> Self {
        Self {
            text: glyph.into(),
            kind: SpanKind::Icon(name.into()),
        }
    }
}

/// Raised when a link is followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkActivated {
    pub source: WidgetId,
    pub target: String,
}

/// A paragraph of rich text.
pub struct RichText {
    spans: Vec<Span>,
    wrap: bool,
    focus_link: usize,
    width: f32,
}

impl RichText {
    pub fn new(spans: Vec<Span>) -> Self {
        Self {
            spans,
            wrap: true,
            focus_link: 0,
            width: 0.0,
        }
    }
    pub fn no_wrap(mut self) -> Self {
        self.wrap = false;
        self
    }

    fn links(&self) -> Vec<usize> {
        self.spans
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.kind, SpanKind::Link(_)))
            .map(|(i, _)| i)
            .collect()
    }

    fn style(&self, theme: &crate::style::Theme) -> TextStyle {
        let mut s = body(theme);
        s.wrap = self.wrap;
        s
    }

    fn rich(&self) -> Vec<RichSpan<'_>> {
        self.spans
            .iter()
            .enumerate()
            .map(|(i, s)| RichSpan {
                text: &s.text,
                weight: matches!(s.kind, SpanKind::Strong).then_some(600),
                italic: matches!(s.kind, SpanKind::Emphasis),
                mono: matches!(s.kind, SpanKind::Code),
                tag: u16::try_from(i).unwrap_or(u16::MAX),
            })
            .collect()
    }

    fn colors(&self) -> Vec<ColorRole> {
        self.spans
            .iter()
            .map(|s| match s.kind {
                SpanKind::Muted | SpanKind::Icon(_) => ColorRole::FgMuted,
                _ => ColorRole::FgPrimary,
            })
            .collect()
    }

    /// The plain text a screen reader reads (icons by their names).
    pub fn plain_text(&self) -> String {
        self.spans
            .iter()
            .map(|s| match &s.kind {
                SpanKind::Icon(name) => name.clone(),
                _ => s.text.clone(),
            })
            .collect()
    }

    fn link_at(&self, cx: &mut EventCx, pos: Point) -> Option<usize> {
        let r = cx.rect();
        let style = self.style(cx.theme());
        let wrap = self.wrap.then_some(r.w);
        let rich = self.rich();
        let (run, _) = cx.text().layout_rich(&rich, &style, wrap);
        let t = cx.text();
        self.links().into_iter().find(|&i| {
            t.tag_rects(run, i)
                .iter()
                .any(|lr| lr.translate(r.x, r.y).contains(pos))
        })
    }

    fn activate(&self, cx: &mut EventCx, span: usize) {
        if let Some(Span {
            kind: SpanKind::Link(target),
            ..
        }) = self.spans.get(span)
        {
            let source = cx.id();
            cx.action(LinkActivated {
                source,
                target: target.clone(),
            });
        }
    }
}

impl Widget for RichText {
    fn role(&self) -> Role {
        Role::Paragraph
    }
    fn focusable(&self) -> bool {
        !self.links().is_empty()
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        avail: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let style = self.style(cx.theme);
        let width = if self.wrap {
            known.width.or(match avail.width {
                taffy::AvailableSpace::Definite(w) => Some(w),
                taffy::AvailableSpace::MinContent => Some(0.0),
                taffy::AvailableSpace::MaxContent => None,
            })
        } else {
            None
        };
        self.width = width.unwrap_or(0.0);
        let rich = self.rich();
        cx.text.layout_rich(&rich, &style, width).1
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let links = self.links();
        if links.is_empty() {
            return Handled::No;
        }
        match ev {
            UiEvent::Key(k) if k.pressed => match k.code {
                KeyCode::Right | KeyCode::Down => {
                    self.focus_link = (self.focus_link + 1) % links.len();
                    cx.request_paint();
                    cx.request_a11y();
                    Handled::Yes
                }
                KeyCode::Left | KeyCode::Up => {
                    self.focus_link = (self.focus_link + links.len() - 1) % links.len();
                    cx.request_paint();
                    cx.request_a11y();
                    Handled::Yes
                }
                KeyCode::Enter => {
                    let s = links[self.focus_link.min(links.len() - 1)];
                    self.activate(cx, s);
                    Handled::Yes
                }
                _ => Handled::No,
            },
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                if let Some(s) = self.link_at(cx, *pos) {
                    self.focus_link = links.iter().position(|l| *l == s).unwrap_or(0);
                    self.activate(cx, s);
                    cx.request_paint();
                    return Handled::Yes;
                }
                Handled::No
            }
            UiEvent::A11yChildAction(local, Action::Click) => {
                self.activate(cx, *local as usize);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let style = self.style(cx.theme());
        let rich = self.rich();
        let colors = self.colors();
        let (run, _) = cx.rich_text(&rich, &style, Point::new(r.x, r.y), &colors);
        let links = self.links();
        let bg = cx.current_bg();
        let focused = cx.state().focus_visible;
        for (li, &span) in links.iter().enumerate() {
            let rects = cx.text_system().tag_rects(run, span);
            for lr in rects {
                let lr = lr.translate(r.x, r.y);
                // Links are underlined: not colour alone (WCAG 1.4.1).
                let thick = if focused && li == self.focus_link {
                    2.0
                } else {
                    1.0
                };
                cx.mark(
                    Rect::new(lr.x, lr.bottom() - 2.0, lr.w, thick),
                    ColorRole::FgPrimary,
                    bg,
                    0.0,
                );
                if focused && li == self.focus_link {
                    cx.panel(lr.outset(1.0), None, Some(ColorRole::FocusRing), 2.0, None);
                }
            }
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.plain_text());
    }
    fn a11y_children(&self, _cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        for i in self.links() {
            let mut n = accesskit::Node::new(Role::Link);
            n.set_label(self.spans[i].text.as_str());
            if let SpanKind::Link(t) = &self.spans[i].kind {
                n.set_url(t.as_str());
            }
            n.add_action(Action::Click);
            out.push((i as u64, n));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        self.links().get(self.focus_link).map(|i| *i as u64)
    }
}
