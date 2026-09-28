//! The shipped game around the core: the game-UI runtime's menus (`forge_runtime::game_ui`,
//! WP-U11) over the 2D view, and the loop that runs the level at a fixed 60 Hz while the
//! HUD shows (feature `app`).
//!
//! * **Screens**: main menu (Play, Settings, Credits, Quit), settings, credits with the E-64
//!   engine entry first, the HUD (coins as the score, the pad prompt), the pause menu (Start,
//!   East or Escape), and a level-clear banner. Every visible string is a localisation key
//!   ([`strings`]: English, French and the pseudo-locale).
//! * **Input**: one pad stream. While the level plays, the stick, the d-pad and South go to
//!   the game; Start and East go to the menus (pause). Everywhere else the pad drives the
//!   menus. The window maps held keys onto the same pad inputs ([`Session::key`]).
//! * **Time**: step `n` of a play stretch is due at `n / 60` s after the stretch began, on
//!   the UI clock; at most [`MAX_STEPS_PER_TURN`] run per turn, and a longer stall drops the
//!   backlog instead of fast-forwarding. Pausing costs nothing: no deadline, no redraw.
//! * **Determinism**: the simulation sees only the pad state at each step, so a scripted
//!   run through the menus reaches the same bits as [`crate::play_script`] (the I2 corpus
//!   row), whatever the frame rate.

use std::time::Duration;

use forge_runtime::game_ui::{GameEvent, GameMenu, GameSettings, GameUiConfig, Screen};
use forge_ui::game::{ButtonGlyphs, Credits, GlyphSet, PadButton, PadInput};
use forge_ui::l10n::{LocalisedTexts, Localiser};
use forge_ui::render::{ExternalTexture, Primitive};
use forge_ui::widget::PaintCx;
use forge_ui::widgets::{Container, Label};
use forge_ui::{Dirty, KeyCode, KeyEvent, NodeStyle, Role, Signal, Ui, UiError, Widget, WidgetId};

use crate::{Fingerprint, Game, PadState};

/// The external texture the game view composites (the 2D renderer's output).
pub const GAME_VIEW: ExternalTexture = ExternalTexture(0x2D);
/// Fixed steps one turn may run at most (a longer stall drops the backlog).
pub const MAX_STEPS_PER_TURN: u32 = 15;

/// The sample's string tables: the runtime's menu strings, with the sample's own title,
/// prompt, banner and window title (source locale `en`).
#[must_use]
pub fn strings() -> Localiser {
    forge_runtime::game_ui::sample_strings()
        .with(
            "en",
            &[
                ("menu.title", "Lantern Run"),
                (
                    "game2d.window_title",
                    "Lantern Run \u{2014} a Forge 2D sample",
                ),
                ("game2d.view", "The level"),
                ("hud.prompt", "{confirm} Jump    {back} {menu} Pause"),
                ("game2d.clear", "Level clear! {coins} coins"),
                ("game2d.clear_prompt", "{confirm} Main menu"),
                ("credits.section.Level", "Level"),
            ],
        )
        .with(
            "fr",
            &[
                ("menu.title", "Course \u{e0} la lanterne"),
                (
                    "game2d.window_title",
                    "Course \u{e0} la lanterne \u{2014} un exemple 2D Forge",
                ),
                ("game2d.view", "Le niveau"),
                ("hud.prompt", "{confirm} Sauter    {back} {menu} Pause"),
                ("game2d.clear", "Niveau termin\u{e9} ! {coins} pi\u{e8}ces"),
                ("game2d.clear_prompt", "{confirm} Menu principal"),
                ("credits.section.Level", "Niveau"),
            ],
        )
}

/// The sample's credits: the engine entry (E-64, added by `Credits::new`) first, then the
/// sample's own sections. Names and licences are not translated.
#[must_use]
pub fn credits() -> Credits {
    Credits::new()
        .section(
            "Level",
            &["Level, hero rig and art generated in code (forge_2d::scenes)"],
        )
        .section("Fonts", &["Roboto (Apache-2.0)"])
}

/// What the player is doing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// In the main menu, settings or credits (the level idles behind them).
    Menu,
    Playing,
    Paused,
    /// The level is clear: the banner shows until South.
    Won,
}

/// How the session starts.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub glyphs: GlyphSet,
    /// A locale tag (`en`, `fr`, `qps-ploc`).
    pub language: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            glyphs: GlyphSet::Xbox,
            language: "en".into(),
        }
    }
}

/// The game view: composites the 2D renderer's texture under the menus.
struct GameView {
    label: String,
}

impl Widget for GameView {
    fn role(&self) -> Role {
        Role::Image
    }
    fn paint(&self, cx: &mut PaintCx) {
        let rect = cx.rect();
        cx.primitive(Primitive::Viewport {
            tex: GAME_VIEW,
            rect,
        });
    }
    fn a11y(&self, _cx: &forge_ui::widget::A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
    }
}

/// The game session (see the module docs).
pub struct Session {
    menu: GameMenu,
    game: Game,
    pad: PadState,
    mode: Mode,
    view: WidgetId,
    banner: WidgetId,
    texts: LocalisedTexts,
    clear_text: Signal<String>,
    prompt_text: Signal<String>,
    /// Where the current play stretch began (UI clock) and the game step it began at.
    origin: Duration,
    origin_step: u64,
    fingerprint: Fingerprint,
    /// The view needs a new frame (the game moved, or it was never drawn).
    view_dirty: bool,
    quit: bool,
    /// Everything the menus asked for, in order (tests and the binary's log read it).
    log: Vec<GameEvent>,
    /// A scripted pad: the inputs for game step `i` are applied just before it.
    script: Vec<Vec<PadInput>>,
}

impl Session {
    /// Build the view and the menus under the window root; the main menu opens with the
    /// level idling behind it.
    pub fn build(ui: &mut Ui, cfg: &SessionConfig) -> Result<Session, UiError> {
        let loc = strings();
        let view = ui.add(
            ui.root(),
            "view",
            NodeStyle::cover(),
            GameView {
                label: {
                    let mut l = loc.clone();
                    l.set_current(&cfg.language);
                    l.get("game2d.view", &[])
                },
            },
        )?;
        let menu = GameMenu::build(
            ui,
            GameUiConfig {
                credits: credits(),
                strings: loc,
                glyphs: cfg.glyphs,
                settings: GameSettings {
                    language: cfg.language.clone(),
                    ..GameSettings::default()
                },
                ..GameUiConfig::default()
            },
        )?;
        // The level-clear banner floats over the HUD (the HUD's own rows are the menu's).
        let mut style = NodeStyle::cover();
        let col = NodeStyle::column(ui.theme().space[3]);
        style.layout.flex_direction = col.layout.flex_direction;
        style.layout.gap = col.layout.gap;
        style.layout.align_items = Some(taffy::AlignItems::CENTER);
        style.layout.justify_content = Some(taffy::JustifyContent::CENTER);
        let banner = ui.add(
            menu.root(),
            "clear",
            style.no_hit_test(),
            Container::group(),
        )?;
        let mut texts = LocalisedTexts::new();
        let glyph = cfg.glyphs.glyph(PadButton::South);
        let clear_text = texts.bind_args(
            ui.rt_mut(),
            menu.localiser(),
            "game2d.clear",
            &[("coins", "0")],
        );
        let prompt_text = texts.bind_args(
            ui.rt_mut(),
            menu.localiser(),
            "game2d.clear_prompt",
            &[("confirm", glyph)],
        );
        ui.add(
            banner,
            "title",
            NodeStyle::leaf(),
            Label::new(clear_text).heading(),
        )?;
        ui.add(banner, "prompt", NodeStyle::leaf(), Label::new(prompt_text))?;
        ui.set_hidden(banner, true)?;
        let game = Game::new().map_err(|e| UiError::Layout(e.to_string()))?;
        Ok(Session {
            menu,
            game,
            pad: PadState::default(),
            mode: Mode::Menu,
            view,
            banner,
            texts,
            clear_text,
            prompt_text,
            origin: Duration::ZERO,
            origin_step: 0,
            fingerprint: Fingerprint::default(),
            view_dirty: true,
            quit: false,
            log: Vec::new(),
            script: Vec::new(),
        })
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }
    #[must_use]
    pub fn game(&self) -> &Game {
        &self.game
    }
    #[must_use]
    pub fn menu(&self) -> &GameMenu {
        &self.menu
    }
    /// The game view's widget.
    #[must_use]
    pub fn view(&self) -> WidgetId {
        self.view
    }
    /// The level-clear banner.
    #[must_use]
    pub fn banner(&self) -> WidgetId {
        self.banner
    }
    /// The current play's fingerprint (reset by Play).
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }
    /// The player chose Quit.
    #[must_use]
    pub fn quit_requested(&self) -> bool {
        self.quit
    }
    /// What the menus asked for so far.
    #[must_use]
    pub fn log(&self) -> &[GameEvent] {
        &self.log
    }
    /// Take the "the view needs a new frame" flag (the renderer host asks once per frame).
    pub fn take_view_dirty(&mut self) -> bool {
        std::mem::take(&mut self.view_dirty)
    }

    /// The window title, in the current language.
    #[must_use]
    pub fn title(&self) -> String {
        self.menu.localiser().get("game2d.window_title", &[])
    }

    /// One pad input at `now` (the UI clock).
    pub fn pad(&mut self, ui: &mut Ui, now: Duration, i: PadInput) {
        match self.mode {
            Mode::Playing if Self::is_gameplay(i) => self.pad.apply(i),
            Mode::Won => {
                if let PadInput::Button {
                    button: PadButton::South,
                    pressed: true,
                } = i
                {
                    self.back_to_main_menu(ui);
                }
            }
            _ => {
                self.menu.pad(ui, now, i);
                self.after_menu(ui, now);
            }
        }
    }

    fn is_gameplay(i: PadInput) -> bool {
        match i {
            PadInput::Stick { .. } => true,
            PadInput::Button { button, .. } => matches!(
                button,
                PadButton::South | PadButton::DPadLeft | PadButton::DPadRight
            ),
        }
    }

    /// A key before the UI routes it: while the level plays (or its banner shows), the
    /// held movement keys become pad inputs — arrows or A/D run, Space, Up or W jump — and
    /// are consumed; everything else (Escape pauses) goes to the menus.
    pub fn key(&mut self, ui: &mut Ui, now: Duration, ev: &KeyEvent) -> bool {
        if !matches!(self.mode, Mode::Playing | Mode::Won) || ev.repeat {
            return false;
        }
        let button = match ev.code {
            KeyCode::Left | KeyCode::Char('a') => PadButton::DPadLeft,
            KeyCode::Right | KeyCode::Char('d') => PadButton::DPadRight,
            KeyCode::Space | KeyCode::Up | KeyCode::Char('w') | KeyCode::Enter => PadButton::South,
            _ => return false,
        };
        self.pad(
            ui,
            now,
            PadInput::Button {
                button,
                pressed: ev.pressed,
            },
        );
        true
    }

    /// The window's actions (the runner's `on_actions`).
    pub fn on_actions(
        &mut self,
        ui: &mut Ui,
        now: Duration,
        actions: Vec<forge_ui::ActionEnvelope>,
    ) {
        self.menu.on_actions(ui, actions);
        self.after_menu(ui, now);
    }

    /// Settings the player changed, and the menus' events.
    fn after_menu(&mut self, ui: &mut Ui, now: Duration) {
        self.menu.update(ui);
        for e in self.menu.take_events() {
            match &e {
                GameEvent::StartGame => self.start(ui, now),
                GameEvent::Paused => self.mode = Mode::Paused,
                GameEvent::Resume => {
                    self.mode = Mode::Playing;
                    self.origin = now;
                    self.origin_step = self.game.steps;
                }
                GameEvent::QuitToMenu => self.mode = Mode::Menu,
                GameEvent::Quit => self.quit = true,
                GameEvent::SettingsChanged(_) => {
                    // A language switch: the banner follows the menus' localiser.
                    self.texts.apply(ui.rt_mut(), self.menu.localiser());
                }
            }
            self.log.push(e);
        }
    }

    /// Play: a new run of the level from the start.
    fn start(&mut self, ui: &mut Ui, now: Duration) {
        match Game::new() {
            Ok(g) => self.game = g,
            // The level is generated code; it builds or the menus stay (never a panic).
            Err(_) => return,
        }
        self.pad = PadState::default();
        self.fingerprint = Fingerprint::default();
        self.mode = Mode::Playing;
        self.origin = now;
        self.origin_step = 0;
        self.menu.set_hud(ui, 1.0, 0);
        self.view_dirty = true;
        ui.invalidate(self.view, Dirty::PAINT);
    }

    fn back_to_main_menu(&mut self, ui: &mut Ui) {
        let _ = ui.set_hidden(self.banner, true);
        self.menu.show(ui, Screen::Main);
        self.mode = Mode::Menu;
    }

    /// A scripted pad for the level (`--demo`, `--play-script`, tests): the inputs for game
    /// step `i` are applied just before step `i`, on top of any live input.
    pub fn set_script(&mut self, script: Vec<Vec<PadInput>>) {
        self.script = script;
    }

    /// One fixed step with the pad as it is now (tests drive the level step by step; the
    /// window runs [`Session::advance`]).
    pub fn step_once(&mut self, ui: &mut Ui) {
        if self.mode != Mode::Playing {
            return;
        }
        let i = usize::try_from(self.game.steps).unwrap_or(usize::MAX);
        if let Some(inputs) = self.script.get(i) {
            for x in inputs {
                self.pad.apply(*x);
            }
        }
        if self.game.step(&self.pad).is_err() {
            // The core refuses only malformed bodies, which the fixed level never has.
            return;
        }
        self.fingerprint.observe(self.game.steps - 1, &self.game);
        self.menu.set_hud(ui, 1.0, u64::from(self.game.coins_taken));
        self.view_dirty = true;
        ui.invalidate(self.view, Dirty::PAINT);
        if self.game.won {
            self.mode = Mode::Won;
            let coins = self.game.coins_taken.to_string();
            self.texts.set_args(
                ui.rt_mut(),
                self.menu.localiser(),
                self.clear_text,
                &[("coins", &coins)],
            );
            let _ = ui.set_hidden(self.banner, false);
        }
    }

    /// Run the steps due by `now` (at most [`MAX_STEPS_PER_TURN`]). Returns how many ran.
    pub fn advance(&mut self, ui: &mut Ui, now: Duration) -> u32 {
        if self.mode != Mode::Playing {
            return 0;
        }
        let due = Self::steps_in(now.saturating_sub(self.origin));
        let taken = self.game.steps - self.origin_step;
        if due.saturating_sub(taken) > u64::from(MAX_STEPS_PER_TURN) {
            // A stall (a dragged window, a debugger): drop the backlog, keep the cadence.
            self.origin = now;
            self.origin_step = self.game.steps;
            return 0;
        }
        let mut n = 0;
        while self.mode == Mode::Playing && self.game.steps - self.origin_step < due {
            self.step_once(ui);
            n += 1;
        }
        n
    }

    /// Whole steps in `d` (step `n` is due at `n / 60` s).
    fn steps_in(d: Duration) -> u64 {
        (d.as_micros() * 60 / 1_000_000) as u64
    }

    /// When the next step is due (`None` unless playing: a menu or a pause costs nothing).
    #[must_use]
    pub fn next_deadline(&self) -> Option<Duration> {
        if self.mode != Mode::Playing {
            return self.menu.next_deadline();
        }
        let next = self.game.steps - self.origin_step + 1;
        // Step n is due at ceil(n * 10^6 / 60) microseconds.
        let us = (u128::from(next) * 1_000_000).div_ceil(60);
        Some(self.origin + Duration::from_micros(u64::try_from(us).unwrap_or(u64::MAX)))
    }

    /// Run pad repeats that are due (the menus' auto-repeat).
    pub fn tick(&mut self, ui: &mut Ui, now: Duration) {
        self.menu.tick(ui, now);
    }

    /// The prompt under the banner (tests read it).
    #[must_use]
    pub fn banner_texts(&self, ui: &Ui) -> (String, String) {
        (
            self.clear_text.get(ui.rt()).clone(),
            self.prompt_text.get(ui.rt()).clone(),
        )
    }
}
