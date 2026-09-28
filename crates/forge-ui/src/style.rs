//! Design tokens, themes and style resolution (Ch.21 §21.8).
//!
//! Widgets name **tokens** ([`ColorRole`], spacing steps, radii, the type scale, shadows,
//! motion) and never raw colours: `tests/test_ui_contrast.rs` rejects a colour literal
//! anywhere under `src/widgets/`. A theme is a RON file of token values; the three
//! first-party themes are embedded, and users add more by copying one.
//!
//! Style resolution goes widget type → variant → state → local override. It runs in a
//! widget's `paint`, which only runs when the widget is `PAINT`-dirty (a state change),
//! so it is resolved once per change and never per frame.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::UiError;
use crate::geom::{Color, contrast_ratio};

/// Every colour token a theme supplies.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ColorRole {
    BgBase,
    BgRaised,
    BgSunken,
    BgHover,
    BgPressed,
    FgPrimary,
    FgMuted,
    FgOnAccent,
    FgOnDanger,
    Accent,
    AccentHover,
    AccentPressed,
    Danger,
    DangerHover,
    Warning,
    Success,
    FocusRing,
    Selection,
    Border,
    /// The dimming layer behind a modal dialog (never a text or boundary pair).
    Scrim,
}

impl ColorRole {
    pub const ALL: [ColorRole; 20] = [
        ColorRole::BgBase,
        ColorRole::BgRaised,
        ColorRole::BgSunken,
        ColorRole::BgHover,
        ColorRole::BgPressed,
        ColorRole::FgPrimary,
        ColorRole::FgMuted,
        ColorRole::FgOnAccent,
        ColorRole::FgOnDanger,
        ColorRole::Accent,
        ColorRole::AccentHover,
        ColorRole::AccentPressed,
        ColorRole::Danger,
        ColorRole::DangerHover,
        ColorRole::Warning,
        ColorRole::Success,
        ColorRole::FocusRing,
        ColorRole::Selection,
        ColorRole::Border,
        ColorRole::Scrim,
    ];

    /// The token's name in theme files (`bg.base`, `focus.ring`, ...).
    pub fn token(self) -> &'static str {
        match self {
            ColorRole::BgBase => "bg.base",
            ColorRole::BgRaised => "bg.raised",
            ColorRole::BgSunken => "bg.sunken",
            ColorRole::BgHover => "bg.hover",
            ColorRole::BgPressed => "bg.pressed",
            ColorRole::FgPrimary => "fg.primary",
            ColorRole::FgMuted => "fg.muted",
            ColorRole::FgOnAccent => "fg.on_accent",
            ColorRole::FgOnDanger => "fg.on_danger",
            ColorRole::Accent => "accent",
            ColorRole::AccentHover => "accent.hover",
            ColorRole::AccentPressed => "accent.pressed",
            ColorRole::Danger => "danger",
            ColorRole::DangerHover => "danger.hover",
            ColorRole::Warning => "warning",
            ColorRole::Success => "success",
            ColorRole::FocusRing => "focus.ring",
            ColorRole::Selection => "selection",
            ColorRole::Border => "border",
            ColorRole::Scrim => "scrim",
        }
    }
}

/// What a colour pair is used for, which decides its contrast floor.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PairKind {
    /// Text (or a glyph icon carrying meaning) on a background: the theme's text floor.
    Text,
    /// A UI boundary (border, focus ring, check mark, slider thumb): the theme's UI floor.
    Boundary,
}

/// One `(foreground, background)` use the contrast guard checks.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColorPair {
    pub fg: ColorRole,
    pub bg: ColorRole,
    pub kind: PairKind,
}

/// An elevation shadow token.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Shadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
}

impl Shadow {
    /// How far the shadow reaches beyond its rect (damage outset, §21.11).
    pub fn outset(&self) -> f32 {
        self.blur + self.spread + self.offset_x.abs().max(self.offset_y.abs())
    }
}

/// A corner radius: a theme token or explicit logical pixels (rounded panel backgrounds).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Radius {
    Sm,
    Md,
    Lg,
    Px(f32),
}

impl Radius {
    pub fn resolve(self, theme: &Theme) -> f32 {
        match self {
            Radius::Sm => theme.radius.sm,
            Radius::Md => theme.radius.md,
            Radius::Lg => theme.radius.lg,
            Radius::Px(p) => p,
        }
    }
}

/// Radii tokens.
#[derive(Copy, Clone, Debug, Deserialize, PartialEq)]
pub struct Radii {
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
}

/// The type scale, in logical pixels.
#[derive(Copy, Clone, Debug, Deserialize, PartialEq)]
pub struct TypeScale {
    pub small: f32,
    pub body: f32,
    pub heading: f32,
    pub mono: f32,
}

/// Motion tokens (durations; reduced motion makes them 0, §21.15).
#[derive(Copy, Clone, Debug, Deserialize, PartialEq)]
pub struct Motion {
    pub fast_ms: u32,
    pub normal_ms: u32,
    pub slow_ms: u32,
}

/// A full token set.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,
    colors: BTreeMap<ColorRole, Color>,
    /// Spacing steps `space[0]..space[5]`.
    pub space: [f32; 6],
    pub radius: Radii,
    pub border_width: f32,
    pub focus_ring_width: f32,
    pub type_scale: TypeScale,
    /// Elevation shadows, lowest first.
    pub shadows: [Shadow; 3],
    pub motion: Motion,
    /// Minimum contrast for text on its background (4.5 AA, 7 AAA for high contrast).
    pub text_contrast_floor: f32,
    /// Minimum contrast for UI boundaries (3.0).
    pub ui_contrast_floor: f32,
}

#[derive(Deserialize)]
struct ShadowFile {
    offset_x: f32,
    offset_y: f32,
    blur: f32,
    spread: f32,
    color: String,
}

#[derive(Deserialize)]
struct ThemeFile {
    name: String,
    text_contrast_floor: f32,
    ui_contrast_floor: f32,
    colors: BTreeMap<String, String>,
    space: Vec<f32>,
    radius: Radii,
    border_width: f32,
    focus_ring_width: f32,
    type_scale: TypeScale,
    shadows: Vec<ShadowFile>,
    motion: Motion,
}

pub const DARK_RON: &str = include_str!("../themes/forge.dark.ron");
pub const LIGHT_RON: &str = include_str!("../themes/forge.light.ron");
pub const HIGH_CONTRAST_RON: &str = include_str!("../themes/forge.high-contrast.ron");

impl Theme {
    /// Parse and validate a theme file: every token present, every colour well-formed.
    pub fn from_ron(src: &str) -> Result<Theme, UiError> {
        let f: ThemeFile = ron::from_str(src).map_err(|e| UiError::Theme(e.to_string()))?;
        let mut colors = BTreeMap::new();
        for role in ColorRole::ALL {
            let hex = f.colors.get(role.token()).ok_or_else(|| {
                UiError::Theme(format!("{}: missing token {}", f.name, role.token()))
            })?;
            let c = Color::parse_hex(hex).ok_or_else(|| {
                UiError::Theme(format!(
                    "{}: {} = {hex:?} is not #rrggbb[aa]",
                    f.name,
                    role.token()
                ))
            })?;
            colors.insert(role, c);
        }
        if let Some(unknown) = f
            .colors
            .keys()
            .find(|k| !ColorRole::ALL.iter().any(|r| r.token() == k.as_str()))
        {
            return Err(UiError::Theme(format!(
                "{}: unknown token {unknown}",
                f.name
            )));
        }
        let space: [f32; 6] = f.space.as_slice().try_into().map_err(|_| {
            UiError::Theme(format!(
                "{}: space needs 6 steps, has {}",
                f.name,
                f.space.len()
            ))
        })?;
        if f.shadows.len() != 3 {
            return Err(UiError::Theme(format!(
                "{}: shadows needs 3 elevations, has {}",
                f.name,
                f.shadows.len()
            )));
        }
        let mut shadows = [Shadow::default(); 3];
        for (i, s) in f.shadows.iter().enumerate() {
            shadows[i] = Shadow {
                offset_x: s.offset_x,
                offset_y: s.offset_y,
                blur: s.blur,
                spread: s.spread,
                color: Color::parse_hex(&s.color).ok_or_else(|| {
                    UiError::Theme(format!("{}: shadow colour {:?}", f.name, s.color))
                })?,
            };
        }
        Ok(Theme {
            name: f.name,
            colors,
            space,
            radius: f.radius,
            border_width: f.border_width,
            focus_ring_width: f.focus_ring_width,
            type_scale: f.type_scale,
            shadows,
            motion: f.motion,
            text_contrast_floor: f.text_contrast_floor,
            ui_contrast_floor: f.ui_contrast_floor,
        })
    }

    /// `forge.dark`, the default.
    pub fn dark() -> Theme {
        Self::builtin(DARK_RON)
    }
    pub fn light() -> Theme {
        Self::builtin(LIGHT_RON)
    }
    pub fn high_contrast() -> Theme {
        Self::builtin(HIGH_CONTRAST_RON)
    }
    /// The three first-party themes.
    pub fn builtins() -> [Theme; 3] {
        [Self::dark(), Self::light(), Self::high_contrast()]
    }

    fn builtin(src: &str) -> Theme {
        // The embedded files are validated by `builtin_themes_parse`; a broken one is a
        // build defect, and the fallback keeps the UI drawable rather than panicking.
        Theme::from_ron(src).unwrap_or_else(|_| Theme::fallback())
    }

    /// A minimal legible theme used only if an embedded theme failed to parse.
    fn fallback() -> Theme {
        let mut colors = BTreeMap::new();
        for r in ColorRole::ALL {
            let v = match r {
                ColorRole::FgPrimary
                | ColorRole::FgMuted
                | ColorRole::FgOnAccent
                | ColorRole::FgOnDanger
                | ColorRole::Border
                | ColorRole::FocusRing => 255,
                _ => 0,
            };
            colors.insert(r, Color::from_rgba8(v, v, v, 255));
        }
        Theme {
            name: "forge.fallback".into(),
            colors,
            space: [2.0, 4.0, 8.0, 12.0, 16.0, 24.0],
            radius: Radii {
                sm: 2.0,
                md: 4.0,
                lg: 8.0,
            },
            border_width: 1.0,
            focus_ring_width: 2.0,
            type_scale: TypeScale {
                small: 12.0,
                body: 14.0,
                heading: 18.0,
                mono: 13.0,
            },
            shadows: [Shadow::default(); 3],
            motion: Motion {
                fast_ms: 0,
                normal_ms: 0,
                slow_ms: 0,
            },
            text_contrast_floor: 4.5,
            ui_contrast_floor: 3.0,
        }
    }

    /// The colour of a token.
    pub fn color(&self, role: ColorRole) -> Color {
        self.colors
            .get(&role)
            .copied()
            .unwrap_or(Color::TRANSPARENT)
    }

    /// Override one token (users' theme edits, and the contrast guard's positive control).
    pub fn set_color(&mut self, role: ColorRole, c: Color) {
        self.colors.insert(role, c);
    }

    /// The floor a pair must meet in this theme.
    pub fn floor(&self, kind: PairKind) -> f32 {
        match kind {
            PairKind::Text => self.text_contrast_floor,
            PairKind::Boundary => self.ui_contrast_floor,
        }
    }

    /// The contrast of a pair in this theme (fg composited over bg).
    pub fn contrast(&self, p: ColorPair) -> f32 {
        let bg = self.color(p.bg).over(self.color(ColorRole::BgBase));
        contrast_ratio(self.color(p.fg).over(bg), bg)
    }
}

/// Interaction state that selects a style (§21.8).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WidgetState {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    /// Focus arrived by keyboard: the focus ring is drawn (§21.9).
    pub focus_visible: bool,
    pub disabled: bool,
}

/// Button variants (§21.16).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Variant {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
}

impl Variant {
    pub const ALL: [Variant; 4] = [
        Variant::Primary,
        Variant::Secondary,
        Variant::Ghost,
        Variant::Danger,
    ];
}

/// A resolved button-like style: background, foreground and optional border tokens.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ButtonStyle {
    /// `None`: transparent, the parent's background shows through.
    pub bg: Option<ColorRole>,
    pub fg: ColorRole,
    pub border: Option<ColorRole>,
}

/// type → variant → state resolution for buttons.
pub fn button_style(variant: Variant, s: WidgetState) -> ButtonStyle {
    let pick = |base, hover, pressed| {
        if s.pressed {
            pressed
        } else if s.hovered {
            hover
        } else {
            base
        }
    };
    match variant {
        Variant::Primary => ButtonStyle {
            bg: Some(pick(
                ColorRole::Accent,
                ColorRole::AccentHover,
                ColorRole::AccentPressed,
            )),
            fg: ColorRole::FgOnAccent,
            border: None,
        },
        Variant::Secondary => ButtonStyle {
            bg: Some(pick(
                ColorRole::BgRaised,
                ColorRole::BgHover,
                ColorRole::BgPressed,
            )),
            fg: ColorRole::FgPrimary,
            border: Some(ColorRole::Border),
        },
        Variant::Ghost => ButtonStyle {
            bg: if s.pressed {
                Some(ColorRole::BgPressed)
            } else if s.hovered {
                Some(ColorRole::BgHover)
            } else {
                None
            },
            fg: ColorRole::FgPrimary,
            border: None,
        },
        Variant::Danger => ButtonStyle {
            bg: Some(pick(
                ColorRole::Danger,
                ColorRole::DangerHover,
                ColorRole::DangerHover,
            )),
            fg: ColorRole::FgOnDanger,
            border: None,
        },
    }
}

/// Every interaction state a style can resolve to (for the contrast guard).
pub fn all_states() -> [WidgetState; 3] {
    [
        WidgetState::default(),
        WidgetState {
            hovered: true,
            ..WidgetState::default()
        },
        WidgetState {
            hovered: true,
            pressed: true,
            ..WidgetState::default()
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_parse() {
        for src in [DARK_RON, LIGHT_RON, HIGH_CONTRAST_RON] {
            let t = Theme::from_ron(src).unwrap_or_else(|e| panic!("{e}"));
            assert!(t.name.starts_with("forge."));
        }
        assert_eq!(Theme::high_contrast().text_contrast_floor, 7.0);
    }

    #[test]
    fn missing_token_is_rejected() {
        let broken = DARK_RON.replace("\"focus.ring\"", "\"focus.rung\"");
        let e = Theme::from_ron(&broken).err().map(|e| e.to_string());
        assert!(e.is_some_and(|e| e.contains("focus.ring")));
    }
}
