//! Shared by the forge-render guards.
//!
//! A GPU guard never passes silently for want of an adapter: locally it prints
//! `AWAITING(no GPU adapter)` and records an observation so `cargo xtask gate` shows its row
//! as AWAITING instead of BOUND; under `CI=true` it fails (W9).

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use forge_frames::{FrameId, WorldFrame};
use forge_gpu::{AdapterPool, GpuDevice, GpuTimer, PoolOptions, wgpu};

pub fn on_ci() -> bool {
    std::env::var("CI").is_ok_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// `<workspace>/target/gate-observations/<row>.txt`.
pub fn observation_file(row: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/gate-observations")
        .join(format!("{row}.txt"))
}

/// What having no adapter means: `Ok(reason)` = AWAITING locally, `Err` = fail on CI.
pub fn no_adapter_outcome(error: &str, ci: bool) -> Result<String, String> {
    let reason = format!("no GPU adapter: {error}");
    if ci {
        Err(format!(
            "CI=true and {reason}: forge-render's GPU guards must not pass silently (W9)"
        ))
    } else {
        Ok(reason)
    }
}

/// A pool with per-pass timestamps where the adapter has them, or `None` after recording
/// AWAITING for `row` (locally); panics under `CI=true`.
pub fn pool(row: &str) -> Option<AdapterPool> {
    let opts = PoolOptions {
        wanted_features: GpuTimer::FEATURES,
        max_devices: Some(1),
        // `FORGE_RENDER_SOFTWARE=1` runs the guards on the software rasteriser only (WARP /
        // lavapipe): what a GPU-less CI runner sees.
        software: if std::env::var_os("FORGE_RENDER_SOFTWARE").is_some_and(|v| v == "1") {
            forge_gpu::SoftwarePolicy::Only
        } else {
            forge_gpu::SoftwarePolicy::FallbackOnly
        },
        ..PoolOptions::default()
    };
    match AdapterPool::new(&opts) {
        Ok(p) => {
            let _ = std::fs::remove_file(observation_file(row));
            Some(p)
        }
        Err(e) => match no_adapter_outcome(&e.to_string(), on_ci()) {
            Ok(reason) => {
                println!("{row}: AWAITING({reason})");
                let f = observation_file(row);
                if let Some(dir) = f.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&f, &reason);
                None
            }
            Err(loud) => panic!("{loud}"),
        },
    }
}

/// An sRGB render target that can be read back.
pub fn target(dev: &GpuDevice, w: u32, h: u32) -> wgpu::Texture {
    dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// RGBA8 pixels of `t`.
pub fn read(dev: &GpuDevice, t: &wgpu::Texture) -> Vec<u8> {
    forge_gpu::transfer::read_texture(dev, t).expect("readback")
}

/// The pixel at `(x, y)`.
pub fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// The world frame's resolver, and the world frame.
pub fn tree() -> (WorldFrame, FrameId) {
    (WorldFrame, FrameId::WORLD)
}

/// ACES fit + sRGB encode + 8-bit quantisation, exactly as tonemap.wgsl and an sRGB target
/// do it (f64 reference).
pub fn display(linear_exposed: f64) -> u8 {
    let x = linear_exposed.max(0.0);
    let a = ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
    let s = if a <= 0.003_130_8 {
        a * 12.92
    } else {
        1.055 * a.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Directory of committed golden images.
pub fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

pub fn bless() -> bool {
    std::env::var_os("FORGE_BLESS").is_some_and(|v| v == "1")
}

/// Write an RGBA8 PNG, creating its directory first (a fresh `CARGO_TARGET_DIR` has none).
pub fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("create png directory");
    }
    let f =
        std::fs::File::create(path).unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().expect("png header");
    wr.write_image_data(rgba).expect("png data");
}

pub fn read_png(path: &std::path::Path) -> Option<(u32, u32, Vec<u8>)> {
    let f = std::fs::File::open(path).ok()?;
    let dec = png::Decoder::new(std::io::BufReader::new(f));
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    Some((info.width, info.height, buf))
}

/// The representative frame of `test_render_perf` and `test_prepare_alloc`: a floor and
/// 2,500 instances (cubes and spheres) on the surface of a 6,371 km ground sphere, 256 point lights, the
/// sun casting shadows; the camera 8 m up looking along the grid.
pub fn perf_scene(
    r: &mut forge_render::Renderer,
    root: FrameId,
) -> (forge_render::Scene, forge_render::Camera) {
    use forge_frames::{DQuat, DVec3, FramePos};
    use forge_render::{
        Camera, DirectionalLight, Instance, Material, MeshData, PunctualLight, Scene,
    };
    let up = DVec3::new(0.61, 0.52, 0.597).try_normalize().expect("up");
    let east = DVec3::Z.cross(up).try_normalize().expect("east");
    let north = up.cross(east);
    let base = up * 6.371e6;
    let axes = forge_render::quat_from_basis(east, up, -north);
    let cube = r.add_mesh(&MeshData::cube()).expect("cube");
    let sphere = r.add_mesh(&MeshData::uv_sphere(32, 16)).expect("sphere");
    let mut mats = Vec::new();
    for i in 0..16 {
        let f = f64::from(i) / 15.0;
        mats.push(r.add_material(Material {
            base_color: [0.3 + 0.6 * f, 0.5, 0.9 - 0.6 * f, 1.0],
            metallic: if i % 4 == 0 { 1.0 } else { 0.0 },
            roughness: 0.2 + 0.6 * f,
            ..Material::default()
        }));
    }
    let at = |x: f64, y: f64, z: f64| base + east * x + up * y - north * z;
    let mut scene = Scene {
        instances: vec![
            Instance::new(FramePos::new(root, at(0.0, -0.05, -50.0)), cube, mats[0])
                .oriented(axes)
                .scaled3(DVec3::new(200.0, 0.1, 200.0)),
        ],
        sun: Some(DirectionalLight {
            frame: root,
            toward_light: (up + east * 0.4 + north * 0.3)
                .try_normalize()
                .expect("sun"),
            color: [1.0, 0.95, 0.9],
            illuminance: 5.0,
            casts_shadows: true,
        }),
        ambient: [0.05, 0.06, 0.08],
        ..Scene::default()
    };
    for i in 0..2500u32 {
        let (gx, gz) = (f64::from(i % 50), f64::from(i / 50));
        let mesh = if i % 3 == 0 { sphere } else { cube };
        scene.instances.push(
            Instance::new(
                FramePos::new(root, at(-50.0 + 2.0 * gx, 0.5, -2.0 * gz)),
                mesh,
                mats[(i % 16) as usize],
            )
            .oriented(axes * DQuat::from_axis_angle(DVec3::Y, 0.1 * f64::from(i)))
            .scaled(0.8),
        );
    }
    for i in 0..256u32 {
        let (gx, gz) = (f64::from(i % 16), f64::from(i / 16));
        scene.lights.push(PunctualLight {
            pos: FramePos::new(root, at(-45.0 + 6.0 * gx, 1.5, -3.0 - 6.0 * gz)),
            color: [1.0, 0.8, 0.6],
            intensity: 8.0,
            range: 6.0,
            spot: None,
        });
    }
    let cam = Camera::looking(
        FramePos::new(root, at(0.0, 8.0, 10.0)),
        at(0.0, 0.0, -30.0) - at(0.0, 8.0, 10.0),
        up,
        1.0,
    )
    .expect("camera");
    (scene, cam)
}

/// `<CARGO_TARGET_DIR or workspace/target>/<name>`, its directory created — where test
/// output (perf tables, golden diffs) goes, so it survives a lane-specific target dir.
pub fn target_file(name: &str) -> PathBuf {
    let dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let path = dir.join(name);
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    path
}
