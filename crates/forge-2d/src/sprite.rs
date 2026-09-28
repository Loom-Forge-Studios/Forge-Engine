//! Sprites, 2D meshes, the frame a 2D renderer draws, and the **batcher** (Ch.35 §35.2).
//!
//! A frame is a list of sprites (tiles, characters, particles, cutout parts — every quad in
//! a 2D game is a sprite) and meshes (spline shapes), each on a `layer` with an `order`
//! inside it. The batcher sorts by `(layer, order, kind, texture)` — the draw order a user
//! asked for, and within one `(layer, order)` whatever order draws fewest times — and cuts
//! the sorted list into runs sharing a texture: **one instanced draw call per run**. With
//! the atlas packer putting a level's sprites on one or two pages, a screen of ten thousand
//! tiles, characters and particles is a handful of draw calls
//! (`tests/test_2d_render.rs`, perf row `render2d.frame.draw_calls`).

use std::collections::BTreeMap;

use forge_frames::FramePos2;
use forge_num::DVec2;

use crate::atlas::Region;
use crate::camera::Camera2d;
use crate::light::{Light2d, Occluder};
use crate::parallax::ParallaxLayer;

/// A texture the renderer holds (an atlas page, a tile set, a white pixel).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TextureId(pub u32);

/// Normalised texture coordinates of a sprite's image (`u` right, `v` down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvRect {
    pub u0: f64,
    pub v0: f64,
    pub u1: f64,
    pub v1: f64,
}

impl Default for UvRect {
    fn default() -> Self {
        Self::FULL
    }
}

impl UvRect {
    pub const FULL: Self = Self {
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
    };

    /// The rectangle of an atlas region on a `page_w x page_h` page.
    #[must_use]
    pub fn from_region(r: &Region, page_w: u32, page_h: u32) -> Self {
        Self::from_pixels(
            f64::from(r.x),
            f64::from(r.y),
            f64::from(r.w),
            f64::from(r.h),
            page_w,
            page_h,
        )
    }

    /// The rectangle `x, y, w, h` (pixels) of a `page_w x page_h` texture.
    #[must_use]
    pub fn from_pixels(x: f64, y: f64, w: f64, h: f64, page_w: u32, page_h: u32) -> Self {
        let (pw, ph) = (f64::from(page_w.max(1)), f64::from(page_h.max(1)));
        Self {
            u0: x / pw,
            v0: y / ph,
            u1: (x + w) / pw,
            v1: (y + h) / ph,
        }
    }
}

/// A textured quad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    /// Where its pivot is.
    pub pos: FramePos2,
    /// Width and height in world units.
    pub size: DVec2,
    /// The pivot inside the quad, `(0, 0)` bottom-left to `(1, 1)` top-right.
    pub pivot: DVec2,
    /// Radians, counter-clockwise about the pivot.
    pub angle: f64,
    pub uv: UvRect,
    pub texture: TextureId,
    /// Linear RGBA multiplier.
    pub tint: [f64; 4],
    pub layer: i32,
    pub order: i32,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl Sprite {
    /// A sprite of `size` centred on `pos`, the whole of `texture`, untinted.
    #[must_use]
    pub fn new(pos: FramePos2, size: DVec2, texture: TextureId) -> Self {
        Self {
            pos,
            size,
            pivot: DVec2::new(0.5, 0.5),
            angle: 0.0,
            uv: UvRect::FULL,
            texture,
            tint: [1.0; 4],
            layer: 0,
            order: 0,
            flip_x: false,
            flip_y: false,
        }
    }

    /// The same sprite on `layer` / `order`.
    #[must_use]
    pub fn at_layer(self, layer: i32, order: i32) -> Self {
        Self {
            layer,
            order,
            ..self
        }
    }

    /// World-space distance from the pivot to the farthest corner (for culling).
    #[must_use]
    pub fn reach(&self) -> f64 {
        let px = self.pivot.x.max(1.0 - self.pivot.x) * self.size.x.abs();
        let py = self.pivot.y.max(1.0 - self.pivot.y) * self.size.y.abs();
        DVec2::new(px, py).length()
    }
}

/// A vertex of a 2D mesh: an offset from the mesh origin (world units), a texture
/// coordinate and a linear RGBA colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex2d {
    pub offset: DVec2,
    pub uv: DVec2,
    pub color: [f64; 4],
}

/// Triangles (spline shape fills and edges).
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh2d {
    pub origin: FramePos2,
    pub verts: Vec<Vertex2d>,
    pub indices: Vec<u32>,
    pub texture: TextureId,
    pub layer: i32,
    pub order: i32,
}

/// Everything one 2D frame draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame2d {
    pub camera: Camera2d,
    pub sprites: Vec<Sprite>,
    pub meshes: Vec<Mesh2d>,
    pub lights: Vec<Light2d>,
    /// Shadow casters.
    pub occluders: Vec<Occluder>,
    /// Linear ambient light (1, 1, 1 with no lights: unlit colours).
    pub ambient: [f64; 3],
    /// Parallax per layer (a layer with none moves with the camera).
    pub parallax: BTreeMap<i32, ParallaxLayer>,
    /// Background colour (linear RGBA), also the letterbox bars' colour.
    pub clear: [f64; 4],
}

impl Frame2d {
    /// An empty frame seen by `camera`, fully lit by ambient light.
    #[must_use]
    pub fn new(camera: Camera2d) -> Self {
        Self {
            camera,
            sprites: Vec::new(),
            meshes: Vec::new(),
            lights: Vec::new(),
            occluders: Vec::new(),
            ambient: [1.0; 3],
            parallax: BTreeMap::new(),
            clear: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// What a draw is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DrawKind {
    /// Spline shapes and other meshes (drawn before sprites of the same layer and order).
    Mesh = 0,
    Sprite = 1,
}

/// A thing to sort: its layer, order, kind, texture and index in its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DrawKey {
    pub layer: i32,
    pub order: i32,
    pub kind: DrawKind,
    pub texture: TextureId,
    pub index: u32,
}

/// One draw call: `count` consecutive entries of the sorted list, all of `kind` on `texture`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    pub kind: DrawKind,
    pub texture: TextureId,
    /// Into the sorted key list.
    pub first: u32,
    pub count: u32,
}

/// Sort `keys` in place into draw order and cut them into batches. With `split_all`
/// (a W2 control) every entry is its own batch.
pub fn batch(keys: &mut [DrawKey], split_all: bool) -> Vec<Batch> {
    let mut out = Vec::new();
    batch_into(keys, split_all, &mut out);
    out
}

/// [`batch`] into a reused list (the renderer's last mile: no allocation once warm).
pub fn batch_into(keys: &mut [DrawKey], split_all: bool, out: &mut Vec<Batch>) {
    keys.sort_unstable();
    out.clear();
    for (i, k) in keys.iter().enumerate() {
        if !split_all
            && let Some(last) = out.last_mut()
            && last.kind == k.kind
            && last.texture == k.texture
        {
            last.count += 1;
            continue;
        }
        out.push(Batch {
            kind: k.kind,
            texture: k.texture,
            first: i as u32,
            count: 1,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(layer: i32, order: i32, kind: DrawKind, tex: u32, index: u32) -> DrawKey {
        DrawKey {
            layer,
            order,
            kind,
            texture: TextureId(tex),
            index,
        }
    }

    #[test]
    fn layers_on_one_page_are_one_draw_and_keep_their_order() {
        let mut keys: Vec<DrawKey> = (0..10_000)
            .map(|i| key((i % 4) as i32, 0, DrawKind::Sprite, 0, i))
            .collect();
        let b = batch(&mut keys, false);
        assert_eq!(
            b.len(),
            1,
            "four layers on one page: instances keep layer order inside one draw"
        );
        assert!(keys.windows(2).all(|w| w[0].layer <= w[1].layer));
        assert_eq!(batch(&mut keys, true).len(), 10_000);
    }

    #[test]
    fn textures_group_within_an_order_but_never_across_one() {
        let mut keys = vec![
            key(0, 0, DrawKind::Sprite, 1, 0),
            key(0, 0, DrawKind::Sprite, 2, 1),
            key(0, 0, DrawKind::Sprite, 1, 2),
            key(0, 1, DrawKind::Sprite, 1, 3),
            key(0, 0, DrawKind::Mesh, 1, 0),
        ];
        let b = batch(&mut keys, false);
        let shape: Vec<(DrawKind, u32, u32)> =
            b.iter().map(|x| (x.kind, x.texture.0, x.count)).collect();
        assert_eq!(
            shape,
            vec![
                (DrawKind::Mesh, 1, 1),
                (DrawKind::Sprite, 1, 2),
                (DrawKind::Sprite, 2, 1),
                (DrawKind::Sprite, 1, 1)
            ]
        );
    }
}
