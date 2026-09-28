//! [`FeedRelay`]: an invisible widget that owns a live feed (§21.11) and turns each refresh
//! the UI grants it into an action its panel handles (WP-U10; the connect panels keep their
//! own copy, WP-U9).
//!
//! A panel that shows data changing off the bus — a teammate's presence, a claim, the review
//! queue — registers the feed on a relay. The UI's live-feed rules decide *when* the panel
//! may look (only while visible, only when the source's generation moved, at most `max_hz`);
//! the relay raises [`FeedTicked`] then, and the panel asks for a turn in which its sync step
//! reads the source. An idle source never wakes the editor.

use accesskit::Role;
use forge_ui::input::{Handled, UiEvent};
use forge_ui::widget::{A11yCx, EventCx, MeasureCx, PaintCx, Widget};
use forge_ui::{Size, WidgetId};

/// The relay's feed refreshed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FeedTicked {
    pub relay: WidgetId,
}

/// The relay's own timer (see [`FeedRelay`]'s event handling).
const FLUSH_TAG: u64 = 0xF2_u64 << 48;

/// See the module docs.
#[derive(Debug, Default)]
pub struct FeedRelay {
    /// Refreshes the UI granted (tests read it).
    pub refreshes: u64,
}

impl FeedRelay {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Widget for FeedRelay {
    fn role(&self) -> Role {
        Role::GenericContainer
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
        Size::new(0.0, 0.0)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::FeedChanged(_) => {
                self.refreshes += 1;
                let relay = cx.id();
                cx.action(FeedTicked { relay });
                // A refresh may arrive after the app took this frame's actions (a feed
                // refreshed as its panel became visible): a due-now timer makes the loop turn
                // once more, so the panel sees the action now.
                cx.set_timer(std::time::Duration::ZERO, FLUSH_TAG);
                Handled::Yes
            }
            UiEvent::Timer(FLUSH_TAG) => Handled::Yes,
            _ => Handled::No,
        }
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_hidden();
    }
}
