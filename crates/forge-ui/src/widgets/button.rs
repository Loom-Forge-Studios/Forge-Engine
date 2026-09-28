use accesskit::{Action, Role};

use crate::damage::Dirty;
use crate::geom::{Point, Size};
use crate::icons::Icon;
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::state::Bind;
use crate::style::{Variant, button_style};
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::{CONTROL_MIN_H, body, control_pad_x, control_pad_y};

/// Raised by every button on activation (pointer release inside, Enter/Space, or an
/// assistive technology's Click).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Pressed(pub WidgetId);

type OnPress = Box<dyn Fn(&mut EventCx)>;

pub(crate) fn activation(ev: &UiEvent) -> bool {
    match ev {
        UiEvent::PointerUp {
            button: PointerButton::Primary,
            inside: true,
            ..
        } => true,
        UiEvent::Key(k) if k.pressed && !k.repeat => {
            // Shift+Space is the dock's maximise chord (Ch.21.17), never an activation.
            matches!(k.code, KeyCode::Enter | KeyCode::Space)
                && !k.mods.ctrl
                && !k.mods.alt
                && !(k.mods.shift && k.code == KeyCode::Space)
        }
        UiEvent::A11yAction(Action::Click) => true,
        _ => false,
    }
}

/// Button sizes (§21.16): the pointer target grows with the size; the type scale follows.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl ButtonSize {
    fn min_h(self) -> f32 {
        match self {
            ButtonSize::Small => 22.0,
            ButtonSize::Medium => CONTROL_MIN_H,
            ButtonSize::Large => 36.0,
        }
    }
    fn style(self, theme: &crate::style::Theme) -> crate::text::TextStyle {
        match self {
            ButtonSize::Small => crate::text::TextStyle::body(theme.type_scale.small),
            ButtonSize::Medium => body(theme),
            ButtonSize::Large => crate::text::TextStyle::body(theme.type_scale.body + 2.0),
        }
    }
    fn pad_x(self, theme: &crate::style::Theme) -> f32 {
        match self {
            ButtonSize::Small => theme.space[2],
            ButtonSize::Medium => control_pad_x(theme),
            ButtonSize::Large => theme.space[4],
        }
    }
}

/// A text button (primary / secondary / ghost / danger; small / medium / large).
pub struct Button {
    label: Bind<String>,
    variant: Variant,
    size: ButtonSize,
    on_press: Option<OnPress>,
}

impl Button {
    pub fn new(label: impl Into<Bind<String>>) -> Self {
        Self {
            label: label.into(),
            variant: Variant::Secondary,
            size: ButtonSize::Medium,
            on_press: None,
        }
    }
    pub fn size(mut self, s: ButtonSize) -> Self {
        self.size = s;
        self
    }
    pub fn variant(mut self, v: Variant) -> Self {
        self.variant = v;
        self
    }
    pub fn primary(self) -> Self {
        self.variant(Variant::Primary)
    }
    /// Run on activation (typically `cx.action(MyAction)`).
    pub fn on_press(mut self, f: impl Fn(&mut EventCx) + 'static) -> Self {
        self.on_press = Some(Box::new(f));
        self
    }
}

impl Widget for Button {
    fn role(&self) -> Role {
        Role::Button
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(self.label.source(), Dirty::LAYOUT);
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
        _known: taffy::Size<Option<f32>>,
        _avail: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let (_, t) = cx
            .text
            .layout(&self.label.get(cx.rt), &self.size.style(cx.theme), None);
        Size::new(
            t.w + 2.0 * self.size.pad_x(cx.theme),
            (t.h + 2.0 * control_pad_y(cx.theme)).max(self.size.min_h()),
        )
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if activation(ev) {
            cx.action(Pressed(cx.id()));
            if let Some(f) = &self.on_press {
                f(cx);
            }
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let st = button_style(self.variant, cx.state());
        let radius = cx.theme().radius.md;
        cx.panel(r, st.bg, st.border, radius, None);
        let text = self.label.get(cx.rt());
        let style = self.size.style(cx.theme());
        let (_, t) = cx.shape(&text, &style);
        let o = Point::new(r.x + (r.w - t.w) * 0.5, r.y + (r.h - t.h) * 0.5);
        cx.text(&text, &style, o, st.fg);
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.get(cx.rt));
        node.add_action(Action::Click);
    }
}

/// An icon-only button. It **must** carry a label for assistive technology; one built
/// without ([`IconButton::unlabelled`]) fails `test_ui_a11y_tree`.
pub struct IconButton {
    glyph: &'static str,
    /// The vector icon drawn (the Forge icon set, [`crate::icons`]); `None`: the glyph.
    icon: Option<Icon>,
    label: Option<String>,
    on_press: Option<OnPress>,
}

impl IconButton {
    /// `glyph` names the icon — one of the set's ([`crate::icons::Icon`]) when it stands
    /// for one (✕, ⚙, ⓘ, ✎), otherwise drawn from the UI font; `label` is what a screen
    /// reader announces.
    pub fn new(glyph: &'static str, label: &str) -> Self {
        Self {
            glyph,
            icon: crate::icons::for_glyph(glyph),
            label: Some(label.to_string()),
            on_press: None,
        }
    }
    /// A button showing `icon` from the Forge icon set.
    pub fn icon(icon: Icon, label: &str) -> Self {
        Self {
            glyph: "",
            icon: Some(icon),
            label: Some(label.to_string()),
            on_press: None,
        }
    }
    /// An icon button with no accessible name — a defect; exists for the a11y guard's
    /// positive control.
    pub fn unlabelled(glyph: &'static str) -> Self {
        Self {
            glyph,
            icon: crate::icons::for_glyph(glyph),
            label: None,
            on_press: None,
        }
    }
    pub fn on_press(mut self, f: impl Fn(&mut EventCx) + 'static) -> Self {
        self.on_press = Some(Box::new(f));
        self
    }
}

impl Widget for IconButton {
    fn role(&self) -> Role {
        Role::Button
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
        _known: taffy::Size<Option<f32>>,
        _avail: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::new(CONTROL_MIN_H, CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if activation(ev) {
            cx.action(Pressed(cx.id()));
            if let Some(f) = &self.on_press {
                f(cx);
            }
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let st = button_style(Variant::Ghost, cx.state());
        let radius = cx.theme().radius.md;
        cx.panel(r, st.bg, None, radius, None);
        if let Some(icon) = self.icon {
            let side = (r.w.min(r.h) * 0.6).round();
            let ir = crate::geom::Rect::new(
                r.x + (r.w - side) * 0.5,
                r.y + (r.h - side) * 0.5,
                side,
                side,
            );
            cx.icon(icon, ir, st.fg);
            return;
        }
        let style = body(cx.theme());
        let (_, t) = cx.shape(self.glyph, &style);
        let o = Point::new(r.x + (r.w - t.w) * 0.5, r.y + (r.h - t.h) * 0.5);
        cx.text(self.glyph, &style, o, st.fg);
    }
    fn tooltip(&self, _rt: &crate::state::Runtime) -> Option<String> {
        self.label.clone()
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        if let Some(l) = &self.label {
            node.set_label(l.as_str());
        }
        node.add_action(Action::Click);
    }
}
