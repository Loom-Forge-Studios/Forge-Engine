//! Shared by the forge-2d GPU guards. A GPU guard never passes silently for want of an
//! adapter: locally it prints `AWAITING(no GPU adapter)` and records an observation so
//! `cargo xtask gate` shows its row as AWAITING; under `CI=true` it fails (W9).

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use forge_2d::atlas::Atlas;
use forge_2d::render::{Filter2d, Renderer2d};
use forge_2d::scenes::{self, SceneTextures};
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

/// A pool (timestamps where available), or `None` after recording AWAITING for `row`.
pub fn pool(row: &str) -> Option<AdapterPool> {
    let opts = PoolOptions {
        wanted_features: GpuTimer::FEATURES,
        max_devices: Some(1),
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
        Err(e) => {
            assert!(
                !on_ci(),
                "CI=true and no GPU adapter ({e}): {row} must not pass silently (W9)"
            );
            println!("{row}: AWAITING(no GPU adapter: {e})");
            let f = observation_file(row);
            if let Some(d) = f.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&f, format!("no GPU adapter: {e}"));
            None
        }
    }
}

/// An sRGB render target that can be read back.
pub fn target(dev: &GpuDevice, w: u32, h: u32) -> wgpu::Texture {
    dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("forge-2d test target"),
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

pub fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// Upload the generated scene textures.
pub fn textures(r: &mut Renderer2d, dev: &GpuDevice) -> (SceneTextures, Atlas) {
    let tiles = r
        .add_texture(dev, &scenes::tileset_image(), None, Filter2d::Nearest)
        .expect("tiles");
    let atlas = scenes::sprite_atlas().expect("atlas");
    let atlas_tex = r
        .add_texture(dev, &atlas.pages[0], None, Filter2d::Nearest)
        .expect("atlas page");
    let (bc, bn) = scenes::bricks();
    let bricks = r
        .add_texture(dev, &bc, Some(&bn), Filter2d::Nearest)
        .expect("bricks");
    (
        SceneTextures {
            tiles,
            atlas: atlas_tex,
            bricks,
            white: r.white(),
        },
        atlas,
    )
}

pub fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

pub fn bless() -> bool {
    std::env::var_os("FORGE_BLESS").is_some_and(|v| v == "1")
}

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

/// `<CARGO_TARGET_DIR or workspace/target>/<name>`, its directory created.
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
