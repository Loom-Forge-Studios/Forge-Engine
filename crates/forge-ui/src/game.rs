//! Game UI on the same layer (Ch.21 §21.20, DoD M2-29): what a shipped game adds on top of
//! the widget catalogue.
//!
//! * **Resolution-independent scaling** — [`GameScaling`]: a reference resolution and a
//!   [`ScaleMode`] (fit, fill, or integer steps for pixel art, Ch.35) give the UI scale for
//!   any window; a runner applies it as the user scale ([`crate::platform_winit::UiApp::user_scale`]).
//! * **Safe-area insets** — [`SafeArea`], applied as padding on the game UI's root.
//! * **Gamepad navigation** — [`GamepadNav`] turns pad input into what the keyboard already
//!   does: the d-pad and left stick are arrow keys (with hold-to-repeat, deadline driven so
//!   an idle pad costs nothing), South activates, East backs out. With [`crate::Ui::game_nav`]
//!   and `spatial_arrows` on, Up/Down move between rows and Left/Right adjust the focused
//!   control or move focus — every catalogue widget is already keyboard-operable (§21.16),
//!   so nothing needs a gamepad-specific code path. Where the pad comes from is the
//!   platform HAL's job (Ch.27); [`GamepadSource`] is that seam.
//! * **Button glyphs** — [`ButtonGlyphs`], per platform family ([`GlyphSet`]), so prompts
//!   read "Ⓐ Select" on one pad and "✕ Select" on another.
//! * **The credits entry (E-64)** — [`Credits`] always starts with the
//!   [`ENGINE_CREDIT`] entry; [`Credits::build`] shows it as static labels. It is plain text
//!   and executes nothing, which keeps attribution inert (what Ch.38's
//!   `test_attribution_is_inert` checks once the packager inserts it, M5-27).

use std::time::Duration;

use crate::UiError;
use crate::focus::Direction;
use crate::geom::{Rect, Size};
use crate::id::{Key, WidgetId};
use crate::input::{InputEvent, KeyCode, KeyEvent, Modifiers};
use crate::layout::NodeStyle;
use crate::ui::Ui;
use crate::widgets::{Build, Container, Label, LabelKind};

/// The engine credit every product containing `forge-runtime` carries (Ch.38 §38.7, E-64).
pub const ENGINE_CREDIT: &str = "Made with Forge Engine";

// ---- scaling ------------------------------------------------------------------------------

/// How the reference resolution maps onto the window.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ScaleMode {
    /// The whole reference area is visible (letterboxed): `min` of the axis ratios.
    #[default]
    Fit,
    /// The reference area covers the window (cropped): `max` of the axis ratios.
    Fill,
    /// Whole-number steps only (pixel art, Ch.35): the largest integer that fits, at least 1.
    Integer,
}

/// A reference resolution plus a scale mode (see the module docs).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GameScaling {
    /// The size the UI was designed at (logical px).
    pub reference: Size,
    pub mode: ScaleMode,
    /// Clamp of the resulting scale (a tiny window stays legible, a huge one sane).
    pub min: f32,
    pub max: f32,
}

impl GameScaling {
    pub fn new(reference: Size, mode: ScaleMode) -> Self {
        Self {
            reference,
            mode,
            min: 0.25,
            max: 8.0,
        }
    }

    /// The UI scale for a window of `window` logical px (at scale 1).
    pub fn scale(&self, window: Size) -> f32 {
        if self.reference.w <= 0.0 || self.reference.h <= 0.0 || window.w <= 0.0 {
            return 1.0;
        }
        let sx = window.w / self.reference.w;
        let sy = window.h / self.reference.h;
        let s = match self.mode {
            ScaleMode::Fit => sx.min(sy),
            ScaleMode::Fill => sx.max(sy),
            ScaleMode::Integer => sx.min(sy).floor().max(1.0),
        };
        s.clamp(self.min, self.max)
    }

    /// Where the scaled reference area sits in the window (centred): the letterbox of
    /// `Fit`/`Integer`, or the crop of `Fill` (negative offsets).
    pub fn placement(&self, window: Size) -> Rect {
        let s = self.scale(window);
        let w = self.reference.w * s;
        let h = self.reference.h * s;
        Rect::new((window.w - w) * 0.5, (window.h - h) * 0.5, w, h)
    }
}

/// Safe-area insets (logical px): the part of the screen a TV or a notch may hide.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct SafeArea {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl SafeArea {
    /// The same inset on every side.
    pub fn uniform(px: f32) -> Self {
        Self {
            left: px,
            top: px,
            right: px,
            bottom: px,
        }
    }
    /// The title-safe area of a TV: 5 % of each dimension.
    pub fn tv(window: Size) -> Self {
        Self {
            left: window.w * 0.05,
            right: window.w * 0.05,
            top: window.h * 0.05,
            bottom: window.h * 0.05,
        }
    }
    /// `r` shrunk by the insets.
    pub fn inset(&self, r: Rect) -> Rect {
        Rect::new(
            r.x + self.left,
            r.y + self.top,
            (r.w - self.left - self.right).max(0.0),
            (r.h - self.top - self.bottom).max(0.0),
        )
    }
    /// `style` with the insets as padding (the game UI's root).
    pub fn pad(&self, mut style: NodeStyle) -> NodeStyle {
        style.layout.padding = taffy::Rect {
            left: taffy::prelude::length(self.left),
            right: taffy::prelude::length(self.right),
            top: taffy::prelude::length(self.top),
            bottom: taffy::prelude::length(self.bottom),
        };
        style
    }
}

// ---- gamepads -----------------------------------------------------------------------------

/// A pad button, by position (the platform's glyph comes from [`ButtonGlyphs`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PadButton {
    /// The bottom face button (Xbox A, PlayStation ✕, Nintendo B): confirm.
    South,
    /// The right face button (Xbox B, PlayStation ○, Nintendo A): back.
    East,
    West,
    North,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    LeftShoulder,
    RightShoulder,
    Start,
    Select,
}

impl PadButton {
    pub const ALL: [PadButton; 12] = [
        PadButton::South,
        PadButton::East,
        PadButton::West,
        PadButton::North,
        PadButton::DPadUp,
        PadButton::DPadDown,
        PadButton::DPadLeft,
        PadButton::DPadRight,
        PadButton::LeftShoulder,
        PadButton::RightShoulder,
        PadButton::Start,
        PadButton::Select,
    ];
    fn direction(self) -> Option<Direction> {
        match self {
            PadButton::DPadUp => Some(Direction::Up),
            PadButton::DPadDown => Some(Direction::Down),
            PadButton::DPadLeft => Some(Direction::Left),
            PadButton::DPadRight => Some(Direction::Right),
            _ => None,
        }
    }
}

/// One pad input sample.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PadInput {
    Button {
        button: PadButton,
        pressed: bool,
    },
    /// The left stick, each axis in −1..=1, `y` up.
    Stick {
        x: f32,
        y: f32,
    },
}

/// What a pad input means to the game, beyond moving focus and activating.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PadAction {
    /// East with no popup open: leave this screen.
    Back,
    /// Start: open or close the pause menu.
    Menu,
    /// The shoulders: the previous / next tab of a tabbed screen.
    PrevTab,
    NextTab,
    /// West, North and Select: the game decides.
    Button(PadButton),
}

/// Where pad input comes from: the platform HAL (Ch.27). The real backends (XInput /
/// Windows.Gaming.Input, evdev) are HAL work (`UNBUILT` until M8); [`ScriptedPad`] is the
/// labelled in-memory source tests and the sample drive (D-4).
pub trait GamepadSource {
    /// A short description (the settings screen shows it).
    fn describe(&self) -> String;
    /// Input since the last call, oldest first.
    fn poll(&mut self) -> Vec<PadInput>;
    /// The glyph family of the connected pad.
    fn glyphs(&self) -> GlyphSet {
        GlyphSet::Generic
    }
}

/// A queue of pad input (see [`GamepadSource`]). **In-memory**: no device is read.
#[derive(Debug, Default)]
pub struct ScriptedPad {
    queue: Vec<PadInput>,
    glyphs: GlyphSet,
}

impl ScriptedPad {
    pub fn new(glyphs: GlyphSet) -> Self {
        Self {
            queue: Vec::new(),
            glyphs,
        }
    }
    /// A press and a release.
    pub fn tap(&mut self, b: PadButton) {
        self.queue.push(PadInput::Button {
            button: b,
            pressed: true,
        });
        self.queue.push(PadInput::Button {
            button: b,
            pressed: false,
        });
    }
    pub fn push(&mut self, i: PadInput) {
        self.queue.push(i);
    }
}

impl GamepadSource for ScriptedPad {
    fn describe(&self) -> String {
        "scripted pad (in-memory; the platform HAL's pad backends are not built yet)".into()
    }
    fn poll(&mut self) -> Vec<PadInput> {
        std::mem::take(&mut self.queue)
    }
    fn glyphs(&self) -> GlyphSet {
        self.glyphs
    }
}

/// Stick deflection that starts a move, and the lower one that ends it (hysteresis, so a
/// stick resting near the threshold does not chatter).
pub const STICK_PRESS: f32 = 0.5;
pub const STICK_RELEASE: f32 = 0.35;
/// Hold-to-repeat: the first repeat after this…
pub const REPEAT_DELAY: Duration = Duration::from_millis(400);
/// …then one every this.
pub const REPEAT_EVERY: Duration = Duration::from_millis(120);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Held {
    DPad(PadButton),
    Stick(Direction),
}

/// Turns pad input into UI input (see the module docs). One per window.
#[derive(Debug, Default)]
pub struct GamepadNav {
    held: Option<(Held, Duration)>,
    stick: Option<Direction>,
    moves: u64,
}

fn arrow(d: Direction) -> KeyCode {
    match d {
        Direction::Up => KeyCode::Up,
        Direction::Down => KeyCode::Down,
        Direction::Left => KeyCode::Left,
        Direction::Right => KeyCode::Right,
    }
}

fn key(ui: &mut Ui, code: KeyCode, repeat: bool) {
    let mut down = KeyEvent::press(code.clone(), Modifiers::NONE);
    down.repeat = repeat;
    ui.handle(InputEvent::Key(down));
    let mut up = KeyEvent::press(code, Modifiers::NONE);
    up.pressed = false;
    up.text = None;
    ui.handle(InputEvent::Key(up));
}

impl GamepadNav {
    pub fn new() -> Self {
        Self::default()
    }

    /// Directional moves delivered so far (presses and repeats).
    pub fn moves(&self) -> u64 {
        self.moves
    }

    fn step(&mut self, ui: &mut Ui, d: Direction, repeat: bool) {
        self.moves += 1;
        key(ui, arrow(d), repeat);
    }

    /// Apply one input at `now` (the UI clock). Returns what the game must handle itself.
    pub fn input(&mut self, ui: &mut Ui, now: Duration, i: PadInput) -> Option<PadAction> {
        match i {
            PadInput::Stick { x, y } => {
                let mag = x.abs().max(y.abs());
                let dir = if x.abs() >= y.abs() {
                    if x >= 0.0 {
                        Direction::Right
                    } else {
                        Direction::Left
                    }
                } else if y >= 0.0 {
                    Direction::Up
                } else {
                    Direction::Down
                };
                match self.stick {
                    Some(_) if mag < STICK_RELEASE => {
                        self.stick = None;
                        if matches!(self.held, Some((Held::Stick(_), _))) {
                            self.held = None;
                        }
                    }
                    Some(cur) if cur != dir && mag >= STICK_PRESS => {
                        self.stick = Some(dir);
                        self.held = Some((Held::Stick(dir), now + REPEAT_DELAY));
                        self.step(ui, dir, false);
                    }
                    None if mag >= STICK_PRESS => {
                        self.stick = Some(dir);
                        self.held = Some((Held::Stick(dir), now + REPEAT_DELAY));
                        self.step(ui, dir, false);
                    }
                    _ => {}
                }
                None
            }
            PadInput::Button { button, pressed } => {
                if let Some(d) = button.direction() {
                    if pressed {
                        self.held = Some((Held::DPad(button), now + REPEAT_DELAY));
                        self.step(ui, d, false);
                    } else if matches!(self.held, Some((Held::DPad(b), _)) if b == button) {
                        self.held = None;
                    }
                    return None;
                }
                if !pressed {
                    return None;
                }
                match button {
                    PadButton::South => {
                        key(ui, KeyCode::Enter, false);
                        None
                    }
                    PadButton::East if ui.has_popups() => {
                        key(ui, KeyCode::Escape, false);
                        None
                    }
                    PadButton::East => Some(PadAction::Back),
                    PadButton::Start => Some(PadAction::Menu),
                    PadButton::LeftShoulder => Some(PadAction::PrevTab),
                    PadButton::RightShoulder => Some(PadAction::NextTab),
                    other => Some(PadAction::Button(other)),
                }
            }
        }
    }

    /// Run the repeats that are due at `now`.
    pub fn tick(&mut self, ui: &mut Ui, now: Duration) {
        while let Some((h, at)) = self.held {
            if at > now {
                break;
            }
            self.held = Some((h, at + REPEAT_EVERY));
            let d = match h {
                Held::DPad(b) => b.direction(),
                Held::Stick(d) => Some(d),
            };
            if let Some(d) = d {
                self.step(ui, d, true);
            }
        }
    }

    /// When the next repeat is due (`None` when nothing is held: an idle pad schedules no
    /// wakeup, D-5).
    pub fn next_deadline(&self) -> Option<Duration> {
        self.held.map(|(_, t)| t)
    }
}

// ---- button glyphs ------------------------------------------------------------------------

/// A pad family's button naming.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum GlyphSet {
    Xbox,
    PlayStation,
    Nintendo,
    /// Position names, for an unknown pad.
    #[default]
    Generic,
}

/// Per-platform button glyphs (the HAL picks the set for the connected pad, Ch.27).
pub trait ButtonGlyphs {
    fn glyph(&self, b: PadButton) -> &'static str;
}

impl ButtonGlyphs for GlyphSet {
    fn glyph(&self, b: PadButton) -> &'static str {
        use PadButton as P;
        match (self, b) {
            (GlyphSet::Xbox, P::South) => "A",
            (GlyphSet::Xbox, P::East) => "B",
            (GlyphSet::Xbox, P::West) => "X",
            (GlyphSet::Xbox, P::North) => "Y",
            (GlyphSet::Xbox, P::LeftShoulder) => "LB",
            (GlyphSet::Xbox, P::RightShoulder) => "RB",
            (GlyphSet::Xbox, P::Start) => "Menu",
            (GlyphSet::Xbox, P::Select) => "View",
            (GlyphSet::PlayStation, P::South) => "\u{2715}",
            (GlyphSet::PlayStation, P::East) => "\u{25cb}",
            (GlyphSet::PlayStation, P::West) => "\u{25a1}",
            (GlyphSet::PlayStation, P::North) => "\u{25b3}",
            (GlyphSet::PlayStation, P::LeftShoulder) => "L1",
            (GlyphSet::PlayStation, P::RightShoulder) => "R1",
            (GlyphSet::PlayStation, P::Start) => "Options",
            (GlyphSet::PlayStation, P::Select) => "Create",
            (GlyphSet::Nintendo, P::South) => "B",
            (GlyphSet::Nintendo, P::East) => "A",
            (GlyphSet::Nintendo, P::West) => "Y",
            (GlyphSet::Nintendo, P::North) => "X",
            (GlyphSet::Nintendo, P::LeftShoulder) => "L",
            (GlyphSet::Nintendo, P::RightShoulder) => "R",
            (GlyphSet::Nintendo, P::Start) => "+",
            (GlyphSet::Nintendo, P::Select) => "\u{2212}",
            (GlyphSet::Generic, P::South) => "South",
            (GlyphSet::Generic, P::East) => "East",
            (GlyphSet::Generic, P::West) => "West",
            (GlyphSet::Generic, P::North) => "North",
            (GlyphSet::Generic, P::LeftShoulder) => "LS",
            (GlyphSet::Generic, P::RightShoulder) => "RS",
            (GlyphSet::Generic, P::Start) => "Start",
            (GlyphSet::Generic, P::Select) => "Select",
            (_, P::DPadUp) => "\u{2191}",
            (_, P::DPadDown) => "\u{2193}",
            (_, P::DPadLeft) => "\u{2190}",
            (_, P::DPadRight) => "\u{2192}",
        }
    }
}

// ---- credits (E-64) -----------------------------------------------------------------------

/// One credits section: a heading and its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreditSection {
    pub heading: String,
    pub lines: Vec<String>,
}

/// The credits (see the module docs). The engine entry is always the first section; it
/// cannot be removed through this type, only by editing source (E-64's force is
/// contractual). Plain text: nothing here executes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credits {
    sections: Vec<CreditSection>,
}

impl Default for Credits {
    fn default() -> Self {
        Self::new()
    }
}

impl Credits {
    /// Credits holding the engine entry alone.
    pub fn new() -> Self {
        Self {
            sections: vec![CreditSection {
                heading: "Engine".into(),
                lines: vec![ENGINE_CREDIT.into()],
            }],
        }
    }
    /// Add the game's own section.
    pub fn section(mut self, heading: &str, lines: &[&str]) -> Self {
        self.sections.push(CreditSection {
            heading: heading.into(),
            lines: lines.iter().map(|l| (*l).to_string()).collect(),
        });
        self
    }
    pub fn sections(&self) -> &[CreditSection] {
        &self.sections
    }
    /// Every line, in order (the engine entry first).
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.sections
            .iter()
            .flat_map(|s| s.lines.iter().map(String::as_str))
    }
    /// Does the engine entry exist (the packager's check, Ch.38 §38.7)?
    pub fn has_engine_entry(&self) -> bool {
        self.lines().any(|l| l == ENGINE_CREDIT)
    }
    /// Show the credits under `parent`: a labelled group, one heading and one label per
    /// line — static text only. Returns the group's id.
    pub fn build(
        &self,
        b: &mut dyn Build,
        parent: WidgetId,
        key: impl Into<Key>,
        style: NodeStyle,
    ) -> Result<WidgetId, UiError> {
        self.build_with(b, parent, key, style, &|h| h.to_string())
    }

    /// As [`Credits::build`], each section heading shown as `heading` gives it (a game's
    /// localised text for the heading). The credit lines themselves — names, licences, and
    /// the engine entry E-64 fixes — are shown as written.
    pub fn build_with(
        &self,
        b: &mut dyn Build,
        parent: WidgetId,
        key: impl Into<Key>,
        style: NodeStyle,
        heading: &dyn Fn(&str) -> String,
    ) -> Result<WidgetId, UiError> {
        let space = b.theme_ref().space[2];
        let mut style = style;
        style.layout.gap = taffy::Size {
            width: taffy::prelude::length(space),
            height: taffy::prelude::length(space),
        };
        let root = b.add(parent, key, style, Container::pane("Credits"))?;
        for (i, s) in self.sections.iter().enumerate() {
            // Credits read centred.
            let mut col = NodeStyle::column(space * 0.5);
            col.layout.align_items = Some(taffy::AlignItems::CENTER);
            let shown = heading(&s.heading);
            let sec = b.add(
                root,
                Key::Str(format!("s{i}").into()),
                col,
                Container::pane(&shown),
            )?;
            b.add(
                sec,
                "heading",
                NodeStyle::leaf(),
                Label::new(shown).kind(LabelKind::Heading),
            )?;
            for (j, l) in s.lines.iter().enumerate() {
                b.add(
                    sec,
                    Key::Str(format!("l{j}").into()),
                    NodeStyle::leaf(),
                    Label::new(l.clone()),
                )?;
            }
        }
        Ok(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_modes() {
        let s = GameScaling::new(Size::new(1280.0, 720.0), ScaleMode::Fit);
        assert_eq!(s.scale(Size::new(2560.0, 1440.0)), 2.0);
        assert_eq!(
            s.scale(Size::new(2560.0, 1080.0)),
            1.5,
            "letterboxed on the short axis"
        );
        let f = GameScaling {
            mode: ScaleMode::Fill,
            ..s
        };
        assert_eq!(f.scale(Size::new(2560.0, 1080.0)), 2.0);
        assert!(f.placement(Size::new(2560.0, 1080.0)).y < 0.0, "cropped");
        let px = GameScaling::new(Size::new(320.0, 180.0), ScaleMode::Integer);
        assert_eq!(px.scale(Size::new(1920.0, 1080.0)), 6.0);
        assert_eq!(px.scale(Size::new(1000.0, 700.0)), 3.0);
        assert_eq!(
            px.scale(Size::new(200.0, 100.0)),
            1.0,
            "never below one step"
        );
        let r = px.placement(Size::new(1000.0, 700.0));
        assert_eq!((r.w, r.h), (960.0, 540.0));
        assert_eq!((r.x, r.y), (20.0, 80.0));
    }

    #[test]
    fn safe_area_insets() {
        let a = SafeArea::tv(Size::new(1920.0, 1080.0));
        let r = a.inset(Rect::new(0.0, 0.0, 1920.0, 1080.0));
        assert_eq!((r.x, r.y, r.w, r.h), (96.0, 54.0, 1728.0, 972.0));
    }

    #[test]
    fn glyphs_follow_the_family() {
        assert_eq!(GlyphSet::Xbox.glyph(PadButton::South), "A");
        assert_eq!(GlyphSet::Nintendo.glyph(PadButton::South), "B");
        assert_eq!(GlyphSet::PlayStation.glyph(PadButton::East), "\u{25cb}");
        for g in [
            GlyphSet::Xbox,
            GlyphSet::PlayStation,
            GlyphSet::Nintendo,
            GlyphSet::Generic,
        ] {
            for b in PadButton::ALL {
                assert!(!g.glyph(b).is_empty());
            }
        }
    }

    #[test]
    fn credits_always_carry_the_engine_entry_first() {
        let c = Credits::new().section("Studio", &["Jane Doe", "John Roe"]);
        assert_eq!(c.lines().next(), Some(ENGINE_CREDIT));
        assert!(c.has_engine_entry());
        assert_eq!(c.lines().count(), 3);
    }
}
