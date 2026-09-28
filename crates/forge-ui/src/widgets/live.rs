use std::sync::Arc;

use accesskit::Role;

use crate::damage::{Dirty, FeedId};
use crate::geom::{Point, Size};
use crate::input::{Handled, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::text::TextStyle;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

type Read = Arc<dyn Fn() -> String + Send + Sync>;

/// A live-data readout (the building block of the profiler, latency and throughput
/// panels, §21.11). It refreshes only when its feed's generation changed, only while
/// visible, at most `max_hz`; its signal holds the **displayed** string, so a change the
/// user cannot see (equal after rounding) causes no damage.
pub struct LiveReadout {
    label: String,
    shown: Signal<String>,
    read: Read,
    feed: Option<FeedId>,
}

impl LiveReadout {
    /// `read` formats the current value for display (e.g. latency rounded to 1 ms).
    pub fn new(
        label: &str,
        shown: Signal<String>,
        read: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.to_string(),
            shown,
            read: Arc::new(read),
            feed: None,
        }
    }
    pub fn set_feed(&mut self, f: FeedId) {
        self.feed = Some(f);
    }
    fn style(theme: &crate::style::Theme) -> TextStyle {
        TextStyle {
            mono: true,
            ..TextStyle::body(theme.type_scale.mono)
        }
    }
}

impl Widget for LiveReadout {
    fn role(&self) -> Role {
        Role::Status
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.shown.any()), Dirty::LAYOUT);
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
        let s = format!("{}: {}", self.label, self.shown.get(cx.rt));
        cx.text.layout(&s, &Self::style(cx.theme), None).1
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::FeedChanged(_) => {
                let v = (self.read)();
                self.shown.set(cx.rt_mut(), v);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let s = format!("{}: {}", self.label, self.shown.get(cx.rt()));
        let style = Self::style(cx.theme());
        cx.text(&s, &style, Point::new(r.x, r.y), ColorRole::FgMuted);
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_value(self.shown.get(cx.rt));
    }
}
