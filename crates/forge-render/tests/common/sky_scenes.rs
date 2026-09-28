//! The sky scenes the atmosphere goldens are rendered from (Ch.11 §11.2): a place on a
//! ground sphere, flat ground to 40 km with towers, the sun as a directional light and as a sky
//! sprite, and the golden-image comparison. Shared by `test_atmosphere_goldens` and by any
//! crate whose guards render a sky over the same ground (they include this file).

#![allow(dead_code)]

use forge_frames::{DVec3, FrameId, FramePos, Tick, WorldFrame};
use forge_gpu::GpuDevice;
use forge_render::{
    AtmosphereSettings, Camera, DirectionalLight, Instance, Material, MeshData, RenderOptions,
    Renderer, Scene, SceneAtmosphere, SkyShine, SkySprite,
};
use forge_sky::AtmosphereBody;

pub const W: u32 = 480;
pub const H: u32 = 270;
pub const CHANNEL_TOLERANCE: u8 = 3;
pub const MAX_DIFF_FRACTION: f64 = 0.002;

/// The Sun's luminous flux, lumens (93 lm/W x 3.828e26 W): 128 klx at 1 AU.
pub const SUN_LUMINOUS_FLUX: f64 = 3.6e28;
/// The Sun's effective temperature, kelvin.
pub const SUN_TEMPERATURE: f64 = 5772.0;
/// The Sun's radius, metres.
pub const SUN_RADIUS: f64 = 6.957e8;
/// The sun's distance, metres.
pub const AU: f64 = 1.495_978_707e11;

/// A blackbody's colour at the renderer's wavelengths, normalised to unit luminance (Planck's
/// law up to a constant; Rec. 709 luminance weights).
pub fn blackbody_rgb(t: f64) -> [f64; 3] {
    const C2: f64 = 1.438_776_877e-2; // h c / k, m K
    const LUMINANCE: [f64; 3] = [0.2126, 0.7152, 0.0722];
    let t = t.clamp(1000.0, 60_000.0);
    let planck = |lambda: f64| {
        let l5 = lambda * lambda * lambda * lambda * lambda;
        1.0 / (l5 * (forge_num::det::exp(C2 / (lambda * t)) - 1.0))
    };
    let c = forge_sky::WAVELENGTHS.map(planck);
    let y = c[0] * LUMINANCE[0] + c[1] * LUMINANCE[1] + c[2] * LUMINANCE[2];
    c.map(|v| v / y)
}

/// The Sun's illuminance `distance` from it, lux.
pub fn sun_lux(distance: f64) -> f64 {
    SUN_LUMINOUS_FLUX / (4.0 * core::f64::consts::PI * distance * distance)
}

/// A place on a ground sphere of radius `radius`: local east / up / north at a fixed direction.
pub struct Place {
    pub up: DVec3,
    pub east: DVec3,
    pub north: DVec3,
    pub radius: f64,
}

impl Place {
    pub fn new(radius: f64) -> Self {
        let up = DVec3::new(0.61, 0.52, 0.597).try_normalize().expect("unit");
        let east = DVec3::Z.cross(up).try_normalize().expect("east");
        let north = up.cross(east);
        Self {
            up,
            east,
            north,
            radius,
        }
    }
    /// `x` east, `y` up (from the surface), `z` south.
    pub fn at(&self, x: f64, y: f64, z: f64) -> DVec3 {
        self.up * (self.radius + y) + self.east * x - self.north * z
    }
    /// A direction from elevation and azimuth (degrees; azimuth 0 = north, 90 = east).
    pub fn dir(&self, elev: f64, az: f64) -> DVec3 {
        let (e, a) = (elev.to_radians(), az.to_radians());
        self.up * e.sin() + (self.north * a.cos() + self.east * a.sin()) * e.cos()
    }
    pub fn axes(&self) -> forge_frames::DQuat {
        forge_render::quat_from_basis(self.east, self.up, -self.north)
    }
}

pub type Build = fn(&GpuDevice, &mut Renderer, FrameId) -> (Scene, Camera);

pub fn sun_light(root: FrameId, toward: DVec3, lux: f64) -> DirectionalLight {
    DirectionalLight {
        frame: root,
        toward_light: toward,
        color: blackbody_rgb(SUN_TEMPERATURE),
        illuminance: lux,
        casts_shadows: true,
    }
}

/// The sun as a sky sprite `distance` from `camera` toward `toward`: its disc and its light
/// there.
pub fn sun_sprite(root: FrameId, camera: DVec3, toward: DVec3, distance: f64) -> SkySprite {
    let e = sun_lux(distance);
    SkySprite {
        pos: FramePos::new(root, camera + toward * distance),
        radius: SUN_RADIUS,
        shine: SkyShine::Emitter {
            illuminance: blackbody_rgb(SUN_TEMPERATURE).map(|c| c * e),
        },
    }
}

/// Flat ground to +-40 km as a grid whose cells grow geometrically from ~2 m at the centre:
/// tessellated like terrain LOD, so no triangle spans many decades of distance (one 80 km quad
/// loses its depth precision to f32 clipping and shows shadow acne, Ch.11 §11.1).
pub fn ground_grid() -> MeshData {
    const N: i32 = 40;
    let s = |i: i32| {
        let u = f64::from(i) / f64::from(N);
        u.signum() * 10.0 * ((u.abs() * 4001f64.ln()).exp() - 1.0)
    };
    let mut m = MeshData::default();
    for j in -N..=N {
        for i in -N..=N {
            m.vertices.push(forge_render::Vertex {
                local: DVec3::new(s(i), 0.0, s(j)),
                normal: DVec3::Y,
                uv: [0.0, 0.0],
            });
        }
    }
    let row = (2 * N + 1) as u32;
    for j in 0..row - 1 {
        for i in 0..row - 1 {
            let a = j * row + i;
            m.indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    m
}

pub fn ground_material(r: &mut Renderer) -> forge_render::MaterialId {
    r.add_material(Material {
        base_color: [0.16, 0.18, 0.11, 1.0],
        roughness: 0.95,
        ..Material::default()
    })
}

/// A standing scene: flat ground to 40 km, three towers, the sun at (elev, az).
pub fn standing(
    dev: &GpuDevice,
    r: &mut Renderer,
    root: FrameId,
    body: &AtmosphereBody,
    sun_elev: f64,
    sun_az: f64,
    ev100: f64,
) -> (Scene, Camera) {
    let pl = Place::new(body.ground_radius_m);
    r.set_atmosphere(
        dev,
        &body.derive().expect("atmosphere"),
        &AtmosphereSettings::fast(),
    )
    .expect("tables");
    let cube = r.add_mesh(&MeshData::cube()).expect("cube");
    let grid = r.add_mesh(&ground_grid()).expect("ground");
    let ground = ground_material(r);
    let tower = r.add_material(Material {
        base_color: [0.6, 0.6, 0.62, 1.0],
        roughness: 0.7,
        ..Material::default()
    });
    // Looking north.
    let look_az = 0.0;
    let fwd = pl.dir(0.0, look_az);
    let side = fwd.cross(pl.up);
    let place = |d: f64, s: f64| pl.at(0.0, 0.0, 0.0) + fwd * d + side * s - pl.up * pl.radius;
    let mut scene = Scene {
        instances: vec![
            Instance::new(FramePos::new(root, pl.at(0.0, 0.0, 0.0)), grid, ground)
                .oriented(pl.axes()),
        ],
        ev100,
        atmosphere: Some(SceneAtmosphere {
            centre: FramePos::new(root, DVec3::ZERO),
        }),
        ..Scene::default()
    };
    for (d, s, size) in [
        (30.0, 6.0, 4.0),
        (3_000.0, -400.0, 120.0),
        (15_000.0, 1_500.0, 600.0),
    ] {
        let base = place(d, s) + pl.up * (pl.radius + size * 0.5);
        scene.instances.push(
            Instance::new(FramePos::new(root, base), cube, tower)
                .oriented(pl.axes())
                .scaled3(DVec3::new(size * 0.3, size, size * 0.3)),
        );
    }
    let toward = pl.dir(sun_elev, sun_az);
    let e_sun = SUN_LUMINOUS_FLUX / (4.0 * core::f64::consts::PI * AU * AU);
    scene.sun = Some(sun_light(root, toward, e_sun));
    let eye = pl.at(0.0, 1.7, 0.0);
    scene.sky_sprites.push(sun_sprite(root, eye, toward, AU));
    let cam = Camera::looking(FramePos::new(root, eye), pl.dir(4.0, look_az), pl.up, 1.0)
        .expect("camera");
    (scene, cam)
}

pub fn render(dev: &GpuDevice, tree: &WorldFrame, root: FrameId, build: Build) -> Vec<u8> {
    render_with(dev, tree, root, build, false)
}

pub fn render_with(
    dev: &GpuDevice,
    tree: &WorldFrame,
    root: FrameId,
    build: Build,
    isotropic_rayleigh: bool,
) -> Vec<u8> {
    let opts = RenderOptions {
        isotropic_rayleigh_for_tests: isotropic_rayleigh,
        ..RenderOptions::new(W, H)
    };
    let mut r = Renderer::new(dev, opts).expect("renderer");
    let (scene, cam) = build(dev, &mut r, root);
    let target = crate::common::target(dev, W, H);
    r.render_scene(dev, &scene, &cam, tree, Tick(0), &target)
        .expect("render");
    assert!(dev.take_uncaptured_errors().is_empty());
    crate::common::read(dev, &target)
}

pub fn diff_fraction(a: &[u8], b: &[u8]) -> f64 {
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

pub fn golden(name: &str) -> Vec<u8> {
    let path = crate::common::goldens_dir().join(format!("{name}.png"));
    let (w, h, px) = crate::common::read_png(&path)
        .unwrap_or_else(|| panic!("missing golden {}: run with FORGE_BLESS=1", path.display()));
    assert_eq!((w, h), (W, H), "golden {name} has the wrong size");
    px
}

/// Mean colour of a rectangle of the image (x0..x1, y0..y1 in pixels).
pub fn mean(img: &[u8], x0: u32, x1: u32, y0: u32, y1: u32) -> [f64; 3] {
    let mut s = [0.0; 3];
    let mut n = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = crate::common::px(img, W, x, y);
            for k in 0..3 {
                s[k] += f64::from(p[k]);
            }
            n += 1.0;
        }
    }
    s.map(|v| v / n)
}
