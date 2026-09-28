//! The runtime binary (Ch.28). With no arguments it prints what it is. `--menu` runs the
//! sample game UI (§21.20, M2-29) in a window on `forge-ui`'s winit runner:
//!
//! ```text
//! forge-runtime --menu [--locale en|fr|qps-ploc] [--glyphs xbox|playstation|nintendo]
//!     [--exit-after SECS] [--gpu-mode single|multi]
//! ```
//!
//! With `--exit-after` the window closes that long after the UI first settles and the run
//! report is printed — the idle frames and wakeups should both be 0 (D-5). It touches no
//! network.

use std::time::Duration;

use forge_runtime::game_ui::{GameEvent, GameMenu, GameUiConfig, end_turn};
use forge_ui::game::GlyphSet;
use forge_ui::platform_winit::{RunOptions, UiApp, run};
use forge_ui::{ActionEnvelope, Size, Ui, UiError};

struct MenuApp {
    cfg: Option<GameUiConfig>,
    menu: Option<GameMenu>,
    window: Size,
    world: forge_core::bevy_ecs::world::World,
    /// The player chose Quit: the loop ends after this turn.
    quit: bool,
}

impl UiApp for MenuApp {
    fn title(&self) -> String {
        format!(
            "Forge sample game \u{2014} {}",
            forge_runtime::ENGINE_CREDIT
        )
    }
    fn build(&mut self, ui: &mut Ui) -> Result<(), UiError> {
        let cfg = self.cfg.take().unwrap_or_default();
        self.menu = Some(GameMenu::build(ui, cfg)?);
        Ok(())
    }
    fn on_actions(&mut self, ui: &mut Ui, actions: Vec<ActionEnvelope>) {
        if let Some(m) = &mut self.menu {
            m.on_actions(ui, actions);
        }
    }
    fn update_window(&mut self, ui: &mut Ui, _key: Option<u64>) {
        let Some(m) = &mut self.menu else { return };
        m.update(ui);
        let now = ui.now();
        m.tick(ui, now);
        let evs = m.take_events();
        for e in &evs {
            println!("game event: {e:?}");
        }
        // Quit ends the loop the ordinary way (`exit_requested`): the run report prints.
        self.quit |= evs.contains(&GameEvent::Quit);
        // The game's systems read these as ECS messages; the buffers advance every turn.
        end_turn(&mut self.world, evs);
    }
    fn exit_requested(&self) -> bool {
        self.quit
    }
    fn window_moved(&mut self, key: Option<u64>, rect: forge_ui::Rect, _m: Option<String>) {
        if key.is_none() {
            self.window = Size::new(rect.w, rect.h);
        }
    }
    fn next_deadline(&self) -> Option<Duration> {
        self.menu.as_ref().and_then(GameMenu::next_deadline)
    }
    fn user_scale(&self) -> Option<f32> {
        let m = self.menu.as_ref()?;
        (self.window.w > 0.0).then(|| m.scale_for(self.window))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--menu") {
        println!("{}", forge_runtime::describe());
        return;
    }
    let mut cfg = GameUiConfig::default();
    let mut opts = RunOptions {
        size: Size::new(1280.0, 720.0),
        // The project's GPU mode (forge.json beside the executable; --gpu-mode overrides).
        gpu_mode: forge_runtime::gpu_mode::startup(
            &args,
            forge_runtime::gpu_mode::exe_dir().as_deref(),
        ),
        ..RunOptions::default()
    };
    let mut i = 0;
    while i < args.len() {
        let next = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--locale" => {
                cfg.settings.language = next;
                i += 1;
            }
            "--glyphs" => {
                cfg.glyphs = match next.as_str() {
                    "playstation" => GlyphSet::PlayStation,
                    "nintendo" => GlyphSet::Nintendo,
                    "xbox" => GlyphSet::Xbox,
                    _ => GlyphSet::Generic,
                };
                i += 1;
            }
            "--exit-after" => {
                opts.exit_after = next.parse::<f64>().ok().map(Duration::from_secs_f64);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    let app = MenuApp {
        cfg: Some(cfg),
        menu: None,
        window: Size::new(0.0, 0.0),
        world: forge_core::bevy_ecs::world::World::new(),
        quit: false,
    };
    match run(app, opts) {
        Ok(r) => println!(
            "frames {} (idle {}), wakeups {} (idle {}), first frame {:.0} ms on {}",
            r.frames_rendered,
            r.idle_frames,
            r.wakeups,
            r.idle_wakeups,
            r.first_frame_ms,
            r.adapter
        ),
        Err(e) => {
            eprintln!("forge-runtime: {e}");
            std::process::exit(1);
        }
    }
}
