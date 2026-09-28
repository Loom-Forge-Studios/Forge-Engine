//! Display-list and pixel goldens (Ch.21 §21.13, §21.23; DoD M2-19).
//!
//! * **Display-list goldens** are exact hashes of the gallery's display list in all three
//!   themes at scale 1.0 and 2.0. They need no GPU. Text lays out with the **bundled** faces
//!   only (`FontConfig::bundled_only`, D-7, ADR 0013): no system fallback, so every machine
//!   resolves the same fonts and the goldens compare for real everywhere. The golden file
//!   records the bundle's fingerprint, so a font change fails with a clear message.
//! * **Pixel goldens** render the gallery through `WgpuRenderer` into an offscreen target
//!   and compare against committed PNGs with a fixed perceptual threshold
//!   ([`CHANNEL_TOLERANCE`], [`MAX_DIFF_FRACTION`]) that is never widened (W5). With no
//!   adapter they cannot run: on a developer machine the test records an observation that
//!   makes `cargo xtask gate` report row `C-ui-pixel-goldens` as `AWAITING(no GPU adapter)`
//!   (never a silent BOUND); with `CI=true` it fails loudly.
//!
//! `FORGE_BLESS=1 cargo test -p forge-ui --test test_ui_goldens` rewrites the goldens.
//!
//! Positive controls: a one-pixel padding change fails both kinds.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use forge_ui::gallery::{self, GalleryFaults};
use forge_ui::geom::PhysicalSize;
use forge_ui::render::TargetId;
use forge_ui::render_wgpu::{GpuContext, WgpuRenderer, wgpu};
use forge_ui::text::FontConfig;
use forge_ui::{NodeStyle, Size, Theme, Ui, UiConfig};
use serde::{Deserialize, Serialize};

/// A channel may differ by this many levels (of 255) before a pixel counts as different.
const CHANNEL_TOLERANCE: u8 = 3;
/// At most this fraction of pixels may differ (anti-aliasing differences between GPUs).
const MAX_DIFF_FRACTION: f64 = 0.001;

const SIZE: Size = Size::new(900.0, 460.0);

fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

fn bless() -> bool {
    std::env::var_os("FORGE_BLESS").is_some_and(|v| v == "1")
}

fn themes() -> Vec<(&'static str, Theme)> {
    vec![
        ("dark", Theme::dark()),
        ("light", Theme::light()),
        ("high-contrast", Theme::high_contrast()),
    ]
}

/// The gallery, laid out and painted, at `scale`, optionally with one panel's padding
/// grown by a pixel (the positive control).
fn gallery_ui(theme: Theme, scale: f32, nudge: bool) -> Ui {
    let mut ui = Ui::new(UiConfig {
        theme,
        size: SIZE,
        scale,
        fonts: FontConfig::bundled_only(),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut ui, GalleryFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    if nudge {
        let space = ui.theme().space;
        let style = NodeStyle::row(space[2])
            .padding(space[3] + 1.0)
            .wrap()
            .background(forge_ui::ColorRole::BgRaised)
            .rounded(forge_ui::style::Radius::Lg)
            .scope(forge_ui::FocusScope::Panel);
        ui.set_style(g.panels[0], style)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    ui.frame(Duration::ZERO);
    ui
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct DisplayListGoldens {
    font: String,
    font_hash: u64,
    hashes: BTreeMap<String, String>,
}

fn current_hashes(nudge_dark_1x: bool) -> (String, u64, BTreeMap<String, String>) {
    let mut out = BTreeMap::new();
    let mut font = (String::new(), 0);
    for (name, theme) in themes() {
        for scale in [1.0f32, 2.0] {
            let ui = gallery_ui(
                theme.clone(),
                scale,
                nudge_dark_1x && name == "dark" && scale == 1.0,
            );
            font = ui.text().font_fingerprint();
            out.insert(
                format!("{name}@{scale}x"),
                format!("{:#018x}", ui.display_list_hash()),
            );
        }
    }
    (font.0, font.1, out)
}

fn load_display_goldens() -> Option<DisplayListGoldens> {
    let s = std::fs::read_to_string(goldens_dir().join("display_lists.ron")).ok()?;
    ron::from_str(&s).ok()
}

/// Compare; `Err` = mismatch.
fn check_display_lists(nudge: bool) -> Result<(), String> {
    let (font, font_hash, hashes) = current_hashes(nudge);
    let Some(g) = load_display_goldens() else {
        return Err("tests/goldens/display_lists.ron is missing: run with FORGE_BLESS=1".into());
    };
    if g.font_hash != font_hash {
        return Err(format!(
            "the bundled fonts changed: goldens made with {:?} {:#x}, the bundle is now {font:?} {font_hash:#x}; re-bless with FORGE_BLESS=1",
            g.font, g.font_hash
        ));
    }
    let bad: Vec<String> = hashes
        .iter()
        .filter(|(k, v)| g.hashes.get(*k) != Some(v))
        .map(|(k, v)| format!("{k}: {v} != golden {:?}", g.hashes.get(k)))
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad.join("\n"))
    }
}

#[test]
fn display_list_goldens() {
    if bless() {
        let (font, font_hash, hashes) = current_hashes(false);
        let g = DisplayListGoldens {
            font,
            font_hash,
            hashes,
        };
        std::fs::create_dir_all(goldens_dir()).unwrap_or_else(|e| panic!("{e}"));
        let text = ron::ser::to_string_pretty(&g, ron::ser::PrettyConfig::default())
            .unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(goldens_dir().join("display_lists.ron"), text)
            .unwrap_or_else(|e| panic!("{e}"));
        println!("blessed display-list goldens");
        return;
    }
    if let Err(e) = check_display_lists(false) {
        panic!("display-list goldens differ:\n{e}");
    }
}

#[test]
fn display_lists_are_deterministic_within_a_run() {
    let a = gallery_ui(Theme::dark(), 1.0, false).display_list_hash();
    let b = gallery_ui(Theme::dark(), 1.0, false).display_list_hash();
    assert_eq!(a, b);
}

#[test]
fn positive_control_one_pixel_padding_change_fails_display_goldens() {
    let base = gallery_ui(Theme::dark(), 1.0, false).display_list_hash();
    let nudged = gallery_ui(Theme::dark(), 1.0, true).display_list_hash();
    assert_ne!(
        base, nudged,
        "a 1 px padding change did not change the display list"
    );
    if bless() {
        return;
    }
    match check_display_lists(true) {
        Err(e) => assert!(e.contains("dark@1x"), "{e}"),
        Ok(()) => panic!("the golden check passed a 1 px padding change"),
    }
}

// ---- pixel goldens --------------------------------------------------------------------

fn render_pixels(ctx: &GpuContext, theme: Theme, nudge: bool) -> (u32, u32, Vec<u8>) {
    let mut r = WgpuRenderer::from_context(ctx).expect("renderer");
    let mut ui = gallery_ui(theme, 1.0, nudge);
    let t = TargetId(1);
    r.add_offscreen_target(
        t,
        PhysicalSize {
            w: SIZE.w as u32,
            h: SIZE.h as u32,
        },
    );
    ui.render(&mut r, t).unwrap_or_else(|e| panic!("{e}"));
    r.read_pixels(t).unwrap_or_else(|e| panic!("{e}"))
}

fn read_png(path: &PathBuf) -> Option<(u32, u32, Vec<u8>)> {
    let f = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    (info.color_type == png::ColorType::Rgba).then_some((info.width, info.height, buf))
}

fn write_png(path: &PathBuf, w: u32, h: u32, px: &[u8]) {
    let f = std::fs::File::create(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.set_compression(png::Compression::High);
    let mut wr = e.write_header().unwrap_or_else(|e| panic!("{e}"));
    wr.write_image_data(px).unwrap_or_else(|e| panic!("{e}"));
}

/// Fraction of pixels whose any channel differs by more than the tolerance.
fn diff_fraction(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len()) / 4;
    let differing = a
        .chunks(4)
        .zip(b.chunks(4))
        .filter(|(p, q)| {
            p.iter()
                .zip(q.iter())
                .any(|(x, y)| x.abs_diff(*y) > CHANNEL_TOLERANCE)
        })
        .count();
    differing as f64 / n.max(1) as f64
}

fn check_pixels(ctx: &GpuContext, name: &str, theme: Theme, nudge: bool) -> Result<f64, String> {
    let (w, h, px) = render_pixels(ctx, theme, nudge);
    let path = goldens_dir().join(format!("pixels/gallery_{name}_1x.png"));
    let (gw, gh, gold) = read_png(&path)
        .ok_or_else(|| format!("{} is missing: run with FORGE_BLESS=1", path.display()))?;
    if (gw, gh) != (w, h) {
        return Err(format!("{name}: size {w}x{h} != golden {gw}x{gh}"));
    }
    let f = diff_fraction(&px, &gold);
    if f > MAX_DIFF_FRACTION {
        let out = std::env::temp_dir().join(format!("forge_ui_{name}_actual.png"));
        write_png(&out, w, h, &px);
        return Err(format!(
            "{name}: {:.4}% of pixels differ (> {:.2}%); actual written to {}",
            f * 100.0,
            MAX_DIFF_FRACTION * 100.0,
            out.display()
        ));
    }
    Ok(f)
}

/// Gate row of the pixel goldens (`tests/gates.ron`).
const PIXEL_ROW: &str = "C-ui-pixel-goldens";

/// `<workspace>/target/gate-observations/<row>.txt`: where a bound test that could not run
/// records why, so `cargo xtask gate` reports the row `AWAITING` instead of `BOUND` (W9).
fn observation_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/gate-observations")
        .join(format!("{PIXEL_ROW}.txt"))
}

fn on_ci() -> bool {
    std::env::var("CI").is_ok_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// What having no adapter means: `Ok(reason)` = AWAITING on a developer machine; `Err` =
/// a loud failure on CI, where a runner without a GPU adapter must never look green.
fn no_adapter_outcome(error: &str, ci: bool) -> Result<String, String> {
    let reason = format!("no GPU adapter: {error}");
    if ci {
        Err(format!(
            "CI=true and {reason}: the pixel goldens cannot run, and a CI leg must not pass them silently (W9)"
        ))
    } else {
        Ok(reason)
    }
}

fn context() -> Option<GpuContext> {
    match GpuContext::headless(false).or_else(|_| GpuContext::headless(true)) {
        Ok(c) => Some(c),
        Err(e) => match no_adapter_outcome(&e.to_string(), on_ci()) {
            Ok(reason) => {
                println!("pixel goldens: AWAITING({reason})");
                let f = observation_file();
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

#[test]
fn no_adapter_is_awaiting_locally() {
    let r = no_adapter_outcome("adapter request failed", false);
    assert!(
        r.as_ref().is_ok_and(|m| m.contains("no GPU adapter")),
        "{r:?}"
    );
}

/// W2 for "never silently pass without an adapter": under `CI=true` the outcome is a failure.
#[test]
fn positive_control_no_adapter_under_ci_fails() {
    let r = no_adapter_outcome("adapter request failed", true);
    assert!(r.as_ref().is_err_and(|m| m.contains("CI=true")), "{r:?}");
}

#[test]
fn pixel_goldens() {
    let Some(ctx) = context() else { return };
    // They run for real on this machine: clear any earlier AWAITING observation.
    let _ = std::fs::remove_file(observation_file());
    let (font, _) = gallery_ui(Theme::dark(), 1.0, false)
        .text()
        .font_fingerprint();
    if bless() {
        std::fs::create_dir_all(goldens_dir().join("pixels")).unwrap_or_else(|e| panic!("{e}"));
        for (name, theme) in themes() {
            let (w, h, px) = render_pixels(&ctx, theme, false);
            write_png(
                &goldens_dir().join(format!("pixels/gallery_{name}_1x.png")),
                w,
                h,
                &px,
            );
        }
        println!(
            "blessed pixel goldens on {} with {font}",
            ctx.adapter_name()
        );
        return;
    }
    for (name, theme) in themes() {
        let f = check_pixels(&ctx, name, theme, false).unwrap_or_else(|e| panic!("{e}"));
        println!(
            "pixel golden {name}: {:.4}% differing on {}",
            f * 100.0,
            ctx.adapter_name()
        );
    }
}

/// A `Viewport` primitive composites an externally rendered texture (the future
/// `forge-render` output) into exactly its rect.
#[test]
fn viewport_primitive_composites_an_external_texture() {
    use forge_ui::render::{ExternalTexture, Primitive};
    use forge_ui::widget::PaintCx;
    struct Viewport;
    impl forge_ui::Widget for Viewport {
        fn role(&self) -> forge_ui::Role {
            forge_ui::Role::Canvas
        }
        fn paint(&self, cx: &mut PaintCx) {
            let rect = cx.rect();
            cx.primitive(Primitive::Viewport {
                tex: ExternalTexture(7),
                rect,
            });
        }
    }
    let Some(ctx) = context() else { return };
    let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fake viewport"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let red: Vec<u8> = (0..16).flat_map(|_| [255u8, 0, 0, 255]).collect();
    ctx.queue.write_texture(
        tex.as_image_copy(),
        &red,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: Some(4),
        },
        wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    let mut r = WgpuRenderer::from_context(&ctx).expect("renderer");
    r.register_external(
        ExternalTexture(7),
        tex.create_view(&wgpu::TextureViewDescriptor::default()),
    );
    let mut ui = Ui::new(UiConfig {
        size: Size::new(200.0, 100.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    ui.add(
        ui.root(),
        "vp",
        NodeStyle::leaf().size(100.0, 50.0),
        Viewport,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    ui.frame(Duration::ZERO);
    let t = TargetId(3);
    r.add_offscreen_target(t, PhysicalSize { w: 200, h: 100 });
    let stats = ui
        .render(&mut r, t)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_default();
    let (w, _, px) = r.read_pixels(t).unwrap_or_else(|e| panic!("{e}"));
    let at =
        |x: u32, y: u32| px[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize].to_vec();
    assert_eq!(
        at(50, 25),
        vec![255, 0, 0, 255],
        "inside the viewport rect: the external texture"
    );
    assert_ne!(
        at(150, 75),
        vec![255, 0, 0, 255],
        "outside: the UI background"
    );
    assert!(stats.draw_calls >= 2, "the viewport is its own draw call");
}

#[test]
fn positive_control_one_pixel_padding_change_fails_pixel_goldens() {
    if bless() {
        return;
    }
    let Some(ctx) = context() else { return };
    let e = check_pixels(&ctx, "dark", Theme::dark(), true)
        .expect_err("a 1 px padding change passed the pixel golden");
    assert!(e.contains("pixels differ"), "{e}");
}
