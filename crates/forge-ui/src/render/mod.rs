//! The display list, the batcher, the `UiRenderer` trait and the recording renderer
//! (Ch.21 §21.13). **No `wgpu` here** — `test_ui_wgpu_confined` enforces it.
//!
//! Batching belongs to this module, not to a backend: [`batch::build`] is
//! renderer-independent, so the draw-call budget is tested headless. All sampled textures
//! share one bind group (the glyph atlas's two planes and the image atlas), and quads,
//! shadows, glyphs and images are instanced through one uber-pipeline, so a batch breaks
//! only on a **clip change** or a **viewport composite**, never on a texture change.

use std::collections::BTreeMap;
use std::ops::Range;

use etagere::{BucketedAtlasAllocator, size2};

use crate::UiError;
use crate::geom::{Color, Corners, PhysicalSize, Point, PxRect, Rect};
use crate::id::Fnv;
use crate::style::Shadow;
use crate::text::GlyphRunId;

pub mod cache;
pub mod mesh;

pub use mesh::{GpuMeshVertex, MeshData, MeshId, MeshStore, MeshVertex, PathBuilder};

/// A quad border.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Border {
    pub width: f32,
    pub color: Color,
}

/// An image in the image atlas.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(pub u32);

/// A texture rendered elsewhere (a `forge-render` viewport) composited as-is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExternalTexture(pub u32);

/// One drawing primitive, in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Quad {
        rect: Rect,
        radii: Corners,
        fill: Color,
        border: Option<Border>,
        shadow: Option<Shadow>,
    },
    /// An atlas-resident shaped run from the shaping cache.
    Glyphs {
        run: GlyphRunId,
        origin: Point,
        color: Color,
    },
    /// A shaped rich-text run whose glyphs take `colors[span tag]` (rich text, links).
    RichGlyphs {
        run: GlyphRunId,
        origin: Point,
        colors: std::sync::Arc<[Color]>,
    },
    /// An icon of the Forge set, drawn from the glyph atlas's mask plane in `color`
    /// (centred in `rect`, in the square its shorter side makes).
    Icon {
        icon: crate::icons::Icon,
        rect: Rect,
        color: Color,
    },
    /// Icons and thumbnails from the image atlas.
    Image {
        image: ImageId,
        rect: Rect,
        tint: Color,
    },
    /// `forge-render` output (a new viewport frame damages only this rect).
    Viewport {
        tex: ExternalTexture,
        rect: Rect,
    },
    /// `lyon`-tessellated geometry from the [`MeshStore`], drawn by the second pipeline.
    Mesh {
        mesh: MeshId,
    },
    PushClip {
        rect: Rect,
        radii: Corners,
    },
    PopClip,
}

impl Primitive {
    /// The logical area this primitive can touch (shadow outsets included, §21.11).
    pub fn bounds(
        &self,
        run_size: impl Fn(GlyphRunId) -> Option<crate::geom::Size>,
        mesh_bounds: impl Fn(MeshId) -> Option<Rect>,
    ) -> Rect {
        match self {
            Primitive::Quad { rect, shadow, .. } => {
                let mut r = *rect;
                if let Some(s) = shadow {
                    r = r.union(
                        &rect
                            .translate(s.offset_x, s.offset_y)
                            .outset(s.blur * 2.0 + s.spread),
                    );
                }
                r
            }
            Primitive::Glyphs { run, origin, .. } | Primitive::RichGlyphs { run, origin, .. } => {
                let s = run_size(*run).unwrap_or_default();
                // Glyph ink can overhang the advance box slightly (italics, accents).
                Rect::new(origin.x, origin.y, s.w, s.h).outset(2.0)
            }
            Primitive::Image { rect, .. } | Primitive::Viewport { rect, .. } => *rect,
            // The mask is the rect's square, snapped to whole pixels.
            Primitive::Icon { rect, .. } => rect.outset(1.0),
            Primitive::Mesh { mesh } => mesh_bounds(*mesh).unwrap_or_default(),
            Primitive::PushClip { .. } | Primitive::PopClip => Rect::ZERO,
        }
    }

    /// Feed an exact, platform-independent hash of this primitive (display-list goldens).
    pub(crate) fn hash_into(&self, h: &mut Fnv) {
        let rect = |h: &mut Fnv, r: &Rect| {
            h.f32(r.x);
            h.f32(r.y);
            h.f32(r.w);
            h.f32(r.h);
        };
        let color = |h: &mut Fnv, c: &Color| {
            h.f32(c.r);
            h.f32(c.g);
            h.f32(c.b);
            h.f32(c.a);
        };
        let corners = |h: &mut Fnv, c: &Corners| {
            h.f32(c.tl);
            h.f32(c.tr);
            h.f32(c.br);
            h.f32(c.bl);
        };
        match self {
            Primitive::Quad {
                rect: r,
                radii,
                fill,
                border,
                shadow,
            } => {
                h.byte(1);
                rect(h, r);
                corners(h, radii);
                color(h, fill);
                if let Some(b) = border {
                    h.byte(1);
                    h.f32(b.width);
                    color(h, &b.color);
                } else {
                    h.byte(0);
                }
                if let Some(s) = shadow {
                    h.byte(1);
                    h.f32(s.offset_x);
                    h.f32(s.offset_y);
                    h.f32(s.blur);
                    h.f32(s.spread);
                    color(h, &s.color);
                } else {
                    h.byte(0);
                }
            }
            Primitive::Glyphs {
                run,
                origin,
                color: c,
            } => {
                h.byte(2);
                h.u64(run.0);
                h.f32(origin.x);
                h.f32(origin.y);
                color(h, c);
            }
            Primitive::Image {
                image,
                rect: r,
                tint,
            } => {
                h.byte(3);
                h.u32(image.0);
                rect(h, r);
                color(h, tint);
            }
            Primitive::Viewport { tex, rect: r } => {
                h.byte(4);
                h.u32(tex.0);
                rect(h, r);
            }
            Primitive::PushClip { rect: r, radii } => {
                h.byte(5);
                rect(h, r);
                corners(h, radii);
            }
            Primitive::PopClip => h.byte(6),
            Primitive::Mesh { mesh } => {
                h.byte(7);
                h.u64(mesh.0);
            }
            Primitive::Icon {
                icon,
                rect: r,
                color: c,
            } => {
                h.byte(9);
                h.u32(*icon as u32);
                rect(h, r);
                color(h, c);
            }
            Primitive::RichGlyphs {
                run,
                origin,
                colors,
            } => {
                h.byte(8);
                h.u64(run.0);
                h.f32(origin.x);
                h.f32(origin.y);
                for c in colors.iter() {
                    color(h, c);
                }
            }
        }
    }
}

// ---- textures -------------------------------------------------------------------------

/// The sampled textures of the one bind group.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TextureId {
    /// The glyph atlas's `R8Unorm` coverage plane.
    GlyphMask,
    /// The glyph atlas's `Rgba8UnormSrgb` colour plane.
    GlyphColor,
    /// The image slot atlas (`Rgba8UnormSrgb`): thumbnails, bitmaps, icons for now.
    Images,
}

/// A change to one of the bind group's textures.
#[derive(Clone, Debug, PartialEq)]
pub enum TextureUpdateKind {
    /// Create at this size (first use).
    Create { w: u32, h: u32 },
    /// Reallocate at a larger size, copying the old contents to the origin.
    Grow { w: u32, h: u32 },
    /// Write tightly packed texels (1 byte per texel for `GlyphMask`, 4 otherwise).
    Write {
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        data: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextureUpdate {
    pub tex: TextureId,
    pub kind: TextureUpdateKind,
}

/// The image slot atlas: RGBA images packed into one texture of the bind group.
pub struct ImageAtlas {
    alloc: BucketedAtlasAllocator,
    size: u32,
    images: BTreeMap<ImageId, (u32, u32, u32, u32)>,
    allocs: BTreeMap<ImageId, etagere::AllocId>,
    next: u32,
    pending: Vec<TextureUpdate>,
}

impl Default for ImageAtlas {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl ImageAtlas {
    pub fn new(size: u32) -> Self {
        let s = i32::try_from(size).unwrap_or(1024);
        Self {
            alloc: BucketedAtlasAllocator::new(size2(s, s)),
            size,
            images: BTreeMap::new(),
            allocs: BTreeMap::new(),
            next: 0,
            pending: vec![TextureUpdate {
                tex: TextureId::Images,
                kind: TextureUpdateKind::Create { w: size, h: size },
            }],
        }
    }

    /// Add an RGBA8 (sRGB, straight alpha) image.
    pub fn add(&mut self, w: u32, h: u32, rgba: Vec<u8>) -> Result<ImageId, UiError> {
        if rgba.len() as u64 != u64::from(w) * u64::from(h) * 4 {
            return Err(UiError::Image(format!(
                "{w}x{h} image needs {} bytes, got {}",
                u64::from(w) * u64::from(h) * 4,
                rgba.len()
            )));
        }
        let a = self
            .alloc
            .allocate(size2(
                i32::try_from(w + 1).unwrap_or(i32::MAX),
                i32::try_from(h + 1).unwrap_or(i32::MAX),
            ))
            .ok_or_else(|| UiError::Image(format!("image atlas full ({w}x{h})")))?;
        let (x, y) = (
            a.rectangle.min.x.max(0) as u32,
            a.rectangle.min.y.max(0) as u32,
        );
        self.next += 1;
        let id = ImageId(self.next);
        self.images.insert(id, (x, y, w, h));
        self.allocs.insert(id, a.id);
        self.pending.push(TextureUpdate {
            tex: TextureId::Images,
            kind: TextureUpdateKind::Write {
                x,
                y,
                w,
                h,
                data: rgba,
            },
        });
        Ok(id)
    }

    /// Free an image's slot (an evicted thumbnail). Its id is never reused; drawing it
    /// afterwards draws nothing. Returns whether it existed.
    pub fn remove(&mut self, id: ImageId) -> bool {
        self.images.remove(&id);
        match self.allocs.remove(&id) {
            Some(a) => {
                self.alloc.deallocate(a);
                true
            }
            None => false,
        }
    }
    /// Images currently resident.
    pub fn len(&self) -> usize {
        self.images.len()
    }
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }
    pub fn uv(&self, id: ImageId) -> Option<(u32, u32, u32, u32)> {
        self.images.get(&id).copied()
    }
    pub fn size(&self) -> u32 {
        self.size
    }
    pub fn take_updates(&mut self) -> Vec<TextureUpdate> {
        std::mem::take(&mut self.pending)
    }
}

// ---- batches --------------------------------------------------------------------------

/// Instance kinds of the uber-pipeline.
pub mod kind {
    pub const QUAD: f32 = 0.0;
    pub const SHADOW: f32 = 1.0;
    pub const GLYPH_MASK: f32 = 2.0;
    pub const GLYPH_COLOR: f32 = 3.0;
    pub const IMAGE: f32 = 4.0;
}

/// One instance of the uber-pipeline, in physical pixels. 96 bytes, all `f32`.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Instance {
    /// x, y, w, h of the drawn quad (physical px, snapped).
    pub rect: [f32; 4],
    /// Texel rect for glyphs/images; the shape rect for shadows.
    pub uv: [f32; 4],
    /// Linear, premultiplied fill / text / tint colour.
    pub color: [f32; 4],
    /// Linear, premultiplied border colour.
    pub border_color: [f32; 4],
    /// Corner radii (tl, tr, br, bl), physical px.
    pub radii: [f32; 4],
    /// x: border width, y: shadow blur, z: kind (see [`kind`]), w: reserved.
    pub params: [f32; 4],
}

impl Instance {
    pub const SIZE: usize = 96;
    /// Little-endian byte image (the GPU vertex buffer layout).
    pub fn write_bytes(&self, out: &mut Vec<u8>) {
        for a in [
            self.rect,
            self.uv,
            self.color,
            self.border_color,
            self.radii,
            self.params,
        ] {
            for v in a {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
}

/// What one batch draws.
#[derive(Clone, Debug, PartialEq)]
pub enum BatchKind {
    /// `count` instances starting at `start` through the uber-pipeline: one draw call.
    Instances { start: u32, count: u32 },
    /// Composite an external texture into `rect`: one draw call.
    Viewport { tex: ExternalTexture, rect: PxRect },
    /// `index_count` indices from `first_index` of the mesh buffers through the second
    /// (mesh) pipeline: one draw call.
    Mesh { first_index: u32, index_count: u32 },
}

/// One draw call's worth of work.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub kind: BatchKind,
    /// Scissor from the clip stack (physical px); `None` = the whole target.
    pub clip: Option<PxRect>,
    /// Texture updates to apply **before** this batch draws (atlas writes, growth, and
    /// the recycling of a plane between atlas epochs).
    pub uploads: Vec<TextureUpdate>,
}

/// The batcher's output for one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatchList {
    pub batches: Vec<Batch>,
    /// The instance buffer image. With the retained batch cache it keeps slack per
    /// segment (zeroed, degenerate instances that draw nothing) so a slice can grow in
    /// place.
    pub instances: Vec<Instance>,
    /// Mesh vertices (physical px) and triangle indices for `BatchKind::Mesh`.
    pub mesh_vertices: Vec<GpuMeshVertex>,
    pub mesh_indices: Vec<u32>,
    /// Atlas epochs this frame needed beyond the first (each costs one extra draw call).
    pub extra_epochs: u32,
    /// Texture updates to apply before anything draws this frame.
    pub pre_uploads: Vec<TextureUpdate>,
    /// Layout generation of `instances`. A renderer that last uploaded another generation
    /// (or none) uploads everything; otherwise only `instance_uploads`.
    pub generation: u64,
    /// Instance ranges changed since the previous frame of the same generation.
    pub instance_uploads: Vec<Range<u32>>,
    /// The mesh buffers changed since the previous frame of the same generation.
    pub meshes_changed: bool,
    /// Mesh vertex ranges changed since the previous frame of the same generation (the
    /// dirty meshes only; a renderer current on this generation uploads just these).
    pub mesh_vertex_uploads: Vec<Range<u32>>,
    /// Mesh index ranges changed since the previous frame of the same generation.
    pub mesh_index_uploads: Vec<Range<u32>>,
}

impl BatchList {
    /// Draw calls needed to draw the list once (one damage rect).
    pub fn draw_calls(&self) -> usize {
        self.batches
            .iter()
            .filter(|b| match &b.kind {
                BatchKind::Instances { count, .. } => *count > 0,
                BatchKind::Viewport { .. } => true,
                BatchKind::Mesh { index_count, .. } => *index_count > 0,
            })
            .count()
    }

    /// Mesh vertices a renderer that last uploaded meshes of `uploaded` generation must
    /// upload: everything after a reassembly, otherwise only the dirty meshes.
    pub fn mesh_vertices_to_upload(&self, uploaded: Option<u64>) -> u64 {
        if uploaded == Some(self.generation) && !self.meshes_changed {
            self.mesh_vertex_uploads
                .iter()
                .map(|r| u64::from(r.end - r.start))
                .sum()
        } else {
            self.mesh_vertices.len() as u64
        }
    }

    /// Instances a renderer that is current on `uploaded` generation must upload.
    pub fn instances_to_upload(&self, uploaded: Option<u64>) -> u64 {
        if uploaded == Some(self.generation) {
            self.instance_uploads
                .iter()
                .map(|r| u64::from(r.end - r.start))
                .sum()
        } else {
            self.instances.len() as u64
        }
    }
}

/// Counters the budget tests read (one rendered frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    pub draw_calls: u32,
    pub batches: u32,
    pub instances: u32,
    pub damage_rects: u32,
    pub damage_px: u64,
    pub texture_uploads: u32,
    pub texture_bytes: u64,
    pub extra_epochs: u32,
    /// Instances written to the GPU this frame (all of them only on a layout change).
    pub instances_uploaded: u32,
    /// Mesh vertices written to the GPU this frame (all of them only after a reassembly).
    pub mesh_vertices_uploaded: u32,
    /// Mesh draw calls (second pipeline).
    pub mesh_draws: u32,
}

/// Identifies a render target (a window's persistent UI target, or an offscreen one).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetId(pub u32);

/// A UI backend. `render` draws only `damage`; the rest of the persistent target keeps
/// its contents (§21.11).
pub trait UiRenderer {
    /// Apply a texture update outside a frame (initial creation, image uploads).
    fn upload(&mut self, update: &TextureUpdate) -> Result<(), UiError>;
    fn resize(&mut self, target: TargetId, size: PhysicalSize) -> Result<(), UiError>;
    /// Renders only `damage`; returns counters the budget tests read.
    fn render(
        &mut self,
        target: TargetId,
        batches: &BatchList,
        damage: &[PxRect],
    ) -> Result<FrameStats, UiError>;
}

/// Headless renderer: records batches and stats. The budget, damage and display-list
/// golden tests use it on every CI leg with no GPU.
#[derive(Default)]
pub struct RecordingRenderer {
    pub frames: Vec<FrameStats>,
    pub last: Option<BatchList>,
    pub last_damage: Vec<PxRect>,
    pub textures: BTreeMap<TextureId, (u32, u32)>,
    pub targets: BTreeMap<TargetId, PhysicalSize>,
    /// The instance-layout generation each target last uploaded.
    pub uploaded: BTreeMap<TargetId, u64>,
}

impl RecordingRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    fn apply(&mut self, u: &TextureUpdate, stats: &mut FrameStats) {
        stats.texture_uploads += 1;
        match &u.kind {
            TextureUpdateKind::Create { w, h } | TextureUpdateKind::Grow { w, h } => {
                self.textures.insert(u.tex, (*w, *h));
            }
            TextureUpdateKind::Write { data, .. } => stats.texture_bytes += data.len() as u64,
        }
    }
}

impl UiRenderer for RecordingRenderer {
    fn upload(&mut self, update: &TextureUpdate) -> Result<(), UiError> {
        let mut s = FrameStats::default();
        self.apply(update, &mut s);
        Ok(())
    }

    fn resize(&mut self, target: TargetId, size: PhysicalSize) -> Result<(), UiError> {
        self.targets.insert(target, size);
        Ok(())
    }

    fn render(
        &mut self,
        target: TargetId,
        batches: &BatchList,
        damage: &[PxRect],
    ) -> Result<FrameStats, UiError> {
        let uploaded = self.uploaded.insert(target, batches.generation);
        let mut stats = FrameStats {
            instances_uploaded: u32::try_from(batches.instances_to_upload(uploaded))
                .unwrap_or(u32::MAX),
            mesh_vertices_uploaded: u32::try_from(batches.mesh_vertices_to_upload(uploaded))
                .unwrap_or(u32::MAX),
            batches: batches.batches.len() as u32,
            instances: batches.instances.len() as u32,
            damage_rects: damage.len() as u32,
            damage_px: damage.iter().map(PxRect::area).sum(),
            extra_epochs: batches.extra_epochs,
            ..FrameStats::default()
        };
        for u in &batches.pre_uploads {
            self.apply(u, &mut stats);
        }
        for b in &batches.batches {
            for u in &b.uploads {
                self.apply(u, &mut stats);
            }
        }
        // Per damage rect, each batch whose scissor meets it is one draw call; plus one
        // composite of the UI target into the swapchain image.
        for d in damage {
            for b in &batches.batches {
                let empty = matches!(
                    b.kind,
                    BatchKind::Instances { count: 0, .. } | BatchKind::Mesh { index_count: 0, .. }
                );
                let meets = b.clip.is_none_or(|c| !c.intersect(d).is_empty());
                if !empty && meets {
                    stats.draw_calls += 1;
                    if matches!(b.kind, BatchKind::Mesh { .. }) {
                        stats.mesh_draws += 1;
                    }
                }
            }
        }
        if !damage.is_empty() {
            stats.draw_calls += 1;
        }
        self.frames.push(stats);
        self.last = Some(batches.clone());
        self.last_damage = damage.to_vec();
        Ok(stats)
    }
}

/// Above this many rects, damage collapses to their bounding box.
pub const MERGE_PAIRWISE_LIMIT: usize = 32;

/// Merge damage rects: overlapping or near rects coalesce, and at most `max` remain, so
/// the per-rect draw-call multiplier stays small (§21.11).
pub fn merge_damage(mut rects: Vec<PxRect>, max: usize) -> Vec<PxRect> {
    rects.retain(|r| !r.is_empty());
    rects.sort_unstable_by_key(|r| (r.y, r.x, r.w, r.h));
    rects.dedup();
    // Pairwise merging is quadratic per step; beyond a handful of rects (a theme switch,
    // a relayout) the frame is a large change anyway, so draw the bounding box.
    if rects.len() > MERGE_PAIRWISE_LIMIT {
        let all = rects.iter().fold(PxRect::default(), |a, r| a.union(r));
        return vec![all];
    }
    loop {
        let mut best: Option<(usize, usize, u64)> = None;
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let u = rects[i].union(&rects[j]);
                let waste = u.area().saturating_sub(rects[i].area() + rects[j].area());
                let overlapping = !rects[i].intersect(&rects[j]).is_empty();
                if (overlapping || waste == 0 || rects.len() > max)
                    && best.is_none_or(|(_, _, w)| waste < w)
                {
                    best = Some((i, j, waste));
                }
            }
        }
        match best {
            Some((i, j, _)) => {
                let u = rects[i].union(&rects[j]);
                rects.swap_remove(j);
                rects[i] = u;
            }
            None => return rects,
        }
    }
}

/// The renderer-independent batcher.
pub mod batch {
    use super::*;
    use crate::text::{Plane, TextSystem};

    forge_trace::control_switches! {
        /// Fault switch (W2): one batch per primitive (the `ui_draw_calls_batched` control).
        #[derive(Clone, Copy, Debug, Default)]
        pub struct BatchFaults {
            pub disable_batching: bool,
        }
    }

    struct Builder {
        out: BatchList,
        start: u32,
        clip: Option<PxRect>,
    }

    impl Builder {
        fn close(&mut self, uploads: Vec<TextureUpdate>) {
            let end = self.out.instances.len() as u32;
            if end > self.start || !uploads.is_empty() {
                self.out.batches.push(Batch {
                    kind: BatchKind::Instances {
                        start: self.start,
                        count: end - self.start,
                    },
                    clip: self.clip,
                    uploads,
                });
            }
            self.start = end;
        }
    }

    fn px(r: &Rect, s: f32) -> [f32; 4] {
        // Snap edges to physical pixels (§21.14): 1-px borders stay crisp at 125%/150%.
        let x0 = (r.x * s).round();
        let y0 = (r.y * s).round();
        let x1 = (r.right() * s).round();
        let y1 = (r.bottom() * s).round();
        [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
    }

    /// Build the frame's batches from primitives in paint order, resolving glyph runs
    /// through the shaping cache and the glyph atlas (rasterising misses).
    pub fn build<'a>(
        prims: impl IntoIterator<Item = &'a Primitive>,
        text: &mut TextSystem,
        images: &mut ImageAtlas,
        meshes: &MeshStore,
        scale: f32,
        faults: BatchFaults,
    ) -> BatchList {
        let mut b = Builder {
            out: BatchList::default(),
            start: 0,
            clip: None,
        };
        let mut clips: Vec<Option<PxRect>> = Vec::new();
        let mut pre = text.atlas.take_updates();
        pre.extend(images.take_updates());
        let mut pending_uploads = pre;
        for p in prims {
            match p {
                Primitive::PushClip { rect, .. } => {
                    b.close(std::mem::take(&mut pending_uploads));
                    clips.push(b.clip);
                    let r = rect.to_px(s_or(scale));
                    b.clip = Some(match b.clip {
                        Some(c) => c.intersect(&r),
                        None => r,
                    });
                }
                Primitive::PopClip => {
                    b.close(std::mem::take(&mut pending_uploads));
                    b.clip = clips.pop().flatten();
                }
                Primitive::Viewport { tex, rect } => {
                    b.close(std::mem::take(&mut pending_uploads));
                    b.out.batches.push(Batch {
                        kind: BatchKind::Viewport {
                            tex: *tex,
                            rect: rect.to_px(scale),
                        },
                        clip: b.clip,
                        uploads: Vec::new(),
                    });
                }
                Primitive::Mesh { mesh } => {
                    b.close(std::mem::take(&mut pending_uploads));
                    if let Some(m) = meshes.get(*mesh) {
                        push_mesh(&mut b.out, m, scale, b.clip);
                    }
                }
                Primitive::Quad {
                    rect,
                    radii,
                    fill,
                    border,
                    shadow,
                } => {
                    let rad = [
                        radii.tl * scale,
                        radii.tr * scale,
                        radii.br * scale,
                        radii.bl * scale,
                    ];
                    if let Some(s) = shadow
                        && s.color.a > 0.0
                    {
                        let shape = rect.translate(s.offset_x, s.offset_y).outset(s.spread);
                        let reach = shape.outset(s.blur * 2.0);
                        b.out.instances.push(Instance {
                            rect: px(&reach, scale),
                            uv: px(&shape, scale),
                            color: s.color.to_linear_premul(),
                            border_color: [0.0; 4],
                            radii: rad,
                            params: [0.0, s.blur * scale, kind::SHADOW, 0.0],
                        });
                    }
                    let (bw, bc) = border
                        .map(|b| (b.width * scale, b.color.to_linear_premul()))
                        .unwrap_or((0.0, [0.0; 4]));
                    b.out.instances.push(Instance {
                        rect: px(rect, scale),
                        uv: [0.0; 4],
                        color: fill.to_linear_premul(),
                        border_color: bc,
                        radii: rad,
                        params: [bw, 0.0, kind::QUAD, 0.0],
                    });
                }
                Primitive::Icon { icon, rect, color } => {
                    let inst = match cache::icon_instance(text, *icon, rect, color, scale) {
                        Ok(i) => i,
                        Err(full) => {
                            // Atlas epoch, as for a glyph.
                            pending_uploads.extend(text.atlas.take_updates());
                            b.close(std::mem::take(&mut pending_uploads));
                            text.atlas.begin_epoch(full_plane(full));
                            b.out.extra_epochs += 1;
                            cache::icon_instance(text, *icon, rect, color, scale)
                                .ok()
                                .flatten()
                        }
                    };
                    pending_uploads.extend(text.atlas.take_updates());
                    b.out.instances.extend(inst);
                }
                Primitive::Image { image, rect, tint } => {
                    if let Some((x, y, w, h)) = images.uv(*image) {
                        b.out.instances.push(Instance {
                            rect: px(rect, scale),
                            uv: [x as f32, y as f32, w as f32, h as f32],
                            color: tint.to_linear_premul(),
                            border_color: [0.0; 4],
                            radii: [0.0; 4],
                            params: [0.0, 0.0, kind::IMAGE, 0.0],
                        });
                    }
                }
                Primitive::Glyphs { .. } | Primitive::RichGlyphs { .. } => {
                    let (run, origin, cols): (_, _, Vec<[f32; 4]>) = match p {
                        Primitive::Glyphs { run, origin, color } => {
                            (*run, *origin, vec![color.to_linear_premul()])
                        }
                        Primitive::RichGlyphs {
                            run,
                            origin,
                            colors,
                        } => (
                            *run,
                            *origin,
                            colors.iter().map(|c| c.to_linear_premul()).collect(),
                        ),
                        _ => continue,
                    };
                    let o = ((origin.x * scale).round(), (origin.y * scale).round());
                    for (key, gx, gy, tag) in text.glyphs_tagged(run, o) {
                        let col = cols
                            .get(tag.min(cols.len().saturating_sub(1)))
                            .copied()
                            .unwrap_or([1.0; 4]);
                        let entry = match text.glyph(key) {
                            Ok(e) => e,
                            Err(full) => {
                                // Atlas epoch: draw what is resident, recycle the plane,
                                // place the rest. The frame never draws missing glyphs.
                                pending_uploads.extend(text.atlas.take_updates());
                                b.close(std::mem::take(&mut pending_uploads));
                                text.atlas.begin_epoch(full_plane(full));
                                b.out.extra_epochs += 1;
                                text.glyph(key).ok().flatten()
                            }
                        };
                        let Some(e) = entry else { continue };
                        pending_uploads.extend(text.atlas.take_updates());
                        let is_color = e.plane == Plane::Color;
                        b.out.instances.push(Instance {
                            rect: [
                                (gx + e.left) as f32,
                                (gy - e.top) as f32,
                                e.w as f32,
                                e.h as f32,
                            ],
                            uv: [e.x as f32, e.y as f32, e.w as f32, e.h as f32],
                            color: if is_color { [1.0; 4] } else { col },
                            border_color: [0.0; 4],
                            radii: [0.0; 4],
                            params: [
                                0.0,
                                0.0,
                                if is_color {
                                    kind::GLYPH_COLOR
                                } else {
                                    kind::GLYPH_MASK
                                },
                                0.0,
                            ],
                        });
                    }
                }
            }
            if faults.disable_batching() {
                b.close(std::mem::take(&mut pending_uploads));
            }
        }
        pending_uploads.extend(text.atlas.take_updates());
        b.close(pending_uploads);
        b.out
    }

    /// Append a mesh's vertices and indices and its draw (one batch of the mesh pipeline).
    pub(crate) fn push_mesh(out: &mut BatchList, m: &MeshData, scale: f32, clip: Option<PxRect>) {
        if m.is_empty() {
            return;
        }
        let base = u32::try_from(out.mesh_vertices.len()).unwrap_or(u32::MAX);
        let first = u32::try_from(out.mesh_indices.len()).unwrap_or(u32::MAX);
        out.mesh_vertices.extend(
            m.verts
                .iter()
                .map(|v| GpuMeshVertex::from_logical(v, scale)),
        );
        out.mesh_indices.extend(m.indices.iter().map(|i| i + base));
        out.batches.push(Batch {
            kind: BatchKind::Mesh {
                first_index: first,
                index_count: u32::try_from(m.indices.len()).unwrap_or(u32::MAX),
            },
            clip,
            uploads: Vec::new(),
        });
    }

    pub(crate) fn px_rect(r: &Rect, s: f32) -> [f32; 4] {
        px(r, s)
    }

    fn s_or(s: f32) -> f32 {
        if s > 0.0 { s } else { 1.0 }
    }

    fn full_plane(_f: crate::text::AtlasFull) -> Plane {
        // Only the mask plane can realistically overflow within one frame (colour glyphs
        // are rare); recycling it is always correct because the closed batch holds every
        // instance that referenced its old contents.
        Plane::Mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_merge_coalesces_overlaps_and_caps_count() {
        let r = |x, y, w, h| PxRect { x, y, w, h };
        let m = merge_damage(vec![r(0, 0, 10, 10), r(5, 5, 10, 10)], 4);
        assert_eq!(m, vec![r(0, 0, 15, 15)]);
        let many: Vec<_> = (0..10).map(|i| r(i * 100, 0, 10, 10)).collect();
        assert!(merge_damage(many, 4).len() <= 4);
        let apart = merge_damage(vec![r(0, 0, 10, 10), r(500, 500, 10, 10)], 4);
        assert_eq!(apart.len(), 2);
    }

    #[test]
    fn instance_is_96_bytes() {
        let mut v = Vec::new();
        Instance::default().write_bytes(&mut v);
        assert_eq!(v.len(), Instance::SIZE);
    }
}
