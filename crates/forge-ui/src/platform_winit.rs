//! The winit runner (Ch.21 §21.14) — feature `winit`.
//!
//! * `run` implements `winit`'s `ApplicationHandler`. The loop starts in
//!   `ControlFlow::Wait` and only ever uses `Wait` or `WaitUntil(next deadline)`, from
//!   [`Ui::next_wake`] — never `Poll` (`ui_idle_no_busy_loop`).
//! * **Multi-window.** One `Ui` per OS window (the main window, floating dock windows,
//!   detached viewports), opened by the [`OpenWindow`] action. All windows share one
//!   device, one renderer, one glyph atlas, one image atlas and one shaping cache; each has
//!   its own surface, persistent UI target and damage set.
//! * The AccessKit adapter of each window is created before the window is first shown, as
//!   `accesskit_winit` requires, and activates that window's a11y tree lazily.
//! * Per-window DPI: the window's scale factor × the user's UI scale (50–300%); layout
//!   stays logical, a scale change re-shapes text and repaints.
//! * IME is enabled only while a text widget has focus; the caret rect follows the caret.
//! * Surfaces present with `Fifo` (vsync), and a redraw is requested only on damage.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::UiError;
use crate::damage::{UiWaker, Wake};
use crate::dock::MonitorInfo;
use crate::geom::{PhysicalSize, Point, Rect, Size};
use crate::input::{ImeEvent, InputEvent, KeyCode, KeyEvent, Modifiers, PointerButton};
use crate::render::{TargetId, UiRenderer};
use crate::render_wgpu::{AdapterPool, GpuMode, WgpuRenderer};
use crate::style::Theme;
use crate::ui::{ActionEnvelope, CloseWindow, OpenWindow, Ui, UiConfig};

/// An application on `forge-ui`.
pub trait UiApp: 'static {
    fn title(&self) -> String;
    /// Build the main window's retained tree once.
    fn build(&mut self, ui: &mut Ui) -> Result<(), UiError>;
    /// Build a window opened by an [`OpenWindow`] action.
    fn build_window(&mut self, _ui: &mut Ui, _key: u64) -> Result<(), UiError> {
        Ok(())
    }
    /// Actions no widget-level handler consumed.
    fn on_actions(&mut self, _ui: &mut Ui, _actions: Vec<ActionEnvelope>) {}
    /// Called for every window after each turn's actions (`key`: `None` for the main
    /// window). A multi-window application pushes shared state into each window here:
    /// the dock layout (`DockController::sync_window`).
    fn update_window(&mut self, _ui: &mut Ui, _key: Option<u64>) {}
    /// A window moved or resized: its client rect in desktop coordinates (physical pixels
    /// of the virtual screen divided by the primary monitor's scale factor, one linear
    /// space across monitors of different DPI) and the monitor it is on.
    fn window_moved(&mut self, _key: Option<u64>, _rect: Rect, _monitor: Option<String>) {}
    /// The user closed a secondary window with the OS close button.
    fn window_closed(&mut self, _key: u64) {}
    /// The monitors (desktop logical px), at start-up.
    fn monitors(&mut self, _monitors: Vec<MonitorInfo>) {}
    /// The application's own next deadline (a debounced autosave), on the runner's clock
    /// (`Ui::now`). The loop sleeps in `WaitUntil` for it and runs a turn when it is due,
    /// so an application never needs a polling timer. `None`: nothing is due.
    fn next_deadline(&self) -> Option<Duration> {
        None
    }
    /// The user's UI scale (50–300%) if the application changed it at run time (the
    /// settings window). Asked once per turn; a change re-lays out every window.
    fn user_scale(&self) -> Option<f32> {
        None
    }
    /// The loop is exiting (the main window closed): flush what must survive (autosave).
    fn exiting(&mut self) {}
    /// A key event before the UI routes it — presses **and** releases (the UI's own routing
    /// drops releases). `true` consumes it. A game reads its held movement keys here while it
    /// plays and returns `false` for everything else, so menus keep their keyboard
    /// behaviour. The editor never overrides it.
    fn key_first(&mut self, _ui: &mut Ui, _key: Option<u64>, _ev: &KeyEvent) -> bool {
        false
    }
    /// The application wants the loop to end (a game menu's Quit). Asked after every turn;
    /// `true` leaves the loop as closing the main window does: `exiting` runs and `run`
    /// returns its report (no `process::exit` skipping either).
    fn exit_requested(&self) -> bool {
        false
    }
    /// Every window has drawn its first frame and has nothing left to do: the application
    /// is **interactive** (Ch.21 §21.22 `ui_startup_budget` measures cold start to here).
    /// Called once per run.
    fn settled(&mut self) {}
    /// A window's frame was recorded and is about to be drawn: render the textures it
    /// composites (`Primitive::Viewport` — a `forge-render` viewport) on the UI's device
    /// and register them. Called only for a frame that has damage, so an idle window
    /// renders nothing here either.
    fn render_external(
        &mut self,
        _ui: &mut Ui,
        _key: Option<u64>,
        _cx: &mut crate::render_wgpu::ExternalCx<'_>,
    ) {
    }
}

/// Runner options.
#[derive(Clone)]
pub struct RunOptions {
    /// Initial main-window size (logical px).
    pub size: Size,
    pub theme: Theme,
    /// The user's UI scale (50–300%), multiplied with each monitor's scale factor.
    pub user_scale: f32,
    /// Exit this long after the UI first settles (smoke runs, idle measurement).
    pub exit_after: Option<Duration>,
    pub reduced_motion: bool,
    /// The GPU adapter pool to render on (D-3). `None`: the runner builds a default pool;
    /// an editor passes its own so the UI, the viewport renderer and Tier-0 compute share one.
    pub gpu: Option<Arc<AdapterPool>>,
    /// The GPU mode of the pool the runner builds when `gpu` is `None` (an exported game's
    /// Graphics setting): [`GpuMode::Single`] unless the game opted in to Multi. The pool
    /// is built once, at startup, so a change applies at the next start.
    pub gpu_mode: GpuMode,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            size: Size::new(1100.0, 640.0),
            theme: Theme::dark(),
            user_scale: 1.0,
            exit_after: None,
            reduced_motion: false,
            gpu: None,
            gpu_mode: GpuMode::Single,
        }
    }
}

/// What a run did (the idle measurements read it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunReport {
    pub frames_rendered: u64,
    /// Loop wakeups not caused by input: timer/animation deadlines and waker calls.
    pub wakeups: u64,
    /// Frames and wakeups after the UI first settled (the idle window).
    pub idle_frames: u64,
    pub idle_wakeups: u64,
    pub adapter: String,
    pub first_frame_ms: f64,
    /// When every window first had nothing left to do (the first interactive frame), from
    /// the start of `run`.
    pub settled_ms: f64,
    pub idle_window: Duration,
    pub windows_opened: u32,
}

/// Events other threads send into the loop.
#[derive(Debug)]
pub enum UserEvent {
    Wake,
    AccessKit(accesskit_winit::Event),
    Exit,
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(e: accesskit_winit::Event) -> Self {
        UserEvent::AccessKit(e)
    }
}

struct ProxyWaker(Mutex<EventLoopProxy<UserEvent>>);

impl UiWaker for ProxyWaker {
    fn wake(&self) {
        if let Ok(p) = self.0.lock() {
            let _ = p.send_event(UserEvent::Wake);
        }
    }
}

struct Win {
    window: Arc<Window>,
    ui: Ui,
    target: TargetId,
    access: accesskit_winit::Adapter,
    cursor: Point,
    main: bool,
    /// The `OpenWindow` key (`None`: the main window).
    key: Option<u64>,
}

/// The monitor under a window's client area.
fn monitor_name(w: &Window) -> Option<String> {
    w.current_monitor().and_then(|m| m.name())
}

/// Desktop coordinates: physical pixels of the virtual screen divided by the primary
/// monitor's scale factor (one linear space across monitors of different DPI).
fn desktop_scale(el: &ActiveEventLoop) -> f64 {
    el.primary_monitor()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0)
        .max(0.25)
}

/// A window's client rect in desktop coordinates.
fn desktop_rect(w: &Window, desktop: f64) -> Rect {
    let pos = w.inner_position().unwrap_or(PhysicalPosition::new(0, 0));
    let size = w.inner_size();
    Rect::new(
        (f64::from(pos.x) / desktop) as f32,
        (f64::from(pos.y) / desktop) as f32,
        (f64::from(size.width) / desktop) as f32,
        (f64::from(size.height) / desktop) as f32,
    )
}

struct Runner<A: UiApp> {
    app: A,
    opts: RunOptions,
    proxy: EventLoopProxy<UserEvent>,
    windows: BTreeMap<WindowId, Win>,
    renderer: Option<WgpuRenderer>,
    next_target: u32,
    start: Instant,
    report: RunReport,
    mods: Modifiers,
    error: Option<UiError>,
    settled_at: Option<Instant>,
    exit_armed: bool,
    pending_opens: Vec<OpenWindow>,
    /// Desktop coordinate scale (see [`desktop_scale`]).
    desktop: f64,
}

fn map_key(k: &Key) -> KeyCode {
    match k {
        Key::Named(n) => match n {
            NamedKey::Tab => KeyCode::Tab,
            NamedKey::Enter => KeyCode::Enter,
            NamedKey::Escape => KeyCode::Escape,
            NamedKey::Space => KeyCode::Space,
            NamedKey::Backspace => KeyCode::Backspace,
            NamedKey::Delete => KeyCode::Delete,
            NamedKey::ArrowLeft => KeyCode::Left,
            NamedKey::ArrowRight => KeyCode::Right,
            NamedKey::ArrowUp => KeyCode::Up,
            NamedKey::ArrowDown => KeyCode::Down,
            NamedKey::Home => KeyCode::Home,
            NamedKey::End => KeyCode::End,
            NamedKey::PageUp => KeyCode::PageUp,
            NamedKey::PageDown => KeyCode::PageDown,
            NamedKey::F1 => KeyCode::F1,
            NamedKey::F2 => KeyCode::F2,
            NamedKey::F3 => KeyCode::F3,
            NamedKey::F4 => KeyCode::F4,
            NamedKey::F5 => KeyCode::F5,
            NamedKey::F6 => KeyCode::F6,
            NamedKey::F7 => KeyCode::F7,
            NamedKey::F8 => KeyCode::F8,
            NamedKey::F9 => KeyCode::F9,
            NamedKey::F10 => KeyCode::F10,
            NamedKey::F11 => KeyCode::F11,
            NamedKey::F12 => KeyCode::F12,
            NamedKey::Insert => KeyCode::Insert,
            NamedKey::ContextMenu => KeyCode::ContextMenu,
            _ => KeyCode::Other,
        },
        Key::Character(s) => s
            .chars()
            .next()
            .map(|c| KeyCode::Char(c.to_lowercase().next().unwrap_or(c)))
            .unwrap_or(KeyCode::Other),
        _ => KeyCode::Other,
    }
}

impl<A: UiApp> Runner<A> {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    fn scale_of(&self, w: &Window) -> f32 {
        (w.scale_factor() as f32 * self.opts.user_scale.clamp(0.5, 3.0)).max(0.25)
    }

    /// Create an OS window with its a11y adapter, surface target and `Ui`.
    fn open(
        &mut self,
        el: &ActiveEventLoop,
        title: &str,
        size: Size,
        key: Option<u64>,
        position: Option<Point>,
    ) -> Result<(), UiError> {
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(f64::from(size.w), f64::from(size.h)))
            .with_visible(false);
        if let Some(p) = position {
            // Desktop coordinates back to physical pixels of the virtual screen.
            attrs = attrs.with_position(PhysicalPosition::new(
                (f64::from(p.x) * self.desktop).round() as i32,
                (f64::from(p.y) * self.desktop).round() as i32,
            ));
        }
        let window = Arc::new(
            el.create_window(attrs)
                .map_err(|e| UiError::Platform(e.to_string()))?,
        );
        // Must exist before the window is first shown.
        let access =
            accesskit_winit::Adapter::with_event_loop_proxy(el, &window, self.proxy.clone());
        let phys = window.inner_size();
        let psize = PhysicalSize {
            w: phys.width.max(1),
            h: phys.height.max(1),
        };
        let target = TargetId(self.next_target);
        self.next_target += 1;
        match self.renderer.as_mut() {
            None => {
                let (r, adapter) = match self.opts.gpu.clone() {
                    Some(pool) => WgpuRenderer::for_window_in(pool, window.clone(), target, psize)?,
                    None => {
                        WgpuRenderer::for_window(window.clone(), target, psize, self.opts.gpu_mode)?
                    }
                };
                self.report.adapter = adapter;
                self.renderer = Some(r);
            }
            Some(r) => r.add_window(window.clone(), target, psize)?,
        }
        let scale = self.scale_of(&window);
        let cfg = UiConfig {
            theme: self.opts.theme.clone(),
            size: Size::new(psize.w as f32 / scale, psize.h as f32 / scale),
            scale,
            ..UiConfig::default()
        };
        // Every window after the first shares the first window's atlas and shaping cache.
        let mut ui = match self.windows.values().find(|w| w.main) {
            Some(main) => Ui::with_resources(cfg, main.ui.resources())?,
            None => Ui::new(cfg)?,
        };
        ui.set_reduced_motion(self.opts.reduced_motion);
        ui.set_window_key(key);
        ui.set_title(title);
        ui.set_waker(Arc::new(ProxyWaker(Mutex::new(self.proxy.clone()))));
        // The real OS clipboard (arboard; D-8). It reports the in-process fallback itself
        // when the platform clipboard cannot be opened.
        ui.set_clipboard(Box::new(crate::clipboard::OsClipboard::new()));
        let rect = desktop_rect(&window, self.desktop);
        ui.set_window_origin(Point::new(rect.x, rect.y), monitor_name(&window));
        match key {
            None => self.app.build(&mut ui)?,
            Some(k) => self.app.build_window(&mut ui, k)?,
        }
        ui.damage_all();
        window.set_visible(true);
        self.report.windows_opened += 1;
        self.windows.insert(
            window.id(),
            Win {
                window,
                ui,
                target,
                access,
                cursor: Point::ZERO,
                main: key.is_none(),
                key,
            },
        );
        Ok(())
    }

    /// Route one window's actions: window requests to the runner, the rest to the app.
    fn route_actions(
        app: &mut A,
        w: &mut Win,
        id: WindowId,
        now: Duration,
        opens: &mut Vec<OpenWindow>,
        close: &mut Vec<WindowId>,
    ) {
        let mut app_actions = Vec::new();
        for a in w.ui.take_actions() {
            if let Some(o) = a.get::<OpenWindow>() {
                opens.push(o.clone());
            } else if a.get::<CloseWindow>().is_some() {
                if !w.main {
                    close.push(id);
                }
            } else {
                app_actions.push(a);
            }
        }
        if !app_actions.is_empty() {
            app.on_actions(&mut w.ui, app_actions);
            w.ui.frame(now);
        }
    }

    /// Steps 1 and 4–7 for every window, then hand damage, IME, a11y and window requests
    /// to the platform.
    fn pump(&mut self) {
        let now = self.now();
        let mut close = Vec::new();
        // Pass 1: every window's frame and actions (one window's actions can change what
        // another shows: a panel dropped from a floating window into the main one).
        for (id, w) in self.windows.iter_mut() {
            w.ui.frame(now);
            Self::route_actions(
                &mut self.app,
                w,
                *id,
                now,
                &mut self.pending_opens,
                &mut close,
            );
        }
        // A UI-scale change from the application re-lays out every window.
        if let Some(s) = self.app.user_scale().map(|s| s.clamp(0.5, 3.0))
            && (s - self.opts.user_scale).abs() > f32::EPSILON
        {
            self.opts.user_scale = s;
            for w in self.windows.values_mut() {
                let scale = (w.window.scale_factor() as f32 * s).max(0.25);
                let p = w.window.inner_size();
                w.ui.set_viewport(
                    Size::new(p.width as f32 / scale, p.height as f32 / scale),
                    scale,
                );
                w.ui.damage_all();
            }
        }
        // Pass 2: the app pushes shared state into each window, then damage, IME, a11y.
        for (id, w) in self.windows.iter_mut() {
            self.app.update_window(&mut w.ui, w.key);
            let rep = w.ui.frame(now);
            Self::route_actions(
                &mut self.app,
                w,
                *id,
                now,
                &mut self.pending_opens,
                &mut close,
            );
            if !rep.damage.is_empty() || w.ui.has_damage() {
                w.window.request_redraw();
            }
            if let Some(req) = w.ui.take_ime_request() {
                w.window.set_ime_allowed(req.allowed);
                if let Some(c) = req.caret {
                    let s = f64::from(w.ui.scale()) / w.window.scale_factor();
                    w.window.set_ime_cursor_area(
                        LogicalPosition::new(f64::from(c.x) * s, f64::from(c.y) * s),
                        LogicalSize::new(f64::from(c.w.max(1.0)) * s, f64::from(c.h) * s),
                    );
                }
            }
            if let Some(update) = w.ui.a11y_take_update() {
                w.access.update_if_active(|| update);
            }
        }
        for id in close {
            if let Some(w) = self.windows.remove(&id)
                && let Some(r) = self.renderer.as_mut()
            {
                r.remove_target(w.target);
            }
        }
    }

    fn redraw(&mut self, id: WindowId) {
        let now = self.now();
        let (Some(w), Some(r)) = (self.windows.get_mut(&id), self.renderer.as_mut()) else {
            return;
        };
        w.ui.frame(now);
        if w.ui.has_damage() {
            let mut cx = crate::render_wgpu::ExternalCx::new(r);
            self.app.render_external(&mut w.ui, w.key, &mut cx);
        }
        match w.ui.render(r, w.target) {
            Ok(Some(_)) => {
                self.report.frames_rendered += 1;
                if self.report.first_frame_ms == 0.0 {
                    self.report.first_frame_ms = self.start.elapsed().as_secs_f64() * 1000.0;
                }
                if self.settled_at.is_some() {
                    self.report.idle_frames += 1;
                }
            }
            Ok(None) => {}
            Err(e) => self.error = Some(e),
        }
    }

    fn count_wakeup(&mut self) {
        self.report.wakeups += 1;
        if self.settled_at.is_some() {
            self.report.idle_wakeups += 1;
        }
    }
}

impl<A: UiApp> ApplicationHandler<UserEvent> for Runner<A> {
    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.app.exiting();
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.count_wakeup();
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.windows.is_empty() {
            self.desktop = desktop_scale(el);
            let desktop = self.desktop;
            let monitors: Vec<MonitorInfo> = el
                .available_monitors()
                .map(|m| {
                    let (p, s) = (m.position(), m.size());
                    let primary = el.primary_monitor().is_some_and(|pm| pm == m);
                    MonitorInfo {
                        name: m.name(),
                        rect: Rect::new(
                            (f64::from(p.x) / desktop) as f32,
                            (f64::from(p.y) / desktop) as f32,
                            (f64::from(s.width) / desktop) as f32,
                            (f64::from(s.height) / desktop) as f32,
                        ),
                        primary,
                    }
                })
                .collect();
            self.app.monitors(monitors);
            let (title, size) = (self.app.title(), self.opts.size);
            if let Err(e) = self.open(el, &title, size, None, None) {
                self.error = Some(e);
                el.exit();
            }
            self.pump();
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Wake => self.count_wakeup(),
            UserEvent::Exit => {
                if let Some(t) = self.settled_at {
                    self.report.idle_window = t.elapsed();
                }
                el.exit();
                return;
            }
            UserEvent::AccessKit(e) => {
                let Some(w) = self.windows.get_mut(&e.window_id) else {
                    return;
                };
                match e.window_event {
                    accesskit_winit::WindowEvent::InitialTreeRequested => {
                        let tree = w.ui.a11y_activate();
                        w.access.update_if_active(|| tree);
                    }
                    accesskit_winit::WindowEvent::ActionRequested(req) => w.ui.a11y_action(&req),
                    accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                        w.ui.a11y_deactivate()
                    }
                }
            }
        }
        self.pump();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let user_scale = self.opts.user_scale.clamp(0.5, 3.0);
        let mods = self.mods;
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        w.access.process_event(&w.window, &event);
        let logical = |w: &Win, x: f64, y: f64| {
            let s = f64::from(w.ui.scale());
            Point::new((x / s) as f32, (y / s) as f32)
        };
        match event {
            WindowEvent::CloseRequested => {
                if w.main {
                    el.exit();
                    return;
                }
                let target = w.target;
                if let Some(k) = w.key {
                    self.app.window_closed(k);
                }
                self.windows.remove(&id);
                if let Some(r) = self.renderer.as_mut() {
                    r.remove_target(target);
                }
                return;
            }
            WindowEvent::Resized(size) => {
                let p = PhysicalSize {
                    w: size.width.max(1),
                    h: size.height.max(1),
                };
                if let Some(r) = self.renderer.as_mut()
                    && let Err(e) = r.resize(w.target, p)
                {
                    self.error = Some(e);
                }
                let s = w.ui.scale();
                w.ui.set_viewport(Size::new(p.w as f32 / s, p.h as f32 / s), s);
                w.ui.damage_all();
                let r = desktop_rect(&w.window, self.desktop);
                self.app.window_moved(w.key, r, monitor_name(&w.window));
            }
            WindowEvent::Moved(_) => {
                let r = desktop_rect(&w.window, self.desktop);
                let m = monitor_name(&w.window);
                w.ui.set_window_origin(Point::new(r.x, r.y), m.clone());
                self.app.window_moved(w.key, r, m);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let s = (w.window.scale_factor() as f32 * user_scale).max(0.25);
                let p = w.window.inner_size();
                w.ui.set_viewport(Size::new(p.width as f32 / s, p.height as f32 / s), s);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = logical(w, position.x, position.y);
                w.cursor = p;
                w.ui.handle(InputEvent::PointerMoved(p));
            }
            WindowEvent::CursorLeft { .. } => w.ui.handle(InputEvent::PointerLeft),
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => PointerButton::Primary,
                    MouseButton::Right => PointerButton::Secondary,
                    MouseButton::Middle => PointerButton::Middle,
                    _ => return,
                };
                let pos = w.cursor;
                w.ui.handle(InputEvent::PointerButton {
                    pos,
                    button,
                    pressed: state == ElementState::Pressed,
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 40.0, y * 40.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let pos = w.cursor;
                w.ui.handle(InputEvent::Wheel { pos, dx, dy });
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.mods = Modifiers {
                    shift: s.shift_key(),
                    ctrl: s.control_key(),
                    alt: s.alt_key(),
                    meta: s.super_key(),
                };
                w.ui.handle(InputEvent::Modifiers(self.mods));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let k = KeyEvent {
                    code: map_key(&event.logical_key),
                    mods,
                    pressed,
                    repeat: event.repeat,
                    text: if pressed && !mods.ctrl && !mods.alt {
                        event.text.as_ref().map(|t| t.to_string())
                    } else {
                        None
                    },
                };
                // A game reads held keys (presses and releases) before the UI routes them.
                if !self.app.key_first(&mut w.ui, w.key, &k) {
                    w.ui.handle(InputEvent::Key(k));
                }
            }
            WindowEvent::Ime(ime) => {
                let e = match ime {
                    winit::event::Ime::Enabled => ImeEvent::Enabled,
                    winit::event::Ime::Preedit(text, cursor) => ImeEvent::Preedit { text, cursor },
                    winit::event::Ime::Commit(s) => ImeEvent::Commit(s),
                    winit::event::Ime::Disabled => ImeEvent::Disabled,
                };
                w.ui.handle(InputEvent::Ime(e));
            }
            WindowEvent::DroppedFile(path) => {
                let pos = w.cursor;
                w.ui.handle(InputEvent::FileDropped { pos, path });
            }
            WindowEvent::Focused(f) => w.ui.handle(InputEvent::WindowFocused(f)),
            WindowEvent::Occluded(o) => w.ui.set_window_visible(!o),
            WindowEvent::RedrawRequested => self.redraw(id),
            _ => {}
        }
        self.pump();
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.pump();
        for o in std::mem::take(&mut self.pending_opens) {
            if let Err(e) = self.open(el, &o.title, o.size, Some(o.key), o.position) {
                self.error = Some(e);
            }
            self.pump();
        }
        // The application asked to end (a game's Quit): leave the loop the ordinary way, so
        // `exiting` runs and `run` returns its report.
        if self.app.exit_requested() {
            if let Some(t) = self.settled_at {
                self.report.idle_window = t.elapsed();
            }
            el.exit();
            return;
        }
        // The earliest deadline over every window; Wait only if all are idle.
        let mut wake = Wake::Wait;
        for w in self.windows.values() {
            wake = match (wake, w.ui.next_wake()) {
                (Wake::Now, _) | (_, Wake::Now) => Wake::Now,
                (Wake::WaitUntil(a), Wake::WaitUntil(b)) => Wake::WaitUntil(a.min(b)),
                (Wake::WaitUntil(a), Wake::Wait) | (Wake::Wait, Wake::WaitUntil(a)) => {
                    Wake::WaitUntil(a)
                }
                (Wake::Wait, Wake::Wait) => Wake::Wait,
            };
        }
        if let Some(d) = self.app.next_deadline() {
            wake = match wake {
                Wake::Now => Wake::Now,
                Wake::WaitUntil(a) => Wake::WaitUntil(a.min(d)),
                Wake::Wait => Wake::WaitUntil(d),
            };
        }
        // The idle window starts the first time every window has nothing left to do.
        if wake == Wake::Wait && self.settled_at.is_none() {
            self.settled_at = Some(Instant::now());
            self.report.settled_ms = self.start.elapsed().as_secs_f64() * 1000.0;
            self.app.settled();
            if let Some(d) = self.opts.exit_after
                && !self.exit_armed
            {
                self.exit_armed = true;
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(d);
                    let _ = proxy.send_event(UserEvent::Exit);
                });
            }
        }
        el.set_control_flow(match wake {
            Wake::Wait | Wake::Now => ControlFlow::Wait,
            Wake::WaitUntil(t) => ControlFlow::WaitUntil(self.start + t),
        });
    }
}

/// Run `app` until its main window is closed (or `exit_after` elapses).
pub fn run(app: impl UiApp, opts: RunOptions) -> Result<RunReport, UiError> {
    let el = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| UiError::Platform(e.to_string()))?;
    el.set_control_flow(ControlFlow::Wait);
    let mut runner = Runner {
        app,
        opts,
        proxy: el.create_proxy(),
        windows: BTreeMap::new(),
        renderer: None,
        next_target: 0,
        start: Instant::now(),
        report: RunReport::default(),
        mods: Modifiers::NONE,
        error: None,
        settled_at: None,
        exit_armed: false,
        pending_opens: Vec::new(),
        desktop: 1.0,
    };
    el.run_app(&mut runner)
        .map_err(|e| UiError::Platform(e.to_string()))?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(runner.report),
    }
}
