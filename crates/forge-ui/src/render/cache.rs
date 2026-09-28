//! The retained batch cache (Ch.21 §21.11, §21.13; owner rule 2).
//!
//! The display list is retained per widget slice, and so is its **batched** form: each
//! slice is turned into GPU instances once, when it is re-recorded, and kept. A frame then
//! costs work proportional to what changed:
//!
//! * **Patch** (the common interaction frame — hover, press, caret, a slider thumb): every
//!   re-recorded slice kept its shape (the same breaks, each instance run still fits its
//!   slot). Its instances are written into their slot in the persistent instance image,
//!   and only those ranges are uploaded. Nothing else is walked, batched or copied.
//! * **Reassemble** (tree structure, clips or visibility changed, or a slice outgrew its
//!   slot): the paint order is walked once, clean slices contribute their cached batches
//!   (no glyph lookups, no re-batching), slots get power-of-two slack so the next growth
//!   is a patch, and the whole image is uploaded once (a new layout generation).
//! * **Legacy full build** — `batch::build` over the display list — only for an atlas epoch
//!   frame (§21.7) and the batching-disabled positive control.
//!
//! Slack instances are zero: a zero-sized quad covers no pixel, so it draws nothing.

use std::collections::HashMap;

use super::{
    Batch, BatchKind, BatchList, ExternalTexture, GpuMeshVertex, ImageAtlas, Instance, MeshStore,
    Primitive, kind,
};
use crate::geom::{PxRect, Rect};
use crate::text::{AtlasFull, Plane, TextSystem};

/// One run of a slice's batched output between breaks.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Seg {
    Inst(Vec<Instance>),
    /// A clip pushed inside the slice (logical rect; intersected with the stack).
    PushClip(Rect),
    PopClip,
    Viewport(ExternalTexture, Rect),
    Mesh(Vec<GpuMeshVertex>, Vec<u32>),
}

/// A slice, batched.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SliceBatch {
    pub(crate) segs: Vec<Seg>,
}

impl SliceBatch {
    /// Close the open instance run (a break follows).
    fn flush(&mut self, cur: &mut Vec<Instance>) {
        if !cur.is_empty() {
            self.segs.push(Seg::Inst(std::mem::take(cur)));
        }
    }
}

/// Batch one slice's primitives. `Err(AtlasFull)`: the glyph atlas needs an epoch, which
/// only the legacy full build can draw.
pub(crate) fn batch_slice(
    prims: &[Primitive],
    text: &mut TextSystem,
    images: &ImageAtlas,
    meshes: &MeshStore,
    scale: f32,
) -> Result<SliceBatch, AtlasFull> {
    let mut sb = SliceBatch::default();
    let mut cur: Vec<Instance> = Vec::new();
    for p in prims {
        match p {
            Primitive::PushClip { rect, .. } => {
                sb.flush(&mut cur);
                sb.segs.push(Seg::PushClip(*rect));
            }
            Primitive::PopClip => {
                sb.flush(&mut cur);
                sb.segs.push(Seg::PopClip);
            }
            Primitive::Viewport { tex, rect } => {
                sb.flush(&mut cur);
                sb.segs.push(Seg::Viewport(*tex, *rect));
            }
            Primitive::Mesh { mesh } => {
                sb.flush(&mut cur);
                if let Some(m) = meshes.get(*mesh)
                    && !m.is_empty()
                {
                    sb.segs.push(Seg::Mesh(
                        m.verts
                            .iter()
                            .map(|v| GpuMeshVertex::from_logical(v, scale))
                            .collect(),
                        m.indices.clone(),
                    ));
                }
            }
            Primitive::Quad {
                rect,
                radii,
                fill,
                border,
                shadow,
            } => {
                let out = &mut cur;
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
                    out.push(Instance {
                        rect: super::batch::px_rect(&reach, scale),
                        uv: super::batch::px_rect(&shape, scale),
                        color: s.color.to_linear_premul(),
                        border_color: [0.0; 4],
                        radii: rad,
                        params: [0.0, s.blur * scale, kind::SHADOW, 0.0],
                    });
                }
                let (bw, bc) = border
                    .map(|b| (b.width * scale, b.color.to_linear_premul()))
                    .unwrap_or((0.0, [0.0; 4]));
                out.push(Instance {
                    rect: super::batch::px_rect(rect, scale),
                    uv: [0.0; 4],
                    color: fill.to_linear_premul(),
                    border_color: bc,
                    radii: rad,
                    params: [bw, 0.0, kind::QUAD, 0.0],
                });
            }
            Primitive::Image { image, rect, tint } => {
                if let Some((x, y, w, h)) = images.uv(*image) {
                    cur.push(Instance {
                        rect: super::batch::px_rect(rect, scale),
                        uv: [x as f32, y as f32, w as f32, h as f32],
                        color: tint.to_linear_premul(),
                        border_color: [0.0; 4],
                        radii: [0.0; 4],
                        params: [0.0, 0.0, kind::IMAGE, 0.0],
                    });
                }
            }
            Primitive::Glyphs { run, origin, color } => {
                glyph_instances(text, *run, *origin, &[*color], scale, &mut cur)?;
            }
            Primitive::Icon { icon, rect, color } => {
                cur.extend(icon_instance(text, *icon, rect, color, scale)?);
            }
            Primitive::RichGlyphs {
                run,
                origin,
                colors,
            } => {
                glyph_instances(text, *run, *origin, colors, scale, &mut cur)?;
            }
        }
    }
    sb.flush(&mut cur);
    Ok(sb)
}

/// The instance for an icon of the Forge set: its atlas mask, pixel-snapped and centred in
/// the square `rect`'s shorter side makes.
pub(crate) fn icon_instance(
    text: &mut TextSystem,
    icon: crate::icons::Icon,
    rect: &crate::geom::Rect,
    color: &crate::geom::Color,
    scale: f32,
) -> Result<Option<Instance>, AtlasFull> {
    let side = rect.w.min(rect.h);
    let px = (side * scale).round();
    if px < 1.0 {
        return Ok(None);
    }
    let x = ((rect.x + (rect.w - side) * 0.5) * scale).round();
    let y = ((rect.y + (rect.h - side) * 0.5) * scale).round();
    let stroke = crate::icons::stroke_width(side) * scale;
    let Some(e) = text.icon(icon, px as u32, stroke)? else {
        return Ok(None);
    };
    Ok(Some(Instance {
        rect: [x, y, e.w as f32, e.h as f32],
        uv: [e.x as f32, e.y as f32, e.w as f32, e.h as f32],
        color: color.to_linear_premul(),
        border_color: [0.0; 4],
        radii: [0.0; 4],
        params: [0.0, 0.0, kind::GLYPH_MASK, 0.0],
    }))
}

/// Instances for a shaped run; `colors[tag]` colours each glyph (tag = span tag, clamped).
pub(crate) fn glyph_instances(
    text: &mut TextSystem,
    run: crate::text::GlyphRunId,
    origin: crate::geom::Point,
    colors: &[crate::geom::Color],
    scale: f32,
    out: &mut Vec<Instance>,
) -> Result<(), AtlasFull> {
    let o = ((origin.x * scale).round(), (origin.y * scale).round());
    let lin: Vec<[f32; 4]> = colors.iter().map(|c| c.to_linear_premul()).collect();
    for (key, gx, gy, tag) in text.glyphs_tagged(run, o) {
        let Some(e) = text.glyph(key)? else { continue };
        let is_color = e.plane == Plane::Color;
        let col = lin
            .get(tag.min(lin.len().saturating_sub(1)))
            .copied()
            .unwrap_or([1.0; 4]);
        out.push(Instance {
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
    Ok(())
}

/// Where one mesh segment lives in the persistent mesh buffers. Like instance runs, each
/// gets power-of-two slack so a curve that gains a few vertices is still patched in place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MeshSlot {
    /// Its batch index in the list.
    batch: usize,
    vstart: u32,
    vcap: u32,
    istart: u32,
    ilen: u32,
    icap: u32,
}

/// Where a slice's segments live in the persistent buffers.
#[derive(Clone, Debug, Default)]
struct Slot {
    /// Per `Seg::Inst`: `(start, len, cap)` in the instance image.
    inst: Vec<(u32, u32, u32)>,
    /// Per `Seg::Mesh`: where its geometry lives and its batch.
    mesh: Vec<MeshSlot>,
    /// The shape the patch path must match: segment kinds and clip/viewport rects.
    shape: Vec<ShapeKey>,
    /// The node-level clip this slot pushed for its children (px), if any.
    node_clip: Option<PxRect>,
}

#[derive(Clone, Debug, PartialEq)]
enum ShapeKey {
    Inst,
    Push(PxRect),
    Pop,
    Viewport(ExternalTexture, PxRect),
    Mesh,
}

/// Whether a re-batched slice still has its slot's shape (compared without allocating).
fn shape_matches(shape: &[ShapeKey], sb: &SliceBatch, scale: f32) -> bool {
    shape.len() == sb.segs.len()
        && shape.iter().zip(&sb.segs).all(|(k, s)| match (k, s) {
            (ShapeKey::Inst, Seg::Inst(_))
            | (ShapeKey::Pop, Seg::PopClip)
            | (ShapeKey::Mesh, Seg::Mesh(..)) => true,
            (ShapeKey::Push(p), Seg::PushClip(r)) => *p == r.to_px(scale),
            (ShapeKey::Viewport(t, p), Seg::Viewport(tt, r)) => t == tt && *p == r.to_px(scale),
            _ => false,
        })
}

fn shape_of(sb: &SliceBatch, scale: f32) -> Vec<ShapeKey> {
    sb.segs
        .iter()
        .map(|s| match s {
            Seg::Inst(_) => ShapeKey::Inst,
            Seg::PushClip(r) => ShapeKey::Push(r.to_px(scale)),
            Seg::PopClip => ShapeKey::Pop,
            Seg::Viewport(t, r) => ShapeKey::Viewport(*t, r.to_px(scale)),
            Seg::Mesh(..) => ShapeKey::Mesh,
        })
        .collect()
}

/// One step of the paint-order walk handed to [`BatchCache::assemble`].
pub(crate) enum Step<'a, K> {
    Slice(K, &'a SliceBatch),
    /// A node clipping its children (logical rect), owned by slice `K`.
    PushNodeClip(K, Rect),
    PopNodeClip,
}

/// The persistent batch list plus the slot map that lets a frame patch it in place.
pub struct BatchCache<K: std::hash::Hash + Eq + Copy> {
    pub(crate) list: BatchList,
    slots: HashMap<K, Slot>,
    /// Set by tree edits (add, remove, reorder, hide, restyle): the next frame reassembles.
    pub(crate) structure_dirty: bool,
    /// Glyph-atlas generation the cached glyph instances were built against.
    pub(crate) atlas_generation: u64,
    pub(crate) scale: f32,
}

impl<K: std::hash::Hash + Eq + Copy> Default for BatchCache<K> {
    fn default() -> Self {
        Self {
            list: BatchList::default(),
            slots: HashMap::new(),
            structure_dirty: true,
            atlas_generation: 0,
            scale: 0.0,
        }
    }
}

fn slack(len: u32) -> u32 {
    len.max(4).next_power_of_two()
}

/// Grow-only capacity: keep the previous capacity when the new length fits it.
fn cap_for(len: u32, prev_cap: u32) -> u32 {
    if len <= prev_cap {
        prev_cap
    } else {
        slack(len)
    }
}

fn u32_len(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Write mesh geometry into its slot: vertices at `vstart`, indices (rebased onto
/// `vstart`) at `istart`. Slack indices point at the slot's first vertex and are never
/// drawn (the batch's index count is the real length).
fn write_mesh(list: &mut BatchList, m: &MeshSlot, v: &[GpuMeshVertex], idx: &[u32]) {
    let vs = m.vstart as usize;
    list.mesh_vertices[vs..vs + v.len()].copy_from_slice(v);
    let is = m.istart as usize;
    for (dst, i) in list.mesh_indices[is..is + idx.len()].iter_mut().zip(idx) {
        *dst = i + m.vstart;
    }
}

impl<K: std::hash::Hash + Eq + Copy> BatchCache<K> {
    /// Rebuild the persistent list from the paint-order walk. Every instance run and mesh
    /// gets a power-of-two slot. The whole image is uploaded once (a new generation).
    pub(crate) fn assemble<'a>(&mut self, steps: impl Iterator<Item = Step<'a, K>>, scale: f32)
    where
        K: 'a,
    {
        let generation = self.list.generation.wrapping_add(1);
        let pre = std::mem::take(&mut self.list.pre_uploads);
        let mut out = BatchList {
            generation,
            pre_uploads: pre,
            ..BatchList::default()
        };
        // Reuse the old allocations.
        out.instances = std::mem::take(&mut self.list.instances);
        out.instances.clear();
        out.batches = std::mem::take(&mut self.list.batches);
        out.batches.clear();
        out.mesh_vertices = std::mem::take(&mut self.list.mesh_vertices);
        out.mesh_vertices.clear();
        out.mesh_indices = std::mem::take(&mut self.list.mesh_indices);
        out.mesh_indices.clear();
        out.instance_uploads = std::mem::take(&mut self.list.instance_uploads);
        out.instance_uploads.clear();
        out.mesh_vertex_uploads = std::mem::take(&mut self.list.mesh_vertex_uploads);
        out.mesh_vertex_uploads.clear();
        out.mesh_index_uploads = std::mem::take(&mut self.list.mesh_index_uploads);
        out.mesh_index_uploads.clear();
        let mut slots: HashMap<K, Slot> = HashMap::with_capacity(self.slots.len());
        let mut clip: Option<PxRect> = None;
        let mut stack: Vec<Option<PxRect>> = Vec::new();
        let mut open: Option<u32> = None; // start of the open instance batch
        let close = |out: &mut BatchList, open: &mut Option<u32>, clip: Option<PxRect>| {
            if let Some(start) = open.take() {
                let end = out.instances.len() as u32;
                if end > start {
                    out.batches.push(Batch {
                        kind: BatchKind::Instances {
                            start,
                            count: end - start,
                        },
                        clip,
                        uploads: Vec::new(),
                    });
                }
            }
        };
        let push_clip = |r: Rect,
                         out: &mut BatchList,
                         open: &mut Option<u32>,
                         clip: &mut Option<PxRect>,
                         stack: &mut Vec<Option<PxRect>>| {
            close(out, open, *clip);
            stack.push(*clip);
            let p = r.to_px(if scale > 0.0 { scale } else { 1.0 });
            *clip = Some(match *clip {
                Some(c) => c.intersect(&p),
                None => p,
            });
            p
        };
        for step in steps {
            match step {
                Step::Slice(k, sb) => {
                    let old = self.slots.get(&k);
                    let mut slot = Slot {
                        shape: shape_of(sb, scale),
                        ..Slot::default()
                    };
                    let mut inst_i = 0usize;
                    let mut mesh_i = 0usize;
                    for seg in &sb.segs {
                        match seg {
                            Seg::Inst(v) => {
                                let len = v.len() as u32;
                                let prev_cap = old
                                    .and_then(|o| o.inst.get(inst_i))
                                    .map(|s| s.2)
                                    .unwrap_or(0);
                                let cap = cap_for(len, prev_cap);
                                if open.is_none() {
                                    open = Some(out.instances.len() as u32);
                                }
                                let start = out.instances.len() as u32;
                                out.instances.extend_from_slice(v);
                                out.instances
                                    .resize((start + cap) as usize, Instance::default());
                                slot.inst.push((start, len, cap));
                                inst_i += 1;
                            }
                            Seg::PushClip(r) => {
                                push_clip(*r, &mut out, &mut open, &mut clip, &mut stack);
                            }
                            Seg::PopClip => {
                                close(&mut out, &mut open, clip);
                                clip = stack.pop().flatten();
                            }
                            Seg::Viewport(tex, r) => {
                                close(&mut out, &mut open, clip);
                                out.batches.push(Batch {
                                    kind: BatchKind::Viewport {
                                        tex: *tex,
                                        rect: r.to_px(scale),
                                    },
                                    clip,
                                    uploads: Vec::new(),
                                });
                            }
                            Seg::Mesh(v, idx) => {
                                close(&mut out, &mut open, clip);
                                let prev = old.and_then(|o| o.mesh.get(mesh_i)).copied();
                                let m = MeshSlot {
                                    batch: out.batches.len(),
                                    vstart: u32_len(out.mesh_vertices.len()),
                                    vcap: cap_for(u32_len(v.len()), prev.map_or(0, |p| p.vcap)),
                                    istart: u32_len(out.mesh_indices.len()),
                                    ilen: u32_len(idx.len()),
                                    icap: cap_for(u32_len(idx.len()), prev.map_or(0, |p| p.icap)),
                                };
                                out.mesh_vertices
                                    .resize((m.vstart + m.vcap) as usize, GpuMeshVertex::default());
                                out.mesh_indices
                                    .resize((m.istart + m.icap) as usize, m.vstart);
                                write_mesh(&mut out, &m, v, idx);
                                out.batches.push(Batch {
                                    kind: BatchKind::Mesh {
                                        first_index: m.istart,
                                        index_count: m.ilen,
                                    },
                                    clip,
                                    uploads: Vec::new(),
                                });
                                slot.mesh.push(m);
                                mesh_i += 1;
                            }
                        }
                    }
                    slots.insert(k, slot);
                }
                Step::PushNodeClip(k, r) => {
                    let p = push_clip(r, &mut out, &mut open, &mut clip, &mut stack);
                    if let Some(s) = slots.get_mut(&k) {
                        s.node_clip = Some(p);
                    }
                }
                Step::PopNodeClip => {
                    close(&mut out, &mut open, clip);
                    clip = stack.pop().flatten();
                }
            }
        }
        close(&mut out, &mut open, clip);
        out.meshes_changed = true;
        self.slots = slots;
        self.list = out;
        self.structure_dirty = false;
        self.scale = scale;
    }

    /// Patch re-batched slices into their slots. Returns `false` (the caller reassembles)
    /// if any slice changed shape or outgrew a slot. `changed` holds each slice's key and
    /// its current node-level clip; `batch_of` looks its new batch up, so the caller keeps
    /// no per-frame list of borrowed batches. Only the ranges written are queued for
    /// upload: dirty instance runs in `instance_uploads`, dirty meshes in
    /// `mesh_vertex_uploads` / `mesh_index_uploads`.
    pub(crate) fn patch<'b>(
        &mut self,
        changed: &[(K, Option<Rect>)],
        batch_of: impl Fn(K) -> Option<&'b SliceBatch>,
        scale: f32,
    ) -> bool {
        if self.structure_dirty || (scale - self.scale).abs() > f32::EPSILON {
            return false;
        }
        // Validate everything first: a patch is all or nothing.
        for (k, node_clip) in changed {
            let (Some(slot), Some(sb)) = (self.slots.get(k), batch_of(*k)) else {
                return false;
            };
            if !shape_matches(&slot.shape, sb, scale) {
                return false;
            }
            if slot.node_clip != node_clip.map(|r| r.to_px(scale)) {
                return false;
            }
            let (mut i, mut m) = (0usize, 0usize);
            for seg in &sb.segs {
                match seg {
                    Seg::Inst(v) => {
                        if v.len() as u32 > slot.inst[i].2 {
                            return false;
                        }
                        i += 1;
                    }
                    Seg::Mesh(v, idx) => {
                        let ms = &slot.mesh[m];
                        if u32_len(v.len()) > ms.vcap || u32_len(idx.len()) > ms.icap {
                            return false;
                        }
                        m += 1;
                    }
                    _ => {}
                }
            }
        }
        self.list.instance_uploads.clear();
        self.list.mesh_vertex_uploads.clear();
        self.list.mesh_index_uploads.clear();
        self.list.meshes_changed = false;
        for (k, _) in changed {
            let (Some(slot), Some(sb)) = (self.slots.get_mut(k), batch_of(*k)) else {
                continue;
            };
            let (mut i, mut m) = (0usize, 0usize);
            for seg in &sb.segs {
                match seg {
                    Seg::Inst(v) => {
                        let (start, old_len, cap) = slot.inst[i];
                        let len = v.len() as u32;
                        let s = start as usize;
                        self.list.instances[s..s + v.len()].copy_from_slice(v);
                        if old_len > len {
                            for x in &mut self.list.instances[s + v.len()..s + old_len as usize] {
                                *x = Instance::default();
                            }
                        }
                        let dirty_end = start + old_len.max(len).min(cap);
                        if dirty_end > start {
                            self.list.instance_uploads.push(start..dirty_end);
                        }
                        slot.inst[i].1 = len;
                        i += 1;
                    }
                    Seg::Mesh(v, idx) => {
                        let ms = &mut slot.mesh[m];
                        write_mesh(&mut self.list, ms, v, idx);
                        ms.ilen = u32_len(idx.len());
                        if let Some(b) = self.list.batches.get_mut(ms.batch) {
                            b.kind = BatchKind::Mesh {
                                first_index: ms.istart,
                                index_count: ms.ilen,
                            };
                        }
                        if !v.is_empty() {
                            let end = ms.vstart + u32_len(v.len());
                            self.list.mesh_vertex_uploads.push(ms.vstart..end);
                        }
                        if ms.ilen > 0 {
                            let end = ms.istart + ms.ilen;
                            self.list.mesh_index_uploads.push(ms.istart..end);
                        }
                        m += 1;
                    }
                    _ => {}
                }
            }
        }
        true
    }

    /// Adopt a list built by the legacy full path (atlas epochs, the batching-disabled
    /// control). The next frame reassembles from scratch.
    pub(crate) fn adopt_legacy(&mut self, mut list: BatchList) {
        list.generation = self.list.generation.wrapping_add(1);
        list.meshes_changed = true;
        self.list = list;
        self.slots.clear();
        self.structure_dirty = true;
    }
}
