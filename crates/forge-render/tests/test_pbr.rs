//! `test_pbr` — PBR metal-rough and the light types, checked against an independent `f64`
//! reference (Ch.10 §10.2/§10.4, DoD M1-7).
//!
//! A flat quad fills the view 2 m away, facing the camera and tilted 57 degrees; for each
//! material (dielectric and metal, smooth to rough, coloured) and each light — the sun at
//! several angles including the mirror direction, a point light, a spot light — the
//! rendered pixel at nine screen positions (so the view vector varies) must equal the
//! reference: Lambert + GGX / height-correlated Smith / Schlick, inverse-square windowed
//! falloff, smoothstep-free spot cone, ACES and sRGB, quantised to 8 bits — within 2 levels.
//!
//! Positive control (W2): `positive_control_a_wrong_brdf_is_caught` repeats the comparison
//! with a reference missing one term (Fresnel, or the GGX peak) and asserts the pixels
//! disagree, so the comparison can see a wrong specular lobe.

mod common;

use forge_frames::{DQuat, DVec3, FrameId, FramePos, Tick, WorldFrame};
use forge_gpu::AdapterPool;
use forge_render::{
    Camera, DirectionalLight, Instance, Material, MeshData, PunctualLight, RenderOptions, Renderer,
    Scene, Spot,
};

const ROW: &str = "C-pbr-matches-reference";
const W: u32 = 96;
const H: u32 = 96;
const FOV: f64 = 1.0;
const DIST: f64 = 2.0;

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    None,
    NoFresnel,
    FlatDistribution,
}

fn norm(v: DVec3) -> DVec3 {
    v.try_normalize().expect("unit")
}

/// Reference reflected radiance per unit illuminance times N.L (the shader's brdf_nl).
fn brdf_nl(n: DVec3, v: DVec3, l: DVec3, m: &Material, fault: Fault) -> [f64; 3] {
    let nl = n.dot(l).clamp(0.0, 1.0);
    if nl <= 0.0 {
        return [0.0; 3];
    }
    let h = norm(v + l);
    let nv = n.dot(v).max(1e-4);
    let nh = n.dot(h).clamp(0.0, 1.0);
    let vh = v.dot(h).clamp(0.0, 1.0);
    let rough = m.roughness.clamp(0.045, 1.0);
    let a = rough * rough;
    let a2 = a * a;
    let f = (nh * a2 - nh) * nh + 1.0;
    let d = if fault == Fault::FlatDistribution {
        1.0 / core::f64::consts::PI
    } else {
        a2 / (core::f64::consts::PI * f * f)
    };
    let gv = nl * (nv * nv * (1.0 - a2) + a2).sqrt();
    let gl = nv * (nl * nl * (1.0 - a2) + a2).sqrt();
    let vis = 0.5 / (gv + gl).max(1e-7);
    let fres = (1.0 - vh).powi(5);
    let mut out = [0.0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let base = m.base_color[k];
        let diffuse = base * (1.0 - m.metallic);
        let d0 = 0.16 * m.reflectance * m.reflectance;
        let f0 = d0 + (base - d0) * m.metallic;
        let fr = if fault == Fault::NoFresnel {
            f0
        } else {
            f0 + (1.0 - f0) * fres
        };
        *o = (diffuse / core::f64::consts::PI + fr * d * vis) * nl;
    }
    out
}

enum Light {
    Sun(DVec3),
    Point(DVec3),
    Spot(DVec3, DVec3),
}

const SUN_LUX: f64 = 2.0;
const CANDELA: f64 = 3.0;
const RANGE: f64 = 6.0;

fn materials() -> Vec<Material> {
    let m = |base: [f64; 3], metallic: f64, roughness: f64| Material {
        base_color: [base[0], base[1], base[2], 1.0],
        metallic,
        roughness,
        reflectance: 0.5,
        emissive: [0.0; 3],
    };
    vec![
        m([0.8, 0.8, 0.8], 0.0, 0.9),
        m([0.9, 0.2, 0.1], 0.0, 0.35),
        m([0.1, 0.4, 0.9], 0.0, 0.08),
        m([0.95, 0.64, 0.54], 1.0, 0.3),
        m([0.91, 0.92, 0.92], 1.0, 0.6),
        m([1.0, 0.78, 0.34], 1.0, 0.12),
    ]
}

/// Quad tilts (radians about view +x) and the lights used with each. The tilted quad is
/// seen at 57 degrees from its normal, with the sun and a point light near the mirror
/// direction, where Fresnel and the GGX peak dominate.
fn configs() -> Vec<(f64, Vec<Light>)> {
    let tilt = 1.0;
    let n = plane_normal(tilt);
    let v = DVec3::Z;
    let mirror = norm(n * (2.0 * n.dot(v)) - v);
    vec![
        (
            0.0,
            vec![
                Light::Sun(norm(DVec3::new(0.0, 0.0, 1.0))),
                Light::Sun(norm(DVec3::new(0.4, 0.3, 1.0))),
                Light::Sun(norm(DVec3::new(-0.9, 0.2, 0.5))),
                Light::Point(DVec3::new(0.25, -0.15, -1.2)),
                Light::Spot(
                    DVec3::new(-0.2, 0.1, -0.9),
                    norm(DVec3::new(0.1, -0.05, -1.0)),
                ),
            ],
        ),
        (
            tilt,
            vec![
                Light::Sun(mirror),
                Light::Sun(norm(mirror + DVec3::new(0.3, 0.1, 0.0))),
                Light::Point(DVec3::new(0.0, 0.0, -DIST) + mirror * 1.5),
            ],
        ),
    ]
}

fn plane_normal(tilt: f64) -> DVec3 {
    DQuat::from_axis_angle(DVec3::X, tilt).rotate(DVec3::Z)
}

const INNER: f64 = 0.25;
const OUTER: f64 = 0.45;

/// The expected display value at pixel `(px, py)`, or `None` if its ray misses the quad.
#[allow(clippy::too_many_arguments)]
fn expected(
    m: &Material,
    tilt: f64,
    light: &Light,
    px: u32,
    py: u32,
    exposure: f64,
    fault: Fault,
) -> Option<[u8; 3]> {
    let t = (0.5 * FOV).tan();
    let aspect = f64::from(W) / f64::from(H);
    let nx = 2.0 * (f64::from(px) + 0.5) / f64::from(W) - 1.0;
    let ny = 1.0 - 2.0 * (f64::from(py) + 0.5) / f64::from(H);
    let ray = norm(DVec3::new(nx * aspect * t, ny * t, -1.0));
    let n = plane_normal(tilt);
    let p0 = DVec3::new(0.0, 0.0, -DIST);
    let denom = ray.dot(n);
    if denom.abs() < 1e-6 {
        return None;
    }
    let hit = p0.dot(n) / denom;
    let p = ray * hit;
    if hit <= 0.0 || (p - p0).length() > 0.45 * QUAD {
        return None;
    }
    let v = norm(-p);
    let (l, e) = match light {
        Light::Sun(l) => (*l, SUN_LUX),
        Light::Point(c) | Light::Spot(c, _) => {
            let to = *c - p;
            let d2 = to.length_squared();
            let x = d2 / (RANGE * RANGE);
            let win = (1.0 - x * x).clamp(0.0, 1.0);
            let mut att = win * win / d2;
            let l = norm(to);
            if let Light::Spot(_, dir) = light {
                let (co, ci) = (OUTER.cos(), INNER.cos());
                let scale = 1.0 / (ci - co).max(1e-4);
                let s = ((-l).dot(*dir) * scale - co * scale).clamp(0.0, 1.0);
                att *= s * s;
            }
            (l, CANDELA * att)
        }
    };
    let b = brdf_nl(n, v, l, m, fault);
    Some([0, 1, 2].map(|k| common::display(b[k] * e * exposure)))
}

const QUAD: f64 = 40.0;

fn scene_for(r: &mut Renderer, root: FrameId, m: Material, tilt: f64, light: &Light) -> Scene {
    let quad = r.add_mesh(&MeshData::quad()).expect("quad");
    let mat = r.add_material(m);
    let mut s = Scene {
        instances: vec![
            Instance::new(FramePos::new(root, DVec3::new(0.0, 0.0, -DIST)), quad, mat)
                .oriented(DQuat::from_axis_angle(DVec3::X, tilt))
                .scaled(QUAD),
        ],
        ..Scene::default()
    };
    match light {
        Light::Sun(l) => {
            s.sun = Some(DirectionalLight {
                frame: root,
                toward_light: *l,
                color: [1.0; 3],
                illuminance: SUN_LUX,
                casts_shadows: false,
            });
        }
        Light::Point(c) => s.lights.push(PunctualLight {
            pos: FramePos::new(root, *c),
            color: [1.0; 3],
            intensity: CANDELA,
            range: RANGE,
            spot: None,
        }),
        Light::Spot(c, d) => s.lights.push(PunctualLight {
            pos: FramePos::new(root, *c),
            color: [1.0; 3],
            intensity: CANDELA,
            range: RANGE,
            spot: Some(Spot {
                direction: *d,
                inner_angle: INNER,
                outer_angle: OUTER,
            }),
        }),
    }
    s
}

/// Worst channel difference between the render and the reference, over every material,
/// light and sample pixel.
fn worst(pool: &AdapterPool, fault: Fault) -> (u8, String) {
    let dev = pool.primary();
    let (tree, root): (WorldFrame, FrameId) = common::tree();
    let cam = Camera::looking(
        FramePos::origin_of(root),
        DVec3::new(0.0, 0.0, -1.0),
        DVec3::Y,
        FOV,
    )
    .expect("cam");
    let mut worst = (0u8, String::new());
    let target = common::target(dev, W, H);
    for (mi, m) in materials().into_iter().enumerate() {
        for (li, (tilt, light)) in configs()
            .iter()
            .flat_map(|(t, ls)| ls.iter().map(move |l| (*t, l)))
            .enumerate()
        {
            let mut opts = RenderOptions::new(W, H);
            opts.shadows = None;
            let mut r = Renderer::new(dev, opts).expect("renderer");
            let scene = scene_for(&mut r, root, m, tilt, light);
            r.render_scene(dev, &scene, &cam, &tree, Tick(0), &target)
                .expect("render");
            let img = common::read(dev, &target);
            for py in [10, H / 2, H - 11] {
                for px in [10, W / 2, W - 11] {
                    let got = common::px(&img, W, px, py);
                    let Some(want) = expected(&m, tilt, light, px, py, scene.exposure(), fault)
                    else {
                        continue;
                    };
                    for k in 0..3 {
                        let d = got[k].abs_diff(want[k]);
                        if d > worst.0 {
                            worst = (
                                d,
                                format!(
                                    "material {mi} light {li} pixel ({px},{py}): got {got:?} want {want:?}"
                                ),
                            );
                        }
                    }
                }
            }
        }
    }
    worst
}

#[test]
fn rendered_pbr_matches_the_f64_reference() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let (d, at) = worst(&pool, Fault::None);
    println!("worst difference {d} levels at {at}");
    assert!(d <= 2, "{d} levels off the reference at {at}");
}

#[test]
fn positive_control_a_wrong_brdf_is_caught() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    for fault in [Fault::NoFresnel, Fault::FlatDistribution] {
        let (d, at) = worst(&pool, fault);
        assert!(
            d > 2,
            "a reference with a missing term should disagree (worst {d} at {at})"
        );
    }
}
