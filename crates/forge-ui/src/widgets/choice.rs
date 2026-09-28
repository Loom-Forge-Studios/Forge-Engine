//! Toggles and choices (§21.16): toggle button, tri-state checkbox, switch, segmented
//! control. Each is one tab stop; Space toggles, arrows move inside the segmented control
//! (WAI-ARIA radio group), and every state is visible without relying on colour alone.

use accesskit::{Action, Role, Toggled};

use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::state::Signal;
use crate::style::{ColorRole, Variant, button_style};
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::{CONTROL_MIN_H, activation, body, control_pad_x};

/// A button that stays pressed (bold, snap, wireframe).
pub struct ToggleButton {
    on: Signal<bool>,
    label: String,
}

impl ToggleButton {
    pub fn new(on: Signal<bool>, label: &str) -> Self {
        Self {
            on,
            label: label.to_string(),
        }
    }
}

impl Widget for ToggleButton {
    fn role(&self) -> Role {
        Role::Button
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.on.any()), Dirty::PAINT | Dirty::A11Y);
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
        let (_, t) = cx.text.layout(&self.label, &body(cx.theme), None);
        Size::new(t.w + 2.0 * control_pad_x(cx.theme), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if activation(ev) {
            let v = !self.on.get(cx.rt());
            self.on.set(cx.rt_mut(), v);
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let on = self.on.get(cx.rt());
        let st = button_style(
            if on {
                Variant::Primary
            } else {
                Variant::Secondary
            },
            cx.state(),
        );
        let radius = cx.theme().radius.md;
        cx.panel(r, st.bg, st.border, radius, None);
        let style = if on {
            body(cx.theme()).strong()
        } else {
            body(cx.theme())
        };
        let (_, t) = cx.shape(&self.label, &style);
        cx.text(
            &self.label,
            &style,
            Point::new(r.x + (r.w - t.w) * 0.5, r.y + (r.h - t.h) * 0.5),
            st.fg,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_toggled(if self.on.get(cx.rt) {
            Toggled::True
        } else {
            Toggled::False
        });
        node.add_action(Action::Click);
    }
}

/// A tri-state checkbox value.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CheckState {
    #[default]
    Off,
    On,
    /// Some children on, some off ("select all" over a mixed selection).
    Mixed,
}

/// A checkbox with a mixed state. Activating a mixed box turns it on.
pub struct TriCheckbox {
    state: Signal<CheckState>,
    label: String,
}

const BOX: f32 = 16.0;

impl TriCheckbox {
    pub fn new(state: Signal<CheckState>, label: &str) -> Self {
        Self {
            state,
            label: label.to_string(),
        }
    }
    /// The state shown.
    pub fn state(&self, rt: &crate::state::Runtime) -> CheckState {
        self.state.get(rt)
    }
}

impl Widget for TriCheckbox {
    fn role(&self) -> Role {
        Role::CheckBox
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.state.any()), Dirty::PAINT | Dirty::A11Y);
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
        let (_, t) = cx.text.layout(&self.label, &body(cx.theme), None);
        Size::new(BOX + cx.theme.space[2] + t.w, t.h.max(CONTROL_MIN_H))
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let toggle = match ev {
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                inside: true,
                ..
            } => true,
            // Plain Space only: Shift+Space is the dock's maximise chord (Ch.21.17).
            UiEvent::Key(k) => {
                k.pressed && k.code == KeyCode::Space && k.mods == crate::input::Modifiers::NONE
            }
            UiEvent::A11yAction(Action::Click) => true,
            _ => false,
        };
        if toggle {
            let next = match self.state.get(cx.rt()) {
                CheckState::On => CheckState::Off,
                CheckState::Off | CheckState::Mixed => CheckState::On,
            };
            self.state.set(cx.rt_mut(), next);
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let bx = Rect::new(r.x, r.y + (r.h - BOX) * 0.5, BOX, BOX);
        let radius = cx.theme().radius.sm;
        let bg = cx.parent_bg();
        match self.state.get(cx.rt()) {
            CheckState::Off => {
                let hover = cx.state().hovered;
                cx.panel(
                    bx,
                    hover.then_some(ColorRole::BgHover),
                    Some(ColorRole::Border),
                    radius,
                    None,
                );
            }
            CheckState::On => {
                cx.mark(bx, ColorRole::Accent, bg, radius);
                // A check: two strokes drawn as bars (shape, not colour, carries "on").
                cx.mark(
                    Rect::new(bx.x + 3.0, bx.y + 8.0, 4.0, 2.0),
                    ColorRole::FgOnAccent,
                    ColorRole::Accent,
                    1.0,
                );
                cx.mark(
                    Rect::new(bx.x + 6.0, bx.y + 5.0, 7.0, 2.0),
                    ColorRole::FgOnAccent,
                    ColorRole::Accent,
                    1.0,
                );
            }
            CheckState::Mixed => {
                cx.mark(bx, ColorRole::Accent, bg, radius);
                cx.mark(
                    Rect::new(bx.x + 4.0, bx.y + 7.0, BOX - 8.0, 2.0),
                    ColorRole::FgOnAccent,
                    ColorRole::Accent,
                    1.0,
                );
            }
        }
        let style = body(cx.theme());
        let (_, t) = cx.shape(&self.label, &style);
        let x = bx.right() + cx.theme().space[2];
        cx.text(
            &self.label,
            &style,
            Point::new(x, r.y + (r.h - t.h) * 0.5),
            ColorRole::FgPrimary,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_toggled(match self.state.get(cx.rt) {
            CheckState::On => Toggled::True,
            CheckState::Off => Toggled::False,
            CheckState::Mixed => Toggled::Mixed,
        });
        node.add_action(Action::Click);
    }
}

/// An on/off switch (settings). The thumb position, not only its colour, shows the state.
pub struct Switch {
    on: Signal<bool>,
    label: String,
}

impl Switch {
    pub fn new(on: Signal<bool>, label: &str) -> Self {
        Self {
            on,
            label: label.to_string(),
        }
    }
}

const TRACK_W: f32 = 34.0;
const TRACK_H: f32 = 18.0;

impl Widget for Switch {
    fn role(&self) -> Role {
        Role::Switch
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.on.any()), Dirty::PAINT | Dirty::A11Y);
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
        let (_, t) = cx.text.layout(&self.label, &body(cx.theme), None);
        Size::new(TRACK_W + cx.theme.space[2] + t.w, CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let set = match ev {
            UiEvent::Key(k) if k.pressed => match k.code {
                KeyCode::Space | KeyCode::Enter => Some(!self.on.get(cx.rt())),
                KeyCode::Right => Some(true),
                KeyCode::Left => Some(false),
                _ => None,
            },
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                inside: true,
                ..
            }
            | UiEvent::A11yAction(Action::Click) => Some(!self.on.get(cx.rt())),
            _ => None,
        };
        match set {
            Some(v) => {
                self.on.set(cx.rt_mut(), v);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let on = self.on.get(cx.rt());
        let bg = cx.parent_bg();
        let track = Rect::new(r.x, r.y + (r.h - TRACK_H) * 0.5, TRACK_W, TRACK_H);
        if on {
            cx.mark(track, ColorRole::Accent, bg, TRACK_H * 0.5);
        } else {
            cx.panel(
                track,
                Some(ColorRole::BgSunken),
                Some(ColorRole::Border),
                TRACK_H * 0.5,
                None,
            );
        }
        let d = TRACK_H - 6.0;
        let tx = if on {
            track.right() - d - 3.0
        } else {
            track.x + 3.0
        };
        let (thumb, on_bg) = if on {
            (ColorRole::FgOnAccent, ColorRole::Accent)
        } else {
            (ColorRole::FgMuted, ColorRole::BgSunken)
        };
        cx.mark(Rect::new(tx, track.y + 3.0, d, d), thumb, on_bg, d * 0.5);
        let style = body(cx.theme());
        let (_, t) = cx.shape(&self.label, &style);
        cx.text(
            &self.label,
            &style,
            Point::new(track.right() + cx.theme().space[2], r.y + (r.h - t.h) * 0.5),
            ColorRole::FgPrimary,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_toggled(if self.on.get(cx.rt) {
            Toggled::True
        } else {
            Toggled::False
        });
        node.add_action(Action::Click);
    }
}

/// A segmented control: mutually exclusive options in one strip (one tab stop; arrows
/// move the selection, like a radio group).
pub struct SegmentedControl {
    label: String,
    options: Vec<String>,
    selected: Signal<usize>,
}

impl SegmentedControl {
    pub fn new(label: &str, options: &[&str], selected: Signal<usize>) -> Self {
        Self {
            label: label.to_string(),
            options: options.iter().map(|s| s.to_string()).collect(),
            selected,
        }
    }
    fn widths(&self, text: &mut crate::text::TextSystem, theme: &crate::style::Theme) -> Vec<f32> {
        self.options
            .iter()
            .map(|o| text.layout(o, &body(theme), None).1.w + 2.0 * control_pad_x(theme))
            .collect()
    }
}

impl Widget for SegmentedControl {
    fn role(&self) -> Role {
        Role::RadioGroup
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.selected.any()), Dirty::PAINT | Dirty::A11Y);
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
        Size::new(self.widths(cx.text, cx.theme).iter().sum(), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let n = self.options.len();
        if n == 0 {
            return Handled::No;
        }
        let cur = self.selected.get(cx.rt()).min(n - 1);
        let next = match ev {
            UiEvent::Key(k) if k.pressed => match k.code {
                KeyCode::Right | KeyCode::Down => Some((cur + 1) % n),
                KeyCode::Left | KeyCode::Up => Some((cur + n - 1) % n),
                KeyCode::Home => Some(0),
                KeyCode::End => Some(n - 1),
                _ => None,
            },
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let theme = cx.theme().clone();
                let widths = self.widths(&mut cx.text(), &theme);
                let r = cx.rect();
                let mut x = r.x;
                let mut hit = None;
                for (i, w) in widths.iter().enumerate() {
                    if pos.x >= x && pos.x < x + w {
                        hit = Some(i);
                    }
                    x += w;
                }
                hit
            }
            UiEvent::A11yChildAction(i, Action::Click) => Some((*i as usize).min(n - 1)),
            _ => None,
        };
        match next {
            Some(i) => {
                self.selected.set(cx.rt_mut(), i);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let sel = self.selected.get(cx.rt());
        let radius = cx.theme().radius.md;
        cx.panel(
            r,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            radius,
            None,
        );
        let pad = control_pad_x(cx.theme());
        let style = body(cx.theme());
        let mut x = r.x;
        for (i, o) in self.options.iter().enumerate() {
            let (_, t) = cx.shape(o, &style);
            let w = t.w + 2.0 * pad;
            let seg = Rect::new(x, r.y, w, r.h);
            if i == sel {
                cx.fill(seg.outset(-2.0), ColorRole::Accent, radius - 1.0);
                let st = body(cx.theme()).strong();
                let (_, ts) = cx.shape(o, &st);
                cx.text(
                    o,
                    &st,
                    Point::new(x + (w - ts.w) * 0.5, r.y + (r.h - ts.h) * 0.5),
                    ColorRole::FgOnAccent,
                );
            } else {
                cx.fill(seg.outset(-2.0), ColorRole::BgSunken, 0.0);
                cx.text(
                    o,
                    &style,
                    Point::new(x + pad, r.y + (r.h - t.h) * 0.5),
                    ColorRole::FgPrimary,
                );
            }
            x += w;
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        if let Some(o) = self.options.get(self.selected.get(cx.rt)) {
            node.set_value(o.as_str());
        }
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let sel = self.selected.get(cx.rt);
        let n = self.options.len();
        for (i, o) in self.options.iter().enumerate() {
            let mut node = accesskit::Node::new(Role::RadioButton);
            node.set_label(o.as_str());
            node.set_toggled(if i == sel {
                Toggled::True
            } else {
                Toggled::False
            });
            node.set_position_in_set(i + 1);
            node.set_size_of_set(n);
            node.add_action(Action::Click);
            out.push((i as u64, node));
        }
    }
    fn a11y_focus(&self, rt: &crate::state::Runtime) -> Option<u64> {
        Some(self.selected.get(rt) as u64)
    }
}
