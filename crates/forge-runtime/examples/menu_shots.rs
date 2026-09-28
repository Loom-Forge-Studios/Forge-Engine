//! Render every screen of the sample game UI to PNG, headless, in English and under the
//! pseudo-locale (a design-review aid for §21.20; nothing gates on it).
//!
//! ```text
//! cargo run -p forge-runtime --example menu_shots -- <out-dir>
//! ```

use std::path::PathBuf;
use std::time::Duration;

use forge_runtime::game_ui::{GameMenu, GameUiConfig, Screen};
use forge_ui::geom::PhysicalSize;
use forge_ui::render::TargetId;
use forge_ui::render_wgpu::{GpuContext, WgpuRenderer};
use forge_ui::{Size, Ui, UiConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "menu_shots".into()),
    );
    std::fs::create_dir_all(&out)?;
    let size = Size::new(1280.0, 720.0);
    let ctx = GpuContext::headless(false).or_else(|_| GpuContext::headless(true))?;
    let mut r = WgpuRenderer::from_context(&ctx)?;
    let t = TargetId(1);
    r.add_offscreen_target(
        t,
        PhysicalSize {
            w: size.w as u32,
            h: size.h as u32,
        },
    );
    let mut frame = 0u64;
    for locale in ["en", "qps-ploc"] {
        let mut ui = Ui::new(UiConfig {
            size,
            ..UiConfig::default()
        })?;
        let mut m = GameMenu::build(&mut ui, GameUiConfig::default())?;
        m.set_language(&mut ui, locale);
        m.set_hud(&mut ui, 0.7, 1200);
        for (name, s) in [
            ("main", Screen::Main),
            ("settings", Screen::Settings),
            ("credits", Screen::Credits),
            ("hud", Screen::Hud),
            ("pause", Screen::Pause),
        ] {
            m.show(&mut ui, s);
            frame += 1;
            ui.frame(Duration::from_millis(16 * frame));
            ui.damage_all();
            ui.render(&mut r, t)?;
            let (w, h, px) = r.read_pixels(t)?;
            let file = out.join(format!("{locale}_{name}.png"));
            let f = std::fs::File::create(&file)?;
            let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header()?.write_image_data(&px)?;
            println!("{}", file.display());
        }
    }
    Ok(())
}
