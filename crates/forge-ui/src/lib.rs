//! `forge-ui` — the one retained widget layer for the Forge editor **and** shipped games
//! (Ch.21, ADR 0001).
//!
//! Built on `taffy` (layout), `cosmic-text` (shaping), `AccessKit` (accessibility),
//! `wgpu` (rendering, one module) and `winit` (windows and input, one module).
//!
//! The pipeline (§21.3): a retained tree of [`widget::Widget`]s bound to fine-grained
//! [`state::Signal`]s; a change marks only its subscribers dirty; layout, painting and the
//! a11y tree are updated only for dirty widgets; and when nothing is damaged nothing is
//! rendered and the loop sleeps in `Wait` (the zero-idle rule, D-5).
//!
//! Module map (§21.2): each module does exactly the job its name says.
//! `render` holds the display list, the batcher, the [`render::UiRenderer`] trait and the
//! recording renderer, and names no `wgpu`; `render_wgpu` (feature `wgpu`) is the only
//! module that does. `platform_winit` (feature `winit`) is the runner.
#![forbid(unsafe_code)]

pub mod a11y;
pub mod anim;
pub mod clipboard;
pub mod color;
pub mod controls;
pub mod damage;
pub mod dnd;
pub mod dock;
pub mod focus;
pub mod fuzzy;
pub mod gallery;
pub mod gallery_catalogue;
pub mod game;
pub mod geom;
pub mod icons;
pub mod id;
pub mod ime;
pub mod input;
pub mod l10n;
pub mod layout;
pub mod overlay;
pub mod render;
pub mod state;
pub mod style;
pub mod testing;
pub mod text;
pub mod ui;
pub mod units;
pub mod virtualized;
pub mod widget;
pub mod widgets;

#[cfg(feature = "wgpu")]
pub mod render_wgpu;

#[cfg(feature = "winit")]
pub mod platform_winit;

pub use accesskit::Role;
pub use damage::{Dirty, LiveCell, LiveFeed, LiveSource, UiTime, UiWaker, Wake};
pub use geom::{Color, Point, PxRect, Rect, Size};
pub use icons::Icon;
pub use id::{Key, WidgetId};
pub use input::{
    Handled, ImeEvent, InputEvent, KeyCode, KeyEvent, Modifiers, PointerButton, UiEvent,
};
pub use layout::{FocusScope, NodeStyle};
pub use state::{Bind, Memo, Runtime, Signal};
pub use style::{ColorRole, Theme};
pub use ui::{ActionEnvelope, FrameReport, ScrollState, Ui, UiConfig};
pub use widget::Widget;

/// `forge-ui` errors. Every variant carries a stable code allocated in
/// `docs/error-codes.md` (Ch.1.2, W8).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UiError {
    /// UI-0001: a theme file is malformed or incomplete.
    Theme(String),
    /// UI-0002: a widget id is not in the tree.
    UnknownWidget(WidgetId),
    /// UI-0003: two children of one parent share a key.
    DuplicateKey(WidgetId),
    /// UI-0004: the layout engine rejected an operation.
    Layout(String),
    /// UI-0005: an image could not be added to the image atlas.
    Image(String),
    /// UI-0006: the GPU renderer failed (adapter, device, surface or readback).
    Gpu(String),
    /// UI-0007: the platform (window system, event loop) failed.
    Platform(String),
    /// UI-0008: a dock layout is invalid: a layout file does not parse or is from a newer
    /// editor, or an operation names a panel, area or split that is not there.
    Dock(String),
}

impl UiError {
    /// The stable error code.
    pub fn code(&self) -> &'static str {
        match self {
            UiError::Theme(_) => "UI-0001",
            UiError::UnknownWidget(_) => "UI-0002",
            UiError::DuplicateKey(_) => "UI-0003",
            UiError::Layout(_) => "UI-0004",
            UiError::Image(_) => "UI-0005",
            UiError::Gpu(_) => "UI-0006",
            UiError::Platform(_) => "UI-0007",
            UiError::Dock(_) => "UI-0008",
        }
    }
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code();
        match self {
            UiError::Theme(m) => write!(f, "{code}: theme: {m}"),
            UiError::UnknownWidget(id) => write!(f, "{code}: unknown widget {id:?}"),
            UiError::DuplicateKey(id) => write!(f, "{code}: duplicate child key (id {id:?})"),
            UiError::Layout(m) => write!(f, "{code}: layout: {m}"),
            UiError::Image(m) => write!(f, "{code}: image: {m}"),
            UiError::Gpu(m) => write!(f, "{code}: gpu: {m}"),
            UiError::Platform(m) => write!(f, "{code}: platform: {m}"),
            UiError::Dock(m) => write!(f, "{code}: dock: {m}"),
        }
    }
}

impl std::error::Error for UiError {}
