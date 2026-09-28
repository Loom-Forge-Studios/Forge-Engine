//! Progress bar (§21.16): determinate (a value 0..=1) or indeterminate (a moving band).
//! The indeterminate animation follows the spinner's rule (§21.3): it schedules frames
//! only while visible and while its task is live, and reduced motion replaces the moving
//! band with a static striped bar.

use accesskit::Role;

use crate::damage::Dirty;
use crate::geom::{Rect, Size};
use crate::input::{Handled, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

/// What a progress bar shows.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum Progress {
    /// A known fraction, 0..=1.
    Fraction(f32),
    /// Working, amount unknown.
    Indeterminate,
    /// Not running.
    #[default]
    Idle,
}

pub struct ProgressBar {
    progress: Signal<Progress>,
    label: String,
    phase: f32,
    running: bool,
    visible: bool,
}

impl ProgressBar {
    pub fn new(progress: Signal<Progress>, label: &str) -> Self {
        Self {
            progress,
            label: label.to_string(),
            phase: 0.0,
            running: false,
            visible: false,
        }
    }
    pub fn is_animating(&self) -> bool {
        self.running
    }
    fn sync(&mut self, cx: &mut EventCx) {
        let run = self.visible
            && !cx.reduced_motion()
            && self.progress.get(cx.rt()) == Progress::Indeterminate;
        if run && !self.running {
            cx.request_anim_frame();
        } else if !run && self.running {
            cx.stop_anim();
        }
        self.running = run;
    }
}

impl Widget for ProgressBar {
    fn role(&self) -> Role {
        Role::ProgressIndicator
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.progress.any()), Dirty::PAINT | Dirty::A11Y);
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
        Size::new(known.width.unwrap_or(200.0), 8.0)
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
                if self.visible
                    && !cx.reduced_motion()
                    && self.progress.get(cx.rt()) == Progress::Indeterminate
                {
                    self.phase = (self.phase + 0.02) % 1.0;
                    cx.request_paint();
                    cx.request_anim_frame();
                } else {
                    // Hidden, finished, or reduced motion switched on mid-run: stop, and
                    // repaint once as the static bar.
                    self.running = false;
                    cx.request_paint();
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let bg = cx.parent_bg();
        let track = Rect::new(r.x, r.y + (r.h - 6.0) * 0.5, r.w, 6.0);
        cx.mark(track, ColorRole::Border, bg, 3.0);
        match self.progress.get(cx.rt()) {
            Progress::Fraction(f) => {
                let f = f.clamp(0.0, 1.0);
                cx.mark(
                    Rect::new(track.x, track.y, track.w * f, track.h),
                    ColorRole::Accent,
                    bg,
                    3.0,
                );
            }
            Progress::Indeterminate if cx.reduced_motion() => {
                // Static stripes: "busy" without motion.
                let mut x = track.x;
                while x < track.right() {
                    cx.mark(
                        Rect::new(x, track.y, 8.0f32.min(track.right() - x), track.h),
                        ColorRole::Accent,
                        bg,
                        3.0,
                    );
                    x += 16.0;
                }
            }
            Progress::Indeterminate => {
                let w = track.w * 0.3;
                let x = track.x - w + (track.w + w) * self.phase;
                let x0 = x.max(track.x);
                let x1 = (x + w).min(track.right());
                if x1 > x0 {
                    cx.mark(
                        Rect::new(x0, track.y, x1 - x0, track.h),
                        ColorRole::Accent,
                        bg,
                        3.0,
                    );
                }
            }
            Progress::Idle => {}
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        match self.progress.get(cx.rt) {
            Progress::Fraction(f) => {
                node.set_numeric_value(f64::from(f.clamp(0.0, 1.0)) * 100.0);
                node.set_min_numeric_value(0.0);
                node.set_max_numeric_value(100.0);
            }
            Progress::Indeterminate => node.set_busy(),
            Progress::Idle => {}
        }
    }
}
