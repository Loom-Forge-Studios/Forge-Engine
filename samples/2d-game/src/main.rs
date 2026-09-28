//! The 2D sample game (Ch.35 §35.3, DoD M4-12) — the binary an export ships.
//!
//! ```text
//! forge-2d-game [--locale en|fr|qps-ploc] [--glyphs xbox|playstation|nintendo]
//!               [--demo] [--exit-after SECS] [--report-startup]
//!     the game in a window: the main menu over the level; arrows / A D run, Space / Up / W
//!     jump, Escape pauses. --demo presses Play and plays the scripted run.
//! forge-2d-game --play-script
//!     headless, no GPU: the pad presses Play in the menus and plays the scripted run to the
//!     level-clear banner; prints the run's fingerprint (exit 0 only on a win)
//! forge-2d-game --probe
//!     headless startup: GPU device, 2D renderer and textures, the level, the menus, and the
//!     first frame rendered and read back; prints `startup_ms=`
//! forge-2d-game --shot out.png [--steps N]
//!     headless: the scripted run for N steps (default 600), the last frame as a PNG
//! ```
//!
//! The keyboard is mapped onto the game's pad inputs (`forge_ui::game::PadInput`), the seam a
//! platform gamepad backend fills (Ch.27; not built: `forge-play`, M5-8). It touches no
//! network.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use forge_2d_game::session::{GAME_VIEW, Mode, Session, SessionConfig};
use forge_2d_game::view::GameViewHost;
use forge_2d_game::{SCRIPT_STEPS, demo_script};
use forge_gpu::{AdapterPool, PoolOptions};
use forge_ui::game::{GlyphSet, PadButton, PadInput};
use forge_ui::platform_winit::{RunOptions, UiApp, run};
use forge_ui::testing::Harness;
use forge_ui::{ActionEnvelope, KeyEvent, Size, Ui, UiConfig, UiError};

fn started() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// W2 (`control-link-3d` only): the exported-build guard's positive control links a 3D-only
/// crate for real — the sky's derived atmosphere, kept alive by `black_box`.
#[cfg(feature = "control-link-3d")]
fn sky_radius() -> f64 {
    let air = std::hint::black_box(forge_sky::atmosphere::AtmosphereBody::earth());
    air.derive().map_or(0.0, |p| p.top_radius)
}

struct App {
    cfg: SessionConfig,
    session: Option<Session>,
    host: GameViewHost,
    demo: bool,
    report_startup: bool,
    settled: bool,
    title: String,
}

impl App {
    fn press(s: &mut Session, ui: &mut Ui, b: PadButton) {
        let now = ui.now();
        for pressed in [true, false] {
            s.pad(ui, now, PadInput::Button { button: b, pressed });
        }
    }
}

impl UiApp for App {
    fn title(&self) -> String {
        self.title.clone()
    }
    fn build(&mut self, ui: &mut Ui) -> Result<(), UiError> {
        let mut s = Session::build(ui, &self.cfg)?;
        if self.demo {
            s.set_script(demo_script(u64::MAX >> 40));
        }
        self.session = Some(s);
        Ok(())
    }
    fn on_actions(&mut self, ui: &mut Ui, actions: Vec<ActionEnvelope>) {
        if let Some(s) = &mut self.session {
            let now = ui.now();
            s.on_actions(ui, now, actions);
        }
    }
    fn update_window(&mut self, ui: &mut Ui, _key: Option<u64>) {
        let Some(s) = &mut self.session else { return };
        if self.demo && self.settled && s.mode() == Mode::Menu && s.game().steps == 0 {
            // --demo: Play from the main menu once the window has settled.
            Self::press(s, ui, PadButton::South);
        }
        let now = ui.now();
        s.tick(ui, now);
        s.advance(ui, now);
    }
    fn key_first(&mut self, ui: &mut Ui, _key: Option<u64>, ev: &KeyEvent) -> bool {
        let Some(s) = &mut self.session else {
            return false;
        };
        let now = ui.now();
        s.key(ui, now, ev)
    }
    fn next_deadline(&self) -> Option<Duration> {
        let s = self.session.as_ref()?;
        if self.demo && self.settled && s.mode() == Mode::Menu && s.game().steps == 0 {
            // --demo: a turn now, to press Play.
            return Some(Duration::ZERO);
        }
        s.next_deadline()
    }
    fn exit_requested(&self) -> bool {
        self.session.as_ref().is_some_and(Session::quit_requested)
    }
    fn settled(&mut self) {
        self.settled = true;
        if self.report_startup {
            let ms = started().elapsed().as_secs_f64() * 1000.0;
            println!("forge-2d-game: interactive {ms:.1} ms after start");
        }
    }
    fn render_external(
        &mut self,
        ui: &mut Ui,
        _key: Option<u64>,
        cx: &mut forge_ui::render_wgpu::ExternalCx<'_>,
    ) {
        let (Some(s), Some(dev)) = (&mut self.session, cx.device()) else {
            return;
        };
        if !s.take_view_dirty() {
            return;
        }
        let Some(r) = ui.rect(s.view()) else { return };
        let scale = ui.scale();
        let size = (
            (r.w * scale).round().max(1.0) as u32,
            (r.h * scale).round().max(1.0) as u32,
        );
        match self.host.render(dev, s.game(), size) {
            Ok(view) => cx.register(GAME_VIEW, view),
            Err(e) => eprintln!("forge-2d-game: {e}"),
        }
    }
}

/// The session in a headless UI (no window, no GPU).
fn headless(cfg: &SessionConfig) -> Result<(Harness, Session), String> {
    let mut h = Harness::new(UiConfig {
        size: Size::new(1280.0, 720.0),
        ..UiConfig::default()
    })
    .map_err(|e| e.to_string())?;
    let s = Session::build(&mut h.ui, cfg).map_err(|e| e.to_string())?;
    h.settle();
    Ok((h, s))
}

/// `--play-script`: Play from the main menu, the scripted run to the level-clear banner.
fn play_script(cfg: &SessionConfig) -> Result<bool, String> {
    let (mut h, mut s) = headless(cfg)?;
    App::press(&mut s, &mut h.ui, PadButton::South);
    if s.mode() != Mode::Playing {
        return Err(format!("Play did not start the level: {:?}", s.mode()));
    }
    s.set_script(demo_script(SCRIPT_STEPS as u64));
    for _ in 0..SCRIPT_STEPS {
        s.step_once(&mut h.ui);
        if s.mode() != Mode::Playing {
            break;
        }
    }
    h.settle();
    let g = s.game();
    println!(
        "play-script: won={} steps={} coins={} fingerprint={:#018x} on {}-{}",
        g.won,
        g.steps,
        g.coins_taken,
        s.fingerprint().0,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    Ok(g.won && s.mode() == Mode::Won)
}

/// `--probe`: startup to the first rendered frame, headless.
fn probe(cfg: &SessionConfig) -> Result<(), String> {
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let pool = AdapterPool::new(&PoolOptions::default()).map_err(|e| e.to_string())?;
    let dev = pool.primary();
    let gpu_ms = ms(t);
    let t = Instant::now();
    let (_h, s) = headless(cfg)?;
    let ui_ms = ms(t);
    let t = Instant::now();
    let mut host = GameViewHost::new();
    host.render(dev, s.game(), (1280, 720))
        .map_err(|e| e.to_string())?;
    let tex = host.texture().ok_or("no frame")?;
    let px = forge_gpu::transfer::read_texture(dev, tex).map_err(|e| e.to_string())?;
    let frame_ms = ms(t);
    println!(
        "probe: startup_ms={:.1} (gpu device {gpu_ms:.1}, level and menus {ui_ms:.1}, first frame {frame_ms:.1}) frame_bytes={} adapter={}",
        ms(started()),
        px.len(),
        dev.label()
    );
    Ok(())
}

fn shot(path: &str, steps: u64) -> Result<(), String> {
    let pool = AdapterPool::new(&PoolOptions::default()).map_err(|e| e.to_string())?;
    let dev = pool.primary();
    let (w, h) = (1280, 720);
    let game = forge_2d_game::play_script(&demo_script(steps), steps as usize, |_, _| {})
        .map_err(|e| e.to_string())?;
    let mut host = GameViewHost::new();
    host.render(dev, &game, (w, h)).map_err(|e| e.to_string())?;
    let tex = host.texture().ok_or("no frame")?;
    let px = forge_gpu::transfer::read_texture(dev, tex).map_err(|e| e.to_string())?;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(&px).map_err(|e| e.to_string())?;
    let hero = game.hero().map_err(|e| e.to_string())?;
    println!(
        "{steps} steps: hero at ({:.2}, {:.2}), {} coins, won {}; wrote {path}",
        hero.local.x, hero.local.y, game.coins_taken, game.won
    );
    Ok(())
}

fn main() {
    let _ = started();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let has = |name: &str| args.iter().any(|a| a == name);
    #[cfg(feature = "control-link-3d")]
    {
        let r = std::hint::black_box(sky_radius());
        if has("--sky-radius") {
            println!("sky radius {r}");
        }
    }
    let mut cfg = SessionConfig::default();
    if let Some(l) = arg("--locale") {
        cfg.language.clone_from(l);
    }
    if let Some(g) = arg("--glyphs") {
        cfg.glyphs = match g.as_str() {
            "playstation" => GlyphSet::PlayStation,
            "nintendo" => GlyphSet::Nintendo,
            "xbox" => GlyphSet::Xbox,
            _ => GlyphSet::Generic,
        };
    }
    let result = if has("--play-script") {
        match play_script(&cfg) {
            Ok(true) => Ok(()),
            Ok(false) => Err("the scripted run did not clear the level".to_string()),
            Err(e) => Err(e),
        }
    } else if has("--probe") {
        probe(&cfg)
    } else if let Some(path) = arg("--shot") {
        let steps = arg("--steps").and_then(|s| s.parse().ok()).unwrap_or(600);
        shot(path, steps)
    } else {
        let mut loc = forge_2d_game::session::strings();
        loc.set_current(&cfg.language);
        let app = App {
            title: loc.get("game2d.window_title", &[]),
            cfg,
            session: None,
            host: GameViewHost::new(),
            demo: has("--demo"),
            report_startup: has("--report-startup"),
            settled: false,
        };
        let opts = RunOptions {
            size: Size::new(1280.0, 720.0),
            // The project's GPU mode: forge.json beside the exported executable (the
            // packager writes it), `--gpu-mode` overrides; Single unless the game opted in.
            gpu_mode: forge_runtime::gpu_mode::startup(
                &args,
                forge_runtime::gpu_mode::exe_dir().as_deref(),
            ),
            exit_after: arg("--exit-after")
                .and_then(|s| s.parse::<f64>().ok())
                .map(Duration::from_secs_f64),
            ..RunOptions::default()
        };
        run(app, opts)
            .map(|r| {
                println!(
                    "frames {} (idle {}), wakeups {} (idle {}), first frame {:.0} ms on {}",
                    r.frames_rendered,
                    r.idle_frames,
                    r.wakeups,
                    r.idle_wakeups,
                    r.first_frame_ms,
                    r.adapter
                );
            })
            .map_err(|e| e.to_string())
    };
    if let Err(e) = result {
        eprintln!("forge-2d-game: {e}");
        std::process::exit(1);
    }
}
