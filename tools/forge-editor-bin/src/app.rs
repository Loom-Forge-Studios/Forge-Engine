//! The `forge-editor` program (Ch.21 §21.18) and its headless mode (Ch.34 §34.4), in the
//! library so an edition's binary runs exactly this and adds its own flags, plugins and
//! services through [`EditorEdition`] (the base binary adds none: [`BaseEdition`]).
//!
//! ```text
//! forge-editor [--preset 2d|3d|<a plugin's preset>] [--config-dir DIR | --no-user-config]
//!              [--exit-after SECS] [--no-remote-host] [--play-2d-sample]
//! forge-editor --connect ADDR [--pair CODE] [--device-name NAME]   (the split editor)
//! forge-editor --headless [--script FILE] [--accept-project-security] [--trust-project]
//!                                                (commands from FILE or stdin)
//! forge-editor --headless --remote-host [--port N] [--lan] [--script FILE]
//!                                                (the split editor's host, no UI)
//! ```
//!
//! The GUI is the shell (a client of the core through a `BusClient`) plus the first-party
//! panel plugins, on the winit runner: it sleeps whenever nothing changes (idle = no
//! frames, no wakeups; D-5). `--exit-after` exits that long after the UI first settles and
//! prints the frames and wakeups of that idle window.
//! `--play-2d-sample` makes the 2D sample game (`samples/2d-game`, M4-12) the play backend:
//! the play controls run its level in the viewport (play-in-editor), arrows / A D run and
//! Space / Up / W jump while it plays, Stop returns the viewport to the project.
//! `--headless` runs the same core with no UI at all; `--accept-project-security` lets the
//! plugin grants and default automation policy a project the script opens carries take
//! effect (a script's open otherwise holds them for a person, WP-33).
//!
//! **WASM plugins (WP-21).** The GUI hosts drop-in WASM plugins from `<user config>/plugins/`
//! and the open project's `plugins/` folder (`forge_editor::hosting`): they load with the
//! source plugins at start, and one dropped in while the editor runs installs the next time
//! it wakes; changed code reloads in place. Nothing polls while the editor is idle.
//! **A project's own plugins run only once you trust it (WP-34):** the first time you open a
//! project that carries plugins or plugin grants, the editor asks (in the Plugin manager);
//! the answer is kept in `<user config>/trusted-projects.ron` with what the project carried
//! then (WP-35): when a pull brings a new plugin, changed plugin code or a new grant, what
//! changed waits and you are asked again; unchanged content never asks. Headless runs trust
//! nothing unless launched with `--trust-project`.
//!
//! **The split editor (Ch.34, M2-16).** The GUI hosts its core to paired devices over QUIC
//! (`forge-remote`), on **loopback** unless the Remote panel's LAN exposure was chosen; the
//! Remote panel pairs devices and lists their sessions (`--no-remote-host` turns the host
//! off). `--connect ADDR` runs this editor as a device of the core at `ADDR`: the panels
//! and the viewport are local and native, the project is the host's, and edits show at once
//! (O-13). `--pair CODE` pairs first, with the code shown on the host.
//! `--headless --remote-host` hosts with no window, GPU or panels (a workstation serving a
//! laptop): stdin is the host console (`forge_remote::console`), where a person shows a
//! pairing code (`pair`) and allows a device (`allow NAME`) with the Remote panel's own calls.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_cmd::Issuer;
use forge_editor::client::BusClient;
use forge_editor::core::{EditorCore, SharedCore};
use forge_editor::presets::preset_in;
use forge_editor::shell::{Shell, ShellConfig, assemble_hosted, gui_issuer};
use forge_editor::user_config::user_config_dir;
use forge_plugin::SourcePlugin;
use forge_ui::dock::MonitorInfo;
use forge_ui::platform_winit::{RunOptions, UiApp, run};
use forge_ui::ui::Poster;
use forge_ui::{ActionEnvelope, Rect, Size, Ui, UiError};

/// What an edition's editor binary adds to the base editor: its own flags, its own source
/// plugins, and the services it attaches to the core. The base binary is [`BaseEdition`],
/// which adds nothing; an edition built from another crate passes its own to [`main_with`].
/// Nothing here runs for a split-editor device (`--connect`: the core is the host's) or for
/// `--headless`, except the flags, which are parsed for every mode.
pub trait EditorEdition {
    /// Its flags' usage, appended to the base usage (empty: none).
    fn usage(&self) -> &'static str {
        ""
    }
    /// If `args[i]` is one of its flags, take it (and its value): `Some(Ok(n))` consumed
    /// `n` arguments; `Some(Err(why))` is a usage error; `None`: not its flag.
    fn parse_arg(&mut self, _args: &[String], _i: usize) -> Option<Result<usize, String>> {
        None
    }
    /// Refuse a combination of flags, given whether this run is headless.
    fn check_args(&self, _headless: bool) -> Result<(), String> {
        Ok(())
    }
    /// Its source plugins, loaded after the first-party ones.
    fn plugins(&self) -> Vec<&dyn SourcePlugin> {
        Vec::new()
    }
    /// Attach its services to the GUI editor's own core, once the connect services follow
    /// it and before the shell is built.
    fn attach(
        &mut self,
        _core: &SharedCore,
        _cfg: &mut ShellConfig,
        _config_dir: Option<&std::path::Path>,
    ) -> Result<(), String> {
        Ok(())
    }
    /// The GUI editor's shell is built over its own core: start what runs beside it.
    fn started(&mut self, _core: &SharedCore) {}
    /// Renderers for its plugins' viewport worlds, added to the GPU host (the base edition
    /// has none).
    fn world_renderers(
        &self,
        _services: &forge_editor::services::EditorServices,
    ) -> Vec<Box<dyn crate::viewport_gpu::WorldRenderer>> {
        Vec::new()
    }
}

/// The base editor: no flags, plugins or services beyond the first-party ones.
#[derive(Debug, Default, Clone, Copy)]
pub struct BaseEdition;

impl EditorEdition for BaseEdition {}

struct EditorApp {
    shell: Shell,
    /// The viewport's forge-render host (renders on the UI's device, D-3).
    viewports: crate::viewport_gpu::ViewportRenderHost,
    title: String,
    /// Filled at build time: how another client's change wakes this loop.
    wake: Arc<Mutex<Option<Poster>>>,
    /// `--report-startup`: print when the editor became interactive (`ui_startup_budget`).
    report_startup: Option<String>,
    /// `--play-2d-sample`: play-in-editor of the 2D sample game.
    sample: Option<Sample2d>,
}

impl UiApp for EditorApp {
    fn title(&self) -> String {
        self.title.clone()
    }
    fn build(&mut self, ui: &mut Ui) -> Result<(), UiError> {
        if let Ok(mut w) = self.wake.lock() {
            *w = Some(ui.poster());
        }
        self.shell.build_main(ui)
    }
    fn build_window(&mut self, ui: &mut Ui, key: u64) -> Result<(), UiError> {
        self.shell.build_window(ui, key)
    }
    fn on_actions(&mut self, ui: &mut Ui, actions: Vec<ActionEnvelope>) {
        self.shell.on_actions(ui, actions);
    }
    fn update_window(&mut self, ui: &mut Ui, _key: Option<u64>) {
        self.shell.update_window(ui);
    }
    fn window_moved(&mut self, key: Option<u64>, rect: Rect, monitor: Option<String>) {
        self.shell.window_moved(key, rect, monitor);
    }
    fn window_closed(&mut self, key: u64) {
        self.shell.window_closed(key);
    }
    fn monitors(&mut self, monitors: Vec<MonitorInfo>) {
        self.shell.monitors(monitors);
    }
    fn next_deadline(&self) -> Option<Duration> {
        self.shell.next_deadline()
    }
    fn user_scale(&self) -> Option<f32> {
        Some(self.shell.user_scale())
    }
    fn exiting(&mut self) {
        self.shell.exiting();
    }
    fn settled(&mut self) {
        // Ch.21 §21.22 `ui_startup_budget`: cold start (process start, as `main` saw it) to
        // the first interactive frame — every window drawn, nothing left to do.
        if let Some(adapter) = &self.report_startup {
            let ms = started().elapsed().as_secs_f64() * 1000.0;
            println!("forge-editor: interactive {ms:.1} ms after start | adapter {adapter}");
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
    }
    fn render_external(
        &mut self,
        _ui: &mut Ui,
        _key: Option<u64>,
        cx: &mut forge_ui::render_wgpu::ExternalCx<'_>,
    ) {
        let Some(dev) = cx.device() else {
            return;
        };
        // Play-in-editor of the 2D sample: while its level runs, the viewports show it.
        if let Some(sample) = &mut self.sample {
            let play = sample.play.borrow();
            if let Some(game) = play.game() {
                let mut done = Vec::new();
                for (id, size) in self.viewports.take_dirty_sizes() {
                    match sample.hosts.entry(id).or_default().render(dev, game, size) {
                        Ok(view) => done.push((id, view)),
                        Err(e) => eprintln!("forge-editor: the 2D sample's frame: {e}"),
                    }
                }
                drop(play);
                for (id, view) in done {
                    cx.register(forge_ui::render::ExternalTexture(id), view);
                }
                return;
            }
        }
        let done = self.viewports.render_dirty(dev);
        for (id, view) in done {
            cx.register(forge_ui::render::ExternalTexture(id), view);
        }
    }
    fn key_first(&mut self, _ui: &mut Ui, _key: Option<u64>, ev: &forge_ui::KeyEvent) -> bool {
        // While the 2D sample plays in the editor, its movement keys are the game's.
        let Some(sample) = &self.sample else {
            return false;
        };
        let mut play = sample.play.borrow_mut();
        if play.state() != forge_editor::play::PlayState::Playing || ev.repeat {
            return false;
        }
        use forge_editor::play::PlayBackend as _;
        use forge_ui::KeyCode as K;
        use forge_ui::game::{PadButton, PadInput};
        let button = match ev.code {
            K::Left | K::Char('a') => PadButton::DPadLeft,
            K::Right | K::Char('d') => PadButton::DPadRight,
            K::Space | K::Up | K::Char('w') => PadButton::South,
            _ => return false,
        };
        play.pad(PadInput::Button {
            button,
            pressed: ev.pressed,
        });
        true
    }
}

/// `--play-2d-sample`: the 2D sample's play backend (the same object as the services' play
/// backend) and a 2D renderer per viewport.
struct Sample2d {
    play: std::rc::Rc<std::cell::RefCell<forge_2d_game::pie::SamplePlay>>,
    hosts: std::collections::BTreeMap<u32, forge_2d_game::view::GameViewHost>,
}

struct Args {
    headless: bool,
    script: Option<String>,
    accept_project_security: bool,
    trust_project: bool,
    preset: String,
    config_dir: Option<std::path::PathBuf>,
    no_user_config: bool,
    exit_after: Option<Duration>,
    report_startup: bool,
    play_sample: bool,
    no_remote_host: bool,
    connect: Option<String>,
    pair: Option<String>,
    device_name: Option<String>,
    remote_host: bool,
    port: Option<u16>,
    lan: bool,
}

/// The base usage (an edition's flags follow it).
pub const USAGE: &str = "usage: forge-editor [--preset 2d|3d|<a plugin's preset>] [--config-dir DIR | \
     --no-user-config] [--exit-after SECS] [--report-startup] [--play-2d-sample] \
     [--no-remote-host]\n       \
     forge-editor --connect ADDR [--pair CODE] [--device-name NAME]\n       \
     forge-editor --headless [--script FILE] [--accept-project-security] \
     [--trust-project]\n       \
     forge-editor --headless --remote-host [--port N] [--lan] \
     [--config-dir DIR | --no-user-config] [--script FILE] [--trust-project]";

/// The usage text of `edition`'s binary.
#[must_use]
pub fn usage(edition: &dyn EditorEdition) -> String {
    let extra = edition.usage();
    if extra.is_empty() {
        USAGE.to_string()
    } else {
        format!("{USAGE}\n{extra}")
    }
}

fn parse_args(v: &[String], edition: &mut dyn EditorEdition) -> Result<Args, String> {
    let mut a = Args {
        headless: false,
        script: None,
        accept_project_security: false,
        trust_project: false,
        preset: "3d".into(),
        config_dir: None,
        no_user_config: false,
        exit_after: None,
        report_startup: false,
        play_sample: false,
        no_remote_host: false,
        connect: None,
        pair: None,
        device_name: None,
        remote_host: false,
        port: None,
        lan: false,
    };
    let mut i = 0;
    let value = |i: usize, name: &str| -> Result<String, String> {
        v.get(i + 1)
            .cloned()
            .ok_or_else(|| format!("{name} needs a value"))
    };
    while i < v.len() {
        match v[i].as_str() {
            "--headless" => a.headless = true,
            "--accept-project-security" => a.accept_project_security = true,
            "--trust-project" => a.trust_project = true,
            "--script" => {
                a.script = Some(value(i, "--script")?);
                i += 1;
            }
            "--preset" => {
                a.preset = value(i, "--preset")?;
                i += 1;
            }
            "--config-dir" => {
                a.config_dir = Some(value(i, "--config-dir")?.into());
                i += 1;
            }
            "--no-user-config" => a.no_user_config = true,
            "--exit-after" => {
                let s: f64 = value(i, "--exit-after")?
                    .parse()
                    .map_err(|e| format!("--exit-after: {e}"))?;
                a.exit_after = Some(Duration::from_secs_f64(s.max(0.0)));
                i += 1;
            }
            "--play-2d-sample" => a.play_sample = true,
            "--report-startup" => a.report_startup = true,
            "--no-remote-host" => a.no_remote_host = true,
            "--remote-host" => a.remote_host = true,
            "--lan" => a.lan = true,
            "--port" => {
                let p = value(i, "--port")?;
                a.port = Some(p.parse().map_err(|e| format!("--port {p}: {e}"))?);
                i += 1;
            }
            "--connect" => {
                a.connect = Some(value(i, "--connect")?);
                i += 1;
            }
            "--pair" => {
                a.pair = Some(value(i, "--pair")?);
                i += 1;
            }
            "--device-name" => {
                a.device_name = Some(value(i, "--device-name")?);
                i += 1;
            }
            "-h" | "--help" => return Err(usage(edition)),
            other => match edition.parse_arg(v, i) {
                Some(Ok(n)) => {
                    i += n.max(1);
                    continue;
                }
                Some(Err(why)) => return Err(why),
                None => return Err(format!("unknown argument {other} (try --help)")),
            },
        }
        i += 1;
    }
    if a.remote_host && !a.headless {
        return Err(
            "--remote-host is for --headless (the GUI hosts from its Remote panel; --no-remote-host turns that off)".into(),
        );
    }
    if a.accept_project_security && !a.headless {
        return Err(
            "--accept-project-security is for --headless (the GUI accepts held security settings in the Plugin manager)".into(),
        );
    }
    if a.trust_project && !a.headless {
        return Err(
            "--trust-project is for --headless (the GUI asks you, once per project, in the Plugin manager)".into(),
        );
    }
    if !a.remote_host && (a.lan || a.port.is_some()) {
        return Err("--lan and --port configure the headless split-editor host: add --headless --remote-host".into());
    }
    edition.check_args(a.headless)?;
    Ok(a)
}

fn headless(a: &Args) -> Result<(), String> {
    use forge_editor::headless::{HeadlessOptions, run};
    let core = EditorCore::new();
    if a.trust_project {
        // The launcher vouches for what the run opens (WP-34); the remote host too.
        EditorCore::set_trust_book(&core, Arc::new(forge_editor::trust::TrustEvery));
    }
    let run_opts = HeadlessOptions {
        accept_project_security: a.accept_project_security,
        trust_project: a.trust_project,
        ..HeadlessOptions::default()
    };
    if a.remote_host {
        // The split editor's headless host (M2-16): stdin is the host console, so a script
        // (a file) only prepares the project first.
        if let Some(path) = &a.script {
            let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
            let mut out = std::io::stdout().lock();
            run(
                &core,
                path,
                Some(std::io::BufReader::new(f)),
                &run_opts,
                &mut out,
            )
            .map_err(|e| e.to_string())?;
        }
        let opts = forge_remote::console::ConsoleOptions {
            port: a.port,
            lan: a.lan,
            config_dir: if a.no_user_config {
                None
            } else {
                a.config_dir.clone().or_else(user_config_dir)
            },
            program: "forge-editor".into(),
            ..forge_remote::console::ConsoleOptions::default()
        };
        let mut out = std::io::stdout();
        return forge_remote::console::serve(
            &core,
            &opts,
            std::io::BufReader::new(std::io::stdin()),
            &mut out,
        )
        .map_err(|e| e.to_string());
    }
    let mut out = std::io::stdout().lock();
    let r = match &a.script {
        Some(path) => {
            let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
            run(
                &core,
                path,
                Some(std::io::BufReader::new(f)),
                &run_opts,
                &mut out,
            )
        }
        None => run(
            &core,
            "stdin",
            Some(std::io::stdin().lock()),
            &run_opts,
            &mut out,
        ),
    };
    r.map(drop).map_err(|e| e.to_string())
}

/// Teams and collaboration (WP-U10): [`crate::wiring::attach_team_server_with`] as the person
/// running the editor, gated on this machine's licence — the signed entitlement file, verified
/// offline (`forge-licence`, WP-47; `config_dir`: where it is looked for).
fn attach_team_server(core: &forge_editor::core::SharedCore, config_dir: Option<&std::path::Path>) {
    let owner = match gui_issuer() {
        Issuer::Human { user } => user,
        other => other.tag(),
    };
    let licence = Arc::new(crate::licence::SignedFileEntitlement::open(config_dir));
    crate::wiring::attach_team_server_with(core, owner, "team-baseline", licence);
}

fn gui(a: &Args, edition: &mut dyn EditorEdition) -> Result<(), String> {
    // W2 positive control of `ui_startup_budget` (test builds only, ADR 0046): a stall
    // injected into startup must fail the budget.
    #[cfg(feature = "controls")]
    if let Some(ms) = std::env::var("FORGE_STARTUP_STALL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(Duration::from_millis(ms));
    }
    // The UI language (M2-31): `FORGE_UI_LOCALE=qps-ploc` runs the editor under the
    // pseudo-locale, so a string that was not looked up shows as plain English. No
    // translation tables ship yet; any other value keeps the source language.
    if std::env::var("FORGE_UI_LOCALE").is_ok_and(|l| l == forge_ui::l10n::PSEUDO_LOCALE) {
        forge_ui::l10n::set_ui_locale(forge_ui::l10n::PSEUDO_LOCALE, Default::default());
    }
    // The profiler's live feed (forge-trace's counters sink): the play core's steps and the
    // viewport host's renders are measured from the start. Perfetto and Tracy stay off.
    forge_trace::global()
        .enable(forge_trace::Sinks::COUNTERS)
        .map_err(|e| e.to_string())?;
    let first_party = crate::first_party::FirstParty::new()?;
    let mut plugins = first_party.plugins();
    plugins.extend(edition.plugins());
    let preset = preset_in(&a.preset, &plugins).map_err(|e| e.to_string())?;
    let title = format!("Forge editor \u{2014} {}", preset.workspace.label);
    let config_dir = if a.no_user_config {
        None
    } else {
        a.config_dir.clone().or_else(user_config_dir)
    };
    let core = EditorCore::new();
    // Project trust (WP-34): the person's answers live in their user config, never in a
    // project. With no user config they last for this run.
    if let Some(dir) = &config_dir {
        let book = forge_editor::trust::FileTrust::open(dir);
        if let Some(e) = &book.error {
            eprintln!(
                "forge-editor: the trusted-projects file was not read ({e}); nothing is trusted until you decide again"
            );
        }
        EditorCore::set_trust_book(&core, Arc::new(book));
    }
    // WASM plugins (WP-21): the user plugin directory and the open project's `plugins/`,
    // hosted on this editor's core. A split-editor device runs the host's core, so it hosts
    // none of its own.
    let hosting = a
        .connect
        .is_none()
        .then(|| forge_editor::hosting::HostingSetup {
            core: core.clone(),
            dirs: forge_editor::hosting::PluginDirs::for_user(config_dir.as_deref()),
        });
    let mut cfg = assemble_hosted(preset, &plugins, &[], config_dir.clone(), hosting)
        .map_err(|e| e.to_string())?;
    // D-4: the asset database over an in-memory store (crate::wiring).
    cfg.services.set_assets(crate::wiring::asset_catalog(&cfg)?);
    // The split editor (M2-16): as a device, this editor's client is the host's core.
    let remote_client = match &a.connect {
        Some(addr) => Some(split_client(addr, a, config_dir.as_deref())?),
        None => None,
    };
    if remote_client.is_none() {
        // The connect panels follow the core: its grant table, its audit book and its held
        // security settings (WP-U9); then the edition attaches its own services to it.
        cfg.services.connect.attach(&core, config_dir.as_deref());
        edition.attach(&core, &mut cfg, config_dir.as_deref())?;
        if !a.no_remote_host {
            host_remote(&core, &mut cfg, config_dir.as_deref());
        }
    }
    // WP-U10: the labelled in-memory team server and the collaboration panels.
    attach_team_server(&core, config_dir.as_deref());
    cfg.services.collab.attach(&core);
    // One adapter pool for the UI and the viewport renderer (D-3): the viewport's frames are
    // rendered on the device the UI composites them with. Its GPU mode is the user's Graphics
    // preference (Single unless they chose Multi, ADR 0054), read before the pool is built:
    // a change in the settings window applies at the next start.
    let startup = crate::gpu_mode::startup_settings(config_dir.as_deref());
    let pool = forge_gpu::AdapterPool::new(&crate::gpu_mode::pool_options(&startup))
        .map_err(|e| e.to_string())?;
    // The compute panel lists the pool's adapters (throughput is measured when batch jobs
    // run: forge-jobs, M6-1) and this machine's CPU.
    let compute = forge_editor::connect::compute::MemoryComputePool::new();
    let mut adapters = compute_adapters(&pool);
    adapters.extend(forge_editor::connect::compute::ComputePool::snapshot(&compute).adapters);
    compute.set_adapters(adapters);
    cfg.services.connect.compute = Arc::new(compute);
    let wake: Arc<Mutex<Option<Poster>>> = Arc::new(Mutex::new(None));
    let w = wake.clone();
    let waker: forge_editor::core::Waker = Arc::new(move || {
        if let Ok(p) = w.lock()
            && let Some(p) = p.as_ref()
        {
            p.post(|_| {});
        }
    });
    let local = remote_client.is_none();
    let client: Box<dyn BusClient> = match remote_client {
        Some(mut bus) => {
            bus.set_waker(waker);
            Box::new(bus)
        }
        None => {
            let mut c = EditorCore::connect(&core, gui_issuer());
            c.set_waker(waker);
            Box::new(c)
        }
    };
    // --play-2d-sample: the 2D sample game is the play backend (play-in-editor, M4-12).
    let sample = a.play_sample.then(|| Sample2d {
        play: forge_2d_game::pie::SamplePlay::install(&mut cfg.services),
        hosts: std::collections::BTreeMap::new(),
    });
    let shell = Shell::new(cfg, client).map_err(|e| e.to_string())?;
    let adapter = pool.primary().label();
    let mut viewports = crate::viewport_gpu::ViewportRenderHost::new(
        &shell.handles().services_rc(),
        &format!("forge-render on {adapter}"),
    );
    for r in edition.world_renderers(&shell.handles().services_rc()) {
        viewports.add_world_renderer(r);
    }
    // Log records from other threads wake the loop once per drain (zero idle cost).
    let w = wake.clone();
    shell.log().borrow().set_waker(Some(Arc::new(move || {
        if let Ok(p) = w.lock()
            && let Some(p) = p.as_ref()
        {
            p.post(|_| {});
        }
    })));
    if local {
        edition.started(&core);
    }
    let opts = RunOptions {
        size: Size::new(1440.0, 900.0),
        exit_after: a.exit_after,
        gpu: Some(std::sync::Arc::new(pool)),
        ..RunOptions::default()
    };
    let report = run(
        EditorApp {
            shell,
            viewports,
            title,
            wake,
            report_startup: a.report_startup.then(|| adapter.clone()),
            sample,
        },
        opts,
    )
    .map_err(|e| e.to_string())?;
    if a.exit_after.is_some() {
        println!(
            "forge-editor: adapter {} | first frame {:.0} ms | windows {} | frames {} | idle {:.1} s: {} frames, {} wakeups",
            report.adapter,
            report.first_frame_ms,
            report.windows_opened,
            report.frames_rendered,
            report.idle_window.as_secs_f64(),
            report.idle_frames,
            report.idle_wakeups
        );
    }
    Ok(())
}

/// Host this editor's core to paired devices (the split editor, M2-16): loopback unless the
/// pairing store chose the LAN; the Remote panel binds to it. If the default port is taken
/// another is used; if the host cannot start at all the panel keeps the labelled stand-in and
/// the console says why.
fn host_remote(
    core: &forge_editor::core::SharedCore,
    cfg: &mut forge_editor::shell::ShellConfig,
    config_dir: Option<&std::path::Path>,
) {
    let exposure = cfg.services.connect.pairing.borrow().exposure();
    let start = |port: u16| {
        forge_remote::QuicTransport::start(
            core.clone(),
            forge_remote::HostConfig {
                port,
                exposure,
                config_dir: config_dir.map(std::path::Path::to_path_buf),
                ..forge_remote::HostConfig::default()
            },
        )
    };
    match start(forge_remote::DEFAULT_PORT).or_else(|_| start(0)) {
        Ok(t) => {
            t.attach(&mut cfg.services.connect.pairing.borrow_mut());
            if let Some(at) = t.local_addr() {
                println!(
                    "forge-editor: split-editor host on {at} ({})",
                    exposure.label()
                );
            }
            cfg.services.connect.remote = std::rc::Rc::new(std::cell::RefCell::new(t));
        }
        Err(e) => eprintln!(
            "forge-editor: the split-editor host did not start ({e}); the Remote panel shows the stand-in"
        ),
    }
}

/// This editor as a device of the core at `addr`: pair first with `--pair CODE`, or use the
/// pairing kept in the user config.
fn split_client(
    addr: &str,
    a: &Args,
    config_dir: Option<&std::path::Path>,
) -> Result<forge_remote::RemoteBus, String> {
    let addr: std::net::SocketAddr = addr.parse().map_err(|e| format!("--connect {addr}: {e}"))?;
    let identity = forge_remote::Identity::load_or_create(
        config_dir,
        forge_remote::device::DEVICE_IDENTITY_FILE,
        "forge-device",
    )
    .map_err(|e| e.to_string())?;
    let name = a
        .device_name
        .clone()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "this device".into());
    let device = forge_remote::Device::new(identity, &name);
    let mut store =
        forge_remote::device::DeviceStore::open(config_dir).map_err(|e| e.to_string())?;
    let host = match (&a.pair, store.get(addr)) {
        (Some(code), _) => {
            println!(
                "forge-editor: asking {addr} to pair as “{name}”; allow it on the host (Remote › Pair a device)"
            );
            let h = device
                .pair(addr, code, Duration::from_secs(180))
                .map_err(|e| e.to_string())?;
            store.remember(h.clone()).map_err(|e| e.to_string())?;
            h
        }
        (None, Some(h)) => h.clone(),
        (None, None) => {
            return Err(format!(
                "this device is not paired with {addr}: show a code on the host (Remote › Pair a device) and pass --pair CODE"
            ));
        }
    };
    device
        .connect(
            &host,
            forge_remote::ConnectOptions {
                follow_session: true,
                ..forge_remote::ConnectOptions::default()
            },
        )
        .map_err(|e| e.to_string())
}

/// The pool's selected adapters, as the compute panel lists them.
fn compute_adapters(
    pool: &forge_gpu::AdapterPool,
) -> Vec<forge_editor::connect::compute::AdapterInfo> {
    pool.devices()
        .iter()
        .map(|d| forge_editor::connect::compute::AdapterInfo {
            name: d.facts.name.clone(),
            kind: format!("{:?}", d.facts.device_type).to_lowercase(),
            backend: format!("{:?}", d.facts.backend),
            items_per_s: None,
            jobs_done: 0,
            enabled: d.facts.compute,
        })
        .collect()
}

/// When `main` started (the cold-start clock of `--report-startup`).
fn started() -> std::time::Instant {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *START.get_or_init(std::time::Instant::now)
}

/// The editor program's `main`: parse the process arguments (the base flags and
/// `edition`'s), run headless or the GUI, and exit 1 with the error on failure.
pub fn main_with(edition: &mut dyn EditorEdition) {
    let _ = started();
    let v: Vec<String> = std::env::args().skip(1).collect();
    let r = parse_args(&v, edition).and_then(|a| {
        if a.headless {
            headless(&a)
        } else {
            gui(&a, edition)
        }
    });
    if let Err(e) = r {
        eprintln!("forge-editor: {e}");
        std::process::exit(1);
    }
}
