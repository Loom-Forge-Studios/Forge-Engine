//! The thumbnail service: small square RGBA8 pictures of assets for the asset browser
//! (WP-U5) and the asset reference picker.
//!
//! Rendered on the loader's workers from the artefacts alone — no GPU, no window — so the
//! browser can show a thousand thumbnails in a headless session or before `forge-render`
//! exists, and the pixels are the same on every machine (only IEEE-exact `f64` operations
//! and `sqrt`):
//!
//! * `texture`: the mip level closest above the size, box-filtered to fit, aspect kept;
//! * `mesh` / `scene`: a flat-shaded isometric software render of every triangle, framed to
//!   the bounds (a scene draws its meshes at their world transforms);
//! * `material`: a lit sphere in the base colour (times the texture's average colour);
//! * anything else: a neutral tile, so every kind has a picture.
//!
//! Thumbnails are values in memory (handles, refreshed on hot reload), never project data.

use bytes::Bytes;

use crate::AssetId;
use crate::types::{
    AssetValue, KIND_MATERIAL, KIND_MESH, KIND_SCENE, KIND_TEXTURE, MaterialAsset, MeshAsset,
    SceneAsset, TextureAsset, Topology, transform,
};

/// A square RGBA8 image (straight alpha, sRGB).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    /// Width and height in pixels.
    pub size: u32,
    /// `size * size * 4` bytes, row-major, top row first.
    pub rgba: Bytes,
}

type Fetch<'a> = &'a dyn Fn(AssetId) -> Option<Bytes>;

/// Render a thumbnail of an artefact.
pub fn render_thumbnail(
    kind: &str,
    bytes: &Bytes,
    fetch: Fetch<'_>,
    size: u32,
) -> Result<Thumbnail, String> {
    let n = size as usize;
    let rgba = match kind {
        KIND_TEXTURE => texture(&TextureAsset::decode(bytes)?, n),
        KIND_MESH => {
            let m = MeshAsset::decode(bytes)?;
            raster(&[(m, IDENTITY)], n)
        }
        KIND_SCENE => {
            let s = SceneAsset::decode(bytes)?;
            let world = s.world_matrices();
            let mut meshes = Vec::new();
            for (node, m) in s.nodes.iter().zip(world) {
                if let Some(id) = node.mesh
                    && let Some(b) = fetch(id)
                {
                    meshes.push((MeshAsset::decode(&b)?, m));
                }
            }
            raster(&meshes, n)
        }
        KIND_MATERIAL => material(&MaterialAsset::decode(bytes)?, fetch, n),
        _ => vec![96; n * n * 4],
    };
    Ok(Thumbnail {
        size,
        rgba: Bytes::from(rgba),
    })
}

const IDENTITY: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

fn texture(t: &TextureAsset, n: usize) -> Vec<u8> {
    // The smallest level still at least as large as the thumbnail.
    let mut lv = 0;
    while lv + 1 < t.levels.len() && (t.width >> (lv + 1)).max(t.height >> (lv + 1)) as usize >= n {
        lv += 1;
    }
    let (w, h) = (
        (t.width >> lv).max(1) as usize,
        (t.height >> lv).max(1) as usize,
    );
    let src = &t.levels[lv];
    let scale = w.max(h) as f64 / n as f64;
    let (dw, dh) = (
        ((w as f64 / scale).round() as usize).clamp(1, n),
        ((h as f64 / scale).round() as usize).clamp(1, n),
    );
    let (ox, oy) = ((n - dw) / 2, (n - dh) / 2);
    let mut out = vec![0u8; n * n * 4];
    for y in 0..dh {
        let (y0, y1) = (y * h / dh, ((y + 1) * h / dh).max(y * h / dh + 1).min(h));
        for x in 0..dw {
            let (x0, x1) = (x * w / dw, ((x + 1) * w / dw).max(x * w / dw + 1).min(w));
            let mut acc = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    for c in 0..4 {
                        acc[c] += u32::from(src[(sy * w + sx) * 4 + c]);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            let o = ((oy + y) * n + ox + x) * 4;
            for c in 0..4 {
                out[o + c] = ((acc[c] + count / 2) / count) as u8;
            }
        }
    }
    out
}

/// Isometric view: yaw 45 degrees, pitch atan(1/sqrt 2). Returns (screen x, screen y, depth).
fn iso(p: [f64; 3]) -> [f64; 3] {
    let s2 = 0.5f64.sqrt();
    let (x, z) = (s2 * (p[0] - p[2]), s2 * (p[0] + p[2]));
    let (sp, cp) = ((1.0f64 / 3.0).sqrt(), (2.0f64 / 3.0).sqrt());
    [x, cp * p[1] - sp * z, sp * p[1] + cp * z]
}

fn raster(meshes: &[(MeshAsset, [f64; 16])], n: usize) -> Vec<u8> {
    let mut tris: Vec<[[f64; 3]; 3]> = Vec::new();
    for (m, world) in meshes {
        for p in &m.primitives {
            let pos: Vec<[f64; 3]> = p
                .positions()
                .into_iter()
                .map(|v| iso(transform(world, v)))
                .collect();
            let idx = p.index_list();
            let get = |i: u32| pos.get(i as usize).copied();
            match p.topology {
                Topology::Triangles => {
                    for t in idx.as_chunks::<3>().0 {
                        if let (Some(a), Some(b), Some(c)) = (get(t[0]), get(t[1]), get(t[2])) {
                            tris.push([a, b, c]);
                        }
                    }
                }
                Topology::TriangleStrip => {
                    for k in 0..idx.len().saturating_sub(2) {
                        let (i0, i1) = if k % 2 == 0 {
                            (idx[k], idx[k + 1])
                        } else {
                            (idx[k + 1], idx[k])
                        };
                        if let (Some(a), Some(b), Some(c)) = (get(i0), get(i1), get(idx[k + 2])) {
                            tris.push([a, b, c]);
                        }
                    }
                }
                Topology::Points | Topology::Lines | Topology::LineStrip => {}
            }
        }
    }
    let mut out = vec![0u8; n * n * 4];
    if tris.is_empty() {
        return out;
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for t in &tris {
        for v in t {
            for k in 0..2 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
    }
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-12);
    let margin = n as f64 * 0.08;
    let scale = (n as f64 - 2.0 * margin) / extent;
    let cx = (lo[0] + hi[0]) * 0.5;
    let cy = (lo[1] + hi[1]) * 0.5;
    let half = n as f64 * 0.5;
    let to_px = |v: [f64; 3]| [half + (v[0] - cx) * scale, half - (v[1] - cy) * scale, v[2]];
    let mut depth = vec![f64::INFINITY; n * n];
    // Light from the upper left, towards the viewer (screen space).
    let light = {
        let l = [-0.4f64, 0.7, -0.6];
        let len = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / len, l[1] / len, l[2] / len]
    };
    for t in &tris {
        let [a, b, c] = t.map(to_px);
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area.abs() < 1e-12 {
            continue;
        }
        // Face normal in view space (y up) for flat shading, either winding.
        let (u, v) = (
            [t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]],
            [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]],
        );
        let nrm = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
        let lambert = if len > 0.0 {
            ((nrm[0] * light[0] + nrm[1] * light[1] + nrm[2] * light[2]) / len).abs()
        } else {
            0.0
        };
        let shade = 0.25 + 0.75 * lambert;
        let rgb = [
            (70.0 + 150.0 * shade) as u8,
            (80.0 + 150.0 * shade) as u8,
            (95.0 + 155.0 * shade).min(255.0) as u8,
        ];
        let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let x1 = (a[0].max(b[0]).max(c[0]).ceil() as usize).min(n);
        let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
        let y1 = (a[1].max(b[1]).max(c[1]).ceil() as usize).min(n);
        for py in y0..y1 {
            for px in x0..x1 {
                let (fx, fy) = (px as f64 + 0.5, py as f64 + 0.5);
                let w0 = ((b[0] - fx) * (c[1] - fy) - (b[1] - fy) * (c[0] - fx)) / area;
                let w1 = ((c[0] - fx) * (a[1] - fy) - (c[1] - fy) * (a[0] - fx)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                let i = py * n + px;
                if z < depth[i] {
                    depth[i] = z;
                    out[i * 4..i * 4 + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                }
            }
        }
    }
    out
}

fn material(m: &MaterialAsset, fetch: Fetch<'_>, n: usize) -> Vec<u8> {
    // The texture's 1x1 mip is its average colour.
    let mut base = [m.base_color[0], m.base_color[1], m.base_color[2]];
    if let Some(id) = m.base_color_texture
        && let Some(b) = fetch(id)
        && let Ok(t) = TextureAsset::decode(&b)
        && let Some(last) = t.levels.last()
        && last.len() >= 4
    {
        for (c, v) in base.iter_mut().enumerate() {
            let s = f64::from(last[c]) / 255.0;
            // sRGB -> linear, the cheap exact form (square) is plenty for a swatch.
            *v *= if t.srgb { s * s } else { s };
        }
    }
    let mut out = vec![0u8; n * n * 4];
    let r = n as f64 * 0.42;
    let c = n as f64 * 0.5;
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = ((x as f64 + 0.5 - c) / r, (y as f64 + 0.5 - c) / r);
            let d2 = dx * dx + dy * dy;
            if d2 > 1.0 {
                continue;
            }
            let nz = (1.0 - d2).sqrt();
            let lambert = (-0.4 * dx - 0.5 * dy + 0.77 * nz).max(0.0);
            let spec_base = (-0.4 * dx - 0.5 * dy + 0.77 * nz).max(0.0);
            let spec = spec_base * spec_base * spec_base * spec_base * (1.0 - m.roughness) * 0.6;
            let i = (y * n + x) * 4;
            for k in 0..3 {
                let lin = (base[k] * (0.15 + 0.85 * lambert) + spec).clamp(0.0, 1.0);
                // linear -> display, the matching cheap inverse (sqrt).
                out[i + k] = (lin.sqrt() * 255.0 + 0.5) as u8;
            }
            out[i + 3] = 255;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::texture_from_rgba8;

    #[test]
    fn texture_thumbnails_keep_aspect_and_colour() {
        let t = texture_from_rgba8(vec![200; 64 * 32 * 4], 64, 32, false, true);
        let th = render_thumbnail(KIND_TEXTURE, &t.encode().expect("encodes"), &|_| None, 16)
            .expect("renders");
        assert_eq!(th.rgba.len(), 16 * 16 * 4);
        // 16x8 picture centred: rows 0..4 transparent, rows 4..12 filled.
        assert_eq!(&th.rgba[0..4], &[0, 0, 0, 0]);
        let mid = (8 * 16 + 8) * 4;
        assert_eq!(&th.rgba[mid..mid + 4], &[200, 200, 200, 200]);
    }

    #[test]
    fn unknown_kinds_get_a_tile() {
        let th = render_thumbnail("audio", &Bytes::new(), &|_| None, 8).expect("tile");
        assert!(th.rgba.iter().all(|b| *b == 96));
    }
}
