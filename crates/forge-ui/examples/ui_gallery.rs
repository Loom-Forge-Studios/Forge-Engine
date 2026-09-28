//! `ui_gallery` — every `forge-ui` widget in one window (Ch.21 §21.16; WP-U1 starts it).
//!
//! ```text
//! cargo run -p forge-ui --example ui_gallery -- [--theme dark|light|high-contrast]
//!     [--scale 0.5..3.0] [--reduced-motion] [--windows N] [--exit-after SECS]
//! ```
//!
//! With `--exit-after`, the window closes that long after the UI first settles and a
//! report of frames and loop wakeups during the idle window is printed (the zero-idle
//! rule, D-5: both should be 0).

use std::time::Duration;

use forge_ui::gallery::{self, GalleryFaults};
use forge_ui::gallery_catalogue;
use forge_ui::platform_winit::{RunOptions, UiApp, run};
use forge_ui::ui::OpenWindow;
use forge_ui::widgets::{CommandInvoked, MenuChosen, Pressed, Submitted, TabSelected};
use forge_ui::{ActionEnvelope, Theme, Ui, UiError};

struct GalleryApp {
    extra_windows: u32,
    gallery: Option<gallery::Gallery>,
}

impl UiApp for GalleryApp {
    fn title(&self) -> String {
        "forge-ui gallery".into()
    }
    fn build(&mut self, ui: &mut Ui) -> Result<(), UiError> {
        let g = gallery::build(ui, GalleryFaults::default())?;
        let root = g.root;
        self.gallery = Some(g);
        for k in 0..self.extra_windows {
            ui.raise(
                root,
                OpenWindow {
                    key: 100 + u64::from(k),
                    title: format!("forge-ui gallery (window {})", k + 2),
                    size: forge_ui::Size::new(720.0, 460.0),
                    position: None,
                    monitor: None,
                },
            );
        }
        Ok(())
    }
    fn build_window(&mut self, ui: &mut Ui, _key: u64) -> Result<(), UiError> {
        gallery::build(ui, GalleryFaults::default()).map(|_| ())
    }
    fn on_actions(&mut self, ui: &mut Ui, actions: Vec<ActionEnvelope>) {
        for a in actions {
            // Renames, moves, deletes and tab closes: what an editor panel's command
            // handler would do (the gallery has no command bus; widget ids are the same in
            // every window, so the main window's handles serve all of them).
            if let Some(g) = &self.gallery
                && gallery_catalogue::apply_action(ui, &g.catalogue, &a)
            {
                continue;
            }
            if let Some(c) = a.get::<CommandInvoked>() {
                println!("command {}", c.id);
            } else if let Some(m) = a.get::<MenuChosen>() {
                println!("menu {}", m.id);
            } else if let Some(Pressed(id)) = a.get::<Pressed>() {
                println!("pressed {id:?}");
            } else if let Some(s) = a.get::<Submitted>() {
                println!("submitted {:?}", s.text);
            } else if let Some(t) = a.get::<TabSelected>() {
                println!("tab {}", t.index);
            }
        }
    }
}

fn main() {
    let mut opts = RunOptions::default();
    let mut extra_windows = 0u32;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--theme" => {
                opts.theme = match args.get(i + 1).map(String::as_str) {
                    Some("light") => Theme::light(),
                    Some("high-contrast") | Some("hc") => Theme::high_contrast(),
                    _ => Theme::dark(),
                };
                i += 1;
            }
            "--scale" => {
                opts.user_scale = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                i += 1;
            }
            "--reduced-motion" => opts.reduced_motion = true,
            "--windows" => {
                let n: u32 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1);
                extra_windows = n.saturating_sub(1);
                i += 1;
            }
            "--exit-after" => {
                let secs: f64 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
                opts.exit_after = Some(Duration::from_secs_f64(secs));
                i += 1;
            }
            other => eprintln!("ui_gallery: ignoring unknown argument {other}"),
        }
        i += 1;
    }
    match run(
        GalleryApp {
            extra_windows,
            gallery: None,
        },
        opts,
    ) {
        Ok(r) => {
            println!(
                "ui_gallery: adapter {} | windows {} | first frame {:.0} ms | frames {} | wakeups {} | idle window {:.1} s: {} frames, {} wakeups",
                r.adapter,
                r.windows_opened,
                r.first_frame_ms,
                r.frames_rendered,
                r.wakeups,
                r.idle_window.as_secs_f64(),
                r.idle_frames,
                r.idle_wakeups
            );
        }
        Err(e) => {
            eprintln!("ui_gallery: {e}");
            std::process::exit(1);
        }
    }
}
