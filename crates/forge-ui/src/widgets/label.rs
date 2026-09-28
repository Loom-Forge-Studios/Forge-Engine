use accesskit::Role;

use crate::damage::Dirty;
use crate::geom::{Point, Size};
use crate::state::Bind;
use crate::style::ColorRole;
use crate::text::TextStyle;
use crate::widget::{A11yCx, Binder, MeasureCx, PaintCx, Widget};

/// Which text token a label uses.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LabelKind {
    Body,
    Muted,
    Heading,
    Small,
    Mono,
    /// A caution (gimbal lock, lossy action): body text; its author marks it with a
    /// warning glyph so colour is never the only carrier.
    Warning,
}

/// Static or bound text. Re-shapes only when its text, style or scale change (§21.7).
pub struct Label {
    text: Bind<String>,
    kind: LabelKind,
    wrap: bool,
    tip: Option<String>,
    verbatim: bool,
}

/// The AccessKit class of a label shown verbatim (see [`Label::verbatim`]).
pub const VERBATIM_CLASS: &str = "forge-verbatim";

impl Label {
    pub fn new(text: impl Into<Bind<String>>) -> Self {
        Self {
            text: text.into(),
            kind: LabelKind::Body,
            wrap: false,
            tip: None,
            verbatim: false,
        }
    }
    /// Text shown exactly as a file or a build carries it (a `NOTICES` preview, the credit a
    /// build writes): never translated. Marked for assistive technology and for the
    /// pseudo-locale audit by its AccessKit class ([`VERBATIM_CLASS`]).
    pub fn verbatim(mut self) -> Self {
        self.verbatim = true;
        self
    }
    pub fn kind(mut self, k: LabelKind) -> Self {
        self.kind = k;
        self
    }
    pub fn heading(self) -> Self {
        self.kind(LabelKind::Heading)
    }
    pub fn muted(self) -> Self {
        self.kind(LabelKind::Muted)
    }
    /// The text shown.
    pub fn text(&self, rt: &crate::state::Runtime) -> String {
        self.text.get(rt)
    }
    /// Hover text (an inspector row shows its field's doc comment).
    pub fn tooltip(mut self, tip: &str) -> Self {
        self.tip = (!tip.is_empty()).then(|| tip.to_string());
        self
    }
    pub fn wrapping(mut self) -> Self {
        self.wrap = true;
        self
    }

    fn style(&self, theme: &crate::style::Theme) -> TextStyle {
        let ts = &theme.type_scale;
        let mut s = match self.kind {
            LabelKind::Body | LabelKind::Muted | LabelKind::Warning => TextStyle::body(ts.body),
            LabelKind::Heading => TextStyle::body(ts.heading).strong(),
            LabelKind::Small => TextStyle::body(ts.small),
            LabelKind::Mono => TextStyle {
                mono: true,
                ..TextStyle::body(ts.mono)
            },
        };
        s.wrap = self.wrap;
        s
    }

    fn fg(&self) -> ColorRole {
        match self.kind {
            LabelKind::Muted | LabelKind::Small => ColorRole::FgMuted,
            _ => ColorRole::FgPrimary,
        }
    }
}

impl Widget for Label {
    fn role(&self) -> Role {
        // A caution is a status message: an assistive technology announces it when it
        // appears (a polite live region) instead of leaving the user to stumble on it.
        match self.kind {
            LabelKind::Warning => Role::Status,
            _ => Role::Label,
        }
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(self.text.source(), Dirty::LAYOUT);
    }
    fn measured(&self) -> bool {
        true
    }
    fn tooltip(&self, _rt: &crate::state::Runtime) -> Option<String> {
        self.tip.clone()
    }
    fn measure(
        &mut self,
        cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        avail: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let s = self.text.get(cx.rt);
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
        let (_, size) = cx.text.layout(&s, &style, width);
        size
    }
    fn paint(&self, cx: &mut PaintCx) {
        let s = self.text.get(cx.rt());
        let style = self.style(cx.theme());
        let r = cx.rect();
        cx.text(&s, &style, Point::new(r.x, r.y), self.fg());
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        // AccessKit exposes static text through `value` (UIA Name / AT-SPI text).
        let t = self.text.get(cx.rt);
        node.set_value(t.clone());
        node.set_label(t);
        if self.verbatim {
            node.set_class_name(VERBATIM_CLASS);
        }
        if self.kind == LabelKind::Warning {
            node.set_live(accesskit::Live::Polite);
        }
    }
}
