//! `test_render_goldens` — offscreen golden images of the renderer (Ch.10 §10.8, DoD M1-7).
//!
//! Two scenes, each placed 6.371e6 m from the world origin (so the last mile is in every
//! image):
//!
//! * `materials` — a 6x3 grid of spheres from dielectric to metal and smooth to rough, on a
//!   floor, lit by the sun with cascaded shadows, ambient, and one point light;
//! * `lights` — a floor under 48 coloured point lights and two spot lights (clustered).
//!
//! Each is compared with a committed PNG under a fixed threshold ([`CHANNEL_TOLERANCE`],
//! [`MAX_DIFF_FRACTION`]) that is never widened (W5). `FORGE_BLESS=1` rewrites them. With no
//! adapter the test records AWAITING (locally) or fails (`CI=true`).
//!
//! Positive control (W2): `positive_control_a_changed_material_fails_the_golden` renders
//! `materials` with one sphere 0.15 rougher and asserts the comparison fails.

mod common;

use forge_frames::{DQuat, DVec3, FrameId, FramePos, Tick, WorldFrame};
use forge_gpu::GpuDevice;
use forge_render::{
    Camera, DirectionalLight, Instance, Material, MeshData, PunctualLight, RenderOptions, Renderer,
    Scene, Spot,
};

const ROW: &str = "C-render-goldens";
const W: u32 = 480;
const H: u32 = 270;
/// A channel may differ by this many levels before a pixel counts as different.
const CHANNEL_TOLERANCE: u8 = 3;
/// At most this fraction of pixels may differ.
const MAX_DIFF_FRACTION: f64 = 0.002;

struct Place {
    base: DVec3,
    up: DVec3,
    east: DVec3,
    north: DVec3,
}

fn place() -> Place {
    let up = DVec3::new(0.61, 0.52, 0.597).try_normalize().expect("unit");
    let east = DVec3::Z.cross(up).try_normalize().expect("east");
    let north = up.cross(east);
    Place {
        base: up * 6.371e6,
        up,
        east,
        north,
    }
}

impl Place {
    /// A local point: `x` east, `y` up, `z` south (so a camera looking north looks down -z).
    fn at(&self, x: f64, y: f64, z: f64) -> DVec3 {
        self.base + self.east * x + self.up * y - self.north * z
    }
    /// Local axes (x east, y up, z south) as a rotation.
    fn axes(&self) -> DQuat {
        forge_render::quat_from_basis(self.east, self.up, -self.north)
    }
    fn camera(&self, root: FrameId, from: (f64, f64, f64), to: (f64, f64, f64)) -> Camera {
        let p = self.at(from.0, from.1, from.2);
        let q = self.at(to.0, to.1, to.2);
        Camera::looking(FramePos::new(root, p), q - p, self.up, 0.9).expect("camera")
    }
}

fn floor(r: &mut Renderer, root: FrameId, pl: &Place, size: f64) -> Instance {
    let cube = r.add_mesh(&MeshData::cube()).expect("cube");
    let m = r.add_material(Material {
        base_color: [0.45, 0.45, 0.42, 1.0],
        roughness: 0.85,
        ..Material::default()
    });
    Instance::new(FramePos::new(root, pl.at(0.0, -0.05, 0.0)), cube, m)
        .oriented(pl.axes())
        .scaled3(DVec3::new(size, 0.1, size))
}

fn materials_scene(r: &mut Renderer, root: FrameId, rougher: bool) -> (Scene, Camera) {
    let pl = place();
    let sphere = r.add_mesh(&MeshData::uv_sphere(48, 24)).expect("sphere");
    let mut scene = Scene {
        instances: vec![floor(r, root, &pl, 40.0)],
        sun: Some(DirectionalLight {
            frame: root,
            toward_light: (pl.up * 1.0 + pl.east * 0.5 + pl.north * -0.35)
                .try_normalize()
                .expect("sun"),
            color: [1.0, 0.96, 0.9],
            illuminance: 6.0,
            casts_shadows: true,
        }),
        ambient: [0.10, 0.12, 0.16],
        lights: vec![PunctualLight {
            pos: FramePos::new(root, pl.at(-3.0, 1.2, 1.5)),
            color: [0.3, 0.6, 1.0],
            intensity: 6.0,
            range: 6.0,
            spot: None,
        }],
        ..Scene::default()
    };
    let colors = [[0.9, 0.9, 0.9], [0.85, 0.15, 0.1], [1.0, 0.78, 0.34]];
    for (row, color) in colors.iter().enumerate() {
        for col in 0..6 {
            let mut m = Material {
                base_color: [color[0], color[1], color[2], 1.0],
                metallic: if row == 2 {
                    1.0
                } else {
                    f64::from(col as u32) / 5.0 * f64::from(row as u32)
                },
                roughness: 0.05 + 0.19 * col as f64,
                ..Material::default()
            };
            if rougher && row == 1 && col == 2 {
                m.roughness += 0.15;
            }
            let id = r.add_material(m);
            scene.instances.push(Instance::new(
                FramePos::new(
                    root,
                    pl.at(-3.75 + 1.5 * col as f64, 0.6, -1.5 * row as f64),
                ),
                sphere,
                id,
            ));
        }
    }
    (scene, pl.camera(root, (0.0, 3.2, 6.0), (0.0, 0.3, -1.5)))
}

fn lights_scene(r: &mut Renderer, root: FrameId) -> (Scene, Camera) {
    let pl = place();
    let cube = r.add_mesh(&MeshData::cube()).expect("cube");
    let pillar = r.add_material(Material {
        base_color: [0.8, 0.8, 0.8, 1.0],
        roughness: 0.3,
        ..Material::default()
    });
    let mut scene = Scene {
        instances: vec![floor(r, root, &pl, 60.0)],
        ambient: [0.01, 0.01, 0.012],
        ..Scene::default()
    };
    for i in 0..5 {
        scene.instances.push(
            Instance::new(
                FramePos::new(root, pl.at(-8.0 + 4.0 * f64::from(i), 1.0, -6.0)),
                cube,
                pillar,
            )
            .oriented(pl.axes())
            .scaled3(DVec3::new(0.6, 2.0, 0.6)),
        );
    }
    for i in 0..48 {
        let (gx, gz) = (f64::from(i % 8), f64::from(i / 8));
        let hue = f64::from(i) / 48.0 * core::f64::consts::TAU;
        scene.lights.push(PunctualLight {
            pos: FramePos::new(
                root,
                pl.at(-10.5 + 3.0 * gx, 0.4 + 0.1 * (gx % 3.0), 2.0 - 3.0 * gz),
            ),
            color: [
                0.5 + 0.5 * hue.cos(),
                0.5 + 0.5 * (hue + 2.1).cos(),
                0.5 + 0.5 * (hue + 4.2).cos(),
            ],
            intensity: 3.0,
            range: 3.5,
            spot: None,
        });
    }
    for x in [-5.0, 5.0] {
        scene.lights.push(PunctualLight {
            pos: FramePos::new(root, pl.at(x, 5.0, -2.0)),
            color: [1.0, 0.95, 0.8],
            intensity: 60.0,
            range: 12.0,
            spot: Some(Spot {
                direction: (pl.up * -1.0 + pl.north * 0.3)
                    .try_normalize()
                    .expect("dir"),
                inner_angle: 0.25,
                outer_angle: 0.4,
            }),
        });
    }
    (scene, pl.camera(root, (0.0, 7.0, 9.0), (0.0, 0.0, -4.0)))
}

type Build = fn(&mut Renderer, FrameId) -> (Scene, Camera);

fn scenes() -> Vec<(&'static str, Build)> {
    vec![
        ("materials", |r, root| materials_scene(r, root, false)),
        ("lights", lights_scene),
    ]
}

fn render(dev: &GpuDevice, tree: &WorldFrame, root: FrameId, build: Build) -> Vec<u8> {
    let mut r = Renderer::new(dev, RenderOptions::new(W, H)).expect("renderer");
    let (scene, cam) = build(&mut r, root);
    let target = common::target(dev, W, H);
    r.render_scene(dev, &scene, &cam, tree, Tick(0), &target)
        .expect("render");
    common::read(dev, &target)
}

fn diff_fraction(a: &[u8], b: &[u8]) -> f64 {
    let n = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0.iter())
        .filter(|(p, q)| {
            p.iter()
                .zip(q.iter())
                .any(|(x, y)| x.abs_diff(*y) > CHANNEL_TOLERANCE)
        })
        .count();
    n as f64 / (a.len() / 4) as f64
}

fn golden(name: &str) -> Vec<u8> {
    let path = common::goldens_dir().join(format!("{name}.png"));
    let (w, h, px) = common::read_png(&path)
        .unwrap_or_else(|| panic!("missing golden {}: run with FORGE_BLESS=1", path.display()));
    assert_eq!((w, h), (W, H), "golden {name} has the wrong size");
    px
}

#[test]
fn render_goldens() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (tree, root) = common::tree();
    for (name, build) in scenes() {
        let img = render(dev, &tree, root, build);
        let path = common::goldens_dir().join(format!("{name}.png"));
        if common::bless() {
            std::fs::create_dir_all(common::goldens_dir()).expect("goldens dir");
            common::write_png(&path, W, H, &img);
            continue;
        }
        let frac = diff_fraction(&img, &golden(name));
        if frac > MAX_DIFF_FRACTION {
            let out = common::target_file(&format!("forge-render-goldens/{name}.actual.png"));
            common::write_png(&out, W, H, &img);
            panic!(
                "{name}: {:.3} % of pixels differ (actual written to {})",
                frac * 100.0,
                out.display()
            );
        }
        // Per-device parity figures for the evidence files (docs/evidence/linux/).
        println!(
            "{name} on {}: {:.3} % of pixels differ from the golden (limit {:.3} %)",
            dev.label(),
            frac * 100.0,
            MAX_DIFF_FRACTION * 100.0
        );
    }
}

#[test]
fn positive_control_a_changed_material_fails_the_golden() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    if common::bless() {
        return;
    }
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let img = render(dev, &tree, root, |r, root| materials_scene(r, root, true));
    let frac = diff_fraction(&img, &golden("materials"));
    assert!(
        frac > MAX_DIFF_FRACTION,
        "one sphere 0.15 rougher must fail the golden ({frac})"
    );
}
