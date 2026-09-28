//! `test_last_mile` — f64 camera frame -> f32 camera-relative at the last mile only (Ch.2.4,
//! DoD M1-3); no precision shimmer 6.371e6 m from the world origin.
//!
//! * **CPU.** A camera stands 6.371e6 m from the world origin (on no axis) and walks 2,000 steps of 1 mm past a cube 3 m away. At every
//!   step each cube corner goes through exactly what the GPU does — the narrowed model-view
//!   matrix applied to the narrowed vertex, in `f32` — and is compared with the `f64` truth:
//!   the error stays under 2 µm, and the frame-to-frame screen motion of every corner matches
//!   the true motion to 1/1000 px (no shimmer). Positions resolve through the `FrameResolver`
//!   seam, so a resolver with frames beyond the world's is drawn by the same code.
//! * **GPU.** The same lit scene rendered near the world origin and 6.371e6 m from it, at six
//!   sub-millimetre camera positions, gives the same pixels; and the renderer's uploaded
//!   model-view matrices are exactly the last-mile ones.
//!
//! Positive control (W2): `positive_control_naive_absolute_f32_shimmers` builds the matrices
//! the way an engine with `f32` world positions does ([`mutants::naive_model_view`]: narrow
//! object and camera, then subtract) and asserts it fails both CPU checks by orders of
//! magnitude and changes the GPU image.

mod common;

use forge_frames::{
    DQuat, DVec3, FrameId, FramePos, FrameResolver, TICKS_PER_SECOND, Tick, WorldFrame,
};
use forge_render::last_mile::{self, CameraView, apply_f32, mutants};
use forge_render::{
    Camera, DirectionalLight, Instance, Material, MeshData, PunctualLight, RenderOptions, Renderer,
    Scene,
};

const ROW: &str = "C-camera-relative-last-mile";
const R: f64 = 6.371e6;

/// A point 6.371e6 m from the origin with no zero component.
fn surface() -> DVec3 {
    let d = DVec3::new(0.61, 0.52, 0.597).try_normalize().expect("unit");
    d * R
}

struct Walk {
    /// max |f32 path - f64 truth| over all corners and steps, metres
    max_error_m: f64,
    /// max |screen motion (f32 path) - screen motion (truth)| between steps, pixels
    max_shimmer_px: f64,
}

/// Walk a camera past a cube; `naive` builds the model-view matrix the wrong way.
fn walk(tree: &dyn FrameResolver, cam_frame: FrameId, obj: FramePos, t: Tick, naive: bool) -> Walk {
    let up = surface().try_normalize().expect("up");
    let fwd = up.cross(DVec3::Z).try_normalize().expect("tangent");
    let start = surface() - fwd * 3.0;
    let corners: Vec<DVec3> = [-0.5, 0.5]
        .iter()
        .flat_map(|&x| {
            [-0.5, 0.5]
                .iter()
                .flat_map(move |&y| [-0.5, 0.5].iter().map(move |&z| DVec3::new(x, y, z)))
        })
        .collect();
    let rot = DQuat::from_axis_angle(
        DVec3::new(0.3, 0.8, 0.52).try_normalize().expect("axis"),
        0.7,
    );
    let scale = DVec3::new(0.4, 0.4, 0.4);
    let (w, h, fov) = (1920.0, 1080.0, 1.0f64);
    let f = 1.0 / (0.5 * fov).tan();
    let to_px = |v: DVec3| (v.x / -v.z * f * h * 0.5, v.y / -v.z * f * h * 0.5);
    let _ = w;
    let mut out = Walk {
        max_error_m: 0.0,
        max_shimmer_px: 0.0,
    };
    type Px = Vec<(f64, f64)>;
    let mut prev: Option<(Px, Px)> = None;
    for step in 0..2000 {
        let p =
            start + fwd * (1e-3 * f64::from(step)) + up.cross(fwd) * (0.37e-3 * f64::from(step));
        let cam = Camera::looking(FramePos::new(cam_frame, p), fwd, up, fov).expect("camera");
        let mut view = CameraView::new(tree, &cam, t).expect("view");
        let obj_rot = view.rotation_from(obj.frame).expect("rot") * rot;
        let mv = if naive {
            mutants::naive_model_view(&view, obj, obj_rot, scale).expect("naive")
        } else {
            last_mile::model_view(obj_rot, scale, view.offset(obj).expect("offset"))
        };
        let exact_origin = view.offset(obj).expect("offset").0;
        let mut got = Vec::new();
        let mut truth = Vec::new();
        for c in &corners {
            let local = DVec3::new(c.x * scale.x, c.y * scale.y, c.z * scale.z);
            let exact = exact_origin + obj_rot.rotate(local);
            let v = apply_f32(&mv, last_mile::vertex(*c));
            let v = DVec3::new(f64::from(v[0]), f64::from(v[1]), f64::from(v[2]));
            out.max_error_m = out.max_error_m.max((v - exact).length());
            got.push(to_px(v));
            truth.push(to_px(exact));
        }
        if let Some((pg, pt)) = &prev {
            for i in 0..corners.len() {
                let dg = (got[i].0 - pg[i].0, got[i].1 - pg[i].1);
                let dt = (truth[i].0 - pt[i].0, truth[i].1 - pt[i].1);
                let e = ((dg.0 - dt.0).powi(2) + (dg.1 - dt.1).powi(2)).sqrt();
                out.max_shimmer_px = out.max_shimmer_px.max(e);
            }
        }
        prev = Some((got, truth));
    }
    out
}

fn world() -> (WorldFrame, FrameId) {
    (WorldFrame, FrameId::WORLD)
}

#[test]
fn no_precision_shimmer_far_from_the_origin_cpu() {
    let (tree, body) = world();
    let t = Tick(7 * TICKS_PER_SECOND);
    let w = walk(&tree, body, FramePos::new(body, surface()), t, false);
    println!(
        "world frame: max error {:.3e} m, max shimmer {:.3e} px",
        w.max_error_m, w.max_shimmer_px
    );
    assert!(w.max_error_m < 2e-6, "{} m", w.max_error_m);
    assert!(w.max_shimmer_px < 1e-3, "{} px", w.max_shimmer_px);
}

#[test]
fn positive_control_naive_absolute_f32_shimmers() {
    let (tree, body) = world();
    let w = walk(&tree, body, FramePos::new(body, surface()), Tick(0), true);
    println!(
        "naive: max error {:.3e} m, max shimmer {:.3e} px",
        w.max_error_m, w.max_shimmer_px
    );
    assert!(
        w.max_error_m > 0.05,
        "naive f32 should be off by decimetres, got {} m",
        w.max_error_m
    );
    assert!(
        w.max_shimmer_px > 10.0,
        "naive f32 should jump by many pixels, got {} px",
        w.max_shimmer_px
    );
    // And on the GPU: the image changes (checked only where there is an adapter).
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (mut r, scene_o, scene_far, tree, root) = gpu_scenes(dev);
    let target = common::target(dev, W, H);
    let cam_o = cam_at(root, DVec3::ZERO, 0.0);
    let f = r
        .prepare(&scene_o, &cam_o, &tree, Tick(0))
        .expect("prepare");
    r.render(dev, &f, &target).expect("render");
    let near_origin = common::read(dev, &target);
    let cam_far = cam_at(root, surface(), 0.0);
    let f = r
        .prepare_naive_for_tests(&scene_far, &cam_far, &tree, Tick(0))
        .expect("prepare");
    r.render(dev, &f, &target).expect("render");
    let naive = common::read(dev, &target);
    let (differing, _) = diff(&near_origin, &naive);
    assert!(
        differing > 500,
        "the naive path should visibly move the cube: {differing} pixels differ"
    );
}

const W: u32 = 640;
const H: u32 = 400;

fn cam_at(root: FrameId, base: DVec3, walked_mm: f64) -> Camera {
    let pos = base + DVec3::new(-3.0 + walked_mm * 1e-3, 1.2, 0.3);
    Camera::looking(
        FramePos::new(root, pos),
        DVec3::new(1.0, -0.3, -0.1),
        DVec3::Y,
        1.0,
    )
    .expect("camera")
}

/// A lit cube on a slab with a point light, near the origin and 6.371e6 m out.
fn gpu_scenes(dev: &forge_gpu::GpuDevice) -> (Renderer, Scene, Scene, WorldFrame, FrameId) {
    let (tree, root) = common::tree();
    let mut opts = RenderOptions::new(W, H);
    // Cascade texels snap to a grid fixed in the frame, whose phase differs between the two
    // placements; the renderer's cascades under motion at 6.371e6 m are guarded in
    // test_shadows (`renderer_cascades_are_world_anchored_under_motion`).
    opts.shadows = None;
    let mut r = Renderer::new(dev, opts).expect("renderer");
    let cube = r.add_mesh(&MeshData::cube()).expect("cube");
    let red = r.add_material(Material {
        base_color: [0.8, 0.1, 0.05, 1.0],
        roughness: 0.4,
        ..Material::default()
    });
    let grey = r.add_material(Material::default());
    let build = |base: DVec3| Scene {
        instances: vec![
            Instance::new(
                FramePos::new(root, base + DVec3::new(0.0, 0.25, 0.0)),
                cube,
                red,
            )
            .oriented(DQuat::from_axis_angle(DVec3::Y, 0.5))
            .scaled(0.5),
            Instance::new(
                FramePos::new(root, base + DVec3::new(0.0, -0.05, 0.0)),
                cube,
                grey,
            )
            .scaled3(DVec3::new(6.0, 0.1, 6.0)),
        ],
        lights: vec![PunctualLight {
            pos: FramePos::new(root, base + DVec3::new(-1.0, 1.5, 1.0)),
            color: [1.0, 0.9, 0.8],
            intensity: 40.0,
            range: 10.0,
            spot: None,
        }],
        sun: Some(DirectionalLight {
            frame: root,
            toward_light: DVec3::new(0.3, 1.0, 0.4),
            color: [1.0, 1.0, 1.0],
            illuminance: 3.0,
            casts_shadows: false,
        }),
        ambient: [0.05, 0.05, 0.06],
        ..Scene::default()
    };
    let (a, b) = (build(DVec3::ZERO), build(surface()));
    (r, a, b, tree, root)
}

/// (pixels whose any channel differs by more than 2, max channel difference)
fn diff(a: &[u8], b: &[u8]) -> (usize, u8) {
    let mut n = 0;
    let mut m = 0;
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
        let d = pa
            .iter()
            .zip(pb)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        m = m.max(d);
        if d > 2 {
            n += 1;
        }
    }
    (n, m)
}

#[test]
fn the_same_pixels_near_the_origin_and_far_from_it() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (mut r, scene_o, scene_far, tree, root) = gpu_scenes(dev);
    let target = common::target(dev, W, H);
    for k in 0..6 {
        let walked = 0.7 * f64::from(k);
        let f = r
            .prepare(&scene_o, &cam_at(root, DVec3::ZERO, walked), &tree, Tick(0))
            .expect("prepare");
        r.render(dev, &f, &target).expect("render");
        let a = common::read(dev, &target);
        let f = r
            .prepare(&scene_far, &cam_at(root, surface(), walked), &tree, Tick(0))
            .expect("prepare");
        // The upload is exactly the last-mile matrix.
        let cam = cam_at(root, surface(), walked);
        let mut view = CameraView::new(&tree, &cam, Tick(0)).expect("view");
        let inst = &scene_far.instances[0];
        let rot = view.rotation_from(root).expect("rot") * inst.orientation;
        let expect = last_mile::model_view(rot, inst.scale, view.offset(inst.pos).expect("offset"));
        assert_eq!(f.model_view(0), Some(expect));
        r.render(dev, &f, &target).expect("render");
        let b = common::read(dev, &target);
        let lit = a.as_chunks::<4>().0.iter().filter(|p| p[0] > 40).count();
        assert!(lit > 2000, "the scene is on screen ({lit} lit pixels)");
        let (n, m) = diff(&a, &b);
        assert!(
            n == 0,
            "step {k}: {n} pixels differ (max {m} levels) between origin and 6.371e6 m"
        );
    }
}
