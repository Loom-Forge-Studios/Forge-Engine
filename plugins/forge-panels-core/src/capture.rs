//! `ChordCapture` — "press the new keys" for the keybindings editor. It takes focus, eats
//! the next stroke (and a second one within [`SECOND_STROKE`], for two-stroke chords such
//! as `Ctrl+K Ctrl+S`), and raises [`ChordCaptured`]. Esc alone cancels. While it has
//! focus the keymap never sees those keys, so capturing `Ctrl+Z` does not undo.

use std::time::Duration;

use forge_editor::keymap::{Chord, KeyStroke};
use forge_ui::text::TextStyle;
use forge_ui::widget::{A11yCx, EventCx, MeasureCx, PaintCx};
use forge_ui::{
    ColorRole, Dirty, Handled, KeyCode, Modifiers, Point, Role, Signal, Size, UiEvent, Widget,
};

/// How long a first stroke waits for a second one.
pub const SECOND_STROKE: Duration = Duration::from_millis(1200);
const TIMER: u64 = 0x6368_6f72;

/// Raised when capture ends: the chord, or `None` if cancelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordCaptured {
    pub chord: Option<Chord>,
}

/// See the module docs. `prompt` is what it shows before the first stroke.
pub struct ChordCapture {
    strokes: Vec<KeyStroke>,
    prompt: Signal<String>,
}

impl ChordCapture {
    pub fn new(prompt: Signal<String>) -> Self {
        Self {
            strokes: Vec::new(),
            prompt,
        }
    }

    fn text(&self, rt: &forge_ui::Runtime) -> String {
        if self.strokes.is_empty() {
            self.prompt.get(rt)
        } else {
            format!(
                "{} \u{2026}",
                self.strokes
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        }
    }

    fn finish(&mut self, cx: &mut EventCx) {
        cx.cancel_timer(TIMER);
        let chord = (!self.strokes.is_empty()).then(|| Chord(std::mem::take(&mut self.strokes)));
        cx.action(ChordCaptured { chord });
        cx.request_paint();
        cx.request_a11y();
    }
}

impl Widget for ChordCapture {
    fn role(&self) -> Role {
        Role::Button
    }
    fn bind(&self, b: &mut forge_ui::widget::Binder) {
        b.watch(Some(self.prompt.any()), Dirty::LAYOUT | Dirty::A11Y);
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
        let t = self.text(cx.rt);
        let (_, s) = cx
            .text
            .layout(&t, &TextStyle::body(cx.theme.type_scale.body), None);
        Size::new(s.w + 16.0, s.h.max(28.0))
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::Key(k) if k.pressed => {
                if k.code == KeyCode::Escape && k.mods == Modifiers::NONE && self.strokes.is_empty()
                {
                    self.finish(cx);
                    return Handled::Yes;
                }
                let Some(s) = KeyStroke::from_event(k) else {
                    return Handled::Yes; // a lone modifier: keep waiting
                };
                self.strokes.push(s);
                if self.strokes.len() >= 2 {
                    self.finish(cx);
                } else {
                    cx.set_timer(SECOND_STROKE, TIMER);
                    cx.request_layout();
                    cx.request_a11y();
                }
                Handled::Yes
            }
            UiEvent::Key(_) => Handled::Yes,
            UiEvent::Timer(TIMER) => {
                self.finish(cx);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let bg = cx.parent_bg();
        cx.mark(r, ColorRole::Accent, bg, 4.0);
        let t = self.text(cx.rt());
        let style = TextStyle::body(cx.theme().type_scale.body);
        cx.text(
            &t,
            &style,
            Point::new(r.x + 8.0, r.y + 4.0),
            ColorRole::FgOnAccent,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.text(cx.rt));
    }
}
