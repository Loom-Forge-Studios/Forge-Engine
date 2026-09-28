//! Colour *values* edited by the colour picker and the gradient editor (Ch.21 §21.16
//! "colour picker (HDR-aware: linear/sRGB, exposure, alpha)").
//!
//! These are **data** colours — a light's emission, a material tint — not theme colours.
//! Widgets never name raw theme colours (§21.8); data colours are built only here, so the
//! "no colour literal under `src/widgets/`" rule stays checkable.
//!
//! An [`HdrColor`] is **linear-light** RGB with straight alpha, unbounded above (an
//! emissive `4.0` is four times brighter than white). The picker edits it as
//! `hue / saturation / value` of a displayable base colour (perceptual, in sRGB) times an
//! **exposure** of `2^ev` stops. Hue is kept in [`Hsve`] so dragging saturation to zero
//! and back does not lose it.

use crate::geom::{Color, srgb_to_linear};

/// Linear-light RGB (unbounded) + straight alpha.
#[derive(Copy, Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HdrColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// sRGB transfer function (encode).
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

impl HdrColor {
    pub const WHITE: HdrColor = HdrColor::linear(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: HdrColor = HdrColor::linear(0.0, 0.0, 0.0, 1.0);

    pub const fn linear(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// From sRGB-encoded components in 0..=1.
    pub fn from_srgb(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self::linear(
            srgb_to_linear(r.clamp(0.0, 1.0)),
            srgb_to_linear(g.clamp(0.0, 1.0)),
            srgb_to_linear(b.clamp(0.0, 1.0)),
            a.clamp(0.0, 1.0),
        )
    }

    /// The brightest channel (> 1: HDR).
    pub fn intensity(&self) -> f32 {
        self.r.max(self.g).max(self.b)
    }
    pub fn is_hdr(&self) -> bool {
        self.intensity() > 1.0
    }

    /// What a swatch shows: clipped to displayable range, sRGB-encoded for the theme's
    /// colour space. HDR values clip (their exposure is shown next to the swatch).
    pub fn display(&self) -> Color {
        let e = |c: f32| linear_to_srgb(c.clamp(0.0, 1.0));
        let mut c = Color::TRANSPARENT;
        c.r = e(self.r);
        c.g = e(self.g);
        c.b = e(self.b);
        c.a = self.a.clamp(0.0, 1.0);
        c
    }

    /// The display colour at full opacity (swatch halves that ignore alpha).
    pub fn display_opaque(&self) -> Color {
        self.display().with_alpha(1.0)
    }

    /// `#rrggbb` (or `#rrggbbaa` when translucent) of the displayable colour.
    pub fn hex(&self) -> String {
        let d = self.display();
        let b = |c: f32| (c * 255.0).round().clamp(0.0, 255.0) as u8;
        if d.a < 1.0 {
            format!("#{:02x}{:02x}{:02x}{:02x}", b(d.r), b(d.g), b(d.b), b(d.a))
        } else {
            format!("#{:02x}{:02x}{:02x}", b(d.r), b(d.g), b(d.b))
        }
    }

    /// Parse `#rgb`, `#rrggbb` or `#rrggbbaa` (sRGB), with or without the `#`.
    pub fn parse_hex_input(s: &str) -> Option<Self> {
        let h = s.trim().trim_start_matches('#');
        let full = match h.len() {
            3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 | 8 => h.to_string(),
            _ => return None,
        };
        let c = Color::parse_hex(&format!("#{full}"))?;
        Some(Self::from_srgb(c.r, c.g, c.b, c.a))
    }

    /// Components as shown in a mode: linear floats, or sRGB 0..=255 (clipped).
    pub fn components(&self, mode: ColorSpaceMode) -> [f32; 4] {
        match mode {
            ColorSpaceMode::Linear => [self.r, self.g, self.b, self.a],
            ColorSpaceMode::Srgb => {
                let d = self.display();
                [d.r * 255.0, d.g * 255.0, d.b * 255.0, d.a * 255.0]
            }
        }
    }
}

/// How channel numbers are shown and typed.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ColorSpaceMode {
    /// Linear-light floats (what the renderer uses; may exceed 1).
    #[default]
    Linear,
    /// sRGB-encoded 0..=255 (what designers type; clipped to displayable).
    Srgb,
}

/// Hue (0..1), saturation (0..1), value (0..1) of the displayable base colour, exposure
/// in stops (≥ 0), alpha.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Hsve {
    pub h: f32,
    pub s: f32,
    pub v: f32,
    pub ev: f32,
    pub a: f32,
}

/// sRGB-encoded rgb (0..1) from hsv (0..1).
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// hsv from sRGB-encoded rgb; `keep_h` is used when the colour is grey (no hue).
pub fn rgb_to_hsv(rgb: [f32; 3], keep_h: f32) -> (f32, f32, f32) {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d <= 1e-6 {
        keep_h
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h, s, v)
}

impl Hsve {
    /// Decompose `c`; `prev` supplies the hue for greys and the saturation for black.
    pub fn from_color(c: HdrColor, prev: Option<Hsve>) -> Hsve {
        let m = c.intensity();
        let ev = if m > 1.0 { m.log2() } else { 0.0 };
        let k = 2f32.powf(-ev);
        let base = [
            linear_to_srgb((c.r * k).clamp(0.0, 1.0)),
            linear_to_srgb((c.g * k).clamp(0.0, 1.0)),
            linear_to_srgb((c.b * k).clamp(0.0, 1.0)),
        ];
        let (h, mut s, v) = rgb_to_hsv(base, prev.map_or(0.0, |p| p.h));
        if v <= 1e-6
            && let Some(p) = prev
        {
            s = p.s;
        }
        Hsve {
            h,
            s,
            v,
            ev,
            a: c.a,
        }
    }

    pub fn to_color(self) -> HdrColor {
        let [r, g, b] = hsv_to_rgb(self.h, self.s.clamp(0.0, 1.0), self.v.clamp(0.0, 1.0));
        let k = 2f32.powf(self.ev.max(0.0));
        HdrColor::linear(
            srgb_to_linear(r) * k,
            srgb_to_linear(g) * k,
            srgb_to_linear(b) * k,
            self.a.clamp(0.0, 1.0),
        )
    }

    /// The fully saturated, full-value colour of this hue (the SV square's corner).
    pub fn hue_color(&self) -> Color {
        let [r, g, b] = hsv_to_rgb(self.h, 1.0, 1.0);
        srgb(r, g, b, 1.0)
    }
}

/// A displayable colour from sRGB-encoded components (picker gradients).
pub fn srgb(r: f32, g: f32, b: f32, a: f32) -> Color {
    let mut c = Color::TRANSPARENT;
    c.r = r;
    c.g = g;
    c.b = b;
    c.a = a;
    c
}

/// The colour field of a picker surface at `(u, v)`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ColorField {
    /// Saturation along u, value along v (top = 1), at hue `h`.
    SatVal { h: f32 },
    /// Hue along u.
    Hue,
    /// Alpha along u over the (opaque) colour.
    Alpha(Color),
}

impl ColorField {
    pub fn at(&self, u: f32, v: f32) -> Color {
        match self {
            ColorField::SatVal { h } => {
                let [r, g, b] = hsv_to_rgb(*h, u, 1.0 - v);
                srgb(r, g, b, 1.0)
            }
            ColorField::Hue => {
                let [r, g, b] = hsv_to_rgb(u, 1.0, 1.0);
                srgb(r, g, b, 1.0)
            }
            ColorField::Alpha(c) => c.with_alpha(u),
        }
    }
}

/// Sample sorted `(position, colour)` stops at `u`, interpolating in linear light.
pub fn sample_stops(stops: &[(f32, Color)], u: f32) -> Color {
    match stops {
        [] => Color::TRANSPARENT,
        [only] => only.1,
        _ => {
            if u <= stops[0].0 {
                return stops[0].1;
            }
            for w in stops.windows(2) {
                let (p0, c0) = w[0];
                let (p1, c1) = w[1];
                if u <= p1 {
                    let t = if p1 > p0 { (u - p0) / (p1 - p0) } else { 1.0 };
                    let l = |a: f32, b: f32| {
                        linear_to_srgb(
                            srgb_to_linear(a) + (srgb_to_linear(b) - srgb_to_linear(a)) * t,
                        )
                    };
                    return srgb(
                        l(c0.r, c1.r),
                        l(c0.g, c1.g),
                        l(c0.b, c1.b),
                        c0.a + (c1.a - c0.a) * t,
                    );
                }
            }
            stops[stops.len() - 1].1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsve_round_trips_hdr_and_keeps_hue_through_grey() {
        let c = HdrColor::linear(4.0, 2.0, 0.5, 0.75);
        let h = Hsve::from_color(c, None);
        assert!((h.ev - 2.0).abs() < 1e-5, "{h:?}");
        let back = h.to_color();
        for (a, b) in [(back.r, c.r), (back.g, c.g), (back.b, c.b), (back.a, c.a)] {
            assert!((a - b).abs() < 1e-3, "{back:?} vs {c:?}");
        }
        let grey = Hsve { s: 0.0, ..h }.to_color();
        let again = Hsve::from_color(grey, Some(h));
        assert!((again.h - h.h).abs() < 1e-6, "hue survives a grey");
    }

    #[test]
    fn hex_parse_and_print() {
        let c = HdrColor::parse_hex_input("#ff8000").unwrap_or_default();
        assert_eq!(c.hex(), "#ff8000");
        assert_eq!(
            HdrColor::parse_hex_input("f80").map(|c| c.hex()),
            Some("#ff8800".into())
        );
        assert!(HdrColor::parse_hex_input("#12345").is_none());
        let hdr = HdrColor::linear(3.0, 3.0, 3.0, 1.0);
        assert_eq!(hdr.hex(), "#ffffff", "HDR clips for display");
        assert!(hdr.is_hdr());
    }
}
