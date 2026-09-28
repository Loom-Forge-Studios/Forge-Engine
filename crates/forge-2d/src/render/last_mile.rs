//! The 2D last mile: `f64` world -> `f32` render-target pixels, **the only place in
//! forge-2d that narrows** (`tests/liveness/test_no_f32_below_render.rs`, Ch.2.4).
//!
//! Every sprite, mesh vertex and light is made camera-relative in `f64` (after its layer's
//! parallax offset), scaled to pixels, culled against the view, sorted and batched
//! ([`crate::sprite::batch`]), and only then written as `f32` into the byte buffers the GPU
//! reads. Shadow polygons are computed in `f64` ([`crate::light::visibility`]) before they
//! are narrowed.

use forge_num::DVec2;

use crate::Error2d;
use crate::camera::Camera2d;
use crate::light::{LightShape, VisibilityScratch, visibility_with};
use crate::math::Aabb2;
use crate::parallax::ParallaxLayer;
use crate::sprite::{Batch, DrawKey, DrawKind, Frame2d, Sprite, TextureId, batch_into};

/// Bytes per sprite instance: centre, x axis, y axis, pivot (4 x vec2), uv rect (vec4),
/// tint (unorm8x4), flags (u32).
pub const INSTANCE_BYTES: usize = 12 * 4 + 4 + 4;
/// Bytes per mesh vertex: position (vec2), uv (vec2), colour (unorm8x4).
pub const MESH_VERTEX_BYTES: usize = 4 * 4 + 4;
/// Bytes per light-fan vertex: position, light centre (vec2 x 2), colour + radius (vec4),
/// spot direction + cones (vec4), height, spot flag, 2 pad (vec4).
pub const LIGHT_VERTEX_BYTES: usize = 16 * 4;

/// A prepared draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    pub kind: DrawKind,
    pub texture: TextureId,
    /// Sprites: first instance and count. Meshes: first index and count.
    pub first: u32,
    pub count: u32,
}

/// Counters of one prepared frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrepareStats {
    pub sprites: u32,
    pub culled: u32,
    pub meshes: u32,
    pub draw_calls: u32,
    pub lights: u32,
    pub light_triangles: u32,
}

/// Buffers a frame fills and the renderer uploads; kept between frames (no allocation
/// once warm).
#[derive(Debug, Default)]
pub struct Prepared {
    pub instances: Vec<u8>,
    pub mesh_vertices: Vec<u8>,
    pub mesh_indices: Vec<u32>,
    /// `mesh_indices` as little-endian bytes (the upload), kept between frames.
    pub mesh_index_bytes: Vec<u8>,
    pub light_vertices: Vec<u8>,
    pub draws: Vec<Draw>,
    pub stats: PrepareStats,
    keys: Vec<DrawKey>,
    visible: Vec<(Sprite, DVec2)>,
    segments: Vec<(DVec2, DVec2)>,
    fan: Vec<DVec2>,
    vis: VisibilityScratch,
    batches: Vec<Batch>,
}

fn f(v: f64) -> [u8; 4] {
    (v as f32).to_le_bytes()
}

fn unorm4(c: [f64; 4]) -> [u8; 4] {
    let q = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(c[0]), q(c[1]), q(c[2]), q(c[3])]
}

/// World -> pixel mapping for one frame.
#[derive(Clone, Copy)]
struct View {
    center: DVec2,
    scale: f64,
    half_px: DVec2,
}

impl View {
    fn px(&self, world: DVec2) -> DVec2 {
        let d = (world - self.center) * self.scale;
        DVec2::new(self.half_px.x + d.x, self.half_px.y - d.y)
    }
}

impl Prepared {
    /// Fill the buffers for `frame` drawn at `rw x rh`. `known` says whether a texture id
    /// is held by the renderer. `split_all` (a W2 control) disables batching.
    pub fn fill(
        &mut self,
        frame: &Frame2d,
        rw: u32,
        rh: u32,
        known: &dyn Fn(TextureId) -> bool,
        split_all: bool,
    ) -> Result<(), Error2d> {
        let cam: &Camera2d = &frame.camera;
        cam.validate()?;
        let fr = cam.frame();
        let view = View {
            center: cam.center().local,
            scale: cam.scale(),
            half_px: DVec2::new(f64::from(rw) * 0.5, f64::from(rh) * 0.5),
        };
        let half = cam.half_extent(rw, rh);
        self.instances.clear();
        self.mesh_vertices.clear();
        self.mesh_indices.clear();
        self.light_vertices.clear();
        self.draws.clear();
        self.keys.clear();
        self.visible.clear();
        self.stats = PrepareStats::default();
        let layer_of = |l: i32| frame.parallax.get(&l).copied().unwrap_or_default();
        // ---- sprites: parallax, repeats, cull ----
        for s in &frame.sprites {
            if s.pos.frame != fr {
                return Err(Error2d::Frame {
                    why: format!(
                        "a sprite in frame {:?}; the camera is in {:?}",
                        s.pos.frame, fr
                    ),
                });
            }
            if !known(s.texture) {
                return Err(Error2d::invalid(format!(
                    "texture {} is not held by the renderer",
                    s.texture.0
                )));
            }
            let pl: ParallaxLayer = layer_of(s.layer);
            let at = s.pos.local + pl.view_offset(view.center);
            let reach = s.reach();
            let vis = Aabb2::new(view.center - half, view.center + half).inflate(reach);
            let (stats, visible) = (&mut self.stats, &mut self.visible);
            pl.for_each_copy(at, reach, view.center, half, |copy| {
                let p = at + copy;
                if vis.contains(p) {
                    visible.push((*s, p));
                } else {
                    stats.culled += 1;
                }
            });
        }
        for (i, (s, _)) in self.visible.iter().enumerate() {
            self.keys.push(DrawKey {
                layer: s.layer,
                order: s.order,
                kind: DrawKind::Sprite,
                texture: s.texture,
                index: i as u32,
            });
        }
        for (i, m) in frame.meshes.iter().enumerate() {
            if m.origin.frame != fr {
                return Err(Error2d::Frame {
                    why: format!(
                        "a mesh in frame {:?}; the camera is in {:?}",
                        m.origin.frame, fr
                    ),
                });
            }
            if !known(m.texture) {
                return Err(Error2d::invalid(format!(
                    "texture {} is not held by the renderer",
                    m.texture.0
                )));
            }
            if m.indices.iter().any(|x| *x as usize >= m.verts.len()) {
                return Err(Error2d::invalid("a mesh index is out of range"));
            }
            self.keys.push(DrawKey {
                layer: m.layer,
                order: m.order,
                kind: DrawKind::Mesh,
                texture: m.texture,
                index: i as u32,
            });
        }
        batch_into(&mut self.keys, split_all, &mut self.batches);
        // ---- write instances and mesh data in draw order ----
        for b in &self.batches {
            let keys = &self.keys[b.first as usize..(b.first + b.count) as usize];
            match b.kind {
                DrawKind::Sprite => {
                    let first = (self.instances.len() / INSTANCE_BYTES) as u32;
                    for k in keys {
                        let (s, p) = self.visible[k.index as usize];
                        write_instance(&mut self.instances, &view, &s, p);
                    }
                    self.draws.push(Draw {
                        kind: DrawKind::Sprite,
                        texture: b.texture,
                        first,
                        count: b.count,
                    });
                    self.stats.sprites += b.count;
                }
                DrawKind::Mesh => {
                    let first = self.mesh_indices.len() as u32;
                    for k in keys {
                        let m = &frame.meshes[k.index as usize];
                        let base = (self.mesh_vertices.len() / MESH_VERTEX_BYTES) as u32;
                        let pl = layer_of(m.layer);
                        let o = m.origin.local + pl.view_offset(view.center);
                        for v in &m.verts {
                            let px = view.px(o + v.offset);
                            self.mesh_vertices.extend_from_slice(&f(px.x));
                            self.mesh_vertices.extend_from_slice(&f(px.y));
                            self.mesh_vertices.extend_from_slice(&f(v.uv.x));
                            self.mesh_vertices.extend_from_slice(&f(v.uv.y));
                            self.mesh_vertices.extend_from_slice(&unorm4(v.color));
                        }
                        self.mesh_indices.extend(m.indices.iter().map(|i| i + base));
                        self.stats.meshes += 1;
                    }
                    let count = self.mesh_indices.len() as u32 - first;
                    self.draws.push(Draw {
                        kind: DrawKind::Mesh,
                        texture: b.texture,
                        first,
                        count,
                    });
                }
            }
        }
        self.stats.draw_calls = self.draws.len() as u32;
        self.mesh_index_bytes.clear();
        self.mesh_index_bytes
            .extend(self.mesh_indices.iter().flat_map(|i| i.to_le_bytes()));
        // ---- lights: shadow polygons in f64, then fans in pixels ----
        self.segments.clear();
        for o in &frame.occluders {
            if o.frame != fr {
                return Err(Error2d::Frame {
                    why: format!(
                        "an occluder in frame {:?}; the camera is in {:?}",
                        o.frame, fr
                    ),
                });
            }
            self.segments.extend(o.edges());
        }
        let screen = Aabb2::new(view.center - half, view.center + half);
        for l in &frame.lights {
            if l.pos.frame != fr {
                return Err(Error2d::Frame {
                    why: format!(
                        "a light in frame {:?}; the camera is in {:?}",
                        l.pos.frame, fr
                    ),
                });
            }
            if l.radius.is_nan()
                || l.radius <= 0.0
                || l.intensity <= 0.0
                || !screen.inflate(l.radius).contains(l.pos.local)
            {
                continue;
            }
            let c = l.pos.local;
            if l.shadows {
                visibility_with(&mut self.vis, c, l.radius, &self.segments, &mut self.fan);
            } else {
                visibility_with(&mut self.vis, c, l.radius, &[], &mut self.fan);
            }
            let cpx = view.px(c);
            let (dir, ci, co, spot) = match l.shape {
                LightShape::Point => (DVec2::X, -1.0, -1.0, 0.0),
                LightShape::Spot {
                    direction,
                    inner,
                    outer,
                } => {
                    let r = crate::math::Rot2::from_angle(direction);
                    let (ci, co) = (
                        forge_num::det::cos(inner.min(outer)),
                        forge_num::det::cos(outer.max(inner)),
                    );
                    (DVec2::new(r.c, r.s), ci, co, 1.0)
                }
            };
            let vtx = |p: DVec2, out: &mut Vec<u8>| {
                let q = view.px(p);
                for v in [
                    q.x,
                    q.y,
                    cpx.x,
                    cpx.y,
                    l.color[0] * l.intensity,
                    l.color[1] * l.intensity,
                    l.color[2] * l.intensity,
                    l.radius * view.scale,
                    dir.x,
                    dir.y,
                    ci,
                    co,
                    l.height * view.scale,
                    spot,
                    0.0,
                    0.0,
                ] {
                    out.extend_from_slice(&f(v));
                }
            };
            let n = self.fan.len();
            for i in 0..n {
                let (a, b) = (self.fan[i], self.fan[(i + 1) % n]);
                vtx(c, &mut self.light_vertices);
                vtx(c + a, &mut self.light_vertices);
                vtx(c + b, &mut self.light_vertices);
            }
            self.stats.lights += 1;
            self.stats.light_triangles += n as u32;
        }
        Ok(())
    }
}

fn write_instance(out: &mut Vec<u8>, view: &View, s: &Sprite, at: DVec2) {
    let c = view.px(at);
    let rot = crate::math::Rot2::from_angle(s.angle);
    // Axes in pixels (y down): the sprite's x and y axes scaled by its size.
    let ax = rot.apply(DVec2::new(s.size.x, 0.0)) * view.scale;
    let ay = rot.apply(DVec2::new(0.0, s.size.y)) * view.scale;
    for v in [
        c.x, c.y, ax.x, -ax.y, ay.x, -ay.y, s.pivot.x, s.pivot.y, s.uv.u0, s.uv.v0, s.uv.u1,
        s.uv.v1,
    ] {
        out.extend_from_slice(&f(v));
    }
    out.extend_from_slice(&unorm4(s.tint));
    let flags = u32::from(s.flip_x) | (u32::from(s.flip_y) << 1);
    out.extend_from_slice(&flags.to_le_bytes());
}

/// The frame uniform block (see `frame.wgsl`): render size, output size, ambient, clear,
/// viewport (x, y, scale, 0), light repeats.
#[must_use]
pub fn frame_uniform(
    render: (u32, u32),
    out: (u32, u32),
    ambient: [f64; 3],
    clear: [f64; 4],
    vp: (i64, i64, u32),
    light_repeats: u32,
) -> Vec<u8> {
    let mut b = Vec::with_capacity(96);
    for v in [
        f64::from(render.0),
        f64::from(render.1),
        f64::from(out.0),
        f64::from(out.1),
    ] {
        b.extend_from_slice(&f(v));
    }
    for v in [ambient[0], ambient[1], ambient[2], 1.0] {
        b.extend_from_slice(&f(v));
    }
    for v in clear {
        b.extend_from_slice(&f(v));
    }
    for v in [vp.0 as f64, vp.1 as f64, f64::from(vp.2), 0.0] {
        b.extend_from_slice(&f(v));
    }
    for v in [light_repeats, 0, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera2d;
    use crate::light::{Light2d, Occluder};
    use forge_frames::{FrameId, FramePos2};

    fn frame() -> Frame2d {
        let cam =
            Camera2d::pixel_perfect(FramePos2::new(FrameId(0), DVec2::ZERO), (320, 180), 16.0);
        let mut f = Frame2d::new(cam);
        for i in 0..100 {
            f.sprites.push(Sprite::new(
                FramePos2::new(FrameId(0), DVec2::new(f64::from(i) - 50.0, 0.0)),
                DVec2::new(1.0, 1.0),
                TextureId(u32::from(i % 2 == 0)),
            ));
        }
        f
    }

    #[test]
    fn culling_and_batching_and_byte_layout() {
        let f = frame();
        let mut p = Prepared::default();
        p.fill(&f, 320, 180, &|_| true, false).expect("fill");
        // View half-width 10 units: x in [-10, 10] plus reach.
        assert!(
            p.stats.sprites >= 20 && p.stats.sprites <= 23,
            "{:?}",
            p.stats
        );
        assert_eq!(p.stats.sprites + p.stats.culled, 100);
        assert_eq!(p.draws.len(), 2, "two textures, one layer: two draws");
        assert_eq!(p.instances.len(), p.stats.sprites as usize * INSTANCE_BYTES);
        let mut q = Prepared::default();
        q.fill(&f, 320, 180, &|_| true, true).expect("fill");
        assert_eq!(
            q.draws.len(),
            q.stats.sprites as usize,
            "the control: no batching"
        );
    }

    #[test]
    fn foreign_frames_and_unknown_textures_are_refused() {
        let mut f = frame();
        let mut p = Prepared::default();
        assert!(p.fill(&f, 320, 180, &|t| t.0 == 0, false).is_err());
        f.sprites[0].pos.frame = FrameId(9);
        let e = p.fill(&f, 320, 180, &|_| true, false).expect_err("frame");
        assert_eq!(e.code().as_str(), "TWOD-0002");
    }

    #[test]
    fn a_shadowed_light_is_a_fan_of_its_visibility_polygon() {
        let mut f = frame();
        f.sprites.clear();
        f.lights.push(Light2d::point(
            FramePos2::new(FrameId(0), DVec2::ZERO),
            5.0,
            1.0,
        ));
        f.occluders.push(Occluder::rect(
            FrameId(0),
            DVec2::new(1.0, -1.0),
            DVec2::new(2.0, 1.0),
        ));
        let mut p = Prepared::default();
        p.fill(&f, 320, 180, &|_| true, false).expect("fill");
        assert_eq!(p.stats.lights, 1);
        assert!(p.stats.light_triangles > 32);
        assert_eq!(
            p.light_vertices.len(),
            p.stats.light_triangles as usize * 3 * LIGHT_VERTEX_BYTES
        );
    }
}
