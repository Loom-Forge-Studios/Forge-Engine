//! Render every page of the widget catalogue to PNG, headless (a design-review aid for
//! §21.16; nothing gates on it).
//!
//! ```text
//! cargo run -p forge-ui --example catalogue_shots -- <out-dir> [dark|light|high-contrast]
//! ```

use std::path::PathBuf;
use std::time::Duration;

use forge_ui::gallery::{self, GalleryFaults};
use forge_ui::gallery_catalogue::GROUPS;
use forge_ui::geom::PhysicalSize;
use forge_ui::render::TargetId;
use forge_ui::render_wgpu::{GpuContext, WgpuRenderer};
use forge_ui::{Size, Theme, Ui, UiConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().map_or("catalogue_shots", String::as_str));
    let theme = match args.get(1).map(String::as_str) {
        Some("light") => Theme::light(),
        Some("high-contrast") => Theme::high_contrast(),
        _ => Theme::dark(),
    };
    std::fs::create_dir_all(&out)?;
    let size = Size::new(1280.0, 1000.0);
    let ctx = GpuContext::headless(false).or_else(|_| GpuContext::headless(true))?;
    let mut r = WgpuRenderer::from_context(&ctx)?;
    let mut ui = Ui::new(UiConfig {
        theme,
        size,
        ..UiConfig::default()
    })?;
    let g = gallery::build(&mut ui, GalleryFaults::default())?;
    let t = TargetId(1);
    r.add_offscreen_target(
        t,
        PhysicalSize {
            w: size.w as u32,
            h: size.h as u32,
        },
    );
    for (i, name) in GROUPS.iter().enumerate() {
        g.catalogue.tab.set(ui.rt_mut(), i);
        for (j, p) in g.catalogue.pages.iter().enumerate() {
            ui.set_hidden(*p, j != i)?;
        }
        ui.frame(Duration::from_millis(16 * (i as u64 + 1)));
        ui.damage_all();
        ui.render(&mut r, t)?;
        let (w, h, px) = r.read_pixels(t)?;
        let file = out.join(format!("{i:02}_{}.png", name.replace([' ', '&'], "_")));
        let f = std::fs::File::create(&file)?;
        let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?.write_image_data(&px)?;
        println!("{}", file.display());
    }
    Ok(())
}
