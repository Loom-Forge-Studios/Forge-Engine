//! Textures from pixels: an RGBA8 image -> a `TextureAsset` (the KTX2 RGBA8 artefact) with a
//! full mip chain. Every importer that produces textures uses this — the first-party image
//! and glTF importers (the `forge.importers` plugin) and any third-party one (I16) — so a
//! texture has one content address however it arrived.
//!
//! Mips are box-filtered **in linear light** for sRGB textures (averaging sRGB bytes darkens
//! every downsampled edge). The sRGB transfer tables are built with `forge_num::det::pow`,
//! and the filter uses only IEEE-exact operations in a fixed order, so a texture artefact has
//! the same bytes — the same content address — on every platform.

use std::sync::OnceLock;

use bytes::Bytes;

use crate::types::TextureAsset;

struct Srgb {
    to_linear: [f64; 256],
    /// `thresholds[i]`: the linear value halfway (in sRGB space) between code i and i+1.
    thresholds: [f64; 255],
}

fn decode_srgb(s: f64) -> f64 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        forge_num::det::pow((s + 0.055) / 1.055, 2.4)
    }
}

fn srgb() -> &'static Srgb {
    static T: OnceLock<Srgb> = OnceLock::new();
    T.get_or_init(|| {
        let mut to_linear = [0.0; 256];
        for (i, v) in to_linear.iter_mut().enumerate() {
            *v = decode_srgb(i as f64 / 255.0);
        }
        let mut thresholds = [0.0; 255];
        for (i, v) in thresholds.iter_mut().enumerate() {
            *v = decode_srgb((i as f64 + 0.5) / 255.0);
        }
        Srgb {
            to_linear,
            thresholds,
        }
    })
}

fn encode_srgb(linear: f64) -> u8 {
    // The number of thresholds below `linear` is the nearest sRGB code.
    srgb().thresholds.partition_point(|t| *t <= linear) as u8
}

/// The next mip level of an RGBA8 image (box filter; odd edges clamp).
fn downsample(src: &[u8], w: usize, h: usize, srgb_rgb: bool) -> (Vec<u8>, usize, usize) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let tab = srgb();
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            let xs = [(2 * x).min(w - 1), (2 * x + 1).min(w - 1)];
            let ys = [(2 * y).min(h - 1), (2 * y + 1).min(h - 1)];
            for c in 0..4 {
                let px = |xx: usize, yy: usize| src[(yy * w + xx) * 4 + c];
                let o = &mut out[(y * nw + x) * 4 + c];
                if srgb_rgb && c < 3 {
                    let sum = tab.to_linear[usize::from(px(xs[0], ys[0]))]
                        + tab.to_linear[usize::from(px(xs[1], ys[0]))]
                        + tab.to_linear[usize::from(px(xs[0], ys[1]))]
                        + tab.to_linear[usize::from(px(xs[1], ys[1]))];
                    *o = encode_srgb(sum * 0.25);
                } else {
                    let sum = u32::from(px(xs[0], ys[0]))
                        + u32::from(px(xs[1], ys[0]))
                        + u32::from(px(xs[0], ys[1]))
                        + u32::from(px(xs[1], ys[1]));
                    *o = ((sum + 2) / 4) as u8;
                }
            }
        }
    }
    (out, nw, nh)
}

/// Build a texture (with mips if asked) from RGBA8 pixels.
#[must_use]
pub fn texture_from_rgba8(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    srgb: bool,
    mips: bool,
) -> TextureAsset {
    let (mut w, mut h) = (width as usize, height as usize);
    let mut levels = vec![Bytes::from(rgba)];
    while mips && (w > 1 || h > 1) {
        let Some(last) = levels.last() else { break };
        let (next, nw, nh) = downsample(last, w, h, srgb);
        levels.push(Bytes::from(next));
        (w, h) = (nw, nh);
    }
    TextureAsset {
        width,
        height,
        srgb,
        levels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_encode_inverts_decode_for_every_code() {
        let t = srgb();
        for i in 0..=255u8 {
            assert_eq!(encode_srgb(t.to_linear[usize::from(i)]), i);
        }
        assert_eq!(encode_srgb(-1.0), 0);
        assert_eq!(encode_srgb(2.0), 255);
    }

    #[test]
    fn mips_average_in_linear_light_and_reach_1x1() {
        // A black/white checker: the linear mean is 0.5, which is sRGB code 188, not 128.
        let rgba: Vec<u8> = vec![
            0, 0, 0, 255, 255, 255, 255, 255, //
            255, 255, 255, 255, 0, 0, 0, 255,
        ];
        let t = texture_from_rgba8(rgba.clone(), 2, 2, true, true);
        assert_eq!(t.levels.len(), 2);
        assert_eq!(&t.levels[1][..], &[188, 188, 188, 255]);
        let lin = texture_from_rgba8(rgba, 2, 2, false, true);
        assert_eq!(&lin.levels[1][..], &[128, 128, 128, 255]);
        let odd = texture_from_rgba8(vec![10; 5 * 3 * 4], 5, 3, true, true);
        assert_eq!(odd.levels.len(), 3); // 5x3, 2x1, 1x1
        assert_eq!(odd.levels[2].len(), 4);
    }
}
