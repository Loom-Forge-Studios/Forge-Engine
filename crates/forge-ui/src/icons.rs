//! The Forge icon set (WP-U12): vector icons for the editor and for games — crisp at every
//! scale, themed by colour token, no font glyph lookup (so no system-font fallback, and the
//! same pixels on every machine). An icon is rasterised once per pixel size into the glyph
//! atlas's mask plane and drawn as a glyph (`Primitive::Icon`), so a row of icon buttons
//! stays in the text batch (§21.22): drawing icons as meshes cost two draw calls each.
//!
//! **Licence.** The icons are original drawings made for Forge Engine (no third-party
//! source) and are offered under **MIT OR Apache-2.0** (`crates/forge-ui/icons/LICENSE`),
//! so a game may ship them and a plugin may reuse them freely.
//!
//! **Format.** Each icon is a few strokes on a 24 × 24 grid in a tiny path language: `M x y`
//! (move), `L x y` (line), `C x1 y1 x2 y2 x y` (cubic), `Z` (close), `O cx cy r` (circle) and
//! `R x y w h` (rectangle). They are drawn with a 2-unit round-capped stroke scaled to the
//! box, in one colour token; an icon that carries meaning is contrast-checked as text
//! (§21.8).

use crate::geom::{Point, Rect};
use crate::render::mesh::{Path, PathBuilder};

/// One icon of the set.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Icon {
    Close,
    Check,
    Plus,
    Minus,
    ChevronRight,
    ChevronDown,
    ChevronLeft,
    ChevronUp,
    Search,
    Settings,
    Play,
    Pause,
    Stop,
    Record,
    Folder,
    File,
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Trash,
    Undo,
    Redo,
    Save,
    Warning,
    Error,
    Info,
    Grid,
    List,
    Move,
    Rotate,
    Scale,
    Brush,
    Camera,
    Light,
    Cube,
    Globe,
    User,
    Team,
    Link,
    Plug,
    Terminal,
    Menu,
    More,
    Filter,
    History,
    Key,
    Bell,
    Chart,
    Layers,
    Sun,
    Moon,
    Edit,
}

impl Icon {
    /// Every icon, in declaration order (the gallery's icon page, the set's guard).
    pub const ALL: [Icon; 53] = [
        Icon::Close,
        Icon::Check,
        Icon::Plus,
        Icon::Minus,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::ChevronLeft,
        Icon::ChevronUp,
        Icon::Search,
        Icon::Settings,
        Icon::Play,
        Icon::Pause,
        Icon::Stop,
        Icon::Record,
        Icon::Folder,
        Icon::File,
        Icon::Eye,
        Icon::EyeOff,
        Icon::Lock,
        Icon::Unlock,
        Icon::Trash,
        Icon::Undo,
        Icon::Redo,
        Icon::Save,
        Icon::Warning,
        Icon::Error,
        Icon::Info,
        Icon::Grid,
        Icon::List,
        Icon::Move,
        Icon::Rotate,
        Icon::Scale,
        Icon::Brush,
        Icon::Camera,
        Icon::Light,
        Icon::Cube,
        Icon::Globe,
        Icon::User,
        Icon::Team,
        Icon::Link,
        Icon::Plug,
        Icon::Terminal,
        Icon::Menu,
        Icon::More,
        Icon::Filter,
        Icon::History,
        Icon::Key,
        Icon::Bell,
        Icon::Chart,
        Icon::Layers,
        Icon::Sun,
        Icon::Moon,
        Icon::Edit,
    ];

    /// The icon's stable name (`forge.icon.<name>` in themes and plugin manifests).
    pub fn name(self) -> &'static str {
        self.def().0
    }

    /// The icon of this name.
    pub fn from_name(name: &str) -> Option<Icon> {
        Self::ALL.into_iter().find(|i| i.name() == name)
    }

    /// The drawing, in the path language of the module docs.
    pub fn source(self) -> &'static str {
        self.def().1
    }

    fn def(self) -> (&'static str, &'static str) {
        match self {
            Icon::Close => ("close", "M6 6 L18 18 M18 6 L6 18"),
            Icon::Check => ("check", "M5 12.5 L10 17.5 L19 7"),
            Icon::Plus => ("plus", "M12 5 L12 19 M5 12 L19 12"),
            Icon::Minus => ("minus", "M5 12 L19 12"),
            Icon::ChevronRight => ("chevron_right", "M9 6 L15 12 L9 18"),
            Icon::ChevronDown => ("chevron_down", "M6 9 L12 15 L18 9"),
            Icon::ChevronLeft => ("chevron_left", "M15 6 L9 12 L15 18"),
            Icon::ChevronUp => ("chevron_up", "M6 15 L12 9 L18 15"),
            Icon::Search => ("search", "O 10.5 10.5 6 M15 15 L20 20"),
            Icon::Settings => (
                "settings",
                "O 12 12 3 O 12 12 7 M12 2.5 L12 5 M12 19 L12 21.5 M2.5 12 L5 12 M19 12 L21.5 12 \
                 M5.3 5.3 L7 7 M17 17 L18.7 18.7 M5.3 18.7 L7 17 M17 7 L18.7 5.3",
            ),
            Icon::Play => ("play", "M8 5 L19 12 L8 19 Z"),
            Icon::Pause => ("pause", "M8 5 L8 19 M16 5 L16 19"),
            Icon::Stop => ("stop", "R 6 6 12 12"),
            Icon::Record => ("record", "O 12 12 6"),
            Icon::Folder => ("folder", "M3 6 L9 6 L11 8 L21 8 L21 19 L3 19 Z"),
            Icon::File => ("file", "M6 3 L14 3 L19 8 L19 21 L6 21 Z M14 3 L14 8 L19 8"),
            Icon::Eye => (
                "eye",
                "M2 12 C5 5.5 19 5.5 22 12 C19 18.5 5 18.5 2 12 Z O 12 12 3",
            ),
            Icon::EyeOff => (
                "eye_off",
                "M2 12 C5 5.5 19 5.5 22 12 C19 18.5 5 18.5 2 12 Z O 12 12 3 M4 4 L20 20",
            ),
            Icon::Lock => ("lock", "R 5 11 14 10 M8 11 L8 8 C8 3.5 16 3.5 16 8 L16 11"),
            Icon::Unlock => ("unlock", "R 5 11 14 10 M8 11 L8 8 C8 3.5 16 3.5 16 7"),
            Icon::Trash => (
                "trash",
                "M4 7 L20 7 M9 7 L9 4 L15 4 L15 7 M6 7 L7 20 L17 20 L18 7 M10 11 L10 16 \
                 M14 11 L14 16",
            ),
            Icon::Undo => (
                "undo",
                "M9 14 L4 9 L9 4 M4 9 L15 9 C18.3 9 20 11.2 20 14 C20 16.8 18.3 19 15 19 L11 19",
            ),
            Icon::Redo => (
                "redo",
                "M15 14 L20 9 L15 4 M20 9 L9 9 C5.7 9 4 11.2 4 14 C4 16.8 5.7 19 9 19 L13 19",
            ),
            Icon::Save => (
                "save",
                "M4 4 L16 4 L20 8 L20 20 L4 20 Z M8 4 L8 9 L15 9 L15 4 M8 20 L8 14 L16 14 L16 20",
            ),
            Icon::Warning => (
                "warning",
                "M12 3 L22 20 L2 20 Z M12 9 L12 14 M12 17 L12 17.2",
            ),
            Icon::Error => ("error", "O 12 12 9 M9 9 L15 15 M15 9 L9 15"),
            Icon::Info => ("info", "O 12 12 9 M12 11 L12 17 M12 7.5 L12 7.7"),
            Icon::Grid => ("grid", "R 4 4 6 6 R 14 4 6 6 R 4 14 6 6 R 14 14 6 6"),
            Icon::List => (
                "list",
                "M9 6 L20 6 M9 12 L20 12 M9 18 L20 18 M4 6 L5 6 M4 12 L5 12 M4 18 L5 18",
            ),
            Icon::Move => (
                "move",
                "M12 3 L12 21 M3 12 L21 12 M9 6 L12 3 L15 6 M9 18 L12 21 L15 18 M6 9 L3 12 L6 15 \
                 M18 9 L21 12 L18 15",
            ),
            Icon::Rotate => (
                "rotate",
                "M20 12 C20 16.4 16.4 20 12 20 C7.6 20 4 16.4 4 12 C4 7.6 7.6 4 12 4 \
                 C14.8 4 17.2 5.4 18.6 7.6 M19 3 L19 8 L14 8",
            ),
            Icon::Scale => ("scale", "R 4 11 9 9 M13 11 L20 4 M14 4 L20 4 L20 10"),
            Icon::Brush => (
                "brush",
                "M20 4 L11 13 M11 13 C8 12 5 14 5 17 C5 19 4 20 3 21 C7 21 11 20 11 16 Z",
            ),
            Icon::Camera => ("camera", "R 3 7 18 13 M8 7 L10 4 L14 4 L16 7 O 12 13.5 3.5"),
            Icon::Light => (
                "light",
                "M9 17 C9 14 6 12.5 6 9 C6 5.7 8.7 3 12 3 C15.3 3 18 5.7 18 9 C18 12.5 15 14 15 17 Z \
                 M9 20 L15 20 M10.5 22 L13.5 22",
            ),
            Icon::Cube => (
                "cube",
                "M12 3 L20 7.5 L20 16.5 L12 21 L4 16.5 L4 7.5 Z M4 7.5 L12 12 L20 7.5 M12 12 L12 21",
            ),
            Icon::Globe => (
                "globe",
                "O 12 12 9 M3 12 L21 12 M12 3 C8 7 8 17 12 21 M12 3 C16 7 16 17 12 21",
            ),
            Icon::User => (
                "user",
                "O 12 8 4 M4 21 C4 16.5 7.6 14 12 14 C16.4 14 20 16.5 20 21",
            ),
            Icon::Team => (
                "team",
                "O 9 8 3.5 M2 20 C2 16 5 14 9 14 C13 14 16 16 16 20 M15.5 4.8 C18 5.5 18 10.5 15.5 11.2 \
                 M18 14.3 C20.5 15 22 17 22 20",
            ),
            Icon::Link => (
                "link",
                "M9.5 14.5 L14.5 9.5 M11 6.5 L12.5 5 C14.5 3 17.5 3 19.5 5 C21.5 7 21.5 10 19.5 12 \
                 L18 13.5 M13 17.5 L11.5 19 C9.5 21 6.5 21 4.5 19 C2.5 17 2.5 14 4.5 12 L6 10.5",
            ),
            Icon::Plug => (
                "plug",
                "M9 3 L9 8 M15 3 L15 8 M6 8 L18 8 L18 12 C18 15.3 15.3 17 12 17 C8.7 17 6 15.3 6 12 Z \
                 M12 17 L12 21",
            ),
            Icon::Terminal => ("terminal", "R 3 4 18 16 M7 9 L10 12 L7 15 M12 15 L16 15"),
            Icon::Menu => ("menu", "M4 6 L20 6 M4 12 L20 12 M4 18 L20 18"),
            Icon::More => ("more", "O 6 12 1 O 12 12 1 O 18 12 1"),
            Icon::Filter => ("filter", "M4 5 L20 5 L14 12 L14 19 L10 21 L10 12 Z"),
            Icon::History => ("history", "O 12 12 9 M12 7 L12 12 L15.5 14"),
            Icon::Key => (
                "key",
                "O 8 15 4.5 M11.2 11.8 L20 3 M16.5 6.5 L19.5 9.5 M18.5 4.5 L21 7",
            ),
            Icon::Bell => (
                "bell",
                "M6 17 L6 11 C6 7.2 8.7 5 12 5 C15.3 5 18 7.2 18 11 L18 17 L20 19 L4 19 Z \
                 M10 21.5 L14 21.5",
            ),
            Icon::Chart => (
                "chart",
                "M4 20 L20 20 M7 17 L7 11 M12 17 L12 6 M17 17 L17 13",
            ),
            Icon::Layers => (
                "layers",
                "M12 3 L21 8 L12 13 L3 8 Z M3 12.5 L12 17.5 L21 12.5 M3 16.5 L12 21.5 L21 16.5",
            ),
            Icon::Sun => (
                "sun",
                "O 12 12 4 M12 2 L12 4 M12 20 L12 22 M2 12 L4 12 M20 12 L22 12 M4.9 4.9 L6.3 6.3 \
                 M17.7 17.7 L19.1 19.1 M4.9 19.1 L6.3 17.7 M17.7 6.3 L19.1 4.9",
            ),
            Icon::Edit => (
                "edit",
                "M4 20 L8.5 19 L19.5 8 L16 4.5 L5 15.5 Z M14 6.5 L17.5 10",
            ),
            Icon::Moon => (
                "moon",
                "M20 14.5 C18 19 12.5 21 8.5 18.5 C4.5 16 3.5 10.5 6.5 7 C8 5.2 10 4 12 4 \
                 C9 7.5 10 13 14 15 C16 16 18.2 15.8 20 14.5 Z",
            ),
        }
    }
}

/// The icon of the set a symbol glyph stands for (✕, ⚙, ⓘ, ✎), so a widget given a glyph
/// draws the vector icon instead of asking a font for the symbol.
pub fn for_glyph(glyph: &str) -> Option<Icon> {
    match glyph {
        "✕" | "✖" | "×" => Some(Icon::Close),
        "⚙" => Some(Icon::Settings),
        "ⓘ" | "ℹ" => Some(Icon::Info),
        "✎" | "✏" => Some(Icon::Edit),
        "★" => Some(Icon::Record),
        _ => None,
    }
}

/// A parse failure in an icon's drawing (the set's guard reports it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconError(pub String);

/// One drawing command, in grid units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    Move(f32, f32),
    Line(f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
    Circle(f32, f32, f32),
    Rect(f32, f32, f32, f32),
}

/// Parse a drawing (see the module docs).
pub fn parse(src: &str) -> Result<Vec<Cmd>, IconError> {
    let toks: Vec<&str> = src.split_whitespace().collect();
    let num = |t: &str| -> Result<f32, IconError> {
        t.parse::<f32>()
            .map_err(|e| IconError(format!("{t:?} in {src:?}: {e}")))
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        // `M6` is `M 6`: a verb may be glued to its first number.
        let (verb, glued) = t.split_at(t.chars().next().map_or(0, char::len_utf8));
        let arity = match verb {
            "M" | "L" => 2,
            "C" => 6,
            "Z" => 0,
            "O" => 3,
            "R" => 4,
            other => return Err(IconError(format!("unknown verb {other:?} in {src:?}"))),
        };
        let mut args = Vec::with_capacity(arity);
        if !glued.is_empty() {
            args.push(num(glued)?);
        }
        let mut j = i + 1;
        while args.len() < arity {
            let t = toks
                .get(j)
                .ok_or_else(|| IconError(format!("{verb} is missing a number in {src:?}")))?;
            args.push(num(t)?);
            j += 1;
        }
        out.push(match verb {
            "M" => Cmd::Move(args[0], args[1]),
            "L" => Cmd::Line(args[0], args[1]),
            "C" => Cmd::Cubic(args[0], args[1], args[2], args[3], args[4], args[5]),
            "Z" => Cmd::Close,
            "O" => Cmd::Circle(args[0], args[1], args[2]),
            _ => Cmd::Rect(args[0], args[1], args[2], args[3]),
        });
        i = j;
    }
    Ok(out)
}

/// The icon's drawing, parsed once per process (a repaint does not re-parse it).
fn commands(icon: Icon) -> &'static [Cmd] {
    static PARSED: std::sync::OnceLock<Vec<Vec<Cmd>>> = std::sync::OnceLock::new();
    let all = PARSED.get_or_init(|| {
        Icon::ALL
            .iter()
            // The set's guard proves every drawing parses; a broken one draws nothing.
            .map(|i| parse(i.source()).unwrap_or_default())
            .collect()
    });
    Icon::ALL
        .iter()
        .position(|i| *i == icon)
        .and_then(|k| all.get(k))
        .map_or(&[], Vec::as_slice)
}

/// The icon's path laid into `r` (the 24 × 24 grid scaled to the square centred in `r`).
pub fn path(icon: Icon, r: Rect) -> Path {
    let side = r.w.min(r.h);
    let k = side / 24.0;
    let o = Point::new(r.x + (r.w - side) * 0.5, r.y + (r.h - side) * 0.5);
    let p = |x: f32, y: f32| Point::new(o.x + x * k, o.y + y * k);
    let mut b = PathBuilder::new();
    for c in commands(icon).iter().copied() {
        match c {
            Cmd::Move(x, y) => {
                b.move_to(p(x, y));
            }
            Cmd::Line(x, y) => {
                b.line_to(p(x, y));
            }
            Cmd::Cubic(a, bb, c2, d, x, y) => {
                b.cubic_to(p(a, bb), p(c2, d), p(x, y));
            }
            Cmd::Close => {
                b.close();
            }
            Cmd::Circle(cx, cy, rad) => {
                // Four cubic quarter arcs.
                let kk = 0.552_284_8 * rad;
                b.move_to(p(cx + rad, cy));
                b.cubic_to(p(cx + rad, cy + kk), p(cx + kk, cy + rad), p(cx, cy + rad));
                b.cubic_to(p(cx - kk, cy + rad), p(cx - rad, cy + kk), p(cx - rad, cy));
                b.cubic_to(p(cx - rad, cy - kk), p(cx - kk, cy - rad), p(cx, cy - rad));
                b.cubic_to(p(cx + kk, cy - rad), p(cx + rad, cy - kk), p(cx + rad, cy));
                b.close();
            }
            Cmd::Rect(x, y, w, h) => {
                b.move_to(p(x, y));
                b.line_to(p(x + w, y));
                b.line_to(p(x + w, y + h));
                b.line_to(p(x, y + h));
                b.close();
            }
        }
    }
    b.build()
}

/// The stroke width of an icon drawn `side` logical pixels tall (2 grid units, at least
/// one pixel).
pub fn stroke_width(side: f32) -> f32 {
    (side / 12.0).max(1.0)
}

/// The icon's coverage mask, `px` × `px` physical pixels (row-major, one byte a pixel),
/// stroked `stroke` pixels wide with round caps and joins: what the glyph atlas holds.
pub fn mask(icon: Icon, px: u32, stroke: f32) -> Vec<u8> {
    use zeno::{Cap, Command, Join, Mask, Point as Zp, Stroke};
    let k = px as f32 / 24.0;
    let p = |x: f32, y: f32| Zp::new(x * k, y * k);
    let mut d: Vec<Command> = Vec::new();
    for c in commands(icon).iter().copied() {
        match c {
            Cmd::Move(x, y) => d.push(Command::MoveTo(p(x, y))),
            Cmd::Line(x, y) => d.push(Command::LineTo(p(x, y))),
            Cmd::Cubic(a, b, c2, dd, x, y) => {
                d.push(Command::CurveTo(p(a, b), p(c2, dd), p(x, y)));
            }
            Cmd::Close => d.push(Command::Close),
            Cmd::Circle(cx, cy, r) => {
                let kk = 0.552_284_8 * r;
                d.push(Command::MoveTo(p(cx + r, cy)));
                d.push(Command::CurveTo(
                    p(cx + r, cy + kk),
                    p(cx + kk, cy + r),
                    p(cx, cy + r),
                ));
                d.push(Command::CurveTo(
                    p(cx - kk, cy + r),
                    p(cx - r, cy + kk),
                    p(cx - r, cy),
                ));
                d.push(Command::CurveTo(
                    p(cx - r, cy - kk),
                    p(cx - kk, cy - r),
                    p(cx, cy - r),
                ));
                d.push(Command::CurveTo(
                    p(cx + kk, cy - r),
                    p(cx + r, cy - kk),
                    p(cx + r, cy),
                ));
                d.push(Command::Close);
            }
            Cmd::Rect(x, y, w, h) => {
                d.push(Command::MoveTo(p(x, y)));
                d.push(Command::LineTo(p(x + w, y)));
                d.push(Command::LineTo(p(x + w, y + h)));
                d.push(Command::LineTo(p(x, y + h)));
                d.push(Command::Close);
            }
        }
    }
    let mut style = Stroke::new(stroke);
    style.cap(Cap::Round).join(Join::Round);
    Mask::new(&d[..]).style(style).size(px, px).render().0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mask_covers_the_strokes_and_nothing_else() {
        // Close is a cross through the centre: the centre is inked, a corner is not.
        let m = mask(Icon::Close, 24, 2.0);
        assert_eq!(m.len(), 24 * 24);
        assert!(
            m[12 * 24 + 12] > 200,
            "the centre of the cross is not inked"
        );
        assert_eq!(m[0], 0, "the corner is inked");
        // Every icon of the set leaves ink at the editor's size.
        for i in Icon::ALL {
            assert!(
                mask(i, 16, 1.5).iter().any(|&c| c > 0),
                "{i:?} draws nothing"
            );
        }
    }

    #[test]
    fn every_icon_parses_stays_on_its_grid_and_has_a_unique_name() {
        let mut names = std::collections::BTreeSet::new();
        for i in Icon::ALL {
            let cmds = parse(i.source()).unwrap_or_else(|e| panic!("{i:?}: {e:?}"));
            assert!(!cmds.is_empty(), "{i:?} draws nothing");
            let pts: Vec<(f32, f32)> = cmds
                .iter()
                .flat_map(|c| match *c {
                    Cmd::Move(x, y) | Cmd::Line(x, y) => vec![(x, y)],
                    Cmd::Cubic(a, b, c, d, x, y) => vec![(a, b), (c, d), (x, y)],
                    Cmd::Close => vec![],
                    Cmd::Circle(x, y, r) => vec![(x - r, y - r), (x + r, y + r)],
                    Cmd::Rect(x, y, w, h) => vec![(x, y), (x + w, y + h)],
                })
                .collect();
            for (x, y) in pts {
                assert!(
                    (0.0..=24.0).contains(&x) && (0.0..=24.0).contains(&y),
                    "{i:?} leaves its 24 x 24 grid at ({x}, {y})"
                );
            }
            assert!(names.insert(i.name()), "two icons are called {}", i.name());
            assert_eq!(Icon::from_name(i.name()), Some(i));
        }
        assert_eq!(names.len(), Icon::ALL.len());
    }

    #[test]
    fn positive_control_a_broken_drawing_is_reported() {
        assert!(parse("M6 6 L18").is_err(), "a missing coordinate");
        assert!(parse("Q 1 2 3 4").is_err(), "an unknown verb");
        assert!(parse("M6 x").is_err(), "a bad number");
    }
}
