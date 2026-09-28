// timed-gates: exempt(the binning time is printed for the record; the asserts are on culling)
//! `test_clustered_lighting` — clustered forward+ light culling (Ch.10 §10.3, DoD M1-7).
//!
//! * **Exact enough, never short.** 400 lights (some containing the camera, some cut by the
//!   near plane, some off screen) are binned; then 40,000 random fragments — random pixel,
//!   log-uniform depth from 1 cm to 5 km — look up their cluster exactly as the shader does,
//!   and every light whose range reaches the fragment must be in that cluster's list.
//!   Conservative binning may list extra lights; it may never miss one.
//! * **The shader agrees with the CPU.** The same lit floor rendered with the default grid
//!   and with a 1x1x1 grid (every light in one list: brute force) gives the same pixels.
//!
//! Positive control (W2): `positive_control_shrunk_light_radius_is_caught` bins with every
//! radius halved and asserts the fragment check finds lights missing.

mod common;

use forge_frames::{DQuat, DVec3, FramePos, Tick};
use forge_render::cluster::mutants;
use forge_render::{
    Camera, ClusterGrid, ClusterView, Clusters, Instance, LightSphere, Material, MeshData,
    PunctualLight, RenderOptions, Renderer, Scene,
};

const ROW: &str = "C-clustered-lights";

/// A small deterministic generator (the test's own; not engine randomness).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}

fn view() -> ClusterView {
    ClusterView {
        width: 1280,
        height: 720,
        tan_half_fov: (0.5f64).tan(),
    }
}

fn lights(n: usize) -> Vec<LightSphere> {
    let v = view();
    let aspect = f64::from(v.width) / f64::from(v.height);
    let mut g = Lcg(0x5eed);
    let mut out = Vec::new();
    for i in 0..n {
        let d = (g.range(0.0f64, 3.5f64)).exp().mul_add(1.0, -0.8).max(0.02);
        let nx = g.range(-1.3, 1.3);
        let ny = g.range(-1.3, 1.3);
        let r = match i % 10 {
            0 => g.range(2.0, 20.0), // likely contains the camera or crosses the near plane
            1 => g.range(0.005, 0.05),
            _ => g.range(0.2, 0.3 * d + 1.0),
        };
        out.push(LightSphere {
            offset: forge_render::last_mile::ViewOffset(DVec3::new(
                nx * aspect * v.tan_half_fov * d,
                ny * v.tan_half_fov * d,
                -d,
            )),
            radius: r,
        });
    }
    out
}

/// Fragments whose cluster misses a light that reaches them.
fn misses(grid: &ClusterGrid, c: &Clusters, ls: &[LightSphere], samples: usize) -> Vec<String> {
    let v = view();
    let aspect = f64::from(v.width) / f64::from(v.height);
    let mut g = Lcg(0xf4a9);
    let mut out = Vec::new();
    for _ in 0..samples {
        let px = g.range(0.0, f64::from(v.width));
        let py = g.range(0.0, f64::from(v.height));
        let d = (g.range((0.01f64).ln(), (5000.0f64).ln())).exp();
        let nx = 2.0 * px / f64::from(v.width) - 1.0;
        let ny = 1.0 - 2.0 * py / f64::from(v.height);
        let p = DVec3::new(
            nx * aspect * v.tan_half_fov * d,
            ny * v.tan_half_fov * d,
            -d,
        );
        let Some(ci) = grid.cluster_of(&v, px, py, d) else {
            continue;
        };
        let [off, cnt] = c.ranges[ci];
        let list = &c.indices[off as usize..(off + cnt) as usize];
        for (li, l) in ls.iter().enumerate() {
            if (p - l.offset.0).length() < l.radius && !list.contains(&(li as u32)) {
                out.push(format!(
                    "pixel ({px:.1},{py:.1}) depth {d:.3}: light {li} missing"
                ));
            }
        }
    }
    out
}

#[test]
fn every_light_that_reaches_a_fragment_is_in_its_cluster() {
    let grid = ClusterGrid::default();
    let ls = lights(400);
    let t = std::time::Instant::now();
    let c = grid.bin(&view(), &ls);
    let bin_ms = t.elapsed().as_secs_f64() * 1e3;
    let m = misses(&grid, &c, &ls, 40_000);
    assert!(
        m.is_empty(),
        "{} misses, first: {:?}",
        m.len(),
        &m[..m.len().min(5)]
    );
    let clusters = (grid.x * grid.y * grid.z) as usize;
    assert_eq!(c.ranges.len(), clusters);
    // Culling does real work: on average a cluster lists a small fraction of the lights.
    let avg = c.indices.len() as f64 / clusters as f64;
    println!(
        "400 lights: {} entries, {avg:.2} per cluster, binned in {bin_ms:.2} ms (debug)",
        c.indices.len()
    );
    assert!(
        avg < 40.0,
        "{avg} lights per cluster: culling is not culling"
    );
}

#[test]
fn positive_control_shrunk_light_radius_is_caught() {
    let grid = ClusterGrid::default();
    let ls = lights(400);
    let c = mutants::bin_shrunk(&grid, &view(), &ls);
    let m = misses(&grid, &c, &ls, 40_000);
    assert!(!m.is_empty(), "binning with halved radii must miss lights");
}

#[test]
fn the_shader_finds_the_same_lights_as_brute_force() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let (w, h) = (640, 360);
    let render = |grid: ClusterGrid| {
        let mut opts = RenderOptions::new(w, h);
        opts.clusters = grid;
        opts.shadows = None;
        let mut r = Renderer::new(dev, opts).expect("renderer");
        let quad = r.add_mesh(&MeshData::quad()).expect("quad");
        let floor_m = r.add_material(Material {
            base_color: [0.7, 0.7, 0.7, 1.0],
            roughness: 0.6,
            ..Material::default()
        });
        let mut scene = Scene {
            instances: vec![
                Instance::new(
                    FramePos::new(root, DVec3::new(0.0, 0.0, -20.0)),
                    quad,
                    floor_m,
                )
                .oriented(DQuat::from_axis_angle(
                    DVec3::X,
                    -core::f64::consts::FRAC_PI_2,
                ))
                .scaled(60.0),
            ],
            ambient: [0.01; 3],
            ..Scene::default()
        };
        let mut g = Lcg(7);
        for _ in 0..96 {
            scene.lights.push(PunctualLight {
                pos: FramePos::new(
                    root,
                    DVec3::new(
                        g.range(-15.0, 15.0),
                        g.range(0.2, 1.5),
                        g.range(-45.0, -2.0),
                    ),
                ),
                color: [g.range(0.2, 1.0), g.range(0.2, 1.0), g.range(0.2, 1.0)],
                intensity: g.range(2.0, 8.0),
                range: g.range(1.5, 5.0),
                spot: None,
            });
        }
        let cam = Camera::looking(
            FramePos::new(root, DVec3::new(0.0, 4.0, 4.0)),
            DVec3::new(0.0, -0.35, -1.0),
            DVec3::Y,
            1.0,
        )
        .expect("camera");
        let target = common::target(dev, w, h);
        r.render_scene(dev, &scene, &cam, &tree, Tick(0), &target)
            .expect("render");
        common::read(dev, &target)
    };
    let clustered = render(ClusterGrid::default());
    let brute = render(ClusterGrid {
        x: 1,
        y: 1,
        z: 1,
        ..ClusterGrid::default()
    });
    let lit = clustered
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 30)
        .count();
    assert!(lit > 5000, "lights illuminate the floor ({lit} pixels)");
    let worst = clustered
        .iter()
        .zip(&brute)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "clustered and brute-force lighting differ by {worst} levels"
    );
}
