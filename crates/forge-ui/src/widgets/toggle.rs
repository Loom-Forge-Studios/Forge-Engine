use accesskit::{Action, Role, Toggled};

use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::{CONTROL_MIN_H, body};

const BOX: f32 = 16.0;

/// A checkbox bound to a `Signal<bool>` it was given.
pub struct Checkbox {
    checked: Signal<bool>,
    label: String,
}

impl Checkbox {
    pub fn new(checked: Signal<bool>, label: &str) -> Self {
        Self {
            checked,
            label: label.to_string(),
        }
    }
}

impl Widget for Checkbox {
    fn role(&self) -> Role {
        Role::CheckBox
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.checked.any()), Dirty::PAINT | Dirty::A11Y);
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
            let v = !self.checked.get(cx.rt());
            self.checked.set(cx.rt_mut(), v);
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let on = self.checked.get(cx.rt());
        let bx = Rect::new(r.x, r.y + (r.h - BOX) * 0.5, BOX, BOX);
        let radius = cx.theme().radius.sm;
        let bg = cx.parent_bg();
        if on {
            cx.mark(bx, ColorRole::Accent, bg, radius);
            let inner = Rect::new(bx.x + 4.0, bx.y + 4.0, BOX - 8.0, BOX - 8.0);
            cx.mark(inner, ColorRole::FgOnAccent, ColorRole::Accent, 1.0);
        } else {
            let hover = cx.state().hovered;
            cx.panel(
                bx,
                hover.then_some(ColorRole::BgHover),
                Some(ColorRole::Border),
                radius,
                None,
            );
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
        node.set_toggled(if self.checked.get(cx.rt) {
            Toggled::True
        } else {
            Toggled::False
        });
        node.add_action(Action::Click);
    }
}

/// A radio group: one tab stop; arrow keys move the selection (WAI-ARIA radio group).
pub struct RadioGroup {
    label: String,
    options: Vec<String>,
    selected: Signal<usize>,
}

impl RadioGroup {
    pub fn new(label: &str, options: &[&str], selected: Signal<usize>) -> Self {
        Self {
            label: label.to_string(),
            options: options.iter().map(|s| s.to_string()).collect(),
            selected,
        }
    }
    fn row_h(&self) -> f32 {
        CONTROL_MIN_H - 4.0
    }
}

impl Widget for RadioGroup {
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
        let mut w: f32 = 0.0;
        for o in &self.options {
            let (_, t) = cx.text.layout(o, &body(cx.theme), None);
            w = w.max(t.w);
        }
        Size::new(
            BOX + cx.theme.space[2] + w,
            self.row_h() * self.options.len() as f32,
        )
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let n = self.options.len();
        if n == 0 {
            return Handled::No;
        }
        let cur = self.selected.get(cx.rt()).min(n - 1);
        let next = match ev {
            UiEvent::Key(k) if k.pressed => match k.code {
                KeyCode::Down | KeyCode::Right => Some((cur + 1) % n),
                KeyCode::Up | KeyCode::Left => Some((cur + n - 1) % n),
                KeyCode::Home => Some(0),
                KeyCode::End => Some(n - 1),
                _ => None,
            },
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let r = cx.rect();
                let i = ((pos.y - r.y) / self.row_h()).floor();
                (i >= 0.0).then(|| (i as usize).min(n - 1))
            }
            // An assistive technology selects an option (the virtual radio buttons below).
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
        let style = body(cx.theme());
        let bg = cx.parent_bg();
        for (i, o) in self.options.iter().enumerate() {
            let y = r.y + i as f32 * self.row_h();
            let dot = Rect::new(r.x, y + (self.row_h() - BOX) * 0.5, BOX, BOX);
            cx.panel(dot, None, Some(ColorRole::Border), BOX * 0.5, None);
            if i == sel {
                let inner = Rect::new(dot.x + 4.0, dot.y + 4.0, BOX - 8.0, BOX - 8.0);
                cx.mark(inner, ColorRole::Accent, bg, (BOX - 8.0) * 0.5);
            }
            let (_, t) = cx.shape(o, &style);
            cx.text(
                o,
                &style,
                Point::new(
                    dot.right() + cx.theme().space[2],
                    y + (self.row_h() - t.h) * 0.5,
                ),
                ColorRole::FgPrimary,
            );
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        if let Some(o) = self.options.get(self.selected.get(cx.rt)) {
            node.set_value(o.as_str());
        }
    }
    /// Each option is a radio button a screen reader can list and pick (observed missing
    /// over AT-SPI in WP-U12: the group exposed no options).
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
