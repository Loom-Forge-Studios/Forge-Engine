//! Frame preparation (CPU): resolve the scene into the camera frame in `f64`, cull, assign
//! depth passes, bin lights, fit shadow cascades, and pack the GPU buffers — narrowing through
//! [`last_mile`](crate::last_mile) only. The result, a [`PreparedFrame`], is plain data: it
//! is what the GPU sees, and the CPU-side guards inspect it without a device.

use forge_frames::{DQuat, DVec3, FrameResolver, Tick};
use forge_num::det;

use crate::camera::Camera;
use crate::cluster::{ClusterGrid, ClusterScratch, ClusterView, Clusters, LightSphere};
use crate::depth::{self, DepthSetup, DepthSpan, MAX_DEPTH_PASSES, ViewDepth};
use crate::error::RenderError;
use crate::last_mile::{self, CameraView, ViewOffset, narrow};
use crate::scene::{Material, Scene, SkyPoint, SkyShine};
use crate::shadow::{self, Cascade, CascadeSettings};

/// Words in the frame uniform (480 bytes).
pub(crate) const FRAME_WORDS: usize = 120;
/// Words per pass-uniform block (256 bytes: the dynamic-offset alignment).
pub(crate) const PASS_BLOCK_WORDS: usize = 64;
/// Pass blocks: 3 depth passes, 4 cascades, 1 sky.
pub(crate) const PASS_BLOCKS: usize = 8;
pub(crate) const CASCADE_BLOCK: usize = 3;
pub(crate) const SKY_BLOCK: usize = 7;
/// Words per instance record (128 bytes).
pub(crate) const INSTANCE_WORDS: usize = 32;
/// Words per light record (64 bytes).
pub(crate) const LIGHT_WORDS: usize = 16;
/// Words per sprite record (64 bytes).
pub(crate) const SPRITE_WORDS: usize = 16;
/// Words per sky point record (32 bytes).
pub(crate) const POINT_WORDS: usize = 8;
/// Sprite kinds (the shader's `flux.w`).
const SPRITE_LEGACY: f64 = 0.0;
const SPRITE_EMITTER: f64 = 1.0;
const SPRITE_LIT: f64 = 2.0;
/// Words per material record (48 bytes).
pub(crate) const MATERIAL_WORDS: usize = 12;

/// Draw order inside a depth pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawOrder {
    /// Group by mesh, nearest group first, instances front to back inside a group — early
    /// depth rejection discards hidden fragments before shading.
    #[default]
    FrontToBack,
    /// Scene order, one draw per run of equal meshes (for tests that fix draw order).
    Submission,
}

/// Draw groups at and above this are a mesh pool's topology (WP-U21): every instance of a
/// pooled mesh drawn with the same index list is one instanced draw, its vertices pulled
/// from the pool by the instance's vertex base. Below it, a group is a mesh id.
pub const POOLED_DRAW: u32 = 1 << 24;

/// A mesh's object-space bounding sphere, and how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MeshBounds {
    pub(crate) center: DVec3,
    pub(crate) radius: f64,
    /// Its draw group: the mesh id, or [`POOLED_DRAW`] plus its pool topology's group.
    pub(crate) group: u32,
    /// A pooled mesh's first vertex in the renderer's vertex arena (0 for a mesh of its own).
    pub(crate) base: u32,
    /// Its pool (`u32::MAX`: a mesh of its own).
    pub(crate) pool: u32,
}

/// One indexed draw: `count` instances of draw group `mesh` starting at `first` in the
/// draw-index list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// Draw group: a mesh index, or (at and above [`POOLED_DRAW`]) a mesh pool's topology,
    /// whose instances may be many meshes of the pool.
    pub mesh: u32,
    /// First entry in the draw-index list.
    pub first: u32,
    /// Instances.
    pub count: u32,
}

/// The draws of one pass.
#[derive(Clone, Debug, PartialEq)]
pub struct PassDraws {
    /// The depth pass: its range and names (for a cascade pass: the nearest depth pass, and
    /// `cascade` is set).
    pub span: DepthSpan,
    /// Cascade index, for a shadow pass.
    pub cascade: Option<usize>,
    /// Pass-uniform block index.
    pub block: usize,
    /// Draws.
    pub draws: Vec<Draw>,
    /// Whether the pass samples the shadow cascades.
    pub samples_shadows: bool,
}

impl PassDraws {
    /// Instances drawn.
    #[must_use]
    pub fn instances(&self) -> u32 {
        self.draws.iter().map(|d| d.count).sum()
    }
}

/// What preparation found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrepareStats {
    /// Scene instances.
    pub instances: usize,
    /// Culled (outside the frustum, behind the camera, nearer than 1 cm).
    pub culled: usize,
    /// Drawn as sky sprites (beyond the view depth's sprite distance).
    pub sprites: usize,
    /// Instance records uploaded (each visible instance once).
    pub uploaded: usize,
    /// Draw calls across every pass.
    pub draw_calls: usize,
    /// Punctual lights binned.
    pub lights: usize,
    /// Entries in the cluster light-index list.
    pub cluster_entries: usize,
    /// Sky sprites drawn (included in `sprites`).
    pub sky_sprites: usize,
    /// Sky points drawn.
    pub sky_points: usize,
    /// Whether the frame looks through an atmosphere.
    pub atmosphere: bool,
}

/// A sky sprite as uploaded (for inspection).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteRecord {
    /// Unit direction, view axes.
    pub dir: [f64; 3],
    /// True angular radius, pixels.
    pub radius_px: f64,
    /// Illuminance at the camera times exposure (per channel) — for a legacy sky
    /// instance, its radiance.
    pub flux: [f64; 3],
    /// 0 legacy disc, 1 emitter, 2 lit.
    pub kind: u32,
    /// Direction from the sprite toward its light (view axes), for a lit sprite.
    pub light: [f64; 3],
    /// The lit disc's normalisation ([`crate::SkyShine::Lit`]), for a lit sprite.
    pub phase_norm: f64,
}

/// A frame ready for the GPU: plain data, all of it camera-relative `f32`.
#[derive(Clone, Debug)]
pub struct PreparedFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) depth: ViewDepth,
    pub(crate) reversed: bool,
    pub(crate) frame_words: Vec<u32>,
    pub(crate) pass_words: Vec<u32>,
    pub(crate) instance_words: Vec<u32>,
    pub(crate) draw_index: Vec<u32>,
    pub(crate) light_words: Vec<u32>,
    pub(crate) sprite_words: Vec<u32>,
    pub(crate) background: [f64; 4],
    /// The sky pass draws the atmosphere's radiance (else it clears to `background`).
    pub(crate) sky_atmosphere: bool,
    /// Sky point records the sky pass draws.
    pub(crate) point_count: u32,
    /// Depth passes, back to front.
    pub passes: Vec<PassDraws>,
    /// Shadow cascade passes.
    pub cascades: Vec<PassDraws>,
    /// The cascades as fitted (camera-relative `f64`).
    pub cascade_fits: Vec<Cascade>,
    /// The light clusters.
    pub clusters: Clusters,
    scene_to_gpu: Vec<Option<u32>>,
    /// Counts.
    pub stats: PrepareStats,
}

impl PreparedFrame {
    /// The narrowed model-view matrix uploaded for scene instance `i` (column-major), if it
    /// is drawn in any pass.
    #[must_use]
    pub fn model_view(&self, i: usize) -> Option<[f32; 16]> {
        let g = (*self.scene_to_gpu.get(i)?)? as usize;
        let w = self
            .instance_words
            .get(g * INSTANCE_WORDS..g * INSTANCE_WORDS + 16)?;
        let mut m = [0.0; 16];
        for (o, b) in m.iter_mut().zip(w) {
            *o = f32::from_bits(*b);
        }
        Some(m)
    }

    /// The depth pass named `pass` (its render-graph name), if anything is drawn in it.
    #[must_use]
    pub fn pass(&self, pass: &str) -> Option<&PassDraws> {
        self.passes.iter().find(|p| p.span.pass == pass)
    }

    /// The view depth the frame was prepared in.
    #[must_use]
    pub fn view_depth(&self) -> ViewDepth {
        self.depth
    }

    /// Sky sprites.
    #[must_use]
    pub fn sprite_count(&self) -> usize {
        self.sprite_words.len() / SPRITE_WORDS
    }

    /// Sky sprite `i` as uploaded (widened back to `f64`).
    #[must_use]
    pub fn sprite(&self, i: usize) -> Option<SpriteRecord> {
        let w = self
            .sprite_words
            .get(i * SPRITE_WORDS..(i + 1) * SPRITE_WORDS)?;
        let f = |k: usize| f64::from(f32::from_bits(w[k]));
        Some(SpriteRecord {
            dir: [f(0), f(1), f(2)],
            radius_px: f(3),
            flux: [f(4), f(5), f(6)],
            kind: f(7) as u32,
            light: [f(8), f(9), f(10)],
            phase_norm: f(11),
        })
    }
}

struct Words<'a>(&'a mut Vec<u32>);

impl Words<'_> {
    fn f(&mut self, x: f64) {
        self.0.push(narrow(x).to_bits());
    }
    fn f4(&mut self, v: [f64; 4]) {
        for x in v {
            self.f(x);
        }
    }
    fn u4(&mut self, v: [u32; 4]) {
        self.0.extend_from_slice(&v);
    }
    fn m(&mut self, m: &[f32; 16]) {
        self.0.extend(m.iter().map(|x| x.to_bits()));
    }
}

/// Pack sky points for the sky pass: a point source, or a Gaussian of its angular radius.
/// Directions stay in the points' own axes; the frame uniform rotates them into view axes.
pub(crate) fn points_words(points: &[SkyPoint]) -> Vec<u32> {
    let mut out = Vec::with_capacity(points.len() * POINT_WORDS);
    let mut w = Words(&mut out);
    for p in points {
        w.f4([p.dir.x, p.dir.y, p.dir.z, p.lux]);
        w.f4([p.color[0], p.color[1], p.color[2], p.sigma]);
    }
    out
}

/// Pack one material record.
pub(crate) fn material_words(m: &Material, out: &mut Vec<u32>) {
    let mut w = Words(out);
    w.f4(m.base_color);
    w.f4([m.emissive[0], m.emissive[1], m.emissive[2], m.metallic]);
    w.f4([m.roughness, m.reflectance, 0.0, 0.0]);
}

/// One instance resolved into the camera frame.
#[derive(Clone, Copy, Debug)]
struct Resolved {
    offset: ViewOffset,
    rot: DQuat,
    center: DVec3,
    radius: f64,
}

/// Storage [`prepare`] keeps across frames (held by the renderer). Everything a frame needs
/// lives here between frames — the working lists (resolved instances, per-pass lists, the
/// per-mesh nearest-depth table, light spheres, binning scratch) and, once a frame is handed
/// back through [`crate::Renderer::recycle`], every buffer of that [`PreparedFrame`]. A
/// steady-state preparation therefore allocates **nothing** (`test_prepare_alloc`).
#[derive(Debug, Default)]
pub(crate) struct PrepareScratch {
    spheres: Vec<LightSphere>,
    binner: ClusterScratch,
    resolved: Vec<Option<Resolved>>,
    per_pass: Vec<Vec<(usize, f64)>>,
    casters: Vec<(usize, f64)>,
    nearest: Vec<f64>,
    spare_draws: Vec<Vec<Draw>>,
    // A recycled frame's buffers.
    frame_words: Vec<u32>,
    pass_words: Vec<u32>,
    instance_words: Vec<u32>,
    draw_index: Vec<u32>,
    light_words: Vec<u32>,
    sprite_words: Vec<u32>,
    passes: Vec<PassDraws>,
    cascades: Vec<PassDraws>,
    cascade_fits: Vec<Cascade>,
    scene_to_gpu: Vec<Option<u32>>,
    clusters: Clusters,
}

impl PrepareScratch {
    /// Take a rendered frame's buffers back for the next preparation.
    pub(crate) fn recycle(&mut self, f: PreparedFrame) {
        let PreparedFrame {
            frame_words,
            pass_words,
            instance_words,
            draw_index,
            light_words,
            sprite_words,
            mut passes,
            mut cascades,
            cascade_fits,
            clusters,
            scene_to_gpu,
            ..
        } = f;
        for p in passes.drain(..).chain(cascades.drain(..)) {
            self.spare_draws.push(p.draws);
        }
        self.frame_words = frame_words;
        self.pass_words = pass_words;
        self.instance_words = instance_words;
        self.draw_index = draw_index;
        self.light_words = light_words;
        self.sprite_words = sprite_words;
        self.passes = passes;
        self.cascades = cascades;
        self.cascade_fits = cascade_fits;
        self.clusters = clusters;
        self.scene_to_gpu = scene_to_gpu;
    }
}

/// A buffer out of the scratch, emptied (its capacity is kept).
fn reuse<T>(v: &mut Vec<T>) -> Vec<T> {
    let mut v = std::mem::take(v);
    v.clear();
    v
}

pub(crate) struct PrepareInput<'a> {
    pub width: u32,
    pub height: u32,
    pub depth: DepthSetup,
    pub order: DrawOrder,
    pub grid: ClusterGrid,
    pub shadows: Option<CascadeSettings>,
    /// Mesh slots (`None`: removed).
    pub meshes: &'a [Option<MeshBounds>],
    /// Mesh pool topologies (draw groups at and above [`POOLED_DRAW`]).
    pub groups: usize,
    /// W2 control of `test_mesh_pool`: every pooled instance reads the arena's first slot.
    pub pool_ignores_base: bool,
    /// W2 control of `test_mesh_pool`'s allocation guard: one allocation per instance uploaded.
    pub alloc_per_instance: bool,
    pub materials: &'a [Material],
    pub naive_last_mile: bool,
    /// The renderer holds atmosphere tables.
    pub has_atmosphere: bool,
    /// Records in the renderer's sky point buffer.
    pub points_len: u32,
    /// W2 control: sub-pixel sprites as hard discs.
    pub hard_disc_sprites: bool,
    /// W2 control of the perf gate: extra sky evaluations per pixel.
    pub sky_repeats: u32,
    /// W2 control of the clear-sky golden: an isotropic Rayleigh phase in the sky pass.
    pub isotropic_rayleigh: bool,
}

fn finite3(v: DVec3) -> bool {
    v.is_finite()
}

/// Uploads instances (each once) and appends draw-index entries.
struct Uploader<'a, 'v> {
    scene: &'a Scene,
    resolved: &'a [Option<Resolved>],
    naive: bool,
    view: &'a CameraView<'v>,
    scene_to_gpu: &'a mut [Option<u32>],
    instance_words: &'a mut Vec<u32>,
    draw_index: &'a mut Vec<u32>,
    /// Mesh slots: each instance's draw group and (pooled) vertex base.
    meshes: &'a [Option<MeshBounds>],
    /// W2 control: pooled instances read the arena's first slot.
    pool_ignores_base: bool,
    /// W2 control: one allocation per instance uploaded.
    alloc_per_instance: bool,
}

/// Scene instance `i`'s draw group (its mesh id, or its pool topology's group; the mesh was
/// checked to exist when the instance was resolved).
fn group_of(scene: &Scene, meshes: &[Option<MeshBounds>], i: usize) -> u32 {
    let m = scene.instances[i].mesh.0;
    meshes
        .get(m as usize)
        .and_then(Option::as_ref)
        .map_or(m, |b| b.group)
}

/// Where draw group `g` sits in a per-group table of `meshes` mesh slots then the pool
/// topologies.
fn slot_of(g: u32, meshes: usize) -> usize {
    if g >= POOLED_DRAW {
        meshes + (g - POOLED_DRAW) as usize
    } else {
        g as usize
    }
}

impl Uploader<'_, '_> {
    /// Scene instance `i`'s mesh slot.
    fn bounds(&self, i: usize) -> Option<&MeshBounds> {
        let m = self.scene.instances[i].mesh.0 as usize;
        self.meshes.get(m).and_then(Option::as_ref)
    }

    fn upload(&mut self, i: usize) -> Result<u32, RenderError> {
        if let Some(g) = self.scene_to_gpu[i] {
            return Ok(g);
        }
        let inst = &self.scene.instances[i];
        let Some(r) = &self.resolved[i] else {
            return Err(RenderError::scene("unresolved instance"));
        };
        let mv = if self.naive {
            last_mile::mutants::naive_model_view(self.view, inst.pos, r.rot, inst.scale)?
        } else {
            last_mile::model_view(r.rot, inst.scale, r.offset)
        };
        // `y`: a pooled mesh's first vertex in the arena (the pulled vertex stage reads it).
        let base = self
            .bounds(i)
            .map_or(0, |b| if self.pool_ignores_base { 0 } else { b.base });
        let mut w = Words(self.instance_words);
        w.m(&mv);
        w.0.extend(
            last_mile::normal_matrix(r.rot, inst.scale)
                .iter()
                .map(|x| x.to_bits()),
        );
        w.u4([inst.material.0, base, 0, 0]);
        if self.alloc_per_instance {
            std::hint::black_box(Box::new(i));
        }
        let g = (self.instance_words.len() / INSTANCE_WORDS - 1) as u32;
        self.scene_to_gpu[i] = Some(g);
        Ok(g)
    }

    /// Order `list` and append its draws to `draws`. `nearest` is a per-mesh table of
    /// `+inf`, handed back as it was found.
    fn build_draws(
        &mut self,
        list: &mut [(usize, f64)],
        order: DrawOrder,
        nearest: &mut [f64],
        draws: &mut Vec<Draw>,
    ) -> Result<(), RenderError> {
        let scene = self.scene;
        let meshes = self.meshes;
        let mesh_of = |i: usize| group_of(scene, meshes, i);
        // The nearest-depth table: mesh ids first, then the pool topologies' groups.
        let at = |g: u32| slot_of(g, meshes.len());
        if order == DrawOrder::FrontToBack {
            // Nearest instance of each mesh orders the groups; front to back inside. The
            // comparison is a total order (the index breaks ties), so an unstable sort —
            // which needs no buffer — gives the same order every time.
            for &(i, d) in list.iter() {
                let e = &mut nearest[at(mesh_of(i))];
                *e = e.min(d);
            }
            list.sort_unstable_by(|a, b| {
                let (ma, mb) = (mesh_of(a.0), mesh_of(b.0));
                nearest[at(ma)]
                    .total_cmp(&nearest[at(mb)])
                    .then(ma.cmp(&mb))
                    .then(a.1.total_cmp(&b.1))
                    .then(a.0.cmp(&b.0))
            });
            for &(i, _) in list.iter() {
                nearest[at(mesh_of(i))] = f64::INFINITY;
            }
        }
        for &(i, _) in list.iter() {
            let g = self.upload(i)?;
            let mesh = mesh_of(i);
            let slot = self.draw_index.len() as u32;
            self.draw_index.push(g);
            match draws.last_mut() {
                Some(d) if d.mesh == mesh && d.first + d.count == slot => d.count += 1,
                _ => draws.push(Draw {
                    mesh,
                    first: slot,
                    count: 1,
                }),
            }
        }
        Ok(())
    }
}

/// The scene's sky sprites, packed for the sky pass. Direction and angular radius come from
/// each sprite's `f64` camera-relative offset; its brightness is what the scene gives, times
/// the exposure; a lit sprite also carries the direction (view axes) from it toward its
/// light, so its disc can be shaded as a lit sphere once it is resolved.
fn sky_sprites(
    scene: &Scene,
    view: &CameraView<'_>,
    exposure: f64,
    pixel_angle: f64,
    words: &mut Vec<u32>,
    stats: &mut PrepareStats,
) -> Result<(), RenderError> {
    let mut w = Words(words);
    for (i, s) in scene.sky_sprites.iter().enumerate() {
        let (ill, light, norm) = match s.shine {
            SkyShine::Emitter { illuminance } => (illuminance, None, 0.0),
            SkyShine::Lit {
                illuminance,
                light,
                norm,
            } => (illuminance, Some(light), norm),
        };
        if !(s.radius.is_finite() && s.radius > 0.0)
            || !s.pos.local.is_finite()
            || ill.iter().any(|v| !v.is_finite())
            || !norm.is_finite()
        {
            return Err(RenderError::scene(format!(
                "sky sprite {i} has radius {} at {:?}, illuminance {ill:?}, norm {norm}",
                s.radius, s.pos
            )));
        }
        let o = view.offset(s.pos)?.0;
        let d = o.length();
        if d.is_nan() || d <= s.radius {
            // The camera is inside it: it is not in the sky from here.
            stats.culled += 1;
            continue;
        }
        let dir = o / d;
        let angle = det::atan2(s.radius, det::sqrt(d * d - s.radius * s.radius));
        let px = angle / pixel_angle;
        w.f4([dir.x, dir.y, dir.z, px]);
        match light {
            None => {
                w.f4([
                    ill[0] * exposure,
                    ill[1] * exposure,
                    ill[2] * exposure,
                    SPRITE_EMITTER,
                ]);
                w.f4([0.0; 4]);
            }
            Some(light) => {
                let l = match light {
                    Some(p) => {
                        let to_light = view.offset(p)?.0 - o;
                        let dl = to_light.length();
                        if dl.is_nan() || dl <= s.radius {
                            DVec3::ZERO
                        } else {
                            to_light / dl
                        }
                    }
                    None => DVec3::ZERO,
                };
                w.f4([
                    ill[0] * exposure,
                    ill[1] * exposure,
                    ill[2] * exposure,
                    SPRITE_LIT,
                ]);
                w.f4([l.x, l.y, l.z, norm]);
            }
        }
        w.f4([0.0; 4]);
        stats.sprites += 1;
        stats.sky_sprites += 1;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) fn prepare(
    input: &PrepareInput<'_>,
    scratch: &mut PrepareScratch,
    scene: &Scene,
    camera: &Camera,
    tree: &dyn FrameResolver,
    t: Tick,
) -> Result<PreparedFrame, RenderError> {
    let (w, h) = (input.width, input.height);
    let aspect = f64::from(w) / f64::from(h);
    let mut view = CameraView::new(tree, camera, t)?;
    let tan = camera.tan_half_fov();
    let layout = input.depth.resolve(tree.view_depth());
    if layout.passes.is_empty() || layout.passes.len() > MAX_DEPTH_PASSES {
        return Err(RenderError::scene(format!(
            "a view is drawn in 1 to {MAX_DEPTH_PASSES} depth passes, not {}",
            layout.passes.len()
        )));
    }
    let passes = layout.passes;
    let exposure = scene.exposure();

    // The frame's own buffers: a recycled frame's, emptied.
    let mut frame_words = reuse(&mut scratch.frame_words);
    let mut pass_words = reuse(&mut scratch.pass_words);
    let mut instance_words = reuse(&mut scratch.instance_words);
    let mut draw_index = reuse(&mut scratch.draw_index);
    let mut light_words = reuse(&mut scratch.light_words);
    let mut sprite_words = reuse(&mut scratch.sprite_words);
    let mut pass_draws = reuse(&mut scratch.passes);
    let mut cascade_draws = reuse(&mut scratch.cascades);
    let mut cascade_fits = reuse(&mut scratch.cascade_fits);
    let mut scene_to_gpu = reuse(&mut scratch.scene_to_gpu);
    let mut clusters = std::mem::take(&mut scratch.clusters);
    let PrepareScratch {
        spheres,
        binner,
        resolved,
        per_pass,
        casters,
        nearest,
        spare_draws,
        ..
    } = scratch;

    // Resolve every instance: view-space bounding sphere, rotation into view axes.
    resolved.clear();
    let mut stats = PrepareStats {
        instances: scene.instances.len(),
        ..PrepareStats::default()
    };
    for (i, inst) in scene.instances.iter().enumerate() {
        let mb = input
            .meshes
            .get(inst.mesh.0 as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                RenderError::scene(format!(
                    "instance {i} names mesh {} the renderer does not hold",
                    inst.mesh.0
                ))
            })?;
        if input.materials.get(inst.material.0 as usize).is_none() {
            return Err(RenderError::scene(format!(
                "instance {i} names material {} the renderer does not hold",
                inst.material.0
            )));
        }
        let s = inst.scale;
        if !finite3(s) || s.x == 0.0 || s.y == 0.0 || s.z == 0.0 {
            return Err(RenderError::scene(format!("instance {i} has scale {s:?}")));
        }
        if !finite3(inst.pos.local) || !inst.orientation.is_finite() {
            return Err(RenderError::scene(format!("instance {i} is not finite")));
        }
        let offset = view.offset(inst.pos)?;
        let rot = view.rotation_from(inst.pos.frame)? * inst.orientation;
        let local_c = DVec3::new(mb.center.x * s.x, mb.center.y * s.y, mb.center.z * s.z);
        let center = offset.0 + rot.rotate(local_c);
        let radius = mb.radius * s.x.abs().max(s.y.abs()).max(s.z.abs());
        resolved.push(Some(Resolved {
            offset,
            rot,
            center,
            radius,
        }));
    }

    // Assign to depth passes, or to the sky, or cull.
    if per_pass.len() < passes.len() {
        per_pass.resize_with(passes.len(), Vec::new);
    }
    for list in per_pass.iter_mut() {
        list.clear();
    }
    {
        let mut sprites = Words(&mut sprite_words);
        for (i, r) in resolved.iter().enumerate() {
            let Some(r) = r else { continue };
            let depth = -r.center.z;
            if depth + r.radius < layout.nearest()
                || depth::outside_sides(ViewOffset(r.center), r.radius, tan, aspect)
            {
                stats.culled += 1;
                continue;
            }
            if let Some(beyond) = layout.sprites_beyond
                && depth - r.radius >= beyond
            {
                // A mesh instance beyond the last depth pass: a flat disc sprite (at least 0.75 px,
                // so it always covers a pixel centre) of the material's colour. Physical
                // brightness and phase are what a `Scene::sky_sprites` entry carries.
                let dist = r.center.length();
                let dir = r.center / dist;
                let px = (r.radius / dist) / tan * f64::from(h) * 0.5;
                let m = &input.materials[scene.instances[i].material.0 as usize];
                let sun = scene.sun.map_or(0.0, |s| s.illuminance) / core::f64::consts::PI * 0.25;
                let c = |k: usize| {
                    (m.emissive[k] + m.base_color[k] * (scene.ambient[k] + sun)) * exposure
                };
                sprites.f4([dir.x, dir.y, dir.z, px]);
                sprites.f4([c(0), c(1), c(2), SPRITE_LEGACY]);
                sprites.f4([0.0; 4]);
                sprites.f4([0.0; 4]);
                stats.sprites += 1;
                continue;
            }
            let mask = depth::overlapping(passes, depth, r.radius);
            if mask == 0 {
                stats.culled += 1;
                continue;
            }
            for (p, list) in per_pass.iter_mut().take(passes.len()).enumerate() {
                if mask & (1 << p) != 0 {
                    list.push((i, depth));
                }
            }
        }
    }

    // Sky sprites.
    let pixel_angle = 2.0 * tan / f64::from(h);
    sky_sprites(
        scene,
        &view,
        exposure,
        pixel_angle,
        &mut sprite_words,
        &mut stats,
    )?;

    // Sun and cascades.
    let sun_view = match &scene.sun {
        Some(s) => {
            let d = view.direction(s.frame, s.toward_light)?;
            d.try_normalize().map(|d| (d, s))
        }
        None => None,
    };
    if let (Some((d, s)), Some(cs)) = (&sun_view, &input.shadows)
        && s.casts_shadows
        && cs.count > 0
    {
        let toward_frame = camera.orientation.rotate(*d);
        shadow::fit_into(cs, camera, aspect, toward_frame, true, &mut cascade_fits);
    }
    let max_shadow = input.shadows.map_or(0.0, |s| s.max_distance);
    // Cascade passes carry the nearest depth pass (they draw nothing in it).
    let nearest_span = passes[passes.len() - 1];

    // Upload each drawn instance once; draws index it.
    scene_to_gpu.resize(scene.instances.len(), None);
    if nearest.len() < input.meshes.len() + input.groups {
        nearest.resize(input.meshes.len() + input.groups, f64::INFINITY);
    }
    {
        let mut up = Uploader {
            scene,
            resolved,
            naive: input.naive_last_mile,
            view: &view,
            scene_to_gpu: &mut scene_to_gpu,
            instance_words: &mut instance_words,
            draw_index: &mut draw_index,
            meshes: input.meshes,
            pool_ignores_base: input.pool_ignores_base,
            alloc_per_instance: input.alloc_per_instance,
        };
        for (p, list) in per_pass.iter_mut().take(passes.len()).enumerate() {
            if list.is_empty() {
                continue;
            }
            let mut draws = spare_draws.pop().unwrap_or_default();
            draws.clear();
            up.build_draws(list, input.order, nearest, &mut draws)?;
            let span = passes[p];
            pass_draws.push(PassDraws {
                span,
                cascade: None,
                block: depth::pass_block(p, passes.len()),
                draws,
                samples_shadows: !cascade_fits.is_empty() && span.near < max_shadow,
            });
        }
        for (c, fit) in cascade_fits.iter().enumerate() {
            casters.clear();
            for (i, r) in up.resolved.iter().enumerate() {
                let Some(r) = r else { continue };
                if !scene.instances[i].casts_shadows {
                    continue;
                }
                let c_frame = ViewOffset(r.center).to_frame(camera.orientation);
                if fit.captures(c_frame, r.radius) {
                    casters.push((i, -r.center.z));
                }
            }
            // Depth order does not matter for a depth-only pass: group by mesh, one draw
            // each. (mesh, index) is unique, so the unstable sort is deterministic.
            casters.sort_unstable_by_key(|&(i, _)| (group_of(scene, input.meshes, i), i));
            let mut draws = spare_draws.pop().unwrap_or_default();
            draws.clear();
            up.build_draws(casters, DrawOrder::Submission, nearest, &mut draws)?;
            cascade_draws.push(PassDraws {
                span: nearest_span,
                cascade: Some(c),
                block: CASCADE_BLOCK + c,
                draws,
                samples_shadows: false,
            });
        }
    }

    // Lights: view-space spheres, binned; records narrowed.
    spheres.clear();
    {
        let mut lw = Words(&mut light_words);
        for (i, l) in scene.lights.iter().enumerate() {
            if !(l.range > 0.0 && l.range.is_finite() && l.intensity.is_finite()) {
                return Err(RenderError::scene(format!(
                    "light {i} has range {} / intensity {}",
                    l.range, l.intensity
                )));
            }
            let o = view.offset(l.pos)?;
            spheres.push(LightSphere {
                offset: o,
                radius: l.range,
            });
            lw.f4([o.0.x, o.0.y, o.0.z, l.range]);
            let kind = if l.spot.is_some() { 1.0 } else { 0.0 };
            lw.f4([
                l.color[0] * l.intensity,
                l.color[1] * l.intensity,
                l.color[2] * l.intensity,
                kind,
            ]);
            match &l.spot {
                Some(s) => {
                    let d = view
                        .direction(l.pos.frame, s.direction)?
                        .try_normalize()
                        .unwrap_or(DVec3::new(0.0, 0.0, -1.0));
                    let (co, ci) = (det::cos(s.outer_angle), det::cos(s.inner_angle));
                    let scale = 1.0 / (ci - co).max(1e-4);
                    lw.f4([d.x, d.y, d.z, scale]);
                    lw.f4([-co * scale, 0.0, 0.0, 0.0]);
                }
                None => {
                    lw.f4([0.0, 0.0, -1.0, 0.0]);
                    lw.f4([1.0, 0.0, 0.0, 0.0]);
                }
            }
        }
    }
    debug_assert_eq!(light_words.len(), scene.lights.len() * LIGHT_WORDS);
    let cview = ClusterView {
        width: w,
        height: h,
        tan_half_fov: tan,
    };
    // Bin into the buffers of a recycled frame (Renderer::recycle): no allocation once warm.
    input.grid.bin_into(&cview, spheres, binner, &mut clusters);
    stats.lights = spheres.len();
    stats.cluster_entries = clusters.indices.len();

    // The atmosphere: the camera relative to the ground sphere's centre (f64, then km), and the sky
    // points' axes in view axes.
    let atmo_camera = match (&scene.atmosphere, input.has_atmosphere) {
        (Some(a), true) => Some(-view.offset(a.centre)?.0 * 1e-3),
        _ => None,
    };
    let point_axes = match &scene.sky_points {
        Some(sf) if input.points_len > 0 => {
            if !(sf.brightness.is_finite() && sf.brightness >= 0.0) {
                return Err(RenderError::scene(format!(
                    "sky points brightness {}",
                    sf.brightness
                )));
            }
            Some((view.rotation_from(sf.frame)?, sf.brightness))
        }
        _ => None,
    };
    stats.atmosphere = atmo_camera.is_some();
    stats.sky_points = if point_axes.is_some() {
        input.points_len as usize
    } else {
        0
    };

    // Frame uniform.
    let (tw, th) = input.grid.tile_px(&cview);
    {
        let mut fw = Words(&mut frame_words);
        fw.f4([f64::from(w), f64::from(h), f64::from(tw), f64::from(th)]);
        fw.u4([input.grid.x, input.grid.y, input.grid.z, 0]);
        fw.f4([
            input.grid.near,
            input.grid.far,
            input.grid.slice_scale(),
            spheres.len() as f64,
        ]);
        match &sun_view {
            Some((d, s)) => {
                fw.f4([d.x, d.y, d.z, 1.0]);
                fw.f4([
                    s.color[0] * s.illuminance,
                    s.color[1] * s.illuminance,
                    s.color[2] * s.illuminance,
                    if cascade_fits.is_empty() { 0.0 } else { 1.0 },
                ]);
            }
            None => {
                fw.f4([0.0, 0.0, 1.0, 0.0]);
                fw.f4([0.0; 4]);
            }
        }
        fw.f4([
            scene.ambient[0],
            scene.ambient[1],
            scene.ambient[2],
            exposure,
        ]);
        let mut splits = [0.0; 4];
        let mut texels = [0.0; 4];
        for (k, c) in cascade_fits.iter().enumerate() {
            splits[k] = c.split;
            texels[k] = c.texel;
        }
        fw.f4(splits);
        fw.f4(texels);
        for k in 0..4 {
            match cascade_fits.get(k) {
                Some(c) => fw.m(&last_mile::mat4(&c.matrix_from_view(camera.orientation))),
                None => fw.m(&[0.0; 16]),
            }
        }
        fw.u4([
            cascade_fits.len() as u32,
            u32::from(input.isotropic_rayleigh),
            0,
            0,
        ]);
        match atmo_camera {
            Some(c) => fw.f4([c.x, c.y, c.z, 1.0]),
            None => fw.f4([0.0; 4]),
        }
        fw.f4([
            pixel_angle * pixel_angle,
            tan,
            aspect,
            stats.sky_points as f64,
        ]);
        let (q, brightness) = point_axes.unwrap_or((DQuat::IDENTITY, 0.0));
        let (gx, gy, gz) = (q.rotate(DVec3::X), q.rotate(DVec3::Y), q.rotate(DVec3::Z));
        fw.f4([gx.x, gx.y, gx.z, brightness]);
        fw.f4([
            gy.x,
            gy.y,
            gy.z,
            if input.hard_disc_sprites { 1.0 } else { 0.0 },
        ]);
        fw.f4([gz.x, gz.y, gz.z, f64::from(input.sky_repeats)]);
    }
    debug_assert_eq!(frame_words.len(), FRAME_WORDS);

    // Pass uniforms.
    pass_words.resize(PASS_BLOCKS * PASS_BLOCK_WORDS, 0);
    let reversed = input.depth.reversed;
    let pw = &mut pass_words;
    let mut put = |block: usize, m: &[f32; 16], near: f64, far: f64| {
        let base = block * PASS_BLOCK_WORDS;
        for (k, x) in m.iter().enumerate() {
            pw[base + k] = x.to_bits();
        }
        pw[base + 16] = narrow(near).to_bits();
        pw[base + 17] = narrow(far).to_bits();
        pw[base + 18] = narrow(if reversed { 1.0 } else { 0.0 }).to_bits();
    };
    for p in &pass_draws {
        let m = last_mile::mat4(&depth::projection(
            tan,
            aspect,
            p.span.near,
            p.span.far,
            reversed,
        ));
        put(p.block, &m, p.span.near, p.span.far);
    }
    for (c, fit) in cascade_fits.iter().enumerate() {
        let m = last_mile::mat4(&fit.matrix_from_view(camera.orientation));
        put(CASCADE_BLOCK + c, &m, 0.0, 1.0);
    }
    let sky = last_mile::mat4(&depth::projection(tan, aspect, 1.0, 10.0, true));
    put(SKY_BLOCK, &sky, 1.0, 10.0);

    stats.uploaded = instance_words.len() / INSTANCE_WORDS;
    stats.draw_calls = pass_draws
        .iter()
        .chain(cascade_draws.iter())
        .map(|p| p.draws.len())
        .sum::<usize>()
        + usize::from(stats.sprites > 0)
        + usize::from(stats.sky_points > 0)
        + usize::from(stats.atmosphere);

    let bg = scene.background;
    Ok(PreparedFrame {
        width: w,
        height: h,
        depth: layout,
        reversed: input.depth.reversed,
        frame_words,
        pass_words,
        instance_words,
        draw_index,
        light_words,
        sprite_words,
        background: [bg[0] * exposure, bg[1] * exposure, bg[2] * exposure, 1.0],
        sky_atmosphere: stats.atmosphere,
        point_count: stats.sky_points as u32,
        passes: pass_draws,
        cascades: cascade_draws,
        cascade_fits,
        clusters,
        scene_to_gpu,
        stats,
    })
}
