//! The IME bridge (Ch.21 §21.9).
//!
//! * `winit` `Ime::Preedit` / `Ime::Commit` drive the focused text widget.
//! * IME is enabled only while a text widget has focus ([`ImeRequest::allowed`]).
//! * The caret rect is reported whenever it moves, so the platform can call
//!   `Window::set_ime_cursor_area` and the candidate window follows the caret.

use crate::geom::Rect;

/// What the platform should tell the OS input method after a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImeRequest {
    /// A text widget has focus: enable the IME.
    pub allowed: bool,
    /// The caret rect in logical px, when a text widget reported one.
    pub caret: Option<Rect>,
}

/// UI-side IME state.
#[derive(Clone, Debug, Default)]
pub struct ImeState {
    pub(crate) allowed: bool,
    pub(crate) caret: Option<Rect>,
    /// The request last handed to the platform (so it is only re-sent on change).
    pub(crate) sent: Option<ImeRequest>,
}

impl ImeState {
    /// The current request, and whether it changed since the last call.
    pub fn take_request(&mut self) -> Option<ImeRequest> {
        let req = ImeRequest {
            allowed: self.allowed,
            caret: if self.allowed { self.caret } else { None },
        };
        if self.sent == Some(req) {
            None
        } else {
            self.sent = Some(req);
            Some(req)
        }
    }
}
