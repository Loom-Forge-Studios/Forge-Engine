//! `test_shadows` — cascaded sun shadows (Ch.10 §10.5, DoD M1-7; the documented fallback
//! for virtual shadow maps, ADR 0023).
//!
//! * **World-anchored texels (no shadow crawl).** Cascades are fitted for a camera 6.371e6 m
//!   from its frame's origin and again after it moves by a fraction of a texel: a fixed
//!   point on the ground lands on the same sub-texel position in both, in every cascade.
//!   Cascade radii do not change as the camera turns (constant texel size).
//! * **Shadows land where the geometry says.** A floating box over a floor: the floor point
//!   straight down the sun ray from the box's centre renders at the ambient-only value, a
//!   floor point in the open at the fully lit value, and with the box not casting shadows
//!   the first point is lit too.
//!
//! * **The renderer uses the snapped fit.** `renderer_cascades_are_world_anchored_under_motion`
//!   runs the same sub-texel motion through `Renderer::prepare` and checks the prepared
//!   frame's cascades: no texel-phase drift, and equal to `shadow::fit(.., snap = true)`.
//!
//! Positive control (W2): `positive_control_unsnapped_cascades_crawl` fits the same cascades
//! without snapping and asserts the ground point's sub-texel position moves with the camera;
//! the renderer test also asserts the snapped and unsnapped fits differ at its positions, so
//! a renderer that fitted unsnapped cascades fails it.

mod common;

use forge_frames::{DQuat, DVec3, FramePos, Tick};
use forge_render::last_mile::{CameraView, FrameOffset};
use forge_render::shadow::{self, Cascade};
use forge_render::{
    Camera, CascadeSettings, DirectionalLight, Instance, Material, MeshData, RenderOptions,
    Renderer, Scene,
};

const ROW: &str = "C-shadow-cascades";

fn surface() -> DVec3 {
    DVec3::new(0.61, 0.52, 0.597).try_normalize().expect("unit") * 6.371e6
}

fn orientation() -> DQuat {
    let up = surface().try_normalize().expect("up");
    let fwd = up.cross(DVec3::Z).try_normalize().expect("fwd");
    let fwd = (fwd - up * 0.4).try_normalize().expect("fwd");
    Camera::looking(FramePos::origin_of(forge_frames::FrameId(0)), fwd, up, 1.0)
        .expect("cam")
        .orientation
}

fn light() -> DVec3 {
    (surface().try_normalize().expect("up") + DVec3::new(0.2, -0.3, 0.1))
        .try_normalize()
        .expect("light")
}

/// A position in frame 0 (the camera frame of these fits).
fn at(local: DVec3) -> FramePos {
    FramePos::new(forge_frames::FrameId(0), local)
}

/// Cascades for a camera at `cam` with orientation `q`.
fn fits(cam: FramePos, q: DQuat, snap: bool) -> Vec<Cascade> {
    let camera = Camera {
        pos: cam,
        orientation: q,
        fov_y: 1.0,
    };
    shadow::fit(
        &CascadeSettings::default(),
        &camera,
        16.0 / 9.0,
        light(),
        snap,
    )
}

/// The largest change, over every cascade, in the sub-texel position of fixed ground points
/// when the camera moves by a fraction of a texel of cascade 0. `fit_at` returns the
/// cascades for a camera at the given frame-0 position with orientation [`orientation`].
fn texel_phase_drift(mut fit_at: impl FnMut(DVec3) -> Vec<Cascade>) -> f64 {
    let res = f64::from(CascadeSettings::default().resolution);
    let cam_a = surface();
    let q = orientation();
    let a = fit_at(cam_a);
    assert_eq!(a.len(), CascadeSettings::default().count as usize);
    let delta = DVec3::new(0.37, 0.21, -0.11) * a[0].texel;
    let cam_b = cam_a + delta;
    let b = fit_at(cam_b);
    assert_eq!(a.len(), b.len());
    let mut worst: f64 = 0.0;
    for (ca, cb) in a.iter().zip(&b) {
        assert!(
            (ca.texel - cb.texel).abs() < 1e-12 * ca.texel,
            "texel size must not change"
        );
        // A few ground points inside this cascade (camera-frame coordinates, f64).
        for k in 0..5 {
            let offset = q.rotate(DVec3::new(0.1 * f64::from(k), -0.2, -ca.split * 0.8));
            let x = cam_a + offset;
            let pa = ca.project(FrameOffset(x - cam_a));
            let pb = cb.project(FrameOffset(x - cam_b));
            for (ua, ub) in [(pa.x, pb.x), (pa.y, pb.y)] {
                let (ta, tb) = (ua * res * 0.5, ub * res * 0.5);
                let d = (ta - ta.floor()) - (tb - tb.floor());
                let d = d - d.round();
                worst = worst.max(d.abs());
            }
        }
    }
    worst
}

/// [`texel_phase_drift`] for `shadow::fit` called directly.
fn fit_drift(snap: bool) -> f64 {
    texel_phase_drift(|cam| fits(at(cam), orientation(), snap))
}

#[test]
fn cascade_texels_are_world_anchored() {
    let drift = fit_drift(true);
    println!("snapped: sub-texel drift {drift:.2e} texels");
    assert!(drift < 1e-4, "{drift} texels");
    // The radius (so the texel size) does not depend on where the camera looks.
    let a = fits(at(surface()), orientation(), true);
    let b = fits(
        at(surface()),
        DQuat::from_axis_angle(DVec3::Y, 1.1) * orientation(),
        true,
    );
    for (x, y) in a.iter().zip(&b) {
        assert!((x.radius - y.radius).abs() < 1e-9 * x.radius);
    }
    // Splits grow and end at the configured distance.
    let s = shadow::splits(&CascadeSettings::default());
    assert!(s.windows(2).all(|w| w[0] < w[1]));
    assert!((s[3] - CascadeSettings::default().max_distance).abs() < 1e-9);
}

#[test]
fn positive_control_unsnapped_cascades_crawl() {
    let drift = fit_drift(false);
    println!("unsnapped: sub-texel drift {drift:.2e} texels");
    assert!(
        drift > 0.05,
        "without snapping the texel grid should move with the camera ({drift})"
    );
}

/// The renderer's own path: cascades as `Renderer::prepare` fits them for a sun-lit scene
/// 6.371e6 m from the frame origin, under sub-texel camera motion. Guards against the
/// renderer fitting unsnapped cascades (shadow crawl) even though `shadow::fit` itself snaps
/// when asked to.
#[test]
fn renderer_cascades_are_world_anchored_under_motion() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let (w, h) = (1600, 900); // aspect 16:9, as in `fits`
    let mut r = Renderer::new(dev, RenderOptions::new(w, h)).expect("renderer");
    let settings = CascadeSettings::default();
    assert_eq!(
        r.options().shadows,
        Some(settings),
        "the test assumes default cascades"
    );
    let cube = r.add_mesh(&MeshData::cube()).expect("cube");
    let grey = r.add_material(Material::default());
    let up = surface().try_normalize().expect("up");
    let scene = Scene {
        instances: vec![
            Instance::new(FramePos::new(root, surface() - up * 2.0), cube, grey)
                .scaled3(DVec3::new(40.0, 0.5, 40.0)),
        ],
        sun: Some(DirectionalLight {
            frame: root,
            toward_light: light(),
            color: [1.0; 3],
            illuminance: 4.0,
            casts_shadows: true,
        }),
        ..Scene::default()
    };
    let mut prepare_at = |cam: DVec3| {
        let camera = Camera {
            pos: FramePos::new(root, cam),
            orientation: orientation(),
            fov_y: 1.0,
        };
        let f = r.prepare(&scene, &camera, &tree, Tick(0)).expect("prepare");
        // The renderer's fit is exactly the snapped fit (and not the unsnapped one).
        let snapped = shadow::fit(&settings, &camera, 16.0 / 9.0, light(), true);
        let unsnapped = shadow::fit(&settings, &camera, 16.0 / 9.0, light(), false);
        assert_eq!(f.cascade_fits.len(), snapped.len());
        let mut differs_from_unsnapped = false;
        for ((got, s), u) in f.cascade_fits.iter().zip(&snapped).zip(&unsnapped) {
            let err = (got.offset.0 - s.offset.0).length();
            assert!(
                err < 1e-6 * s.texel,
                "renderer cascade centre is {err} m from the snapped fit (texel {})",
                s.texel
            );
            differs_from_unsnapped |= (got.offset.0 - u.offset.0).length() > 1e-3 * s.texel;
        }
        assert!(
            differs_from_unsnapped,
            "control: snapped and unsnapped fits must differ at this camera position"
        );
        f.cascade_fits
    };
    let drift = texel_phase_drift(&mut prepare_at);
    println!("renderer: sub-texel drift {drift:.2e} texels");
    assert!(drift < 1e-4, "renderer cascades crawl: {drift} texels");
}

#[test]
fn a_floating_box_shadows_the_floor_where_the_sun_ray_says() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let (w, h) = (800, 450);
    let base = surface();
    let up = base.try_normalize().expect("up");
    // A local tangent basis on the ground sphere: `up`, `east`, `north`.
    let east = DVec3::Z.cross(up).try_normalize().expect("east");
    let north = up.cross(east);
    let q_local = shadow_basis(east, up, north);
    let sun_dir = (up + east * 0.35 + north * 0.2)
        .try_normalize()
        .expect("sun");
    let floor_material = Material {
        base_color: [0.6, 0.6, 0.6, 1.0],
        roughness: 0.9,
        ..Material::default()
    };
    let render = |box_casts: bool| {
        let mut r = Renderer::new(dev, RenderOptions::new(w, h)).expect("renderer");
        let cube = r.add_mesh(&MeshData::cube()).expect("cube");
        let grey = r.add_material(floor_material);
        let red = r.add_material(Material {
            base_color: [0.7, 0.1, 0.1, 1.0],
            ..Material::default()
        });
        let mut b =
            Instance::new(FramePos::new(root, base + up * 1.5), cube, red).oriented(q_local);
        b.casts_shadows = box_casts;
        let scene = Scene {
            instances: vec![
                Instance::new(FramePos::new(root, base - up * 0.05), cube, grey)
                    .oriented(q_local)
                    .scaled3(DVec3::new(30.0, 0.1, 30.0)),
                b,
            ],
            sun: Some(DirectionalLight {
                frame: root,
                toward_light: sun_dir,
                color: [1.0; 3],
                illuminance: 4.0,
                casts_shadows: true,
            }),
            ambient: [0.1, 0.1, 0.1],
            ..Scene::default()
        };
        let cam_pos = base + up * 6.0 + north * 8.0;
        let cam =
            Camera::looking(FramePos::new(root, cam_pos), base - cam_pos, up, 1.0).expect("cam");
        let target = common::target(dev, w, h);
        let f = r.prepare(&scene, &cam, &tree, Tick(0)).expect("prepare");
        assert_eq!(f.cascades.len(), 4);
        r.render(dev, &f, &target).expect("render");
        (common::read(dev, &target), cam, scene.exposure())
    };
    let (img, cam, exposure) = render(true);
    let view = CameraView::new(&tree, &cam, Tick(0)).expect("view");
    let to_px = |p: DVec3| {
        let o = view.offset(FramePos::new(root, p)).expect("offset").0;
        let t = (0.5f64).tan();
        let aspect = f64::from(w) / f64::from(h);
        let nx = o.x / (-o.z * aspect * t);
        let ny = o.y / (-o.z * t);
        (
            ((nx + 1.0) * 0.5 * f64::from(w)) as u32,
            ((1.0 - ny) * 0.5 * f64::from(h)) as u32,
        )
    };
    // Straight down the sun ray from the box centre to the floor (y = 0 in the local basis).
    let shadow_pt = base + up * 1.5 - sun_dir * (1.5 / sun_dir.dot(up));
    let lit_pt = base + east * 4.0 - north * 2.0;
    let (sx, sy) = to_px(shadow_pt);
    let (lx, ly) = to_px(lit_pt);
    let shadowed = common::px(&img, w, sx, sy);
    let lit = common::px(&img, w, lx, ly);
    // Ambient-only floor: ambient * (diffuse + f0), f0 = 0.16 * 0.5^2 = 0.04.
    let ambient_only = common::display(0.1 * (0.6 + 0.04) * exposure);
    println!("shadowed {shadowed:?} (ambient-only {ambient_only}), lit {lit:?}");
    assert!(
        shadowed[0].abs_diff(ambient_only) <= 3,
        "shadowed floor {shadowed:?} vs ambient-only {ambient_only}"
    );
    assert!(
        lit[0] > shadowed[0] + 60,
        "lit floor {lit:?} vs shadowed {shadowed:?}"
    );
    // Without the box casting, the same point is lit.
    let (img2, _, _) = render(false);
    let open = common::px(&img2, w, sx, sy);
    assert!(
        open[0] > shadowed[0] + 60,
        "with no caster the point is lit: {open:?} vs {shadowed:?}"
    );
}

/// Object axes (x = east, y = up, z = north... right-handed: x cross y = z) as a rotation.
fn shadow_basis(east: DVec3, up: DVec3, north: DVec3) -> DQuat {
    // quat_from_basis wants columns x, y, z with x cross y = z.
    let z = east.cross(up);
    let z = if z.dot(north) > 0.0 { z } else { -z };
    forge_render::quat_from_basis(east, up, z)
}
