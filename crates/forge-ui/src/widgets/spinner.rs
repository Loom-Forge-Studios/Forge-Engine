use accesskit::Role;

use crate::geom::{Rect, Size};
use crate::input::{Handled, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

const DOTS: usize = 8;
const SIZE: f32 = 20.0;

/// An indeterminate spinner. It schedules frames **only while visible and while its task
/// is live** (§21.3): a spinner in a background tab or scrolled out costs nothing.
pub struct Spinner {
    live: Signal<bool>,
    label: String,
    phase: usize,
    running: bool,
    visible: bool,
    /// Fault (W2 control for `ui_idle_zero_redraw`): keep animating while hidden.
    ignore_visibility: crate::controls::Switch,
}

impl Spinner {
    pub fn new(live: Signal<bool>, label: &str) -> Self {
        Self {
            live,
            label: label.to_string(),
            phase: 0,
            running: false,
            visible: false,
            ignore_visibility: crate::controls::Switch::default(),
        }
    }
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn with_fault_ignore_visibility(mut self) -> Self {
        self.ignore_visibility.on = true;
        self
    }
    pub fn is_running(&self) -> bool {
        self.running
    }

    fn should_run(&self, cx: &EventCx) -> bool {
        self.live.get(cx.rt()) && (self.visible || self.ignore_visibility.on())
    }

    fn sync(&mut self, cx: &mut EventCx) {
        let run = self.should_run(cx);
        if run && !self.running {
            cx.request_anim_frame();
        } else if !run && self.running {
            cx.stop_anim();
        }
        self.running = run;
    }
}

impl Widget for Spinner {
    fn role(&self) -> Role {
        Role::ProgressIndicator
    }
    fn bind(&self, b: &mut Binder) {
        // `live` changes restart or stop it via the paint pass below.
        b.watch(Some(self.live.any()), crate::damage::Dirty::PAINT);
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
        Size::new(SIZE, SIZE)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::BindingChanged => {
                self.sync(cx);
                Handled::Yes
            }
            UiEvent::VisibilityChanged(v) => {
                self.visible = *v;
                self.sync(cx);
                Handled::Yes
            }
            UiEvent::AnimFrame => {
                if self.should_run(cx) {
                    self.phase = (self.phase + 1) % DOTS;
                    cx.request_paint();
                    cx.request_anim_frame();
                } else {
                    self.running = false;
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let c = r.center();
        let bg = cx.parent_bg();
        let rad = SIZE * 0.5 - 3.0;
        for i in 0..DOTS {
            let a = (i as f32 / DOTS as f32) * std::f32::consts::TAU;
            let age = (i + DOTS - self.phase) % DOTS;
            let alpha = 1.0 - age as f32 / DOTS as f32 * 0.85;
            let (x, y) = (c.x + rad * a.cos(), c.y + rad * a.sin());
            cx.mark_alpha(
                Rect::new(x - 2.0, y - 2.0, 4.0, 4.0),
                ColorRole::Accent,
                bg,
                2.0,
                alpha,
            );
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        if self.live.get(cx.rt) {
            node.set_busy();
        }
    }
}
