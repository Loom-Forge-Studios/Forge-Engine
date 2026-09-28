//! Meshes: `lyon`-tessellated paths drawn through the **second pipeline** (Ch.21 §21.13).
//!
//! The curve, gradient and graph editors draw what quads cannot: curves, wires, filled
//! areas, smooth gradients. A widget builds a [`MeshData`] (fill or stroke a [`PathBuilder`]
//! path, or a per-vertex-coloured grid), and `PaintCx::mesh` registers it in the shared
//! [`MeshStore`] under a content hash — an unchanged curve re-registers the same
//! [`MeshId`], exactly as unchanged text hits the shaping cache.
//!
//! **Anti-aliasing without MSAA.** Strokes are tessellated one physical pixel wider than
//! asked, and every vertex carries its signed position across the stroke; the fragment
//! shader turns that into coverage, so a 1.5 px curve is smooth at any scale. Fills are
//! solid (their outline, where it matters, is a stroke).

use std::collections::HashMap;

use lyon_tessellation::math::point;
pub use lyon_tessellation::path::Path;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, LineCap, LineJoin, Side,
    StrokeOptions, StrokeTessellator, StrokeVertex, VertexBuffers,
};

use crate::geom::{Color, Point, Rect};
use crate::id::Fnv;

/// A mesh vertex in **logical** pixels (scaled to physical at batch time).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct MeshVertex {
    pub pos: [f32; 2],
    /// sRGB, straight alpha (as the theme speaks); linearised at batch time.
    pub color: Color,
    /// Stroke AA: `x` is the signed position across the stroke (±1 at the edges), `y` is
    /// the stroke's half-extent in logical px including the AA ramp. `y == 0`: a fill.
    pub aa: [f32; 2],
}

/// Tessellated geometry: triangles over `verts`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub verts: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

/// Identifies a registered mesh (content hash).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshId(pub u64);

/// A path in logical pixels, built with the usual verbs.
pub struct PathBuilder {
    b: lyon_tessellation::path::path::Builder,
    open: bool,
}

impl Default for PathBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PathBuilder {
    pub fn new() -> Self {
        Self {
            b: Path::builder(),
            open: false,
        }
    }
    pub fn move_to(&mut self, p: Point) -> &mut Self {
        if self.open {
            self.b.end(false);
        }
        self.b.begin(point(p.x, p.y));
        self.open = true;
        self
    }
    pub fn line_to(&mut self, p: Point) -> &mut Self {
        if !self.open {
            return self.move_to(p);
        }
        self.b.line_to(point(p.x, p.y));
        self
    }
    pub fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) -> &mut Self {
        if !self.open {
            return self.move_to(p);
        }
        self.b
            .cubic_bezier_to(point(c1.x, c1.y), point(c2.x, c2.y), point(p.x, p.y));
        self
    }
    pub fn close(&mut self) -> &mut Self {
        if self.open {
            self.b.end(true);
            self.open = false;
        }
        self
    }
    pub fn build(mut self) -> Path {
        if self.open {
            self.b.end(false);
        }
        self.b.build()
    }
}

/// Curves are flattened to this tolerance (logical px): below a physical pixel at 2×.
const TOLERANCE: f32 = 0.1;

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    fn base(&self) -> u32 {
        u32::try_from(self.verts.len()).unwrap_or(u32::MAX)
    }

    /// Append a filled path.
    pub fn fill(&mut self, path: &Path, color: Color) {
        let mut buf: VertexBuffers<MeshVertex, u32> = VertexBuffers::new();
        let mut t = FillTessellator::new();
        let _ = t.tessellate_path(
            path,
            &FillOptions::tolerance(TOLERANCE),
            &mut BuffersBuilder::new(&mut buf, |v: FillVertex| MeshVertex {
                pos: v.position().to_array(),
                color,
                aa: [0.0, 0.0],
            }),
        );
        self.append(buf);
    }

    /// Append a stroked path, `width` logical px wide, anti-aliased (see the module docs).
    /// `px` is the size of one physical pixel in logical px (1 / scale).
    pub fn stroke(&mut self, path: &Path, width: f32, color: Color, px: f32) {
        let extent = (width + px) * 0.5;
        let mut buf: VertexBuffers<MeshVertex, u32> = VertexBuffers::new();
        let mut t = StrokeTessellator::new();
        let opts = StrokeOptions::tolerance(TOLERANCE)
            .with_line_width(extent * 2.0)
            .with_line_join(LineJoin::Round)
            .with_line_cap(LineCap::Round);
        let _ = t.tessellate_path(
            path,
            &opts,
            &mut BuffersBuilder::new(&mut buf, |v: StrokeVertex| MeshVertex {
                pos: v.position().to_array(),
                color,
                aa: [
                    match v.side() {
                        Side::Positive => 1.0,
                        Side::Negative => -1.0,
                    },
                    extent,
                ],
            }),
        );
        self.append(buf);
    }

    /// Append a stroked polyline as one triangle strip (mitred joins, butt ends), with the
    /// same anti-aliasing attributes as [`MeshData::stroke`]. Many short wires (the node
    /// canvas) cost a few vertices per point this way instead of a full path tessellation
    /// with round joins: ~15x cheaper for 200 wires per frame.
    pub fn stroke_polyline(&mut self, pts: &[Point], width: f32, color: Color, px: f32) {
        if pts.len() < 2 {
            return;
        }
        let extent = (width + px) * 0.5;
        let base = self.base();
        let n = pts.len();
        let seg_normal = |a: Point, b: Point| -> (f32, f32) {
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let l = (dx * dx + dy * dy).sqrt();
            if l <= f32::EPSILON {
                (0.0, 0.0)
            } else {
                (-dy / l, dx / l)
            }
        };
        for i in 0..n {
            let prev = if i > 0 {
                seg_normal(pts[i - 1], pts[i])
            } else {
                (0.0, 0.0)
            };
            let next = if i + 1 < n {
                seg_normal(pts[i], pts[i + 1])
            } else {
                (0.0, 0.0)
            };
            let (mut nx, mut ny) = (prev.0 + next.0, prev.1 + next.1);
            let l = (nx * nx + ny * ny).sqrt();
            if l <= f32::EPSILON {
                (nx, ny) = if i > 0 { prev } else { next };
            } else {
                nx /= l;
                ny /= l;
            }
            // Mitre: lengthen the offset so the stroke keeps its width at a bend (capped
            // at 2x so a hairpin does not spike).
            let cos = (nx * next.0 + ny * next.1)
                .abs()
                .max((nx * prev.0 + ny * prev.1).abs());
            let m = extent / cos.max(0.5);
            let p = pts[i];
            self.verts.push(MeshVertex {
                pos: [p.x + nx * m, p.y + ny * m],
                color,
                aa: [1.0, extent],
            });
            self.verts.push(MeshVertex {
                pos: [p.x - nx * m, p.y - ny * m],
                color,
                aa: [-1.0, extent],
            });
        }
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        for i in 0..n.saturating_sub(1) {
            let a = base + 2 * i;
            self.indices
                .extend_from_slice(&[a, a + 1, a + 2, a + 1, a + 3, a + 2]);
        }
    }

    /// Append a `cols × rows` grid over `rect` whose vertex colours come from `color_at(u, v)`
    /// (`u`, `v` in 0..=1). Colour pickers and gradient previews use it: interpolation is
    /// linear between grid points, so a fine grid approximates any colour field.
    pub fn color_grid(
        &mut self,
        rect: Rect,
        cols: u32,
        rows: u32,
        color_at: impl Fn(f32, f32) -> Color,
    ) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let base = self.base();
        for j in 0..=rows {
            for i in 0..=cols {
                let (u, v) = (i as f32 / cols as f32, j as f32 / rows as f32);
                self.verts.push(MeshVertex {
                    pos: [rect.x + u * rect.w, rect.y + v * rect.h],
                    color: color_at(u, v),
                    aa: [0.0, 0.0],
                });
            }
        }
        let w = cols + 1;
        for j in 0..rows {
            for i in 0..cols {
                let a = base + j * w + i;
                let b = a + 1;
                let c = a + w;
                let d = c + 1;
                self.indices.extend_from_slice(&[a, b, c, c, b, d]);
            }
        }
    }

    fn append(&mut self, buf: VertexBuffers<MeshVertex, u32>) {
        let base = self.base();
        self.verts.extend(buf.vertices);
        self.indices
            .extend(buf.indices.into_iter().map(|i| i + base));
    }

    /// Logical bounds (damage), including the stroke extent.
    pub fn bounds(&self) -> Rect {
        let mut r = Rect::ZERO;
        let mut first = true;
        for v in &self.verts {
            let p = Rect::new(v.pos[0], v.pos[1], 0.0, 0.0).outset(0.5);
            if first {
                r = p;
                first = false;
            } else {
                r = Rect::from_min_max(
                    r.x.min(p.x),
                    r.y.min(p.y),
                    r.right().max(p.right()),
                    r.bottom().max(p.bottom()),
                );
            }
        }
        r
    }

    /// Exact content hash (the [`MeshId`]).
    pub fn content_hash(&self) -> u64 {
        let mut h = Fnv::new();
        for v in &self.verts {
            h.f32(v.pos[0]);
            h.f32(v.pos[1]);
            h.f32(v.color.r);
            h.f32(v.color.g);
            h.f32(v.color.b);
            h.f32(v.color.a);
            h.f32(v.aa[0]);
            h.f32(v.aa[1]);
        }
        h.byte(0xee);
        for i in &self.indices {
            h.u32(*i);
        }
        crate::id::splitmix(h.finish())
    }
}

struct MeshEntry {
    data: MeshData,
    bounds: Rect,
    refs: u32,
}

/// The shared mesh store: registered meshes, reference-counted by the retained slices that
/// draw them (like glyph runs). Unreferenced meshes are dropped at the end of a frame.
#[derive(Default)]
pub struct MeshStore {
    meshes: HashMap<MeshId, MeshEntry>,
    /// Tessellated meshes registered (cache misses).
    pub registered: u64,
}

impl MeshStore {
    /// Register (or find) `data`; returns its id and logical bounds.
    pub fn register(&mut self, data: MeshData) -> (MeshId, Rect) {
        let id = MeshId(data.content_hash());
        if let Some(e) = self.meshes.get(&id) {
            return (id, e.bounds);
        }
        self.registered += 1;
        let bounds = data.bounds();
        self.meshes.insert(
            id,
            MeshEntry {
                data,
                bounds,
                refs: 0,
            },
        );
        (id, bounds)
    }
    pub fn get(&self, id: MeshId) -> Option<&MeshData> {
        self.meshes.get(&id).map(|e| &e.data)
    }
    pub fn bounds(&self, id: MeshId) -> Option<Rect> {
        self.meshes.get(&id).map(|e| e.bounds)
    }
    pub fn retain(&mut self, id: MeshId) {
        if let Some(e) = self.meshes.get_mut(&id) {
            e.refs += 1;
        }
    }
    pub fn release(&mut self, id: MeshId) {
        if let Some(e) = self.meshes.get_mut(&id) {
            e.refs = e.refs.saturating_sub(1);
        }
    }
    /// Drop meshes no retained slice references.
    pub fn collect(&mut self) {
        self.meshes.retain(|_, e| e.refs > 0);
    }
    pub fn len(&self) -> usize {
        self.meshes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }
}

/// One mesh vertex as the GPU sees it: physical px, linear premultiplied colour, AA. 32 bytes.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct GpuMeshVertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
    pub aa: [f32; 2],
}

impl GpuMeshVertex {
    pub const SIZE: usize = 32;
    pub fn write_bytes(&self, out: &mut Vec<u8>) {
        for v in self.pos.iter().chain(&self.color).chain(&self.aa) {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    pub(crate) fn from_logical(v: &MeshVertex, scale: f32) -> Self {
        Self {
            pos: [v.pos[0] * scale, v.pos[1] * scale],
            color: v.color.to_linear_premul(),
            aa: [v.aa[0], v.aa[1] * scale],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Path {
        let mut p = PathBuilder::new();
        p.move_to(Point::new(0.0, 0.0)).cubic_to(
            Point::new(30.0, 0.0),
            Point::new(70.0, 100.0),
            Point::new(100.0, 100.0),
        );
        p.build()
    }

    #[test]
    fn stroke_is_tessellated_with_aa_edges() {
        let mut m = MeshData::default();
        m.stroke(&line(), 2.0, Color::TRANSPARENT, 1.0);
        assert!(m.indices.len() >= 6 && m.indices.len() % 3 == 0);
        assert!(m.verts.iter().any(|v| v.aa[0] > 0.0) && m.verts.iter().any(|v| v.aa[0] < 0.0));
        assert!(m.verts.iter().all(|v| (v.aa[1] - 1.5).abs() < 1e-6));
        let b = m.bounds();
        assert!(b.w >= 100.0 && b.h >= 100.0);
    }

    #[test]
    fn same_content_same_id_and_refcounted_collection() {
        let mut s = MeshStore::default();
        let mut m = MeshData::default();
        m.fill(&line(), Color::TRANSPARENT);
        let (a, _) = s.register(m.clone());
        let (b, _) = s.register(m);
        assert_eq!(a, b);
        assert_eq!(s.registered, 1);
        s.retain(a);
        s.collect();
        assert_eq!(s.len(), 1);
        s.release(a);
        s.collect();
        assert!(s.is_empty());
    }

    #[test]
    fn color_grid_has_expected_topology() {
        let mut m = MeshData::default();
        m.color_grid(Rect::new(0.0, 0.0, 10.0, 10.0), 4, 3, |_, _| {
            Color::TRANSPARENT
        });
        assert_eq!(m.verts.len(), 5 * 4);
        assert_eq!(m.indices.len(), 4 * 3 * 6);
    }
}
