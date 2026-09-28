//! `test_atmosphere_goldens` — golden images of the sky (DoD M1-8; Ch.11 §11.2).
//!
//! Five scenes through the whole renderer — atmosphere tables built from a body's
//! composition, the sky pass (sky radiance, the sun's disc as a sky sprite) and the depth pass
//! with aerial perspective — each compared with a committed PNG under the same fixed threshold
//! as `test_render_goldens` (3 levels, 0.2 % of pixels; never widened, W5). `FORGE_BLESS=1`
//! rewrites them. Every render is also written to
//! `$CARGO_TARGET_DIR/forge-render-atmosphere/<name>.png` for inspection. The scenes are
//! built by `common/sky_scenes.rs`.
//!
//! * `earth_noon` — standing 1.7 m up on a 6,371 km Earth, the sun 40 degrees high behind the
//!   camera, towers at 30 m, 3 km and 15 km fading into the haze;
//! * `earth_sunset` — the sun disc (an emitter sprite) 1.5 degrees above the horizon, reddened
//!   by the air;
//! * `earth_orbit` — 400 km up, the limb of the ground and the thin blue shell of air;
//! * `mars_noon` — Mars' CO2 and dust, the same code: a butterscotch sky;
//! * `earth_clear_sky` (WP-18) — a dust-free Earth (Rayleigh and ozone only; `mars_noon` is
//!   dust-dominated, so it does not guard the Rayleigh phase): the sun 10 degrees up in the
//!   north, looking east along its almucantar, scattering angles ~50 to ~130 degrees across the
//!   frame at one elevation.
//!
//! The physics behind the pictures is asserted too, independently of the goldens: the noon
//! zenith is blue, the sunset sun is red, Mars' sky is redder than blue, and the clear sky has
//! Rayleigh's phase signature along the almucantar (exposed linear radiance, a float target):
//! 40 degrees either side of the 90-degree scattering angle it is at least 1.15x brighter, and
//! forward and backward agree within 15 %.
//!
//! Positive controls (W2): `positive_control_a_changed_composition_fails_the_golden` renders
//! `earth_noon` from a composition with twice the CO2-equivalent scattering (all nitrogen
//! replaced by carbon dioxide) and asserts the comparison fails;
//! `positive_control_an_isotropic_rayleigh_phase_fails_the_clear_sky` — an isotropic phase in
//! the sky pass (a hidden knob) fails the clear-sky golden and the phase signature, and a
//! hazy almucantar fails the signature too.

mod common;
#[path = "common/sky_scenes.rs"]
mod sky_scenes;

use forge_frames::{DVec3, FrameId, FramePos, Tick};
use forge_gpu::GpuDevice;
use forge_render::{
    AtmosphereSettings, Camera, DirectionalLight, Instance, Material, MeshData, RenderOptions,
    Renderer, Scene, SceneAtmosphere,
};
use forge_sky::{Aerosols, AtmosphereBody, Composition, Gas};
use sky_scenes::{
    AU, Build, H, MAX_DIFF_FRACTION, Place, W, diff_fraction, golden, ground_grid, ground_material,
    mean, render, render_with, standing, sun_light, sun_lux, sun_sprite,
};

const ROW: &str = "C-atmosphere-goldens";

fn earth_noon(dev: &GpuDevice, r: &mut Renderer, root: FrameId) -> (Scene, Camera) {
    standing(dev, r, root, &AtmosphereBody::earth(), 40.0, 200.0, 14.0)
}

fn earth_sunset(dev: &GpuDevice, r: &mut Renderer, root: FrameId) -> (Scene, Camera) {
    standing(dev, r, root, &AtmosphereBody::earth(), 1.5, -12.0, 11.0)
}

fn mars_noon(dev: &GpuDevice, r: &mut Renderer, root: FrameId) -> (Scene, Camera) {
    standing(dev, r, root, &AtmosphereBody::mars(), 35.0, 160.0, 13.0)
}

fn co2_earth_noon(dev: &GpuDevice, r: &mut Renderer, root: FrameId) -> (Scene, Camera) {
    let body = AtmosphereBody {
        composition: Composition {
            gases: vec![(Gas::CO2, 0.79), (Gas::O2, 0.21)],
        },
        ..AtmosphereBody::earth()
    };
    standing(dev, r, root, &body, 40.0, 200.0, 14.0)
}

fn earth_orbit(dev: &GpuDevice, r: &mut Renderer, root: FrameId) -> (Scene, Camera) {
    let body = AtmosphereBody::earth();
    let pl = Place::new(body.ground_radius_m);
    r.set_atmosphere(
        dev,
        &body.derive().expect("atmosphere"),
        &AtmosphereSettings::fast(),
    )
    .expect("tables");
    let sphere = r.add_mesh(&MeshData::uv_sphere(256, 128)).expect("sphere");
    let ground = r.add_material(Material {
        base_color: [0.12, 0.16, 0.22, 1.0],
        roughness: 0.9,
        ..Material::default()
    });
    let mut scene = Scene {
        instances: vec![
            Instance::new(FramePos::new(root, DVec3::ZERO), sphere, ground)
                .scaled(2.0 * body.ground_radius_m),
        ],
        ev100: 14.0,
        atmosphere: Some(SceneAtmosphere {
            centre: FramePos::new(root, DVec3::ZERO),
        }),
        ..Scene::default()
    };
    let eye = pl.at(0.0, 400_000.0, 0.0);
    let toward = pl.dir(20.0, 90.0);
    let e_sun = sun_lux(AU);
    scene.sun = Some(DirectionalLight {
        casts_shadows: false,
        ..sun_light(root, toward, e_sun)
    });
    let cam =
        Camera::looking(FramePos::new(root, eye), pl.dir(-12.0, 0.0), pl.up, 1.0).expect("camera");
    (scene, cam)
}

/// The sun's elevation in the clear-sky scene, degrees (azimuth 0: north).
const CLEAR_SUN_ELEV: f64 = 10.0;

/// A dust-free Earth (no aerosols: Rayleigh scattering and ozone only), the sun 10 degrees up
/// in the north, the camera 1.7 m up looking east along the sun's almucantar: the frame spans
/// scattering angles from ~50 degrees (left) through 90 (centre) to ~130 (right) at one
/// elevation, so one optical path and the Rayleigh phase function `1 + cos^2` alone shape it.
fn earth_clear_sky_with(
    dev: &GpuDevice,
    r: &mut Renderer,
    root: FrameId,
    aerosols: Aerosols,
) -> (Scene, Camera) {
    let body = AtmosphereBody {
        aerosols,
        ..AtmosphereBody::earth()
    };
    let pl = Place::new(body.ground_radius_m);
    r.set_atmosphere(
        dev,
        &body.derive().expect("atmosphere"),
        &AtmosphereSettings::fast(),
    )
    .expect("tables");
    let grid = r.add_mesh(&ground_grid()).expect("ground");
    let ground = ground_material(r);
    let mut scene = Scene {
        instances: vec![
            Instance::new(FramePos::new(root, pl.at(0.0, 0.0, 0.0)), grid, ground)
                .oriented(pl.axes()),
        ],
        ev100: 13.0,
        atmosphere: Some(SceneAtmosphere {
            centre: FramePos::new(root, DVec3::ZERO),
        }),
        ..Scene::default()
    };
    let toward = pl.dir(CLEAR_SUN_ELEV, 0.0);
    let e_sun = sun_lux(AU);
    scene.sun = Some(sun_light(root, toward, e_sun));
    let eye = pl.at(0.0, 1.7, 0.0);
    scene.sky_sprites.push(sun_sprite(root, eye, toward, AU));
    let cam = Camera::looking(
        FramePos::new(root, eye),
        pl.dir(CLEAR_SUN_ELEV, 90.0),
        pl.up,
        1.0,
    )
    .expect("camera");
    (scene, cam)
}

fn earth_clear_sky(dev: &GpuDevice, r: &mut Renderer, root: FrameId) -> (Scene, Camera) {
    earth_clear_sky_with(dev, r, root, Aerosols::none())
}

fn hazy_almucantar(dev: &GpuDevice, r: &mut Renderer, root: FrameId) -> (Scene, Camera) {
    earth_clear_sky_with(dev, r, root, Aerosols::earth_continental())
}

fn scenes() -> [(&'static str, Build); 5] {
    [
        ("earth_noon", earth_noon),
        ("earth_sunset", earth_sunset),
        ("earth_orbit", earth_orbit),
        ("mars_noon", mars_noon),
        ("earth_clear_sky", earth_clear_sky),
    ]
}

fn almucantar_ratios(
    dev: &GpuDevice,
    root: FrameId,
    build: Build,
    isotropic_rayleigh: bool,
) -> (f64, f64, f64) {
    let (tree, _) = common::tree();
    let opts = RenderOptions {
        output_format: forge_gpu::wgpu::TextureFormat::Rgba32Float,
        linear_output: true,
        isotropic_rayleigh_for_tests: isotropic_rayleigh,
        ..RenderOptions::new(W, H)
    };
    let mut r = Renderer::new(dev, opts).expect("renderer");
    let (scene, cam) = build(dev, &mut r, root);
    let target = dev
        .device
        .create_texture(&forge_gpu::wgpu::TextureDescriptor {
            label: Some("clear sky linear"),
            size: forge_gpu::wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: forge_gpu::wgpu::TextureDimension::D2,
            format: forge_gpu::wgpu::TextureFormat::Rgba32Float,
            usage: forge_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT
                | forge_gpu::wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
    r.render_scene(dev, &scene, &cam, &tree, Tick(0), &target)
        .expect("render");
    assert!(dev.take_uncaptured_errors().is_empty());
    let raw = forge_gpu::transfer::read_texture(dev, &target).expect("readback");
    let px: Vec<f32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let pl = Place::new(AtmosphereBody::earth().ground_radius_m);
    let t = cam.tan_half_fov();
    let aspect = f64::from(W) / f64::from(H);
    let green_at = |az: f64| {
        let v = cam.orientation.inverse_rotate(pl.dir(CLEAR_SUN_ELEV, az));
        let x = (v.x / -v.z / (t * aspect) + 1.0) * 0.5 * f64::from(W);
        let y = (1.0 - v.y / -v.z / t) * 0.5 * f64::from(H);
        let (cx, cy) = (x.round() as i64, y.round() as i64);
        let mut sum = 0.0;
        let mut n = 0.0;
        for yy in cy - 4..=cy + 4 {
            for xx in cx - 4..=cx + 4 {
                assert!(
                    (0..i64::from(W)).contains(&xx) && (0..i64::from(H)).contains(&yy),
                    "azimuth {az} is out of the frame"
                );
                sum += f64::from(px[((yy * i64::from(W) + xx) * 4 + 1) as usize]);
                n += 1.0;
            }
        }
        sum / n
    };
    let (fwd, side, back) = (green_at(50.0), green_at(90.0), green_at(130.0));
    (fwd / side, back / side, fwd / back)
}

/// A dust-free sky follows Rayleigh's phase function along the almucantar: brighter 40 degrees
/// either side of the 90-degree scattering angle, and as bright forward as backward.
fn rayleigh_signature(ratios: (f64, f64, f64)) -> Result<(), String> {
    let (fwd, back, sym) = ratios;
    if fwd < 1.15 || back < 1.15 {
        return Err(format!(
            "no Rayleigh dip at 90 degrees: forward/side {fwd:.3}, backward/side {back:.3}"
        ));
    }
    if !(0.87..=1.15).contains(&sym) {
        return Err(format!("forward/backward {sym:.3}: not symmetric"));
    }
    Ok(())
}
#[test]
fn atmosphere_goldens() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let mut failures = Vec::new();
    for (name, build) in scenes() {
        let img = render(dev, &tree, root, build);
        let out = common::target_file(&format!("forge-render-atmosphere/{name}.png"));
        common::write_png(&out, W, H, &img);
        // The physics, independent of the golden.
        match name {
            "earth_noon" => {
                let top = mean(&img, 0, W, 0, 20);
                assert!(top[2] > top[0] * 1.3, "the noon sky is blue: {top:?}");
            }
            "earth_sunset" => {
                let horizon = mean(&img, 0, W, 110, 135);
                assert!(
                    horizon[0] > horizon[2],
                    "the sunset horizon is red: {horizon:?}"
                );
            }
            "mars_noon" => {
                let top = mean(&img, 0, W, 0, 40);
                assert!(top[0] > top[2], "Mars' sky is butterscotch: {top:?}");
            }
            "earth_clear_sky" => {
                let ratios = almucantar_ratios(dev, root, earth_clear_sky, false);
                println!(
                    "clear sky almucantar (forward/side, backward/side, forward/backward): {ratios:.3?}"
                );
                if let Err(e) = rayleigh_signature(ratios) {
                    panic!("the dust-free sky: {e}");
                }
            }
            _ => {}
        }
        let path = common::goldens_dir().join(format!("{name}.png"));
        if common::bless() {
            std::fs::create_dir_all(common::goldens_dir()).expect("goldens dir");
            common::write_png(&path, W, H, &img);
            continue;
        }
        let frac = diff_fraction(&img, &golden(name));
        if frac > MAX_DIFF_FRACTION {
            failures.push(format!(
                "{name}: {:.3} % of pixels differ (actual written to {})",
                frac * 100.0,
                out.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn positive_control_a_changed_composition_fails_the_golden() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    if common::bless() {
        return;
    }
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let img = render(dev, &tree, root, co2_earth_noon);
    let frac = diff_fraction(&img, &golden("earth_noon"));
    assert!(
        frac > MAX_DIFF_FRACTION,
        "a CO2 atmosphere must fail Earth's golden ({frac})"
    );
}

/// The clear-sky golden guards the Rayleigh phase function: with sunlight scattered off the
/// air isotropically in the sky pass (a hidden renderer knob), the golden fails and so does
/// the almucantar's phase signature (no dip at 90 degrees). And the signature is Rayleigh's,
/// not any sky's: the same almucantar under Earth's continental haze (forward-scattering Mie)
/// is not symmetric.
#[test]
fn positive_control_an_isotropic_rayleigh_phase_fails_the_clear_sky() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    if common::bless() {
        return;
    }
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let img = render_with(dev, &tree, root, earth_clear_sky, true);
    let frac = diff_fraction(&img, &golden("earth_clear_sky"));
    assert!(
        frac > MAX_DIFF_FRACTION,
        "an isotropic Rayleigh phase must fail the clear-sky golden ({frac})"
    );
    let iso = almucantar_ratios(dev, root, earth_clear_sky, true);
    println!("isotropic phase: {iso:.3?}");
    assert!(
        rayleigh_signature(iso).is_err(),
        "an isotropic phase must lose the dip at 90 degrees: {iso:.3?}"
    );
    let hazy = almucantar_ratios(dev, root, hazy_almucantar, false);
    println!("continental haze: {hazy:.3?}");
    assert!(
        rayleigh_signature(hazy).is_err(),
        "a hazy sky must not pass for Rayleigh's: {hazy:.3?}"
    );
}
