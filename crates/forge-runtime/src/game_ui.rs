//! The sample game UI (Ch.21 §21.20, DoD M2-29): a main menu, a settings screen, a HUD, a
//! pause menu and the credits (with the E-64 engine entry), on `forge-ui` — the same
//! retained layer the editor is built on — **with no editor crate linked**
//! (`tests/liveness/test_game_ui_links_no_editor.rs`).
//!
//! * **Game state, not project state.** Game UI binds to game state through signals, and
//!   what the player asks for goes to game systems as **ECS messages** ([`GameEvent`],
//!   delivered into a `bevy_ecs` world's `Messages<GameEvent>` by [`GameMenu::deliver`]).
//!   There is no `forge-cmd` in this path: a menu changes the game, not the project.
//! * **Gamepad, keyboard, mouse.** `forge_ui::game::GamepadNav` maps the pad onto the
//!   keyboard behaviour every catalogue widget already has; the UI runs with
//!   [`forge_ui::Ui::game_nav`], so Up/Down move between rows and Left/Right adjust a
//!   slider or a switch. South confirms, East (or Escape) goes back, Start pauses.
//! * **Localised.** Every visible string is a key in the [`forge_ui::l10n::Localiser`]
//!   ([`sample_strings`]: English, French, and the pseudo-locale); switching language
//!   re-sets only the labels whose text changed.
//! * **Resolution independent.** [`GameMenu::scale_for`] is the UI scale for a window, from
//!   the reference resolution and scale mode (`forge_ui::game::GameScaling`).
//! * **Zero idle.** A menu that nothing touches renders nothing and schedules nothing — the
//!   same D-5 rule as the editor; a paused game's pause menu costs nothing.

use std::time::Duration;

use forge_core::bevy_ecs;
use forge_ui::game::{
    ButtonGlyphs, Credits, GameScaling, GamepadNav, GlyphSet, PadAction, PadButton, PadInput,
    SafeArea, ScaleMode,
};
use forge_ui::l10n::{LocalisedTexts, Localiser, PSEUDO_LOCALE};
use forge_ui::widget::EventCx;
use forge_ui::widgets::{
    Button, Container, Label, LabelKind, Pressed, Progress, ProgressBar, SegmentedControl, Slider,
    Switch,
};
use forge_ui::{
    ActionEnvelope, Handled, KeyCode, NodeStyle, Role, Signal, Size, Ui, UiError, UiEvent, Widget,
    WidgetId,
};

/// What the player asked for: an ECS message for the game's systems.
#[derive(Clone, Debug, PartialEq)]
pub enum GameEvent {
    /// Start (or continue) playing: the HUD shows.
    StartGame,
    /// The game is paused (the pause menu opened).
    Paused,
    /// Back to playing from the pause menu.
    Resume,
    /// Leave the game for the main menu.
    QuitToMenu,
    /// Exit the application.
    Quit,
    /// The player changed a setting (the new settings, whole).
    SettingsChanged(GameSettings),
}

impl bevy_ecs::message::Message for GameEvent {}

/// The settings screen's values (game state).
#[derive(Clone, Debug, PartialEq)]
pub struct GameSettings {
    /// 0..=1.
    pub master_volume: f32,
    /// 0..=1.
    pub music_volume: f32,
    pub fullscreen: bool,
    pub subtitles: bool,
    /// A locale tag (`en`, `fr`, `qps-ploc`).
    pub language: String,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            master_volume: 0.8,
            music_volume: 0.6,
            fullscreen: false,
            subtitles: true,
            language: "en".into(),
        }
    }
}

/// The screens.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Screen {
    Main,
    Settings,
    Credits,
    /// Playing: the HUD alone.
    Hud,
    /// The pause menu over the HUD.
    Pause,
}

impl Screen {
    /// Every screen (the localisation audit walks them all).
    pub const ALL: [Screen; 5] = [
        Screen::Main,
        Screen::Settings,
        Screen::Credits,
        Screen::Hud,
        Screen::Pause,
    ];
    fn index(self) -> usize {
        match self {
            Screen::Main => 0,
            Screen::Settings => 1,
            Screen::Credits => 2,
            Screen::Hud => 3,
            Screen::Pause => 4,
        }
    }
}

/// The languages the sample offers: `(tag, endonym)`. The pseudo-locale is the translation
/// check every string must pass (`forge_ui::l10n`).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("fr", "Fran\u{e7}ais"),
    (PSEUDO_LOCALE, "Pseudo"),
];

/// The sample's string tables (source locale `en`).
pub fn sample_strings() -> Localiser {
    Localiser::new("en")
        .with(
            "en",
            &[
                ("menu.title", "Forge Sample"),
                ("menu.play", "Play"),
                ("menu.settings", "Settings"),
                ("menu.credits", "Credits"),
                ("menu.quit", "Quit"),
                ("settings.title", "Settings"),
                ("settings.master", "Master volume"),
                ("settings.music", "Music volume"),
                ("settings.fullscreen", "Fullscreen"),
                ("settings.subtitles", "Subtitles"),
                ("settings.language", "Language"),
                ("common.back", "Back"),
                ("credits.title", "Credits"),
                ("credits.section.Engine", "Engine"),
                ("credits.section.Sample", "Sample"),
                ("credits.section.Fonts", "Fonts"),
                ("pause.title", "Paused"),
                ("pause.resume", "Resume"),
                ("pause.settings", "Settings"),
                ("pause.quit", "Quit to menu"),
                ("hud.health", "Health"),
                ("hud.score", "Score: {score}"),
                (
                    "hud.prompt",
                    "{confirm} Select    {back} Back    {menu} Pause",
                ),
            ],
        )
        .with(
            "fr",
            &[
                ("menu.title", "Exemple Forge"),
                ("menu.play", "Jouer"),
                ("menu.settings", "Param\u{e8}tres"),
                ("menu.credits", "Cr\u{e9}dits"),
                ("menu.quit", "Quitter"),
                ("settings.title", "Param\u{e8}tres"),
                ("settings.master", "Volume g\u{e9}n\u{e9}ral"),
                ("settings.music", "Volume de la musique"),
                ("settings.fullscreen", "Plein \u{e9}cran"),
                ("settings.subtitles", "Sous-titres"),
                ("settings.language", "Langue"),
                ("common.back", "Retour"),
                ("credits.title", "Cr\u{e9}dits"),
                ("credits.section.Engine", "Moteur"),
                ("credits.section.Sample", "Exemple"),
                ("credits.section.Fonts", "Polices"),
                ("pause.title", "Pause"),
                ("pause.resume", "Reprendre"),
                ("pause.settings", "Param\u{e8}tres"),
                ("pause.quit", "Retour au menu"),
                ("hud.health", "Sant\u{e9}"),
                ("hud.score", "Score : {score}"),
                (
                    "hud.prompt",
                    "{confirm} Choisir    {back} Retour    {menu} Pause",
                ),
            ],
        )
}

/// How the sample is set up.
#[derive(Clone, Debug)]
pub struct GameUiConfig {
    pub scaling: GameScaling,
    pub safe_area: SafeArea,
    pub credits: Credits,
    pub strings: Localiser,
    pub glyphs: GlyphSet,
    pub settings: GameSettings,
}

impl Default for GameUiConfig {
    fn default() -> Self {
        Self {
            scaling: GameScaling::new(Size::new(1280.0, 720.0), ScaleMode::Fit),
            safe_area: SafeArea::uniform(24.0),
            credits: Credits::new()
                .section("Sample", &["Game design: the Forge team"])
                .section("Fonts", &["Roboto (Apache-2.0)"]),
            strings: sample_strings(),
            glyphs: GlyphSet::Xbox,
            settings: GameSettings::default(),
        }
    }
}

/// Escape (or Back on a keyboard) with nothing focused handling it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BackKey;

/// The window's key sink: turns an unhandled Escape into [`BackKey`].
struct BackRelay;

impl Widget for BackRelay {
    fn role(&self) -> Role {
        Role::Group
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::Key(k) if k.pressed && k.code == KeyCode::Escape => {
                cx.action(BackKey);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, _cx: &mut forge_ui::widget::PaintCx) {}
}

/// The settings screen's controls.
struct SettingsCtl {
    master: Signal<f32>,
    music: Signal<f32>,
    fullscreen: Signal<bool>,
    subtitles: Signal<bool>,
    language: Signal<usize>,
}

/// The sample game UI (see the module docs).
pub struct GameMenu {
    root: WidgetId,
    screens: [WidgetId; 5],
    /// The first control of each screen (focused when the screen opens).
    first: [Option<WidgetId>; 5],
    current: Screen,
    /// Where Back from Settings returns to.
    settings_from: Screen,
    loc: Localiser,
    texts: LocalisedTexts,
    nav: GamepadNav,
    glyphs: GlyphSet,
    scaling: GameScaling,
    events: Vec<GameEvent>,
    ctl: SettingsCtl,
    applied: GameSettings,
    health: Signal<Progress>,
    score: Signal<String>,
    prompt: Signal<String>,
    buttons: Vec<(WidgetId, Action)>,
    credits: Credits,
    gap: f32,
    /// The HUD's health-and-score row (rebuilt on a language switch).
    top: WidgetId,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Action {
    Play,
    OpenSettings,
    OpenCredits,
    Quit,
    Back,
    Resume,
    QuitToMenu,
}

fn screen_style(safe: &SafeArea, gap: f32) -> NodeStyle {
    let mut s = safe.pad(NodeStyle::column(gap).fill());
    s.layout.align_items = Some(taffy::AlignItems::CENTER);
    s.layout.justify_content = Some(taffy::JustifyContent::CENTER);
    s
}

impl GameMenu {
    /// Build every screen under the window root and open the main menu.
    pub fn build(ui: &mut Ui, cfg: GameUiConfig) -> Result<GameMenu, UiError> {
        ui.game_nav = true;
        ui.spatial_arrows = true;
        let mut loc = cfg.strings;
        let lang_ix = LANGUAGES
            .iter()
            .position(|(t, _)| *t == cfg.settings.language)
            .unwrap_or(0);
        loc.set_current(LANGUAGES[lang_ix].0);
        let mut texts = LocalisedTexts::new();
        let gap = ui.theme().space[3];
        let root = ui.add(
            ui.root(),
            "game",
            NodeStyle::default().fill(),
            Container::pane("Game"),
        )?;
        let relay = ui.add(root, "keys", NodeStyle::leaf(), BackRelay)?;
        ui.set_key_sink(Some(relay));
        let mut screens = [root; 5];
        let mut first = [None; 5];
        let mut buttons = Vec::new();
        let names = ["main", "settings", "credits", "hud", "pause"];
        for s in Screen::ALL {
            let style = match s {
                Screen::Hud => cfg.safe_area.pad(NodeStyle::column(gap).fill()),
                // The pause menu floats over the HUD, out of the flow.
                Screen::Pause => {
                    let mut p = screen_style(&cfg.safe_area, gap);
                    let cover = NodeStyle::cover();
                    p.layout.position = cover.layout.position;
                    p.layout.inset = cover.layout.inset;
                    p.background(forge_ui::ColorRole::BgRaised)
                }
                _ => screen_style(&cfg.safe_area, gap),
            };
            let i = s.index();
            screens[i] = ui.add(root, names[i], style, Container::pane(names[i]))?;
        }
        // Main menu.
        let main = screens[Screen::Main.index()];
        let t = texts.bind(ui.rt_mut(), &loc, "menu.title");
        ui.add(main, "title", NodeStyle::leaf(), Label::new(t).heading())?;
        for (key, text, act) in [
            ("play", "menu.play", Action::Play),
            ("settings", "menu.settings", Action::OpenSettings),
            ("credits", "menu.credits", Action::OpenCredits),
            ("quit", "menu.quit", Action::Quit),
        ] {
            let s = texts.bind(ui.rt_mut(), &loc, text);
            let b = ui.add(main, key, NodeStyle::leaf().width(260.0), Button::new(s))?;
            first[Screen::Main.index()].get_or_insert(b);
            buttons.push((b, act));
        }
        // Settings: a title, a body rebuilt on a language switch (so the controls'
        // assistive-technology names are in the new language too), and Back.
        let st = screens[Screen::Settings.index()];
        let t = texts.bind(ui.rt_mut(), &loc, "settings.title");
        ui.add(st, "title", NodeStyle::leaf(), Label::new(t).heading())?;
        let ctl = SettingsCtl {
            master: ui.rt_mut().signal(cfg.settings.master_volume),
            music: ui.rt_mut().signal(cfg.settings.music_volume),
            fullscreen: ui.rt_mut().signal(cfg.settings.fullscreen),
            subtitles: ui.rt_mut().signal(cfg.settings.subtitles),
            language: ui.rt_mut().signal(lang_ix),
        };
        let body = Self::build_settings_body(ui, st, &ctl, &loc, gap)?;
        first[Screen::Settings.index()] = Some(Self::master_of(body));
        let s = texts.bind(ui.rt_mut(), &loc, "common.back");
        let b = ui.add(st, "back", NodeStyle::leaf().width(260.0), Button::new(s))?;
        buttons.push((b, Action::Back));
        // Credits (E-64: the engine entry comes first, and it is static text).
        let cr = screens[Screen::Credits.index()];
        let t = texts.bind(ui.rt_mut(), &loc, "credits.title");
        ui.add(cr, "title", NodeStyle::leaf(), Label::new(t).heading())?;
        Self::build_credits(ui, cr, &cfg.credits, &loc, gap)?;
        let s = texts.bind(ui.rt_mut(), &loc, "common.back");
        let b = ui.add(cr, "back", NodeStyle::leaf().width(260.0), Button::new(s))?;
        first[Screen::Credits.index()] = Some(b);
        buttons.push((b, Action::Back));
        // HUD: the health row (rebuilt on a language switch, like the settings body), the
        // score, and the pad prompt.
        let hud = screens[Screen::Hud.index()];
        let health = ui.rt_mut().signal(Progress::Fraction(1.0));
        let score = texts.bind_args(ui.rt_mut(), &loc, "hud.score", &[("score", "0")]);
        let top = Self::build_health_row(ui, hud, health, score, &loc, gap)?;
        let mut spacer = NodeStyle::leaf();
        spacer.layout.flex_grow = 1.0;
        ui.add(hud, "spacer", spacer, Container::group())?;
        let prompt = texts.bind_args(
            ui.rt_mut(),
            &loc,
            "hud.prompt",
            &Self::glyph_args(cfg.glyphs),
        );
        ui.add(
            hud,
            "prompt",
            NodeStyle::leaf(),
            Label::new(prompt).kind(LabelKind::Small),
        )?;
        // Pause.
        let pa = screens[Screen::Pause.index()];
        let t = texts.bind(ui.rt_mut(), &loc, "pause.title");
        ui.add(pa, "title", NodeStyle::leaf(), Label::new(t).heading())?;
        for (key, text, act) in [
            ("resume", "pause.resume", Action::Resume),
            ("settings", "pause.settings", Action::OpenSettings),
            ("quit", "pause.quit", Action::QuitToMenu),
        ] {
            let s = texts.bind(ui.rt_mut(), &loc, text);
            let b = ui.add(pa, key, NodeStyle::leaf().width(260.0), Button::new(s))?;
            first[Screen::Pause.index()].get_or_insert(b);
            buttons.push((b, act));
        }
        let applied = GameSettings {
            language: LANGUAGES[lang_ix].0.to_string(),
            ..cfg.settings
        };
        let mut m = GameMenu {
            root,
            screens,
            first,
            current: Screen::Main,
            settings_from: Screen::Main,
            loc,
            texts,
            nav: GamepadNav::new(),
            glyphs: cfg.glyphs,
            scaling: cfg.scaling,
            events: Vec::new(),
            ctl,
            applied,
            health,
            score,
            prompt,
            buttons,
            credits: cfg.credits,
            gap,
            top,
        };
        m.show(ui, Screen::Main);
        Ok(m)
    }

    /// The settings rows (labels and controls, in the current language) under `st`.
    fn build_settings_body(
        ui: &mut Ui,
        st: WidgetId,
        ctl: &SettingsCtl,
        loc: &Localiser,
        gap: f32,
    ) -> Result<WidgetId, UiError> {
        let body = ui.add(st, "body", NodeStyle::column(gap), Container::group())?;
        // A row: the setting's name, then its control. A switch names itself (its label is
        // part of the control), so its row has an empty name column instead of a repeat.
        let row = |ui: &mut Ui, key: &'static str, text: Option<&str>| {
            let r = ui.add(
                body,
                key,
                NodeStyle::row(gap).width(560.0),
                Container::group(),
            )?;
            match text {
                Some(t) => ui.add(
                    r,
                    "label",
                    NodeStyle::leaf().width(240.0),
                    Label::new(loc.get(t, &[])),
                )?,
                None => ui.add(
                    r,
                    "label",
                    NodeStyle::leaf().width(240.0),
                    Container::group(),
                )?,
            };
            Ok::<WidgetId, UiError>(r)
        };
        let r = row(ui, "master", Some("settings.master"))?;
        let label = loc.get("settings.master", &[]);
        ui.add(
            r,
            "control",
            NodeStyle::leaf().grow(1.0),
            Slider::new(ctl.master, &label, 0.0, 1.0, 0.05),
        )?;
        let r = row(ui, "music", Some("settings.music"))?;
        let label = loc.get("settings.music", &[]);
        ui.add(
            r,
            "control",
            NodeStyle::leaf().grow(1.0),
            Slider::new(ctl.music, &label, 0.0, 1.0, 0.05),
        )?;
        let r = row(ui, "fullscreen", None)?;
        let label = loc.get("settings.fullscreen", &[]);
        ui.add(
            r,
            "control",
            NodeStyle::leaf(),
            Switch::new(ctl.fullscreen, &label),
        )?;
        let r = row(ui, "subtitles", None)?;
        let label = loc.get("settings.subtitles", &[]);
        ui.add(
            r,
            "control",
            NodeStyle::leaf(),
            Switch::new(ctl.subtitles, &label),
        )?;
        let r = row(ui, "language", Some("settings.language"))?;
        let names: Vec<&str> = LANGUAGES.iter().map(|(_, n)| *n).collect();
        let label = loc.get("settings.language", &[]);
        ui.add(
            r,
            "control",
            NodeStyle::leaf(),
            SegmentedControl::new(&label, &names, ctl.language),
        )?;
        // The body goes before Back.
        if ui.children(st).len() > 2 {
            let _ = ui.reorder(
                st,
                &[
                    forge_ui::Key::Static("title"),
                    forge_ui::Key::Static("body"),
                    forge_ui::Key::Static("back"),
                ],
            );
        }
        Ok(body)
    }

    /// The master-volume slider of a settings body (ids are stable per key path, §21.4).
    fn master_of(body: WidgetId) -> WidgetId {
        body.child(&forge_ui::Key::Static("master"))
            .child(&forge_ui::Key::Static("control"))
    }

    /// The HUD's health row under `hud`.
    fn build_health_row(
        ui: &mut Ui,
        hud: WidgetId,
        health: Signal<Progress>,
        score: Signal<String>,
        loc: &Localiser,
        gap: f32,
    ) -> Result<WidgetId, UiError> {
        let top = ui.add(hud, "top", NodeStyle::row(gap), Container::group())?;
        let label = loc.get("hud.health", &[]);
        ui.add(
            top,
            "health_label",
            NodeStyle::leaf(),
            Label::new(label.clone()),
        )?;
        ui.add(
            top,
            "health",
            NodeStyle::leaf().width(220.0),
            ProgressBar::new(health, &label),
        )?;
        ui.add(
            top,
            "score",
            NodeStyle::leaf().indent(gap * 2.0),
            Label::new(score),
        )?;
        Ok(top)
    }

    /// The credits list under the credits screen `cr`: each section heading in the current
    /// language (`credits.section.<heading>`; a heading with no string shows as written, and
    /// the pseudo-locale audit catches it), the credit lines as written — names and licences
    /// are not translated, and the engine entry is fixed text (E-64).
    fn build_credits(
        ui: &mut Ui,
        cr: WidgetId,
        credits: &Credits,
        loc: &Localiser,
        gap: f32,
    ) -> Result<WidgetId, UiError> {
        let heading = |h: &str| {
            let key = format!("credits.section.{h}");
            if loc.raw(loc.source(), &key).is_some() {
                loc.get(&key, &[])
            } else {
                h.to_string()
            }
        };
        credits.build_with(ui, cr, "list", NodeStyle::column(gap), &heading)
    }

    /// After a language switch: rebuild what holds build-time text (the settings body, the
    /// HUD's health row and the credits' headings), keeping focus where it was (ids are
    /// stable).
    fn relabel(&mut self, ui: &mut Ui) -> Result<(), UiError> {
        let focus = ui.focused();
        let st = self.screens[Screen::Settings.index()];
        let body = st.child(&forge_ui::Key::Static("body"));
        ui.remove(body)?;
        Self::build_settings_body(ui, st, &self.ctl, &self.loc, self.gap)?;
        let hud = self.screens[Screen::Hud.index()];
        let score_row = self.top;
        ui.remove(score_row)?;
        self.top = Self::build_health_row(ui, hud, self.health, self.score, &self.loc, self.gap)?;
        let _ = ui.reorder(
            hud,
            &[
                forge_ui::Key::Static("top"),
                forge_ui::Key::Static("spacer"),
                forge_ui::Key::Static("prompt"),
            ],
        );
        let cr = self.screens[Screen::Credits.index()];
        ui.remove(cr.child(&forge_ui::Key::Static("list")))?;
        Self::build_credits(ui, cr, &self.credits, &self.loc, self.gap)?;
        let _ = ui.reorder(
            cr,
            &[
                forge_ui::Key::Static("title"),
                forge_ui::Key::Static("list"),
                forge_ui::Key::Static("back"),
            ],
        );
        if let Some(f) = focus
            && ui.contains(f)
        {
            ui.set_focus(Some(f), true);
        }
        Ok(())
    }

    fn glyph_args(g: GlyphSet) -> [(&'static str, &'static str); 3] {
        [
            ("confirm", g.glyph(PadButton::South)),
            ("back", g.glyph(PadButton::East)),
            ("menu", g.glyph(PadButton::Start)),
        ]
    }

    /// The game UI's root container.
    pub fn root(&self) -> WidgetId {
        self.root
    }
    pub fn screen(&self) -> Screen {
        self.current
    }
    /// A screen's container.
    pub fn screen_id(&self, s: Screen) -> WidgetId {
        self.screens[s.index()]
    }
    pub fn localiser(&self) -> &Localiser {
        &self.loc
    }
    pub fn texts(&self) -> &LocalisedTexts {
        &self.texts
    }
    pub fn credits(&self) -> &Credits {
        &self.credits
    }
    /// The settings as the game last applied them.
    pub fn settings(&self) -> &GameSettings {
        &self.applied
    }
    /// The UI scale for a window of `window` logical px (resolution independence).
    pub fn scale_for(&self, window: Size) -> f32 {
        self.scaling.scale(window)
    }

    /// Open a screen: the HUD shows under the pause menu, every other screen alone; focus
    /// goes to its first control (a game menu always has a focused control, so a pad can
    /// act at once).
    pub fn show(&mut self, ui: &mut Ui, s: Screen) {
        for x in Screen::ALL {
            let visible = x == s || (s == Screen::Pause && x == Screen::Hud);
            let _ = ui.set_hidden(self.screens[x.index()], !visible);
        }
        if s == Screen::Settings && self.current != Screen::Settings {
            self.settings_from = self.current;
        }
        self.current = s;
        ui.set_focus(self.first[s.index()], true);
    }

    fn act(&mut self, ui: &mut Ui, a: Action) {
        match a {
            Action::Play => {
                self.events.push(GameEvent::StartGame);
                self.show(ui, Screen::Hud);
            }
            Action::OpenSettings => self.show(ui, Screen::Settings),
            Action::OpenCredits => self.show(ui, Screen::Credits),
            Action::Quit => self.events.push(GameEvent::Quit),
            Action::Back => self.back(ui),
            Action::Resume => {
                self.events.push(GameEvent::Resume);
                self.show(ui, Screen::Hud);
            }
            Action::QuitToMenu => {
                self.events.push(GameEvent::QuitToMenu);
                self.show(ui, Screen::Main);
            }
        }
    }

    /// Back (East, Escape): settings return where they were opened from, credits to the
    /// main menu, the pause menu resumes, the HUD pauses; the main menu stays.
    pub fn back(&mut self, ui: &mut Ui) {
        match self.current {
            Screen::Settings => {
                let to = self.settings_from;
                self.show(ui, to);
            }
            Screen::Credits => self.show(ui, Screen::Main),
            Screen::Pause => self.act(ui, Action::Resume),
            Screen::Hud => self.pause(ui),
            Screen::Main => {}
        }
    }

    fn pause(&mut self, ui: &mut Ui) {
        self.events.push(GameEvent::Paused);
        self.show(ui, Screen::Pause);
    }

    /// The window's actions (from the runner's `on_actions`).
    pub fn on_actions(&mut self, ui: &mut Ui, actions: Vec<ActionEnvelope>) {
        for a in actions {
            if let Some(Pressed(id)) = a.get::<Pressed>()
                && let Some((_, act)) = self.buttons.iter().find(|(b, _)| b == id).copied()
            {
                self.act(ui, act);
            } else if a.get::<BackKey>().is_some() {
                self.back(ui);
            }
        }
        self.update(ui);
    }

    /// Apply settings the player changed (sliders and switches write signals; the game
    /// hears about it once per change, and a language switch relabels the UI).
    /// Runs on every turn the window takes (every drawn frame): comparing costs no
    /// allocation — the settings are built only when one changed.
    pub fn update(&mut self, ui: &mut Ui) {
        let rt = ui.rt();
        let lang = LANGUAGES
            .get(self.ctl.language.get(rt))
            .map_or("en", |(t, _)| *t);
        let (master, music, fullscreen, subtitles) = (
            self.ctl.master.get(rt),
            self.ctl.music.get(rt),
            self.ctl.fullscreen.get(rt),
            self.ctl.subtitles.get(rt),
        );
        let a = &self.applied;
        let same = a.master_volume == master
            && a.music_volume == music
            && a.fullscreen == fullscreen
            && a.subtitles == subtitles
            && a.language == lang;
        if !same {
            let now = GameSettings {
                master_volume: master,
                music_volume: music,
                fullscreen,
                subtitles,
                language: lang.to_string(),
            };
            if now.language != self.applied.language {
                self.loc.set_current(&now.language);
                self.texts.apply(ui.rt_mut(), &self.loc);
                // A rebuild fails only on a duplicate key, which these fixed keys rule out;
                // were it to, the old labels stay (never a panic in a menu).
                let _ = self.relabel(ui);
            }
            self.applied = now.clone();
            self.events.push(GameEvent::SettingsChanged(now));
        }
    }

    /// Switch language (as the settings screen does).
    pub fn set_language(&mut self, ui: &mut Ui, tag: &str) -> bool {
        match LANGUAGES.iter().position(|(t, _)| *t == tag) {
            Some(i) => {
                self.ctl.language.set(ui.rt_mut(), i);
                self.update(ui);
                true
            }
            None => false,
        }
    }

    /// One pad input at `now` (the UI clock).
    pub fn pad(&mut self, ui: &mut Ui, now: Duration, i: PadInput) {
        let acted = self.nav.input(ui, now, i);
        let actions = ui.take_actions();
        self.on_actions(ui, actions);
        match acted {
            Some(PadAction::Back) => self.back(ui),
            Some(PadAction::Menu) => match self.current {
                Screen::Hud => self.pause(ui),
                Screen::Pause => self.act(ui, Action::Resume),
                _ => {}
            },
            _ => {}
        }
    }

    /// Run pad repeats that are due.
    pub fn tick(&mut self, ui: &mut Ui, now: Duration) {
        self.nav.tick(ui, now);
    }

    /// When the pad next needs a turn (`None`: nothing held — an idle pad costs nothing).
    pub fn next_deadline(&self) -> Option<Duration> {
        self.nav.next_deadline()
    }

    /// The pad glyph family the prompts use.
    pub fn set_glyphs(&mut self, ui: &mut Ui, g: GlyphSet) {
        self.glyphs = g;
        let args = Self::glyph_args(g);
        self.texts
            .set_args(ui.rt_mut(), &self.loc, self.prompt, &args);
    }
    pub fn glyphs(&self) -> GlyphSet {
        self.glyphs
    }

    /// The HUD's values (game state pushed into the UI; unchanged values cost nothing).
    pub fn set_hud(&mut self, ui: &mut Ui, health: f32, score: u64) {
        self.health
            .set(ui.rt_mut(), Progress::Fraction(health.clamp(0.0, 1.0)));
        let s = score.to_string();
        self.texts
            .set_args(ui.rt_mut(), &self.loc, self.score, &[("score", &s)]);
    }

    /// What the player asked for since the last call.
    pub fn take_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.events)
    }

    /// Deliver the pending events to the game's systems as ECS messages
    /// (`Messages<GameEvent>` in `world`, created on first use). Returns how many.
    pub fn deliver(&mut self, world: &mut bevy_ecs::world::World) -> usize {
        deliver(world, self.take_events())
    }
}

/// One turn of the game's loop for the menu's messages: advance `world`'s
/// `Messages<GameEvent>` a frame (`Messages::update`: the messages of two turns ago are
/// dropped, last turn's stay readable one more turn — Bevy's double buffer), then deliver
/// this turn's `events`. Called every turn, the buffers hold at most two turns of messages
/// however long the game runs; delivering without it, they grow forever
/// (`test_game_menu`'s message-buffer check). Returns how many were delivered.
pub fn end_turn(world: &mut bevy_ecs::world::World, events: Vec<GameEvent>) -> usize {
    use bevy_ecs::message::Messages;
    if let Some(mut q) = world.get_resource_mut::<Messages<GameEvent>>() {
        q.update();
    }
    deliver(world, events)
}

/// Write `events` into `world`'s `Messages<GameEvent>` (created on first use): how game
/// systems receive what the player asked for. Returns how many. A game loop calls
/// [`end_turn`] instead, which also advances the buffers.
pub fn deliver(world: &mut bevy_ecs::world::World, events: Vec<GameEvent>) -> usize {
    use bevy_ecs::message::Messages;
    if world.get_resource::<Messages<GameEvent>>().is_none() {
        world.insert_resource(Messages::<GameEvent>::default());
    }
    let n = events.len();
    let mut q = world.resource_mut::<Messages<GameEvent>>();
    for e in events {
        q.write(e);
    }
    n
}
