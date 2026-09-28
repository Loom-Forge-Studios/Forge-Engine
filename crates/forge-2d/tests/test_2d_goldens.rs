//! `test_2d_goldens` — golden images of the 2D render path (Ch.35; DoD M4-11).
//!
//! Three generated scenes (`forge_2d::scenes`, no asset files):
//!
//! * `tiles` — autotiled blob-47 terrain with platforms and stone blocks, atlas-packed
//!   sprites (one flipped, rotated and tinted coins), a repeating parallax strip, drawn
//!   pixel-perfect at 320x180 and scaled 4x into 1280x720;
//! * `lights` — a normal-mapped brick wall in the dark under a warm point light, a spot
//!   light and a cold light, pillars and a crate casting hard shadows (smooth camera);
//! * `shapes` — a spline hill (fill and edge strip), a particle fountain after 90 fixed
//!   steps, and the `Skeleton2D` cutout figure 0.3 s into its walk.
//!
//! Each is compared with a committed PNG under a fixed threshold ([`CHANNEL_TOLERANCE`],
//! [`MAX_DIFF_FRACTION`]) that is never widened (W5). `FORGE_BLESS=1` rewrites them. With no
//! adapter the test records AWAITING (locally) or fails (`CI=true`). The Linux leg runs the
//! same test on lavapipe in Docker (`just verify-linux`, ADR 0044;
//! `docs/evidence/linux/C-2d-goldens-linux-leg.md`).
//!
//! Positive control (W2): `positive_control_a_moved_light_fails_the_golden` renders
//! `lights` with the warm light 0.25 units to the left and asserts the comparison fails;
//! `positive_control_a_lost_normal_map_fails_the_golden` renders it with the bricks' normal
//! map replaced by a flat one.

mod common;

use forge_2d::DVec2;
use forge_2d::render::{Filter2d, RenderOptions2d, Renderer2d};
use forge_2d::scenes::{self, SceneTextures};
use forge_2d::sprite::Frame2d;
use forge_gpu::GpuDevice;

const ROW: &str = "C-2d-goldens";
const W: u32 = 1280;
const H: u32 = 720;
/// A channel may differ by this many levels before a pixel counts as different.
const CHANNEL_TOLERANCE: u8 = 3;
/// At most this fraction of pixels may differ.
const MAX_DIFF_FRACTION: f64 = 0.002;

type Build = fn(&SceneTextures, &forge_2d::atlas::Atlas) -> Frame2d;

fn scenes_list() -> Vec<(&'static str, Build)> {
    vec![
        ("tiles", |t, a| scenes::tiles_scene(t, a).expect("tiles")),
        ("lights", |t, _| scenes::lights_scene(t).expect("lights")),
        ("shapes", |t, _| scenes::shapes_scene(t).expect("shapes")),
    ]
}

/// Render `build`'s scene, after `textures` may swap a texture and `frame` may edit it.
fn render_with(
    dev: &GpuDevice,
    textures: impl Fn(&mut Renderer2d, &mut SceneTextures),
    frame: impl Fn(&mut Frame2d),
    build: Build,
) -> Vec<u8> {
    let mut r = Renderer2d::new(dev, RenderOptions2d::new(W, H)).expect("renderer");
    let (mut t, atlas) = common::textures(&mut r, dev);
    textures(&mut r, &mut t);
    let mut f = build(&t, &atlas);
    frame(&mut f);
    let target = common::target(dev, W, H);
    r.render(dev, &f, &target).expect("render");
    assert!(dev.take_uncaptured_errors().is_empty());
    common::read(dev, &target)
}

fn render(dev: &GpuDevice, build: Build) -> Vec<u8> {
    render_with(dev, |_, _| {}, |_| {}, build)
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
fn golden_2d_scenes_match() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let mut report = Vec::new();
    let mut failed = Vec::new();
    for (name, build) in scenes_list() {
        let img = render(dev, build);
        let path = common::goldens_dir().join(format!("{name}.png"));
        if common::bless() {
            common::write_png(&path, W, H, &img);
            continue;
        }
        let frac = diff_fraction(&img, &golden(name));
        report.push(format!("{name}: {:.5} of pixels differ", frac));
        if frac > MAX_DIFF_FRACTION {
            // Every scene is compared (and its actual image kept) before the test fails.
            let out = common::target_file(&format!("forge-2d-goldens/{name}.actual.png"));
            common::write_png(&out, W, H, &img);
            failed.push(format!(
                "{name}: {:.4} % of pixels differ (> {:.2} %); actual written to {}",
                frac * 100.0,
                MAX_DIFF_FRACTION * 100.0,
                out.display()
            ));
        }
    }
    println!(
        "{ROW} on {} ({}-{}): {}",
        dev.label(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        report.join("; ")
    );
    assert!(
        failed.is_empty(),
        "on {}:\n{}",
        dev.label(),
        failed.join("\n")
    );
}

/// What the scenes must visibly contain (so a blessed golden cannot be of a broken frame).
#[test]
fn the_scenes_show_what_they_claim() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let scenes = scenes_list();
    let tiles = render(dev, scenes[0].1);
    // Pixel-perfect 4x: every 4x4 block is one colour.
    let mut blocks_uniform = 0;
    let mut blocks = 0;
    for by in (0..H).step_by(4) {
        for bx in (0..W).step_by(4) {
            blocks += 1;
            let p = common::px(&tiles, W, bx, by);
            if (0..4).all(|y| (0..4).all(|x| common::px(&tiles, W, bx + x, by + y) == p)) {
                blocks_uniform += 1;
            }
        }
    }
    assert_eq!(
        blocks_uniform, blocks,
        "a 4x integer upscale draws whole 4x4 blocks"
    );
    // Sky at the top, grass-green somewhere, and the ground's brown near the bottom.
    let sky = common::px(&tiles, W, 640, 8);
    assert!(sky[2] > sky[0], "sky is blue: {sky:?}");
    assert!(
        tiles
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[1] > 130 && p[0] < 110 && p[2] < 100),
        "grass edge tiles are drawn"
    );
    let lights = render(dev, scenes[1].1);
    // 64 px a unit: the warm light at (-1.5, 1.5) is pixel (544, 264). Lit under it, dark
    // on the wall's far corner, and dark in the crate's shadow just below it.
    let lit = |x: u32, y: u32| {
        (0..8)
            .flat_map(|dy| (0..8).map(move |dx| (dx, dy)))
            .map(|(dx, dy)| u32::from(common::px(&lights, W, x + dx, y + dy)[0]))
            .sum::<u32>()
            / 64
    };
    let (near, far, shadow) = (lit(540, 260), lit(1136, 600), lit(636, 342));
    assert!(near > far + 60, "lit {near} vs the far corner {far}");
    assert!(
        near > shadow + 60,
        "lit {near} vs the crate's shadow {shadow}"
    );
}

#[test]
fn positive_control_a_moved_light_fails_the_golden() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    if common::bless() {
        return;
    }
    let dev = pool.primary();
    let img = render_with(
        dev,
        |_, _| {},
        |f| {
            f.lights[0].pos.local += DVec2::new(-0.25, 0.0);
        },
        scenes_list()[1].1,
    );
    let frac = diff_fraction(&img, &golden("lights"));
    assert!(
        frac > MAX_DIFF_FRACTION,
        "a light moved 0.25 units must fail the golden: {frac}"
    );
}

#[test]
fn positive_control_a_lost_normal_map_fails_the_golden() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    if common::bless() {
        return;
    }
    let dev = pool.primary();
    let img = render_with(
        dev,
        |r, t| {
            let (bc, _) = scenes::bricks();
            t.bricks = r
                .add_texture(dev, &bc, None, Filter2d::Nearest)
                .expect("flat bricks");
        },
        |_| {},
        scenes_list()[1].1,
    );
    let frac = diff_fraction(&img, &golden("lights"));
    assert!(
        frac > MAX_DIFF_FRACTION,
        "bricks without their normal map must fail the golden: {frac}"
    );
}
