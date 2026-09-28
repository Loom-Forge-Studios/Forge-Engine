//! Logical-pixel geometry and colour (Ch.21 §21.6, §21.14).
//!
//! Every coordinate above `render` is a **logical** pixel. Physical pixels appear only in
//! [`PxRect`], at paint/render time, where edges snap to the device grid.

use serde::{Deserialize, Serialize};

/// A point in logical pixels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A size in logical pixels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    pub const ZERO: Size = Size { w: 0.0, h: 0.0 };
    pub const fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }
}

/// An axis-aligned rectangle in logical pixels (`x`,`y` is the top-left corner).
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn from_min_max(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0.0),
            h: (y1 - y0).max(0.0),
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }

    /// `other` lies entirely inside `self`.
    pub fn contains_rect(&self, other: &Rect) -> bool {
        other.is_empty()
            || (other.x >= self.x
                && other.y >= self.y
                && other.right() <= self.right() + 1e-3
                && other.bottom() <= self.bottom() + 1e-3)
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect::from_min_max(
            self.x.max(o.x),
            self.y.max(o.y),
            self.right().min(o.right()),
            self.bottom().min(o.bottom()),
        )
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        !self.intersect(o).is_empty()
    }

    /// The smallest rect containing both. An empty rect is the identity.
    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect::from_min_max(
            self.x.min(o.x),
            self.y.min(o.y),
            self.right().max(o.right()),
            self.bottom().max(o.bottom()),
        )
    }

    /// Grow by `d` on every side.
    pub fn outset(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }

    pub fn inset(&self, i: Insets) -> Rect {
        Rect::from_min_max(
            self.x + i.left,
            self.y + i.top,
            self.right() - i.right,
            self.bottom() - i.bottom,
        )
    }

    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    pub fn area(&self) -> f32 {
        if self.is_empty() {
            0.0
        } else {
            self.w * self.h
        }
    }

    /// Scale to physical pixels and snap outwards to the device grid (§21.14).
    pub fn to_px(&self, scale: f32) -> PxRect {
        let x0 = (self.x * scale).floor().max(0.0);
        let y0 = (self.y * scale).floor().max(0.0);
        let x1 = (self.right() * scale).ceil().max(0.0);
        let y1 = (self.bottom() * scale).ceil().max(0.0);
        // Pixel coordinates are non-negative and bounded by the window size.
        PxRect {
            x: x0 as u32,
            y: y0 as u32,
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        }
    }
}

/// Edge insets in logical pixels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Insets {
    pub const fn all(v: f32) -> Self {
        Self {
            left: v,
            top: v,
            right: v,
            bottom: v,
        }
    }
}

/// Per-corner radii (top-left, top-right, bottom-right, bottom-left).
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Corners {
    pub tl: f32,
    pub tr: f32,
    pub br: f32,
    pub bl: f32,
}

impl Corners {
    pub const ZERO: Corners = Corners {
        tl: 0.0,
        tr: 0.0,
        br: 0.0,
        bl: 0.0,
    };
    pub const fn all(r: f32) -> Self {
        Self {
            tl: r,
            tr: r,
            br: r,
            bl: r,
        }
    }
}

/// A rectangle in **physical** pixels. Only `render` and the platform layer use it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PxRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PxRect {
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
    pub fn right(&self) -> u32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> u32 {
        self.y + self.h
    }
    pub fn area(&self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }
    pub fn intersect(&self, o: &PxRect) -> PxRect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        if x1 <= x0 || y1 <= y0 {
            PxRect::default()
        } else {
            PxRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            }
        }
    }
    pub fn union(&self, o: &PxRect) -> PxRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        PxRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }
}

/// A physical size (surface / target size).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PhysicalSize {
    pub w: u32,
    pub h: u32,
}

/// An sRGB-encoded colour with straight alpha, each channel in `0..=1`.
///
/// Themes and widgets speak sRGB (what designers and WCAG use); the renderer converts to
/// linear for gamma-correct blending (§21.7 "Rendering quality").
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    /// Only for theme parsing, tests and `render`; widgets never name raw colours (§21.8).
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: f32::from(a) / 255.0,
        }
    }

    /// Parse `#rrggbb` or `#rrggbbaa`.
    pub fn parse_hex(s: &str) -> Option<Color> {
        let h = s.strip_prefix('#')?;
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            6 => Some(Self::from_rgba8(byte(0)?, byte(2)?, byte(4)?, 255)),
            8 => Some(Self::from_rgba8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Linear-light RGBA, alpha premultiplied, for the GPU.
    pub fn to_linear_premul(self) -> [f32; 4] {
        let l = |c: f32| srgb_to_linear(c) * self.a;
        [l(self.r), l(self.g), l(self.b), self.a]
    }

    /// WCAG 2.x relative luminance (alpha ignored).
    pub fn relative_luminance(self) -> f32 {
        0.2126 * srgb_to_linear(self.r)
            + 0.7152 * srgb_to_linear(self.g)
            + 0.0722 * srgb_to_linear(self.b)
    }

    /// Composite `self` over an opaque `bg` (sRGB space, as WCAG tools do).
    pub fn over(self, bg: Color) -> Color {
        let a = self.a;
        Color {
            r: self.r * a + bg.r * (1.0 - a),
            g: self.g * a + bg.g * (1.0 - a),
            b: self.b * a + bg.b * (1.0 - a),
            a: 1.0,
        }
    }

    /// Linear interpolation in sRGB space (animation of colour tokens).
    pub fn lerp(self, o: Color, t: f32) -> Color {
        Color {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }
}

/// WCAG 2.x contrast ratio of two opaque colours: `(L1 + 0.05) / (L2 + 0.05)`.
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (a.relative_luminance(), b.relative_luminance());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_black_white_is_21() {
        let w = Color::from_rgba8(255, 255, 255, 255);
        let b = Color::from_rgba8(0, 0, 0, 255);
        assert!((contrast_ratio(w, b) - 21.0).abs() < 0.01);
    }

    #[test]
    fn px_snapping_is_outward() {
        let r = Rect::new(10.3, 10.6, 5.0, 5.0).to_px(1.25);
        assert_eq!(
            r,
            PxRect {
                x: 12,
                y: 13,
                w: 8,
                h: 7
            }
        );
    }

    #[test]
    fn hex_parse() {
        assert_eq!(
            Color::parse_hex("#ff000080"),
            Some(Color::from_rgba8(255, 0, 0, 128))
        );
        assert_eq!(Color::parse_hex("ff0000"), None);
    }
}
