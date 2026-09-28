//! Text-and-display widgets (§21.16): icon, image, separator, badge, keyboard-shortcut
//! hint, skeleton placeholder, empty state, group box and collapsible section header.

use accesskit::{Action, Role};

use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::render::ImageId;
use crate::state::Signal;
use crate::style::ColorRole;
use crate::text::TextStyle;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::{CONTROL_MIN_H, body, small};

/// A single-colour glyph icon (the Lucide icon atlas replaces glyphs in WP-U12). An icon
/// that carries meaning has a label; a decorative one is hidden from assistive technology.
pub struct Icon {
    glyph: &'static str,
    label: Option<String>,
    role: ColorRole,
    size: f32,
}

impl Icon {
    pub fn new(glyph: &'static str, label: &str) -> Self {
        Self {
            glyph,
            label: Some(label.to_string()),
            role: ColorRole::FgPrimary,
            size: 16.0,
        }
    }
    /// Purely decorative (next to text that already says it).
    pub fn decorative(glyph: &'static str) -> Self {
        Self {
            glyph,
            label: None,
            role: ColorRole::FgMuted,
            size: 16.0,
        }
    }
    pub fn tint(mut self, role: ColorRole) -> Self {
        self.role = role;
        self
    }
    pub fn size(mut self, px: f32) -> Self {
        self.size = px;
        self
    }
    fn style(&self) -> TextStyle {
        TextStyle::body(self.size)
    }
}

impl Widget for Icon {
    fn role(&self) -> Role {
        if self.label.is_some() {
            Role::Image
        } else {
            Role::GenericContainer
        }
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
        let (_, t) = cx.text.layout(self.glyph, &self.style(), None);
        Size::new(t.w.max(self.size), t.h.max(self.size))
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        // A glyph that stands for an icon of the set draws the vector icon.
        if let Some(icon) = crate::icons::for_glyph(self.glyph) {
            let ir = Rect::new(
                r.x + (r.w - self.size) * 0.5,
                r.y + (r.h - self.size) * 0.5,
                self.size,
                self.size,
            );
            cx.icon(icon, ir, self.role);
            return;
        }
        let (_, t) = cx.shape(self.glyph, &self.style());
        cx.text(
            self.glyph,
            &self.style(),
            Point::new(r.x + (r.w - t.w) * 0.5, r.y + (r.h - t.h) * 0.5),
            self.role,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        match &self.label {
            Some(l) => node.set_label(l.as_str()),
            None => node.set_hidden(),
        }
    }
}

/// An RGBA image from the image atlas (thumbnails), with alt text.
pub struct ImageView {
    image: ImageId,
    size: Size,
    alt: String,
}

impl ImageView {
    pub fn new(image: ImageId, size: Size, alt: &str) -> Self {
        Self {
            image,
            size,
            alt: alt.to_string(),
        }
    }
}

impl Widget for ImageView {
    fn role(&self) -> Role {
        Role::Image
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        _cx: &mut MeasureCx,
        _k: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        self.size
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.image_untinted(self.image, r);
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.alt.as_str());
    }
}

/// A horizontal or vertical rule.
pub struct Separator {
    vertical: bool,
}

impl Separator {
    pub fn horizontal() -> Self {
        Self { vertical: false }
    }
    pub fn vertical() -> Self {
        Self { vertical: true }
    }
}

impl Widget for Separator {
    fn role(&self) -> Role {
        Role::Splitter
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
        if self.vertical {
            Size::new(9.0, known.height.unwrap_or(CONTROL_MIN_H))
        } else {
            Size::new(known.width.unwrap_or(0.0), 9.0)
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let bg = cx.parent_bg();
        let line = if self.vertical {
            Rect::new(r.x + r.w * 0.5, r.y, 1.0, r.h)
        } else {
            Rect::new(r.x, r.y + r.h * 0.5, r.w, 1.0)
        };
        cx.mark(line, ColorRole::Border, bg, 0.0);
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_orientation(if self.vertical {
            accesskit::Orientation::Vertical
        } else {
            accesskit::Orientation::Horizontal
        });
    }
}

/// A small count or status pill. Its text is its meaning (colour is never the only cue).
pub struct Badge {
    text: crate::state::Bind<String>,
    tone: ColorRole,
    on: ColorRole,
}

impl Badge {
    pub fn new(text: impl Into<crate::state::Bind<String>>) -> Self {
        Self {
            text: text.into(),
            tone: ColorRole::Accent,
            on: ColorRole::FgOnAccent,
        }
    }
    /// A danger badge (errors).
    pub fn danger(mut self) -> Self {
        self.tone = ColorRole::Danger;
        self.on = ColorRole::FgOnDanger;
        self
    }
}

impl Widget for Badge {
    fn role(&self) -> Role {
        Role::Status
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(self.text.source(), Dirty::LAYOUT);
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
        let (_, t) = cx
            .text
            .layout(&self.text.get(cx.rt), &small(cx.theme).strong(), None);
        Size::new((t.w + 12.0).max(t.h + 4.0), t.h + 4.0)
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, self.tone, r.h * 0.5);
        let s = self.text.get(cx.rt());
        let style = small(cx.theme()).strong();
        let (_, t) = cx.shape(&s, &style);
        cx.text(
            &s,
            &style,
            Point::new(r.x + (r.w - t.w) * 0.5, r.y + (r.h - t.h) * 0.5),
            self.on,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.text.get(cx.rt));
    }
}

/// A keyboard-shortcut hint (`Ctrl+Shift+P`), drawn as key caps.
pub struct ShortcutHint {
    keys: Vec<String>,
}

impl ShortcutHint {
    /// `chord` like `"Ctrl+Shift+P"`.
    pub fn new(chord: &str) -> Self {
        Self {
            keys: chord.split('+').map(|s| s.trim().to_string()).collect(),
        }
    }
    fn caps(&self, cx: &mut MeasureCx) -> f32 {
        let style = small(cx.theme);
        self.keys
            .iter()
            .map(|k| cx.text.layout(k, &style, None).1.w + 10.0)
            .sum::<f32>()
            + 3.0 * self.keys.len().saturating_sub(1) as f32
    }
}

impl Widget for ShortcutHint {
    fn role(&self) -> Role {
        Role::Label
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
        // l10n: measures the widest modifier keycap; not shown
        let (_, t) = cx.text.layout("Ctrl", &small(cx.theme), None);
        Size::new(self.caps(cx), t.h + 6.0)
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let style = small(cx.theme());
        let mut x = r.x;
        let radius = cx.theme().radius.sm;
        for k in &self.keys {
            let (_, t) = cx.shape(k, &style);
            let cap = Rect::new(x, r.y, t.w + 10.0, r.h);
            cx.panel(
                cap,
                Some(ColorRole::BgSunken),
                Some(ColorRole::Border),
                radius,
                None,
            );
            cx.text(
                k,
                &style,
                Point::new(x + 5.0, r.y + (r.h - t.h) * 0.5),
                ColorRole::FgPrimary,
            );
            x += cap.w + 3.0;
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::trf!(
            "Shortcut {items}",
            items = self.keys.join(" plus ")
        ));
    }
}

/// A loading placeholder: static bars in the layout content will take (no animation, so
/// it costs nothing while it waits, §21.3).
pub struct Skeleton {
    lines: u32,
    label: String,
}

impl Skeleton {
    pub fn new(lines: u32, label: &str) -> Self {
        Self {
            lines: lines.max(1),
            label: label.to_string(),
        }
    }
}

impl Widget for Skeleton {
    fn role(&self) -> Role {
        Role::ProgressIndicator
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
        Size::new(known.width.unwrap_or(220.0), self.lines as f32 * 18.0)
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let bg = cx.parent_bg();
        for i in 0..self.lines {
            let w = if i + 1 == self.lines { r.w * 0.6 } else { r.w };
            cx.mark_alpha(
                Rect::new(r.x, r.y + i as f32 * 18.0 + 3.0, w, 12.0),
                ColorRole::Border,
                bg,
                3.0,
                0.35,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_busy();
    }
}

/// Raised when an empty state's action is activated.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EmptyStateAction(pub WidgetId);

/// The empty state a panel shows when it has nothing to show: a message and at most one
/// action (§21.24, M2-24; WP-U12 audits every panel against it).
pub struct EmptyState {
    message: crate::state::Bind<String>,
    action: Option<String>,
}

impl EmptyState {
    /// A fixed message, or one bound to a signal (a panel whose reason for being empty
    /// changes: no team server, no licence, nothing selected).
    pub fn new(message: impl Into<crate::state::Bind<String>>) -> Self {
        Self {
            message: message.into(),
            action: None,
        }
    }
    /// The message now.
    pub fn message(&self, rt: &crate::state::Runtime) -> String {
        self.message.get(rt)
    }
    /// The one action's label, if it has one.
    pub fn action_label(&self) -> Option<&str> {
        self.action.as_deref()
    }
    /// The one action (a link-styled button inside the widget; Enter activates it).
    pub fn action(mut self, label: &str) -> Self {
        self.action = Some(label.to_string());
        self
    }
}

impl Widget for EmptyState {
    fn bind(&self, b: &mut Binder) {
        b.watch(self.message.source(), Dirty::LAYOUT);
    }
    fn role(&self) -> Role {
        if self.action.is_some() {
            Role::Button
        } else {
            Role::Label
        }
    }
    fn focusable(&self) -> bool {
        self.action.is_some()
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
        // A message wider than the panel wraps (a narrow side panel keeps it readable).
        let width = known.width.or(match avail.width {
            taffy::AvailableSpace::Definite(w) => Some(w),
            _ => None,
        });
        let message = self.message.get(cx.rt);
        let (_, one) = cx.text.layout(&message, &body(cx.theme), None);
        let m = match width {
            Some(w) if one.w + 24.0 > w => {
                let mut st = body(cx.theme);
                st.wrap = true;
                cx.text.layout(&message, &st, Some(w)).1
            }
            _ => one,
        };
        let a = self.action.as_ref().map_or(Size::ZERO, |a| {
            cx.text.layout(a, &body(cx.theme).strong(), None).1
        });
        Size::new(known.width.unwrap_or(m.w.max(a.w) + 24.0), m.h + a.h + 28.0)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if self.action.is_none() {
            return Handled::No;
        }
        if super::activation(ev) {
            let id = cx.id();
            cx.action(EmptyStateAction(id));
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let message = self.message.get(cx.rt());
        let mut style = body(cx.theme());
        let (_, one) = cx.shape(&message, &style);
        let y = r.y + 10.0;
        // Centred on one line when it fits; wrapped to the panel otherwise.
        let (x, m) = if one.w + 24.0 > r.w {
            style.wrap = true;
            (r.x, cx.shape(&message, &style).1)
        } else {
            (r.x + (r.w - one.w) * 0.5, one)
        };
        cx.text(&message, &style, Point::new(x, y), ColorRole::FgMuted);
        if let Some(a) = &self.action {
            let st = body(cx.theme()).strong();
            let (_, t) = cx.shape(a, &st);
            let x = r.x + (r.w - t.w) * 0.5;
            let ay = y + m.h + 8.0;
            cx.text(a, &st, Point::new(x, ay), ColorRole::FgPrimary);
            // Underlined like a link: the affordance is not colour alone.
            let bg = cx.current_bg();
            cx.mark(
                Rect::new(x, ay + t.h - 1.0, t.w, 1.0),
                ColorRole::FgPrimary,
                bg,
                0.0,
            );
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        let message = self.message.get(cx.rt);
        match &self.action {
            Some(a) => {
                node.set_label(a.as_str());
                node.set_description(message);
                node.add_action(Action::Click);
            }
            None => node.set_label(message),
        }
    }
}

/// A titled, bordered group of controls. Children lay out below the title (give the
/// node padding-top ≥ the title height, e.g. `GroupBox::style`).
pub struct GroupBox {
    title: String,
}

impl GroupBox {
    pub fn new(title: &str) -> Self {
        Self {
            title: title.to_string(),
        }
    }
    /// A column style that leaves room for the title.
    pub fn style(theme: &crate::style::Theme) -> crate::layout::NodeStyle {
        let mut s = crate::layout::NodeStyle::column(theme.space[2]).padding(theme.space[3]);
        s.layout.padding.top =
            taffy::prelude::length(theme.space[3] + theme.type_scale.small + 8.0);
        s
    }
}

impl Widget for GroupBox {
    fn role(&self) -> Role {
        Role::Group
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let style = small(cx.theme()).strong();
        let (_, t) = cx.shape(&self.title, &style);
        let radius = cx.theme().radius.md;
        let frame = Rect::new(r.x, r.y + t.h * 0.5, r.w, r.h - t.h * 0.5);
        cx.panel(frame, None, Some(ColorRole::Border), radius, None);
        let bg = cx.parent_bg();
        cx.fill(Rect::new(r.x + 8.0, r.y, t.w + 8.0, t.h), bg, 0.0);
        cx.text(
            &self.title,
            &style,
            Point::new(r.x + 12.0, r.y),
            ColorRole::FgMuted,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.title.as_str());
    }
}

/// The header of a collapsible section: toggles its `body` widget's visibility (a hidden
/// body lays out, paints and schedules nothing, §21.3).
pub struct CollapsibleHeader {
    title: String,
    open: Signal<bool>,
    body: Option<WidgetId>,
}

impl CollapsibleHeader {
    pub fn new(title: &str, open: Signal<bool>) -> Self {
        Self {
            title: title.to_string(),
            open,
            body: None,
        }
    }
    pub fn set_body(&mut self, body: WidgetId) {
        self.body = Some(body);
    }
    fn toggle(&self, cx: &mut EventCx, to: bool) {
        self.open.set(cx.rt_mut(), to);
        if let Some(b) = self.body {
            cx.set_hidden(b, !to);
        }
    }
}

impl Widget for CollapsibleHeader {
    fn role(&self) -> Role {
        Role::Button
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.open.any()), Dirty::PAINT | Dirty::A11Y);
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
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let (_, t) = cx.text.layout(&self.title, &body(cx.theme).strong(), None);
        Size::new(known.width.unwrap_or(t.w + 28.0), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let open = self.open.get(cx.rt());
        let to = match ev {
            UiEvent::Key(k) if k.pressed => match k.code {
                KeyCode::Enter | KeyCode::Space => Some(!open),
                KeyCode::Right => Some(true),
                KeyCode::Left => Some(false),
                _ => None,
            },
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                inside: true,
                ..
            } => Some(!open),
            UiEvent::A11yAction(Action::Expand) => Some(true),
            UiEvent::A11yAction(Action::Collapse) => Some(false),
            UiEvent::A11yAction(Action::Click) => Some(!open),
            _ => None,
        };
        match to {
            Some(v) => {
                self.toggle(cx, v);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        if cx.state().hovered {
            cx.fill(r, ColorRole::BgHover, cx.theme().radius.sm);
        }
        let open = self.open.get(cx.rt());
        let style = body(cx.theme()).strong();
        let (_, t) = cx.shape(&self.title, &style);
        let y = r.y + (r.h - t.h) * 0.5;
        cx.text(
            if open { "▾" } else { "▸" },
            &body(cx.theme()),
            Point::new(r.x + 4.0, y),
            ColorRole::FgPrimary,
        );
        cx.text(
            &self.title,
            &style,
            Point::new(r.x + 22.0, y),
            ColorRole::FgPrimary,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.title.as_str());
        let open = self.open.get(cx.rt);
        node.set_expanded(open);
        node.add_action(if open {
            Action::Collapse
        } else {
            Action::Expand
        });
        node.add_action(Action::Click);
    }
}

/// One icon of the Forge icon set ([`crate::icons`]), shown (not a control): the gallery's
/// icon sheet, a status glyph beside a label. Its accessible name is the icon's meaning.
pub struct IconView {
    icon: crate::icons::Icon,
    label: String,
    size: f32,
    role: ColorRole,
}

impl IconView {
    pub fn new(icon: crate::icons::Icon, label: &str) -> Self {
        Self {
            icon,
            label: label.to_string(),
            size: 20.0,
            role: ColorRole::FgPrimary,
        }
    }
    /// Draw it `px` logical pixels square.
    pub fn size(mut self, px: f32) -> Self {
        self.size = px.max(8.0);
        self
    }
    /// Draw it in `role` (a token; never a raw colour).
    pub fn color(mut self, role: ColorRole) -> Self {
        self.role = role;
        self
    }
}

impl Widget for IconView {
    fn role(&self) -> Role {
        Role::Image
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        _cx: &mut MeasureCx,
        _known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::new(self.size + 8.0, self.size + 8.0)
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let ir = Rect::new(
            r.x + (r.w - self.size) * 0.5,
            r.y + (r.h - self.size) * 0.5,
            self.size,
            self.size,
        );
        cx.icon(self.icon, ir, self.role);
    }
    fn tooltip(&self, _rt: &crate::state::Runtime) -> Option<String> {
        Some(self.label.clone())
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
    }
}
