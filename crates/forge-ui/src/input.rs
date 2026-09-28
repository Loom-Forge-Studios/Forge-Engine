//! Platform-neutral input events and the events widgets receive (Ch.21 §21.9).
//!
//! The winit runner translates OS events into [`InputEvent`]; the `Ui` routes them
//! (hit testing, pointer capture, focus path, bubbling) into [`UiEvent`]s for widgets.
//! Tests drive [`InputEvent`] directly, so every behaviour here is testable headless.

use std::path::PathBuf;

use crate::dnd::DragPayload;
use crate::geom::Point;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    };
    pub const SHIFT: Modifiers = Modifiers {
        shift: true,
        ctrl: false,
        alt: false,
        meta: false,
    };
    pub const CTRL: Modifiers = Modifiers {
        shift: false,
        ctrl: true,
        alt: false,
        meta: false,
    };
}

/// A key, independent of layout for the named keys, and the produced character otherwise.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum KeyCode {
    Tab,
    Enter,
    Escape,
    Space,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Insert,
    /// The context-menu (application) key.
    ContextMenu,
    /// A character key, lower-cased (`Ctrl+Z` arrives as `Char('z')` with `ctrl`).
    Char(char),
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub mods: Modifiers,
    pub pressed: bool,
    pub repeat: bool,
    /// Text this key press inserts, if any (already composed by the OS for dead keys).
    pub text: Option<String>,
}

impl KeyEvent {
    pub fn press(code: KeyCode, mods: Modifiers) -> Self {
        let text = match (&code, mods.ctrl || mods.alt || mods.meta) {
            (KeyCode::Char(c), false) => Some(if mods.shift {
                c.to_uppercase().collect()
            } else {
                c.to_string()
            }),
            (KeyCode::Space, false) => Some(" ".into()),
            _ => None,
        };
        Self {
            code,
            mods,
            pressed: true,
            repeat: false,
            text,
        }
    }
}

/// IME composition (§21.9): pre-edit text is drawn in place and never committed to the
/// model before `Commit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeEvent {
    Enabled,
    /// Replace the pre-edit text; `cursor` is a byte range within it.
    Preedit {
        text: String,
        cursor: Option<(usize, usize)>,
    },
    Commit(String),
    Disabled,
}

/// A window-level input event, in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    PointerMoved(Point),
    PointerButton {
        pos: Point,
        button: PointerButton,
        pressed: bool,
    },
    PointerLeft,
    Wheel {
        pos: Point,
        dx: f32,
        dy: f32,
    },
    Key(KeyEvent),
    Ime(ImeEvent),
    /// An OS file drop (becomes an import command in the editor, §21.9).
    FileDropped {
        pos: Point,
        path: PathBuf,
    },
    /// The window gained or lost OS focus.
    WindowFocused(bool),
    /// The keyboard modifier state changed (pointer events read it: Ctrl/Shift-click).
    Modifiers(Modifiers),
}

/// An event delivered to a widget.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    PointerEnter,
    PointerLeave,
    PointerMove {
        pos: Point,
    },
    PointerDown {
        pos: Point,
        button: PointerButton,
        /// 1 for a single click, 2 for a double click, 3 for a triple click.
        clicks: u8,
    },
    PointerUp {
        pos: Point,
        button: PointerButton,
        /// Released over the widget that received the press.
        inside: bool,
    },
    Wheel {
        dx: f32,
        dy: f32,
    },
    Key(KeyEvent),
    Ime(ImeEvent),
    FocusGained {
        by_keyboard: bool,
    },
    FocusLost,
    /// A timer this widget set fired.
    Timer(u64),
    /// An animation frame this widget requested is due.
    AnimFrame,
    /// The widget became visible or hidden (tab switch, collapse, window occlusion).
    VisibilityChanged(bool),
    /// A signal this widget subscribed to changed (delivered at step 4, after the flush).
    BindingChanged,
    /// A live feed this widget owns changed (already rate-capped, only while visible).
    FeedChanged(crate::damage::FeedId),
    /// An assistive technology requested an action.
    A11yAction(accesskit::Action),
    /// A drag is over this widget: reply with [`crate::widget::EventCx::set_drop_verdict`].
    DragOver(DragPayload),
    /// The drag this widget answered last moved off it, was refused here on release, or
    /// was cancelled: drop any drop preview.
    DragLeave,
    Drop(DragPayload),
    /// Sent to a drag's source when Escape cancelled it (nothing was dropped).
    DragCancelled,
    /// A popup this widget opened was closed (outside click, Escape, or programmatically).
    PopupClosed(crate::id::WidgetId),
    /// The scroll offset or content extent of this widget's scroll viewport changed.
    ScrollChanged,
    /// An assistive technology requested an action on one of this widget's virtual
    /// children (a list row, a menu item): the child's local key and the action.
    A11yChildAction(u64, accesskit::Action),
}

/// Whether a widget consumed an event (unhandled events bubble to the parent).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Handled {
    Yes,
    No,
}

impl Handled {
    pub fn is_yes(self) -> bool {
        self == Handled::Yes
    }
}
