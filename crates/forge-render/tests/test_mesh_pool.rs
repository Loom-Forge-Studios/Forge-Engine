//! `test_mesh_pool` (WP-U21; gate row C-render-mesh-pool): a pool of equal-sized meshes is
//! drawn in one instanced draw per index list per pass, and draws exactly what the same
//! meshes draw one by one.
//!
//! A field of 400 wavy grid patches (17 x 17 vertices, as a streamed terrain's), each with
//! its own heights, two triangulations between them and four materials, lit by a sun that
//! casts shadows (so the cascades draw them too), is rendered twice on this machine's GPU:
//! once with every patch a mesh of its own, once with every patch in one mesh pool.
//!
//! * **The same image.** Every pixel of the pooled frame is within one 8-bit step of the
//!   per-mesh frame (the vertex stage reads the same `f32` vertices from the arena the vertex
//!   buffers hold), and the patches cover a good part of the frame (not vacuous).
//! * **Batched.** The pooled frame's depth and shadow passes draw the field in at most one
//!   call per index list per pass; the per-mesh frame takes one per patch per pass.
//! * **Slots are reused.** Removing every pooled patch and adding them again hands out the
//!   same arena slots: the arena does not grow.
//!
//! * **No allocation per patch** (WP-U21 verifier follow-up,
//!   `a_pooled_fields_steady_frames_allocate_nothing_per_patch`): a pooled field's steady
//!   frames — ten preparations recycled, and ten whole `render_scene` frames — counted with
//!   `allocation-counter`: forge-render's own frame work (preparation, the pooled draw groups
//!   included) allocates nothing, and a whole frame, whose wgpu command encoding and
//!   submission allocate on their own, allocates no more for 576 patches than for 64
//!   (measured: 176 a frame at both, every frame).
//!
//! Positive controls (W2): `positive_control_pooled_instances_ignoring_their_slot_differ` —
//! every pooled instance reads the arena's first slot (a hidden knob): the image differs, so
//! the comparison sees each patch's own vertices; `positive_control_an_allocation_per_instance_is_caught`
//! — one allocation per instance a frame uploads (a hidden knob) fails the allocation guard.

mod common;

use forge_frames::{DVec3, FramePos, Tick};
use forge_render::{
    Camera, DirectionalLight, Instance, Material, MeshData, MeshId, RenderOptions, Renderer, Scene,
    Vertex,
};

const ROW: &str = "C-render-mesh-pool";
const G: u32 = 17;
const SIDE: u32 = 20;
const W: u32 = 320;
const H: u32 = 240;

/// Patch `k`'s vertices: a 1 m grid square with its own waves.
fn patch(k: u32) -> Vec<Vertex> {
    let f = f64::from(k);
    let mut v = Vec::with_capacity((G * G) as usize);
    for j in 0..G {
        for i in 0..G {
            let (x, y) = (
                f64::from(i) / f64::from(G - 1),
                f64::from(j) / f64::from(G - 1),
            );
            let z = 0.12 * ((x * 7.0 + f * 0.9).sin() * (y * 5.0 + f * 0.37).cos());
            let dzdx = 0.12 * 7.0 * (x * 7.0 + f * 0.9).cos() * (y * 5.0 + f * 0.37).cos();
            let dzdy = -0.12 * 5.0 * (x * 7.0 + f * 0.9).sin() * (y * 5.0 + f * 0.37).sin();
            let n = DVec3::new(-dzdx, -dzdy, 1.0);
            v.push(Vertex {
                local: DVec3::new(x, y, z),
                normal: n / n.length(),
                uv: [x, y],
            });
        }
    }
    v
}

/// The two triangulations (the diagonal one way or the other in every cell).
fn topology(flip: bool) -> Vec<u32> {
    let mut ix = Vec::new();
    let at = |i: u32, j: u32| j * G + i;
    for j in 0..G - 1 {
        for i in 0..G - 1 {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            if flip {
                ix.extend([a, b, d, b, c, d]);
            } else {
                ix.extend([a, b, c, a, c, d]);
            }
        }
    }
    ix
}

/// What one render drew.
struct Frame {
    img: Vec<u8>,
    /// Draw calls of the depth shells and the cascades.
    pass_draws: usize,
    arena: u64,
}

fn render(pooled: bool, ignore_base: bool) -> Option<Result<Frame, String>> {
    let pool = common::pool(ROW)?;
    let dev = pool.primary();
    Some(run(dev, pooled, ignore_base))
}

fn run(dev: &forge_gpu::GpuDevice, pooled: bool, ignore_base: bool) -> Result<Frame, String> {
    let mut r = Renderer::new(dev, RenderOptions::new(W, H)).map_err(|e| e.to_string())?;
    if ignore_base {
        r.pool_ignores_base_for_tests();
    }
    let (tree, root) = common::tree();
    let mats: Vec<_> = (0..4)
        .map(|i| {
            let f = f64::from(i) / 3.0;
            r.add_material(Material {
                base_color: [0.2 + 0.7 * f, 0.6, 0.9 - 0.6 * f, 1.0],
                roughness: 0.6,
                ..Material::default()
            })
        })
        .collect();
    let tops = [topology(false), topology(true)];
    let mut meshes: Vec<MeshId> = Vec::new();
    let add = |r: &mut Renderer, meshes: &mut Vec<MeshId>| -> Result<(), String> {
        let p = if pooled {
            let p = r
                .add_mesh_pool(G * G, SIDE * SIDE)
                .map_err(|e| e.to_string())?;
            for t in &tops {
                r.add_pool_topology(p, t).map_err(|e| e.to_string())?;
            }
            Some(p)
        } else {
            None
        };
        for k in 0..SIDE * SIDE {
            let t = (k % 2) as usize;
            let v = patch(k);
            let id = match p {
                Some(p) => r.add_pooled_mesh(p, t as u32, &v),
                None => r.add_mesh(&MeshData {
                    vertices: v,
                    indices: tops[t].clone(),
                }),
            }
            .map_err(|e| e.to_string())?;
            meshes.push(id);
        }
        Ok(())
    };
    add(&mut r, &mut meshes)?;
    let mut arena = r.arena_vertices();
    if pooled {
        // Every slot back, and handed out again: the arena does not grow.
        for id in meshes.drain(..) {
            r.remove_mesh(id).map_err(|e| e.to_string())?;
        }
        let p = forge_render::MeshPoolId(0);
        for k in 0..SIDE * SIDE {
            meshes.push(
                r.add_pooled_mesh(p, k % 2, &patch(k))
                    .map_err(|e| e.to_string())?,
            );
        }
        if r.arena_vertices() != arena {
            return Err(format!(
                "re-adding the pool's meshes grew the arena {arena} -> {}",
                r.arena_vertices()
            ));
        }
        arena = r.arena_vertices();
    }
    // The field: SIDE x SIDE patches 1.25 m apart (gaps between them: no depth ties).
    let instances = meshes
        .iter()
        .enumerate()
        .map(|(k, id)| {
            let (i, j) = ((k as u32 % SIDE) as f64, (k as u32 / SIDE) as f64);
            let pos = FramePos::new(root, DVec3::new(i * 1.25 - 12.5, j * 1.25 - 12.5, 0.0));
            Instance::new(pos, *id, mats[k % 4])
        })
        .collect();
    let scene = Scene {
        instances,
        sun: Some(DirectionalLight {
            frame: root,
            toward_light: DVec3::new(0.3, -0.4, 0.87),
            color: [1.0, 0.97, 0.92],
            illuminance: 20.0,
            casts_shadows: true,
        }),
        ambient: [0.3, 0.3, 0.35],
        ev100: 3.0,
        ..Scene::default()
    };
    let cam_pos = DVec3::new(0.0, -26.0, 20.0);
    let camera = Camera::looking(
        FramePos::new(root, cam_pos),
        -cam_pos / cam_pos.length(),
        DVec3::Z,
        0.9,
    )
    .ok_or("camera")?;
    let target = common::target(dev, W, H);
    let f = r
        .prepare(&scene, &camera, &tree, Tick(0))
        .map_err(|e| e.to_string())?;
    let pass_draws = f
        .passes
        .iter()
        .chain(&f.cascades)
        .map(|p| p.draws.len())
        .sum();
    r.render(dev, &f, &target).map_err(|e| e.to_string())?;
    r.recycle(f);

    Ok(Frame {
        img: common::read(dev, &target),
        pass_draws,
        arena,
    })
}

/// Compare the pooled frame with the per-mesh one (`ignore_base`: the control).
fn compare(ignore_base: bool) -> Option<Result<String, String>> {
    let each = match render(false, false)? {
        Ok(f) => f,
        Err(e) => return Some(Err(e)),
    };
    let pooled = match render(true, ignore_base)? {
        Ok(f) => f,
        Err(e) => return Some(Err(e)),
    };
    Some(check(&each, &pooled))
}

fn check(each: &Frame, pooled: &Frame) -> Result<String, String> {
    let px = (W * H) as usize;
    let lit = each
        .img
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0].max(p[1]).max(p[2]) > 8)
        .count();
    if lit < px / 5 {
        return Err(format!("the field covers {lit} of {px} pixels: vacuous"));
    }
    let off: Vec<usize> = each
        .img
        .iter()
        .zip(&pooled.img)
        .enumerate()
        .filter(|(_, (a, b))| a.abs_diff(**b) > 1)
        .map(|(i, _)| i / 4)
        .collect();
    if !off.is_empty() {
        return Err(format!(
            "the pooled frame differs from the per-mesh frame at {} pixels (first at {:?})",
            off.len(),
            off.first().map(|i| (i % W as usize, i / W as usize))
        ));
    }
    let patches = (SIDE * SIDE) as usize;
    if each.pass_draws < patches {
        return Err(format!(
            "the per-mesh frame drew in {} calls: fewer than its {patches} patches",
            each.pass_draws
        ));
    }
    // Two index lists; the three depth shells and four cascades at most.
    if pooled.pass_draws > 2 * 7 {
        return Err(format!(
            "the pooled frame drew {patches} patches in {} calls (at most two a pass)",
            pooled.pass_draws
        ));
    }
    Ok(format!(
        "{lit} px lit; draws per frame {} per-mesh vs {} pooled; arena {} vertices",
        each.pass_draws, pooled.pass_draws, pooled.arena
    ))
}

#[test]
fn a_mesh_pool_draws_what_its_meshes_draw_in_a_call_per_index_list() {
    match compare(false) {
        None => {}
        Some(Ok(m)) => eprintln!("mesh pool: {m}"),
        Some(Err(e)) => panic!("{e}"),
    }
}

#[test]
fn positive_control_pooled_instances_ignoring_their_slot_differ() {
    match compare(true) {
        None => {}
        Some(Ok(m)) => panic!("instances reading the arena's first slot must differ: {m}"),
        Some(Err(e)) => assert!(e.contains("differs from the per-mesh frame"), "{e}"),
    }
}

/// A pooled field of `side` x `side` patches (as `run`'s), ready to render.
struct Field {
    r: Renderer,
    scene: Scene,
    camera: Camera,
}

fn field(
    dev: &forge_gpu::GpuDevice,
    side: u32,
    root: forge_frames::FrameId,
) -> Result<Field, String> {
    let mut r = Renderer::new(dev, RenderOptions::new(W, H)).map_err(|e| e.to_string())?;
    let mat = r.add_material(Material::default());
    let p = r
        .add_mesh_pool(G * G, side * side)
        .map_err(|e| e.to_string())?;
    for flip in [false, true] {
        r.add_pool_topology(p, &topology(flip))
            .map_err(|e| e.to_string())?;
    }
    let mut instances = Vec::new();
    for k in 0..side * side {
        let id = r
            .add_pooled_mesh(p, k % 2, &patch(k))
            .map_err(|e| e.to_string())?;
        let (i, j) = (f64::from(k % side), f64::from(k / side));
        let at = FramePos::new(root, DVec3::new(i * 1.25 - 12.5, j * 1.25 - 12.5, 0.0));
        instances.push(Instance::new(at, id, mat));
    }
    let scene = Scene {
        instances,
        sun: Some(DirectionalLight {
            frame: root,
            toward_light: DVec3::new(0.3, -0.4, 0.87),
            color: [1.0, 0.97, 0.92],
            illuminance: 20.0,
            casts_shadows: true,
        }),
        ambient: [0.3, 0.3, 0.35],
        ev100: 3.0,
        ..Scene::default()
    };
    let cam_pos = DVec3::new(0.0, -26.0, 20.0);
    let camera = Camera::looking(
        FramePos::new(root, cam_pos),
        -cam_pos / cam_pos.length(),
        DVec3::Z,
        0.9,
    )
    .ok_or("camera")?;
    Ok(Field { r, scene, camera })
}

/// What a pooled field's steady frames allocate: ten preparations (recycled), and each of
/// ten whole `render_scene` frames.
struct Steady {
    patches: usize,
    prepare: u64,
    frames: Vec<u64>,
}

fn steady(dev: &forge_gpu::GpuDevice, side: u32, fault: bool) -> Result<Steady, String> {
    let (tree, root) = common::tree();
    let mut f = field(dev, side, root)?;
    if fault {
        f.r.alloc_per_instance_for_tests();
    }
    let target = common::target(dev, W, H);
    for _ in 0..5 {
        f.r.render_scene(dev, &f.scene, &f.camera, &tree, Tick(0), &target)
            .map_err(|e| e.to_string())?;
        dev.wait_idle().map_err(|e| e.to_string())?;
    }
    let mut err = None;
    let prepare = allocation_counter::measure(|| {
        for _ in 0..10 {
            match f.r.prepare(&f.scene, &f.camera, &tree, Tick(0)) {
                Ok(p) => f.r.recycle(p),
                Err(e) => err = Some(e.to_string()),
            }
        }
    })
    .count_total;
    let mut frames = Vec::new();
    for _ in 0..10 {
        let n = allocation_counter::measure(|| {
            if let Err(e) =
                f.r.render_scene(dev, &f.scene, &f.camera, &tree, Tick(0), &target)
            {
                err = Some(e.to_string());
            }
        })
        .count_total;
        frames.push(n);
        dev.wait_idle().map_err(|e| e.to_string())?;
    }
    if let Some(e) = err {
        return Err(e);
    }
    Ok(Steady {
        patches: (side * side) as usize,
        prepare,
        frames,
    })
}

/// The guard's verdict on a small and a large pooled field.
fn allocation_verdict(small: &Steady, large: &Steady) -> Result<String, String> {
    for s in [small, large] {
        if s.prepare != 0 {
            return Err(format!(
                "ten steady preparations of {} pooled patches allocated {} times (forge-render's \
                 own frame work must allocate nothing)",
                s.patches, s.prepare
            ));
        }
    }
    let (lo, hi) = (
        small.frames.iter().min().copied().unwrap_or(0),
        large.frames.iter().min().copied().unwrap_or(0),
    );
    if hi > lo {
        return Err(format!(
            "a steady frame of {} pooled patches allocated {hi} times, of {} patches {lo}: \
             frame work grows with the patches (per frame {:?} vs {:?})",
            large.patches, small.patches, large.frames, small.frames
        ));
    }
    Ok(format!(
        "steady frames of {} and {} pooled patches: preparation 0 allocations; whole frames \
         (wgpu's encoding and submission included) {:?} and {:?}",
        small.patches, large.patches, small.frames, large.frames
    ))
}

fn allocations(fault: bool) -> Option<Result<String, String>> {
    let pool = common::pool(ROW)?;
    let dev = pool.primary();
    let run = || -> Result<String, String> {
        let small = steady(dev, 8, fault)?;
        let large = steady(dev, 24, fault)?;
        allocation_verdict(&small, &large)
    };
    Some(run())
}

/// A pooled field's steady frames: forge-render's own frame work (preparation, including
/// the pooled draw groups) allocates nothing, and a whole `render_scene` frame — wgpu's
/// command encoding and submission, which allocate on their own, included — allocates no
/// more for 576 patches than for 64 (its allocations are a per-frame floor, not per patch).
#[test]
fn a_pooled_fields_steady_frames_allocate_nothing_per_patch() {
    match allocations(false) {
        None => {}
        Some(Ok(m)) => eprintln!("mesh pool allocations: {m}"),
        Some(Err(e)) => panic!("{e}"),
    }
}

/// W2: one allocation per instance a frame uploads (a hidden knob) fails the guard.
#[test]
fn positive_control_an_allocation_per_instance_is_caught() {
    match allocations(true) {
        None => {}
        Some(Ok(m)) => panic!("an allocation per instance must fail the guard: {m}"),
        Some(Err(e)) => assert!(e.contains("allocated"), "{e}"),
    }
}
