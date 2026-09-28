//! [`SignalRelay`]: an invisible widget that turns a signal change into an action.
//!
//! Signal-bound editors (checkbox, combo box, vector editor) write their signal and raise
//! nothing. A panel that must *act* on a change — emit a command — can watch the signal
//! with an effect, but an effect lives as long as the runtime, so a panel that rebuilds
//! (the inspector on every selection change) would pile up effects. A relay is a widget:
//! it lives exactly as long as the part of the tree it sits in, and raises
//! [`SignalChanged`] (routed to the panel's handler like any widget action) whenever its
//! signal changes. It lays out to nothing and paints nothing.

use accesskit::Role;

use crate::damage::Dirty;
use crate::geom::Size;
use crate::id::WidgetId;
use crate::input::{Handled, UiEvent};
use crate::state::AnySignal;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

/// A watched signal changed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SignalChanged {
    pub relay: WidgetId,
}

/// See the module docs.
pub struct SignalRelay {
    signal: AnySignal,
}

impl SignalRelay {
    pub fn new(signal: AnySignal) -> Self {
        Self { signal }
    }
}

impl Widget for SignalRelay {
    fn role(&self) -> Role {
        Role::GenericContainer
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.signal), Dirty::NONE);
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
        if *ev == UiEvent::BindingChanged {
            let relay = cx.id();
            cx.action(SignalChanged { relay });
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_hidden();
    }
}
