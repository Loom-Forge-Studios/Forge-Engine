//! The editor shell (Ch.21 §21.17, §21.18; DoD M2-1, M2-3, M2-28, M2-41..M2-45): the main
//! window — menu bar, toolbar, docked layout, status bar — the command palette, toasts,
//! the stream pump that feeds the mirror, and the user-config savers.
//!
//! It is a **client of the core and nothing else**: it holds a [`BusClient`], never a
//! `Bus`. Every state-changing action a user can take — a menu item, a toolbar button, a
//! chord, a palette pick, a panel's widget — becomes a command on the bus through the
//! [`CommandEmitter`] (I7), so it is undoable, provenance-tagged and visible to automation
//! sessions. Layout, theme, keymap, selection are session state or user config and change directly.
//!
//! The shell is runner-agnostic: the `forge-editor` binary drives it from the winit
//! runner (`UiApp`), and the tests drive it with headless `Ui`s exactly the same way:
//! [`Shell::build_main`], then every loop turn [`Shell::on_actions`] with the window's
//! unconsumed actions and [`Shell::update_window`] for each window.
//!
//! **Idle is free.** Nothing here polls: the loop sleeps until input, a waker (another
//! client changed the project), or [`Shell::next_deadline`] (a debounced save) — D-5.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use forge_plugin::Registry;
use forge_plugin::points::{Dock, EditorPanel};
use forge_ui::dock::{AreaId, DockController, Layout, PanelId};
use forge_ui::widgets::{
    Button, CommandInvoked, MenuBar, MenuChosen, MenuItem, PickItem, Pressed, StatusBar,
    StatusItem, Toolbar, open_command_palette_ui,
};
use forge_ui::{ActionEnvelope, Dirty, Key, NodeStyle, Signal, Ui, UiError, WidgetId};

use crate::EditorError;
use crate::actions::{Action, ActionCx, ActionItem, ActionKind, Invocation, SessionOp, invoke};
use crate::client::BusClient;
use crate::core::EditorCore;
use crate::emitter::CommandEmitter;
use crate::keybind::ActionSummary;
use crate::keymap::{KeyMap, KeymapFile, default_keymap};
use crate::keys::{ActionTriggered, ChordPending, keymap_host};
use crate::layout_store::{Autosave, LayoutStore};
use crate::mirror::ProjectMirror;
use crate::notify::{Level, NoticeAction};
use crate::overlay::{EditorOverlay, ThemePoint};
use crate::palette::{CommandInfo, Palette, PaletteOutcome};
use crate::panel_rt::{ShellHandles, WindowKey, Wiring};
use crate::panels::{EditorPanelHost, PanelCatalog, PanelCx, open_panel};
use crate::play::{PLAY_FRAME, PlayCommand, PlayState};
use crate::presets::Preset;
use crate::services::EditorServices;
use crate::session::{Reveal, SessionState, ShellRequest};
use crate::settings::{EditorSettings, EditorTheme};
use crate::user_config::{read_optional, write_atomic};

/// The user keymap file under the config directory (user config).
pub const USER_KEYMAP_FILE: &str = "keymap.ron";

/// What the shell is built from.
pub struct ShellConfig {
    /// The workspace preset (its layout is the default; its keymap layer applies).
    pub preset: Preset,
    /// The `EditorPanel` registry after the plugin loader ran.
    pub panels: Registry<EditorPanel<PanelCx>>,
    /// The `Action` registry after the plugin loader ran.
    pub actions: Registry<Action>,
    /// The registered `Invoke` commands (the palette lists them).
    pub commands: Vec<CommandInfo>,
    /// Why missing panels are missing (disabled or removing plugins).
    pub catalog: PanelCatalog,
    /// The per-user config directory. `None`: nothing is loaded or saved (tests).
    pub config_dir: Option<PathBuf>,
    /// The `EditorOverlay` registry after the plugin loader ran.
    pub overlays: Registry<EditorOverlay>,
    /// The `Theme` registry after the plugin loader ran.
    pub themes: Registry<ThemePoint>,
    /// The read-side services panels get (components and the `InspectorWidget` registry,
    /// the console log, the asset catalogue, what plugins added).
    pub services: EditorServices,
    /// Problems to post to the notification centre at start-up (M2-42, M2-70): start-up code
    /// and an edition's `attach` (e.g. a premium editor degraded to base for want of a valid
    /// entitlement, ADR 0063) enqueue a [`crate::notify::Problem`] here so the user sees it in
    /// the editor UI, not only on stderr. Empty for the base editor.
    pub startup_notices: Vec<crate::notify::Problem>,
}

/// Counters the tests and the idle measurement read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShellStats {
    /// Stream pumps that delivered something.
    pub pumps_with_events: u64,
    /// Sync passes run (windows × changes).
    pub sync_passes: u64,
    /// Menu bar rebuilds.
    pub menu_rebuilds: u64,
    pub layout_writes: u64,
    pub settings_writes: u64,
    pub keymap_writes: u64,
}

/// The main window's chrome.
struct Chrome {
    menubar: WidgetId,
    toolbar: BTreeMap<WidgetId, String>,
    status_project: Signal<String>,
    status_undo: Signal<String>,
    status_hint: Signal<String>,
    status_notices: Signal<String>,
    status_user: Signal<String>,
    notices_label: Signal<String>,
    menus: Vec<(String, Vec<MenuItem>)>,
}

/// The shell (see the module docs).
pub struct Shell {
    handles: ShellHandles,
    emitter: CommandEmitter,
    dock: DockController,
    host: EditorPanelHost,
    preset: Preset,
    actions: Registry<Action>,
    commands: Vec<CommandInfo>,
    palette: Palette,
    config_dir: Option<PathBuf>,
    layouts: Option<LayoutStore>,
    layout_save: Autosave,
    settings_save: Autosave,
    keymap_save: Autosave,
    seen_settings_rev: u64,
    seen_keymap_rev: u64,
    /// Per window: the (mirror, session) revisions its sync steps last ran at, and the
    /// editor-settings revision last applied to it.
    synced: HashMap<WindowKey, (u64, u64, u64)>,
    applied_settings: HashMap<WindowKey, u64>,
    chrome: Option<Chrome>,
    /// The chord waiting for its second stroke.
    pending_chord: Option<String>,
    stats: ShellStats,
    now: Duration,
    main_root: Option<WidgetId>,
    overlays: Registry<EditorOverlay>,
    themes: Registry<ThemePoint>,
    /// The newest lifecycle outcome this shell has shown (`None`: none seen yet — the
    /// first status marks what happened before this shell connected as seen).
    seen_outcome: Option<u64>,
    /// The open project's trust question this shell last asked the person (its
    /// `ProjectTrust::id`, WP-34): asked once per question.
    trust_asked: Option<u64>,
}

fn human_issuer_name() -> String {
    ["FORGE_USER", "USERNAME", "USER"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "local".into())
}

/// The issuer the GUI stamps on everything it sends (Ch.21 §21.18: panels cannot forge it).
pub fn gui_issuer() -> forge_cmd::Issuer {
    forge_cmd::Issuer::Human {
        user: human_issuer_name(),
    }
}

impl Shell {
    /// Build the shell over `client` (connected with the GUI's human issuer).
    pub fn new(mut cfg: ShellConfig, client: Box<dyn BusClient>) -> Result<Shell, EditorError> {
        // Start-up notices an edition or the launcher enqueued (e.g. a premium editor degraded
        // to base, ADR 0063) are posted first, then the config-load problems below.
        let mut notices: Vec<crate::notify::Problem> = std::mem::take(&mut cfg.startup_notices);
        // User config: editor settings and the keymap. A bad file is reported and the
        // defaults are used — it never stops the editor starting.
        let settings = match &cfg.config_dir {
            Some(d) => EditorSettings::load(d).unwrap_or_else(|e| {
                notices.push(crate::notify::Problem::coded(
                    crate::notify::Severity::Warning,
                    e.code(),
                    forge_ui::tr!("Editor settings were reset"),
                    &e.to_string(),
                ));
                EditorSettings::default()
            }),
            None => EditorSettings::default(),
        };
        // The launcher's recent projects are user config too.
        if cfg.config_dir.is_some() {
            let mut l = cfg.services.launcher.borrow_mut();
            if l.config_dir.is_none() {
                let (loaded, err) =
                    crate::project::recent::LauncherState::with_config(cfg.config_dir.clone());
                l.recent = loaded.recent;
                l.config_dir = loaded.config_dir;
                if let Some(e) = err {
                    notices.push(crate::notify::Problem::coded(
                        crate::notify::Severity::Warning,
                        e.code(),
                        forge_ui::tr!("The recent projects list was reset"),
                        &e.to_string(),
                    ));
                }
            }
        }
        let defaults = default_keymap()?;
        let mut layers: Vec<KeymapFile> = Vec::new();
        if let Some(k) = &cfg.preset.keymap {
            layers.push(k.clone());
        }
        let base_refs: Vec<&KeymapFile> = layers.iter().collect();
        let (base, base_errs) = KeyMap::from_layers(&defaults, &base_refs);
        let user_layer = match &cfg.config_dir {
            Some(d) => match read_optional(&d.join(USER_KEYMAP_FILE)) {
                Ok(Some(t)) => match KeymapFile::from_ron(&t) {
                    Ok(f) => Some(f),
                    Err(e) => {
                        notices.push(crate::notify::Problem::coded(
                            crate::notify::Severity::Warning,
                            e.code(),
                            forge_ui::tr!("Your keymap file was not loaded"),
                            &e.to_string(),
                        ));
                        None
                    }
                },
                Ok(None) => None,
                Err(e) => {
                    notices.push(crate::notify::Problem::coded(
                        crate::notify::Severity::Warning,
                        e.code(),
                        forge_ui::tr!("Your keymap file was not read"),
                        &e.to_string(),
                    ));
                    None
                }
            },
            None => None,
        };
        let mut all_refs = base_refs.clone();
        if let Some(u) = &user_layer {
            all_refs.push(u);
        }
        let (km, errs) = KeyMap::from_layers(&defaults, &all_refs);
        for e in base_errs.iter().chain(errs.iter().skip(base_errs.len())) {
            notices.push(crate::notify::Problem::coded(
                crate::notify::Severity::Warning,
                e.code(),
                forge_ui::tr!("A key binding was skipped"),
                &e.to_string(),
            ));
        }

        // Panel-open actions for every registered panel (the Window menu and the palette).
        let owner = forge_plugin::PluginId::new("forge.editor")
            .map_err(|e| EditorError::UnknownAction(e.to_string()))?;
        for (k, d) in cfg.panels.iter() {
            let id = format!("forge.panel.open.{k}");
            if cfg.actions.get(&id).is_none() {
                let item = ActionItem::new(
                    &forge_ui::trf!("Show {panel}", panel = forge_ui::l10n::tr_str(&d.title)),
                    forge_ui::tr_key!("Window"),
                    ActionKind::Session(SessionOp::OpenPanel(PanelId::new(k))),
                );
                let _ = cfg
                    .actions
                    .add(owner.clone(), &id, item, forge_plugin::Order::Last);
            }
        }

        // Theme actions for every registered theme the built-in actions do not cover (a
        // plugin's theme appears in View and the palette like the built-in ones).
        let uncovered: Vec<(String, String)> = cfg
            .themes
            .iter()
            .filter(|(k, _)| {
                !cfg.actions.iter().any(|(_, a)| {
                    matches!(&a.kind, ActionKind::Session(SessionOp::SetTheme(id)) if id == k)
                })
            })
            .map(|(k, t)| (k.to_string(), t.label.clone()))
            .collect();
        for (k, label) in uncovered {
            let item = ActionItem::new(
                &forge_ui::trf!("Theme: {label}", label = forge_ui::l10n::tr_str(&label)),
                forge_ui::tr_key!("Preferences"),
                ActionKind::Session(SessionOp::SetTheme(k.clone())),
            );
            let _ = cfg.actions.add(
                owner.clone(),
                &format!("forge.theme.{k}"),
                item,
                forge_plugin::Order::Last,
            );
        }

        let keymap = Rc::new(RefCell::new(km));
        let mut session = SessionState::new(settings, keymap, base);
        session.user_config = cfg.config_dir.is_some();
        // WP-47: the loaded plugins' hints apply to every problem, the start-up ones too.
        session
            .notifications
            .set_hints(cfg.services.notice_hints.clone());
        session.set_actions(
            cfg.actions
                .iter()
                .map(|(id, a)| ActionSummary {
                    id: id.to_string(),
                    title: a.title.clone(),
                    category: a.category.clone(),
                })
                .collect(),
        );
        for p in notices {
            session.problem(p);
        }

        let client = Rc::new(RefCell::new(client));
        let mut mirror = ProjectMirror::new();
        {
            let mut c = client.borrow_mut();
            // The shell hosts the play core: it follows the session commands (the play
            // controls) every issuer sends through the bus.
            c.follow_session(true);
            let (project, next) = c.snapshot();
            mirror.resync(&project, next, c.history());
            mirror.set_targets(c.undo_target(), c.redo_target());
        }
        let emitter = CommandEmitter::over(client);
        let handles = ShellHandles::new(
            Rc::new(RefCell::new(mirror)),
            emitter.clone(),
            Rc::new(RefCell::new(session)),
            Rc::new(RefCell::new(Wiring::default())),
            Rc::new(std::mem::take(&mut cfg.services)),
        );

        let layouts = cfg
            .config_dir
            .as_ref()
            .map(|d| LayoutStore::new(d.join("layouts")));
        let layout = match &layouts {
            Some(s) => match s.load_autosave() {
                Ok(Some(l)) => l,
                Ok(None) => cfg.preset.layout.clone(),
                Err(e) => {
                    handles.session_mut().problem(crate::notify::Problem::coded(
                        crate::notify::Severity::Warning,
                        e.code(),
                        forge_ui::tr!("The saved layout could not be restored"),
                        &e.to_string(),
                    ));
                    cfg.preset.layout.clone()
                }
            },
            None => cfg.preset.layout.clone(),
        };
        let kind = cfg.preset.workspace.family.kind();
        let host =
            EditorPanelHost::with_shell(&cfg.panels, kind, cfg.catalog, handles.clone(), None);
        let factory_host = host.clone();
        let dock = DockController::new(
            layout,
            Box::new(move |area| {
                let w = match area {
                    AreaId::Main => None,
                    AreaId::Floating(k) => Some(k),
                };
                Box::new(factory_host.clone().for_window(w))
            }),
        );
        let seen_settings_rev = handles.session().settings_revision();
        let seen_keymap_rev = handles.session().keymap_revision();
        Ok(Shell {
            handles,
            emitter,
            dock,
            host,
            preset: cfg.preset,
            actions: cfg.actions,
            commands: cfg.commands,
            palette: Palette::default(),
            config_dir: cfg.config_dir,
            layouts,
            layout_save: Autosave::new(),
            settings_save: Autosave::new(),
            keymap_save: Autosave::new(),
            seen_settings_rev,
            seen_keymap_rev,
            synced: HashMap::new(),
            applied_settings: HashMap::new(),
            chrome: None,
            pending_chord: None,
            stats: ShellStats::default(),
            now: Duration::ZERO,
            main_root: None,
            overlays: cfg.overlays,
            themes: cfg.themes,
            seen_outcome: None,
            trust_asked: None,
        })
    }

    // ---- accessors -----------------------------------------------------------------------

    pub fn mirror(&self) -> std::cell::Ref<'_, ProjectMirror> {
        self.handles.mirror()
    }
    pub fn session(&self) -> std::cell::Ref<'_, SessionState> {
        self.handles.session()
    }
    pub fn session_mut(&self) -> std::cell::RefMut<'_, SessionState> {
        self.handles.session_mut()
    }
    /// The emitter the shell and its panels share.
    pub fn emitter(&self) -> &CommandEmitter {
        &self.emitter
    }
    pub fn handles(&self) -> &ShellHandles {
        &self.handles
    }
    pub fn layout(&self) -> &Layout {
        self.dock.layout()
    }
    pub fn dock(&self) -> &DockController {
        &self.dock
    }
    pub fn dock_mut(&mut self) -> &mut DockController {
        &mut self.dock
    }
    /// The plugin commands the palette offers (a WASM plugin's join when it installs).
    pub fn plugin_commands(&self) -> &[CommandInfo] {
        &self.commands
    }
    pub fn actions(&self) -> &Registry<Action> {
        &self.actions
    }
    pub fn stats(&self) -> ShellStats {
        self.stats
    }
    /// The menus as the menu bar shows them now.
    pub fn menus(&self) -> Vec<(String, Vec<MenuItem>)> {
        self.chrome
            .as_ref()
            .map(|c| c.menus.clone())
            .unwrap_or_default()
    }
    /// The toolbar's buttons and their actions.
    pub fn toolbar(&self) -> Vec<(WidgetId, String)> {
        self.chrome
            .as_ref()
            .map(|c| c.toolbar.iter().map(|(k, v)| (*k, v.clone())).collect())
            .unwrap_or_default()
    }
    /// The status bar's texts, left to right.
    pub fn status(&self, ui: &Ui) -> Vec<String> {
        match &self.chrome {
            Some(c) => [
                c.status_project,
                c.status_undo,
                c.status_hint,
                c.status_notices,
                c.status_user,
            ]
            .iter()
            .map(|s| s.get(ui.rt()))
            .collect(),
            None => Vec::new(),
        }
    }
    /// The palette as it was last opened.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }
    /// Every panel the registry provides, with its title.
    pub fn panels(&self) -> Vec<(PanelId, String)> {
        self.host.panels()
    }
    /// The main window's panel host (tests: a panel registered outside any loaded manifest).
    #[doc(hidden)]
    pub fn panel_host(&self) -> &EditorPanelHost {
        &self.host
    }

    // ---- building ------------------------------------------------------------------------

    /// Build the main window: keymap host ⊃ menu bar, toolbar, dock, status bar.
    pub fn build_main(&mut self, ui: &mut Ui) -> Result<(), UiError> {
        ui.set_window_key(None);
        let root = ui.root();
        self.main_root = Some(root);
        let keymap = self.handles.session().keymap().clone();
        let host = keymap_host(ui, root, "shell", NodeStyle::column(0.0).fill(), keymap)?;
        let menus = self.menu_spec();
        let menubar = ui.add(
            host,
            "menubar",
            NodeStyle::leaf(),
            MenuBar::new(menus.iter().map(|(t, i)| (t.as_str(), i.clone())).collect()),
        )?;
        let space = ui.theme().space;
        let tb = ui.add(
            host,
            "toolbar",
            NodeStyle::row(space[1]).padding(space[1]),
            Toolbar::new(forge_ui::tr!("Editor")),
        )?;
        let notices_label = ui
            .rt_mut()
            .signal(forge_ui::tr!("Notifications").to_string());
        let mut toolbar = BTreeMap::new();
        let buttons: [(&str, forge_ui::state::Bind<String>, &str); 5] = [
            ("undo", forge_ui::tr!("Undo").into(), "forge.edit.undo"),
            ("redo", forge_ui::tr!("Redo").into(), "forge.edit.redo"),
            (
                "palette",
                forge_ui::tr!("Commands").into(),
                "forge.palette.open",
            ),
            (
                "settings",
                forge_ui::tr!("Settings").into(),
                "forge.panel.open.forge.settings",
            ),
            (
                "notices",
                notices_label.into(),
                "forge.panel.open.forge.notifications",
            ),
        ];
        for (key, label, action) in buttons {
            if self.actions.get(action).is_none() {
                continue;
            }
            let b = ui.add(tb, key, NodeStyle::leaf(), Button::new(label))?;
            toolbar.insert(b, action.to_string());
        }
        self.dock
            .build_main(ui, host, "dock", NodeStyle::default().grow(1.0))?;
        let sig = |ui: &mut Ui| ui.rt_mut().signal(String::new());
        let (sp, su, sh, sn, us) = (sig(ui), sig(ui), sig(ui), sig(ui), sig(ui));
        let item = |text, right| StatusItem { text, right };
        ui.add(
            host,
            "status",
            NodeStyle::leaf(),
            StatusBar::new(vec![
                item(sp, false),
                item(su, false),
                item(sh, false),
                item(sn, true),
                item(us, true),
            ]),
        )?;
        self.build_overlays(ui, host)?;
        self.chrome = Some(Chrome {
            menubar,
            toolbar,
            status_project: sp,
            status_undo: su,
            status_hint: sh,
            status_notices: sn,
            status_user: us,
            notices_label,
            menus,
        });
        self.apply_settings(ui, None);
        self.refresh_chrome(ui);
        Ok(())
    }

    /// Build every registered overlay into a pointer-transparent layer over the window,
    /// lowest `order` first (Ch.21 §21.19).
    fn build_overlays(&mut self, ui: &mut Ui, host: WidgetId) -> Result<(), UiError> {
        let layer = ui.add(
            host,
            "overlays",
            NodeStyle::cover().no_hit_test(),
            forge_ui::widgets::Container::new(forge_ui::Role::GenericContainer),
        )?;
        let mut items: Vec<(String, crate::overlay::OverlayDescriptor)> = self
            .overlays
            .iter()
            .map(|(k, d)| (k.to_string(), d.clone()))
            .collect();
        items.sort_by_key(|(_, d)| d.order);
        let kind = self.preset.workspace.family.kind();
        for (k, d) in items {
            let slot = ui.add(
                layer,
                forge_ui::Key::Str(format!("overlay:{k}").into()),
                NodeStyle::cover().no_hit_test(),
                forge_ui::widgets::Container::new(forge_ui::Role::GenericContainer),
            )?;
            let mut cx = PanelCx::with_shell(
                PanelId::new(&format!("overlay:{k}")),
                kind,
                self.handles.clone(),
                None,
            );
            (d.build)(&mut cx);
            cx.run(ui, slot)?;
        }
        Ok(())
    }

    /// The theme the settings choose, resolved through the `Theme` registry: a plugin's
    /// theme when one is chosen and loaded, else the built-in one (which a plugin may have
    /// replaced or chained), else the built-in tokens.
    pub fn current_theme_id(&self) -> String {
        let s = self.handles.session();
        let o = &s.settings().theme_override;
        if !o.is_empty() && self.themes.get(o).is_some() {
            o.clone()
        } else {
            s.settings().theme.theme_id().to_string()
        }
    }

    fn current_theme(&self) -> forge_ui::Theme {
        let id = self.current_theme_id();
        match self.themes.get(&id) {
            Some(t) => (*t.tokens).clone(),
            None => self.handles.session().settings().theme.theme(),
        }
    }

    /// The `Theme` registry (after plugins replaced, chained or removed themes).
    pub fn themes(&self) -> &Registry<ThemePoint> {
        &self.themes
    }

    /// Build a floating window's dock (a torn-off panel's window).
    pub fn build_window(&mut self, ui: &mut Ui, key: u64) -> Result<(), UiError> {
        ui.set_window_key(Some(key));
        self.dock.build_floating(ui, key)?;
        self.apply_settings(ui, Some(key));
        Ok(())
    }

    /// Windows the layout wants that are not open yet (the runner opens them).
    pub fn windows_to_open(&mut self) -> Vec<forge_ui::ui::OpenWindow> {
        self.dock.windows_to_open()
    }

    pub fn window_moved(
        &mut self,
        key: Option<u64>,
        rect: forge_ui::Rect,
        monitor: Option<String>,
    ) {
        self.dock
            .window_moved(key.map_or(AreaId::Main, AreaId::Floating), rect, monitor);
    }

    pub fn window_closed(&mut self, key: u64) {
        self.dock.window_closed(key);
        self.handles.wiring.borrow_mut().drop_window(Some(key));
        self.synced.remove(&Some(key));
        self.applied_settings.remove(&Some(key));
    }

    pub fn monitors(&mut self, monitors: Vec<forge_ui::dock::MonitorInfo>) {
        self.dock.set_monitors(monitors);
    }

    // ---- the loop ------------------------------------------------------------------------

    /// Handle a window's actions that no widget consumed: dock operations, chords, menu
    /// items, toolbar buttons, palette picks, then the panels' own handlers.
    pub fn on_actions(&mut self, ui: &mut Ui, actions: Vec<ActionEnvelope>) {
        let window = ui.window_key();
        let rest = self.dock.handle_actions(actions);
        for a in rest {
            if let Some(t) = a.get::<ActionTriggered>() {
                self.pending_chord = None;
                let id = t.action.clone();
                self.run_action(ui, &id);
            } else if let Some(p) = a.get::<ChordPending>() {
                self.pending_chord = Some(p.first.clone());
            } else if let Some(m) = a.get::<MenuChosen>() {
                let id = m.id.clone();
                self.run_action(ui, &id);
            } else if let Some(Pressed(b)) = a.get::<Pressed>()
                && let Some(action) = self.chrome.as_ref().and_then(|c| c.toolbar.get(b)).cloned()
            {
                self.run_action(ui, &action);
            } else if let Some(c) = a.get::<CommandInvoked>() {
                let key = c.id.clone();
                self.pick_palette(ui, &key);
            } else {
                self.handles
                    .dispatch(ui, window, a.source, a.action.as_ref());
            }
        }
        let root = ui.root();
        for o in self.dock.windows_to_open() {
            ui.raise(root, o);
        }
    }

    /// Once per window per loop turn: pump the stream into the mirror, turn refusals into
    /// notifications, apply settings, run the panels' sync steps if anything changed, push
    /// the layout into the window, and write what is due.
    pub fn update_window(&mut self, ui: &mut Ui) {
        let window = ui.window_key();
        self.now = ui.now();
        if window.is_none() {
            self.emitter.next_frame();
        }
        self.pump();
        let requests = self.handles.session_mut().take_requests();
        for r in requests {
            match r {
                ShellRequest::OpenPanel(p) => self.open_panel(&p),
                ShellRequest::RunAction(id) => self.run_action(ui, &id),
            }
        }
        self.apply_settings(ui, window);
        if window.is_none() {
            // The UI's own counters (`ui.self`): shown by the profiler, never waking it.
            let st = ui.stats();
            self.handles
                .services()
                .ui_self
                .observe(st.frames_rendered, st.last_frame.draw_calls);
            self.follow_services();
            // WASM plugins: hot reload and drop-ins, on a turn the loop runs anyway (D-5).
            self.poll_plugins();
            // A playing simulation runs in fixed ticks on the editor's clock.
            let failed = {
                let s = self.handles.services();
                let mut play = s.play.borrow_mut();
                play.advance(self.now);
                play.take_error()
            };
            if let Some(why) = failed {
                self.handles.services().log.borrow_mut().push(
                    crate::console::LogLevel::Error,
                    "forge.play",
                    &forge_ui::trf!("The simulation could not step: {why}", why),
                    crate::console::LogSource::None,
                );
            }
        }
        let services_rev = {
            let s = self.handles.services();
            let log = s.log.borrow().revision();
            let assets = s.assets.as_ref().map_or(0, |a| a.borrow().revision());
            let play = s.play.borrow().revision();
            let view = s.viewport.borrow().revision();
            (log.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ assets).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ play.wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
                ^ view.rotate_left(17)
        };
        let revs = (
            self.handles.mirror().revision(),
            self.handles.session().revision(),
            services_rev,
        );
        // A panel with bounded work left (a filter applied in slices) asked for this turn.
        let wanted = self.handles.take_turn_wanted(window);
        let changed = self.synced.get(&window) != Some(&revs);
        if changed || wanted {
            self.synced.insert(window, revs);
            self.stats.sync_passes += 1;
            if let Err(e) = self.handles.sync(ui, window) {
                self.handles
                    .session_mut()
                    .problem(crate::notify::Problem::coded(
                        crate::notify::Severity::Error,
                        e.code(),
                        forge_ui::tr!("A panel could not update"),
                        &e.to_string(),
                    ));
            }
            // The chrome shows project and session state: unchanged on a turn a panel asked for.
            if window.is_none() && changed {
                self.refresh_chrome(ui);
            }
        }
        self.dock
            .sync_window(ui, window.map_or(AreaId::Main, AreaId::Floating));
        self.save_due();
    }

    /// Pump the client: events into the mirror, refusals into notifications.
    fn pump(&mut self) {
        let p = self.emitter.client().borrow_mut().pump();
        if p.is_empty() {
            return;
        }
        self.stats.pumps_with_events += 1;
        let client = self.emitter.client().borrow();
        let mut mirror = self.handles.mirror_rc().borrow_mut();
        // A remote client's optimistic overlays (O-13) come off before the core's events land
        // on the authoritative state, and go back on (those still pending) after.
        let lifted = (p.gap.is_some() || !p.events.is_empty() || !p.settled.is_empty())
            && mirror.lift_overlays();
        if p.gap.is_some() {
            let (project, next) = client.snapshot();
            mirror.resync(&project, next, client.history());
        }
        // Session commands (the play controls, from any issuer) take effect in stream order:
        // each after the events up to its own, so Play forks the world as it was edited
        // when Play was sent.
        let services = self.handles.services_rc();
        let mut session = p.session.iter().peekable();
        let mut unknown: Vec<String> = Vec::new();
        let mut run_session =
            |sc: &crate::client::SessionCommand, mirror: &ProjectMirror| match crate::play::decode(
                &sc.target, &sc.args,
            ) {
                Some(Ok(cmd)) => {
                    let mut play = services.play.borrow_mut();
                    play.control(cmd, mirror);
                    if let Some(why) = play.take_error() {
                        unknown.push(forge_ui::trf!(
                            "{command}: the play core refused it: {why}",
                            command = sc.target,
                            why
                        ));
                    }
                }
                Some(Err(why)) => unknown.push(format!("{}: {why}", sc.target)),
                // A simulation input (WP-19): the same session command from every client.
                None => match crate::play::decode_input(&sc.target, &sc.args) {
                    Some(Ok(i)) => {
                        if let Err(why) = services.play.borrow_mut().input(i) {
                            unknown.push(forge_ui::trf!(
                                "{command}: the play core refused it: {why}",
                                command = sc.target,
                                why
                            ));
                        }
                    }
                    Some(Err(why)) => unknown.push(format!("{}: {why}", sc.target)),
                    None => unknown.push(forge_ui::trf!(
                        "{command}: no session handler here",
                        command = sc.target
                    )),
                },
            };
        for ev in &p.events {
            mirror.apply(ev, |t| client.txn_info(t));
            while let Some(sc) = session.next_if(|sc| sc.seq <= ev.seq) {
                run_session(sc, &mirror);
            }
        }
        for sc in session {
            run_session(sc, &mirror);
        }
        if lifted || !p.predicted.is_empty() || !p.settled.is_empty() {
            mirror.settle_overlays(lifted, &p.predicted, &p.settled);
        }
        // The project lifecycle: a load (create, open, pull) replaced the project and
        // cleared the undo history — take the history again; then the new status.
        let mut new_outcomes = Vec::new();
        if let Some(st) = &p.project {
            let before = mirror.project().map(|o| o.epoch);
            if before.is_some_and(|e| e != st.epoch) {
                mirror.reset_history(client.history());
            }
            let seen = self
                .seen_outcome
                .unwrap_or_else(|| st.outcomes.last().map_or(0, |o| o.id));
            let me = client.issuer().tag();
            new_outcomes = st
                .outcomes
                .iter()
                .filter(|o| o.id > seen && o.issuer == me)
                .cloned()
                .collect();
            self.seen_outcome = Some(st.outcomes.last().map_or(seen, |o| o.id.max(seen)));
            mirror.set_project(std::sync::Arc::clone(st));
        }
        let opened = mirror
            .project()
            .and_then(|s| s.open.as_ref())
            .map(|o| (o.location.clone(), o.name.clone()));
        let template = match mirror.setting(forge_project::TEMPLATE_SETTING) {
            Some(forge_cmd::Value::Text(t)) => t.clone(),
            _ => String::new(),
        };
        mirror.set_targets(client.undo_target(), client.redo_target());
        drop(mirror);
        drop(client);
        self.show_outcomes(&new_outcomes, opened, &template);
        let mut s = self.handles.session_mut();
        let log = Rc::clone(&self.handles.services().log);
        if p.session_dropped > 0 {
            unknown.push(forge_ui::trf!(
                "{n} session command(s) were dropped: the editor did not keep up",
                n = p.session_dropped
            ));
        }
        for u in unknown {
            log.borrow_mut().push(
                crate::console::LogLevel::Warn,
                "forge.play",
                &forge_ui::trf!("A session command was not run: {u}", u),
                crate::console::LogSource::None,
            );
        }
        for r in p.refused {
            let code = r.rejection.code().as_str().to_string();
            // What was asked, as a person reads it (the bus names it in its canonical form).
            let what = crate::command_labels::history_label(&r.what);
            // The console keeps it (click-through to the command); the toast's "Details"
            // opens the console at this entry (Ch.21 §21.18 "Errors").
            let entry = log.borrow_mut().push(
                crate::console::LogLevel::Error,
                "forge.cmd",
                &forge_ui::trf!("{what} was refused: {why}", what, why = r.rejection),
                crate::console::LogSource::Command {
                    what: what.clone(),
                    code: Some(code.clone()),
                    txn: None,
                },
            );
            s.problem(
                crate::notify::Problem::coded(
                    crate::notify::Severity::Error,
                    &code,
                    &forge_ui::trf!("{what} was refused", what),
                    &r.rejection.to_string(),
                )
                .with_actions(vec![NoticeAction::Reveal {
                    reveal: Reveal::LogEntry(entry),
                    panel: "forge.console".into(),
                    label: forge_ui::tr!("Details").into(),
                }]),
            );
        }
    }

    /// Toast this shell's own lifecycle outcomes, record created and opened projects in the
    /// recent list, and offer a remote after a first save (O-11).
    fn show_outcomes(
        &mut self,
        outcomes: &[crate::project::Outcome],
        opened: Option<(String, String)>,
        template: &str,
    ) {
        if outcomes.is_empty() {
            return;
        }
        let services = self.handles.services_rc();
        let mut s = self.handles.session_mut();
        for o in outcomes {
            let verb = crate::project::op_title(&o.op);
            match &o.result {
                Ok(msg) => {
                    if [
                        crate::project::CREATE_CMD,
                        crate::project::OPEN_CMD,
                        crate::project::CLONE_CMD,
                    ]
                    .contains(&o.op.as_str())
                        && let Some((loc, name)) = &opened
                        && let Err(e) = services.launcher.borrow_mut().touch(loc, name, template)
                    {
                        s.problem(crate::notify::Problem::coded(
                            crate::notify::Severity::Warning,
                            e.code(),
                            forge_ui::tr!("The recent projects list was not saved"),
                            &e.to_string(),
                        ));
                    }
                    if o.first_save {
                        s.notify_with(Level::Info, forge_ui::tr!("Saved \u{2014} link a remote?"), forge_ui::tr!("The project is saved on this computer. Link a remote to back it up and share it; you can do it any time from Revision history."), vec![NoticeAction::OpenPanel {
                                panel: "forge.history".into(),
                                label: forge_ui::tr!("Link a remote\u{2026}").into(),
                            }]);
                    } else {
                        s.notify(Level::Info, &forge_ui::trf!("{verb}: done", verb), msg);
                    }
                }
                Err((code, msg)) => {
                    s.problem(crate::notify::Problem::coded(
                        crate::notify::Severity::Error,
                        code,
                        &forge_ui::trf!("{verb} failed", verb),
                        msg,
                    ));
                }
            }
        }
        drop(s);
        self.ask_trust();
    }

    /// Ask the person, once per question, whether they trust the open project (WP-34,
    /// [`crate::trust`]): when it carries plugins or plugin grants and nobody decided yet, a
    /// notice says what it carries and opens the Plugin manager, where *Trust this project* /
    /// *Don't trust* answer. Called after a lifecycle outcome and after a plugin poll found
    /// project plugins it left out — turns the loop takes anyway.
    fn ask_trust(&mut self) {
        let Some(core) = self.handles.services().trust_core() else {
            return;
        };
        let Some(t) = EditorCore::project_trust(&core) else {
            return;
        };
        if !t.state.asks() || self.trust_asked == Some(t.id) {
            return;
        }
        self.trust_asked = Some(t.id);
        let title = if t.state == crate::trust::TrustState::Changed {
            forge_ui::tr!("This project changed since you trusted it")
        } else {
            forge_ui::tr!("Do you trust this project?")
        };
        self.handles.session_mut().notify_with(
            Level::Info,
            title,
            &forge_ui::trf!(
                "{label} It carries: {what}.",
                label = t.label(),
                what = t.lines().join("; ")
            ),
            vec![NoticeAction::OpenPanel {
                panel: "forge.plugins".into(),
                label: forge_ui::tr!("Decide in the Plugin manager\u{2026}").into(),
            }],
        );
    }

    /// Bring the services up to date on the UI thread: log records other threads sent,
    /// and the asset database following the project's import intents.
    fn follow_services(&mut self) {
        let services = self.handles.services_rc();
        services.log.borrow_mut().drain();
        // Presence (Ch.37, WP-U10): who is looking at what is session state, published
        // read-only to teammates when the selection or the focused panel changed.
        if services.collab.attached() {
            let s = self.handles.session();
            services.collab.publish_presence(
                s.selection_revision(),
                &s.selection,
                s.focused_panel
                    .as_ref()
                    .map(forge_ui::dock::PanelId::as_str),
            );
        }
        if let Some(a) = &services.assets {
            let events = {
                let mirror = self.handles.mirror();
                a.borrow_mut().follow(&mirror)
            };
            let mut log = services.log.borrow_mut();
            for e in events {
                // l10n: matches the asset event variant's name (the log line is the event's Debug form)
                let level = if e.starts_with("Failed") {
                    crate::console::LogLevel::Error
                } else {
                    crate::console::LogLevel::Info
                };
                log.push(level, "forge.asset", &e, crate::console::LogSource::None);
            }
        }
    }

    /// Poll the WASM plugins ([`crate::hosting`]): at most once per
    /// [`crate::hosting::POLL_EVERY`], on a turn the loop runs anyway — never a reason to wake
    /// it (D-5). A plugin dropped in is installed into the running editor: its panels join
    /// the dock's host and the Window actions, its commands the palette (the core already
    /// runs them), its importers the asset database; changed code is live at once.
    fn poll_plugins(&mut self) {
        let services = self.handles.services_rc();
        if !services.hosting.borrow().due(self.now) {
            return;
        }
        let (location, set) = {
            let m = self.handles.mirror();
            (
                m.project()
                    .and_then(|p| p.open.as_ref())
                    .map(|o| o.location.clone()),
                crate::security::plugin_set(m.settings_under("plugins.")),
            )
        };
        let loaded = services.connect.plugins.borrow().loaded_manifests();
        // A plugin installs only when every panel of it can join this host (staged with the
        // rest of its items), so each `add_panel` below takes its panel.
        let host = &self.host;
        let out = services.hosting.borrow_mut().poll(
            self.now,
            location.as_deref(),
            &set,
            &loaded,
            &|k: &str| host.has_panel(&PanelId::new(k)),
        );
        if out.is_empty() {
            return;
        }
        let owner = forge_plugin::PluginId::new("forge.editor").ok();
        let mut actions_changed = false;
        for (plugin, dir, panels) in &out.installed {
            if let Some(h) = services.hosting.borrow().host() {
                services
                    .connect
                    .plugins
                    .borrow_mut()
                    .note_hosted(h, std::slice::from_ref(plugin));
            }
            let mut titles = Vec::new();
            for (k, d) in panels {
                titles.push(d.title.clone());
                let id = format!("forge.panel.open.{k}");
                if self.host.add_panel(PanelId::new(k), d.clone())
                    && self.actions.get(&id).is_none()
                    && let Some(owner) = &owner
                {
                    let item = ActionItem::new(
                        &forge_ui::trf!("Show {panel}", panel = forge_ui::l10n::tr_str(&d.title)),
                        forge_ui::tr_key!("Window"),
                        ActionKind::Session(SessionOp::OpenPanel(PanelId::new(k))),
                    );
                    let _ = self
                        .actions
                        .add(owner.clone(), &id, item, forge_plugin::Order::Last);
                    actions_changed = true;
                }
            }
            let line = if titles.is_empty() {
                forge_ui::trf!(
                    "{plugin} installed from {dir}",
                    plugin = plugin.id(),
                    dir = dir.display()
                )
            } else {
                forge_ui::trf!(
                    "{plugin} installed from {dir}; panels: {panels}",
                    plugin = plugin.id(),
                    dir = dir.display(),
                    panels = titles.join(", ")
                )
            };
            services.log.borrow_mut().push(
                crate::console::LogLevel::Info,
                "forge.plugins",
                &line,
                crate::console::LogSource::None,
            );
            self.handles.session_mut().notify(
                Level::Info,
                forge_ui::tr!("Plugin installed"),
                &forge_ui::trf!(
                    "{line}. It runs in the WASM sandbox with only what you grant it.",
                    line
                ),
            );
        }
        for t in &out.commands {
            if !self.commands.iter().any(|c| &c.target == t) {
                self.commands.push(CommandInfo {
                    target: t.clone(),
                    fields: None,
                });
            }
        }
        for (id, generation) in &out.reloaded {
            services
                .connect
                .plugins
                .borrow_mut()
                .note_reloaded(id, *generation);
            services.log.borrow_mut().push(
                crate::console::LogLevel::Info,
                "forge.plugins",
                &forge_ui::trf!("{id} reloaded (generation {generation})", id, generation),
                crate::console::LogSource::None,
            );
        }
        for p in &out.problems {
            services.log.borrow_mut().push(
                crate::console::LogLevel::Warn,
                "forge.plugins",
                p,
                crate::console::LogSource::None,
            );
            self.handles
                .session_mut()
                .problem(crate::notify::Problem::coded(
                    crate::notify::Severity::Warning,
                    "EDITOR-0010",
                    forge_ui::tr!("A plugin was not loaded"),
                    p,
                ));
        }
        // The open project's plugins, left out until the person trusts it (WP-34).
        for d in &out.untrusted {
            services.connect.plugins.borrow_mut().note_not_loaded(
                d,
                None,
                crate::hosting::UNTRUSTED_WHY,
            );
            services.log.borrow_mut().push(
                crate::console::LogLevel::Info,
                "forge.plugins",
                &format!("{}: {}", d.display(), crate::hosting::UNTRUSTED_WHY),
                crate::console::LogSource::None,
            );
        }
        // A loaded project plugin whose files changed into something the person has not
        // trusted (WP-35): its trusted code keeps running; the change waits for the answer.
        for d in &out.held_reloads {
            services.log.borrow_mut().push(
                crate::console::LogLevel::Info,
                "forge.plugins",
                &forge_ui::trf!(
                    "{dir} changed since you trusted the project: the code you trusted keeps running until you trust the change in the Plugin manager",
                    dir = d.display()
                ),
                crate::console::LogSource::None,
            );
        }
        if !out.untrusted.is_empty() || !out.held_reloads.is_empty() {
            self.ask_trust();
        }
        if out.importers_changed
            && let Some(a) = &services.assets
        {
            a.borrow_mut()
                .plugins_changed(services.hosting.borrow().extensions());
        }
        if actions_changed {
            self.handles.session_mut().set_actions(
                self.actions
                    .iter()
                    .map(|(id, a)| ActionSummary {
                        id: id.to_string(),
                        title: a.title.clone(),
                        category: a.category.clone(),
                    })
                    .collect(),
            );
        }
    }

    /// The console log (the runner installs its waker, engine systems get senders).
    pub fn log(&self) -> Rc<RefCell<crate::console::ConsoleLog>> {
        Rc::clone(&self.handles.services().log)
    }

    /// When the loop must wake next on the shell's account (a debounced save, or now when
    /// a panel asked for another turn to continue bounded work).
    pub fn next_deadline(&self) -> Option<Duration> {
        [
            self.layout_save.deadline(),
            self.settings_save.deadline(),
            self.keymap_save.deadline(),
            self.handles.turn_wanted().then_some(self.now),
            // While a simulation plays, the loop wakes at the display rate; paused or
            // stopped, never (D-5).
            (self.handles.services().play.borrow().state() == PlayState::Playing)
                .then_some(self.now + PLAY_FRAME),
            // W2 control only (`HostingFaults::poll_wakes_the_loop`): a plugin poll on a
            // timer. Plugin polls never wake the loop (D-5).
            self.handles
                .services()
                .hosting
                .borrow()
                .faults
                .poll_wakes_the_loop()
                .then_some(self.now + crate::hosting::POLL_EVERY),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Send a play control (Ch.21 §21.21 "Play controls") as the `forge.play.control`
    /// command on the bus — the same command an automation session sends. The play core runs it
    /// when the stream delivers it (pumped here, so it takes effect this turn): it forks the
    /// simulation from the mirror — the edit world, read-only — so play never writes the
    /// project, and records the control in the play session's log.
    pub fn play(&mut self, cmd: PlayCommand) {
        self.emitter.emit(crate::play::play_command(cmd));
        self.pump();
    }

    /// Send a simulation input as the `forge.play.input` command on the bus — the command
    /// a script, an automation session and `forge --headless` send (WP-19). The play core queues it
    /// for the next step boundary when the stream delivers it (pumped here).
    pub fn play_input(&mut self, i: forge_sim::SimInput) {
        self.emitter.emit(crate::play::input_command(i));
        self.pump();
    }

    /// The user's UI scale (the runner re-lays out every window when it changes).
    pub fn user_scale(&self) -> f32 {
        self.handles.session().settings().scale_factor()
    }

    /// The loop is exiting: write everything unsaved now.
    pub fn exiting(&mut self) {
        self.note_changes();
        self.flush_layout();
        self.flush_settings();
        self.flush_keymap();
    }

    // ---- actions -------------------------------------------------------------------------

    /// The context actions are evaluated in.
    pub fn action_cx(&self, ui: Option<&Ui>) -> ActionCx {
        let m = self.handles.mirror();
        let s = self.handles.session();
        let focused_panel = ui
            .and_then(focused_panel)
            .or_else(|| s.focused_panel.clone());
        ActionCx {
            selection: s.selection.clone(),
            focused_panel,
            open_panels: self.dock.layout().panels(),
            can_undo: m.undo_target().is_some(),
            can_redo: m.redo_target().is_some(),
        }
    }

    /// Run an action by id (menus, toolbar, chords and the palette all come here).
    pub fn run_action(&mut self, ui: &mut Ui, id: &str) {
        let cx = self.action_cx(Some(ui));
        match invoke(&self.actions, id, &cx) {
            Ok(inv) => self.run_invocation(ui, inv, &cx),
            Err(EditorError::ActionDisabled(_)) => {
                let title = self.actions.get(id).map_or(id.to_string(), |a| {
                    forge_ui::l10n::tr_str(&a.title).into_owned()
                });
                self.hint(
                    ui,
                    &forge_ui::trf!("\u{201c}{title}\u{201d} is not available here", title),
                );
            }
            Err(e) => {
                self.handles
                    .session_mut()
                    .problem(crate::notify::Problem::coded(
                        crate::notify::Severity::Error,
                        e.code(),
                        forge_ui::tr!("That action does not exist"),
                        &e.to_string(),
                    ));
            }
        }
    }

    fn run_invocation(&mut self, ui: &mut Ui, inv: Invocation, cx: &ActionCx) {
        match inv {
            Invocation::Commands { label, commands } => {
                if commands.is_empty() {
                    self.hint(
                        ui,
                        &forge_ui::trf!("\u{201c}{label}\u{201d} has nothing to do", label),
                    );
                } else {
                    self.emitter.emit_all(&label, commands);
                }
            }
            Invocation::Undo => {
                if self.emitter.undo().is_none() {
                    self.hint(ui, forge_ui::tr!("Nothing to undo"));
                }
            }
            Invocation::Redo => {
                if self.emitter.redo().is_none() {
                    self.hint(ui, forge_ui::tr!("Nothing to redo"));
                }
            }
            Invocation::Session(op) => self.run_session(ui, op, cx),
        }
    }

    fn run_session(&mut self, ui: &mut Ui, op: SessionOp, cx: &ActionCx) {
        match op {
            SessionOp::OpenPalette => self.open_palette(ui),
            SessionOp::OpenPanel(p) => self.open_panel(&p),
            SessionOp::ClosePanel | SessionOp::FloatPanel | SessionOp::ToggleMaximise => {
                let Some(p) = cx.focused_panel.clone() else {
                    return;
                };
                let mut l = self.dock.layout().clone();
                let changed = match op {
                    SessionOp::ClosePanel => l.close(&p),
                    SessionOp::FloatPanel => {
                        let o = ui.window_origin();
                        l.float(
                            &p,
                            forge_ui::Rect::new(o.x + 160.0, o.y + 120.0, 520.0, 380.0),
                            None,
                        )
                        .is_ok()
                    }
                    _ => l.toggle_maximise(&p),
                };
                if changed {
                    self.dock.set_layout(l);
                }
            }
            SessionOp::SaveLayoutAs => self.save_layout_as(),
            SessionOp::ResetLayout => {
                self.dock.set_layout(self.preset.layout.clone());
                self.hint(ui, forge_ui::tr!("Layout reset to the preset's"));
            }
            SessionOp::SetTheme(id) => {
                if let Some(t) = EditorTheme::from_theme_id(&id) {
                    self.handles.session_mut().update_settings(|s| {
                        s.theme = t;
                        s.theme_override.clear();
                    });
                } else if self.themes.get(&id).is_some() {
                    self.handles
                        .session_mut()
                        .update_settings(|s| s.theme_override = id.clone());
                }
            }
            SessionOp::Custom(name) => self.run_custom(ui, &name),
        }
    }

    fn run_custom(&mut self, ui: &mut Ui, name: &str) {
        let window = ui.window_key();
        if self.handles.run_op(ui, window, name) {
            return;
        }
        match name {
            "select_all" => {
                let all: Vec<_> = self.handles.mirror().entities().map(|(k, _)| *k).collect();
                self.handles.session_mut().set_selection(all);
            }
            // Text editing chords are handled by the focused text widget itself; the keymap
            // lists them so the palette and the keybindings editor show them.
            "text.select_all" | "text.delete" => {}
            // The project lifecycle (WP-U7): session commands the core performs (E-36) —
            // like the play controls, they change no project state themselves, so they are
            // not undoable edits; the outcome arrives as a toast.
            "save" | "project.push" | "project.pull" => {
                let cmd = match name {
                    "save" => crate::project::save_command(""),
                    "project.push" => crate::project::ProjectOp::Push.command(),
                    _ => crate::project::ProjectOp::Pull.command(),
                };
                self.emitter.emit(cmd);
                self.pump();
            }
            "play.toggle" | "play.pause" | "play.step" | "play.stop" => {
                let state = self.handles.services().play.borrow().state();
                let cmd = match (name, state) {
                    ("play.toggle", PlayState::Stopped) => PlayCommand::Play,
                    ("play.toggle", _) | ("play.stop", _) => PlayCommand::Stop,
                    ("play.pause", PlayState::Paused) => PlayCommand::Play,
                    ("play.pause", _) => PlayCommand::Pause,
                    _ => PlayCommand::Step(1),
                };
                self.play(cmd);
            }
            other => {
                let title = self
                    .actions
                    .iter()
                    .find(|(_, a)| matches!(&a.kind, ActionKind::Session(SessionOp::Custom(n)) if n == other))
                    .map_or(other.to_string(), |(_, a)| {
                        forge_ui::l10n::tr_str(&a.title).into_owned()
                    });
                self.hint(
                    ui,
                    &forge_ui::trf!(
                        "\u{201c}{title}\u{201d}: open the panel it belongs to first",
                        title
                    ),
                );
            }
        }
    }

    fn open_panel(&mut self, p: &PanelId) {
        let dock = self.host.default_dock(p).unwrap_or(Dock::Center);
        let mut l = self.dock.layout().clone();
        match open_panel(&mut l, p, dock) {
            Ok(()) => {
                if &l != self.dock.layout() {
                    self.dock.set_layout(l);
                }
            }
            Err(e) => {
                self.handles
                    .session_mut()
                    .problem(crate::notify::Problem::coded(
                        crate::notify::Severity::Error,
                        e.code(),
                        &forge_ui::trf!("{panel} could not be opened", panel = p),
                        &e.to_string(),
                    ));
            }
        }
    }

    fn save_layout_as(&mut self) {
        let Some(store) = &self.layouts else {
            self.handles
                .session_mut()
                .problem(crate::notify::Problem::coded(
                    crate::notify::Severity::Warning,
                    "EDITOR-0006",
                    forge_ui::tr!("Layouts cannot be saved"),
                    forge_ui::tr!(
                        "There is no user configuration directory (set FORGE_CONFIG_DIR)."
                    ),
                ));
            return;
        };
        let taken = store.list().unwrap_or_default();
        // The new layout's name is the user's data (a file name they rename): named in the UI
        // language when it is made.
        let name = (1..)
            .map(|i| forge_ui::trf!("Layout {i}", i))
            .find(|n| !taken.contains(n))
            .unwrap_or_else(|| forge_ui::tr!("Layout").into());
        let r = store.save(&name, self.dock.layout());
        let mut s = self.handles.session_mut();
        match r {
            Ok(()) => {
                s.notify(
                    Level::Success,
                    &forge_ui::trf!("Layout saved as \u{201c}{name}\u{201d}", name),
                    "",
                );
            }
            Err(e) => {
                s.problem(crate::notify::Problem::coded(
                    crate::notify::Severity::Error,
                    e.code(),
                    forge_ui::tr!("The layout was not saved"),
                    &e.to_string(),
                ));
            }
        }
    }

    fn hint(&mut self, ui: &mut Ui, text: &str) {
        if let Some(c) = &self.chrome
            && ui.window_key().is_none()
        {
            c.status_hint.set(ui.rt_mut(), text.to_string());
        }
    }

    // ---- the palette ---------------------------------------------------------------------

    fn open_palette(&mut self, ui: &mut Ui) {
        let cx = self.action_cx(Some(ui));
        let recent = self.palette.recent().to_vec();
        let km = self.handles.session().keymap().clone();
        let mut p = Palette::build(
            &self.actions,
            &km.borrow(),
            &self.host.panels(),
            &self.commands,
            &cx,
        );
        p.set_recent(recent);
        let items: Vec<PickItem> = p.pick_items();
        self.palette = p;
        let owner = ui.root();
        let _ = open_command_palette_ui(ui, owner, Rc::new(items));
    }

    /// Run a palette entry by key (what the popup raises).
    pub fn pick_palette(&mut self, ui: &mut Ui, key: &str) {
        let cx = self.action_cx(Some(ui));
        match self.palette.pick(key, &self.actions, &cx) {
            Ok(PaletteOutcome::Run(inv)) => self.run_invocation(ui, inv, &cx),
            Ok(PaletteOutcome::OpenPanel(p)) => self.open_panel(&p),
            Ok(PaletteOutcome::NeedsArguments { command, fields }) => {
                self.handles.session_mut().notify(Level::Info, &forge_ui::trf!("\u{201c}{command}\u{201d} needs arguments", command), &forge_ui::trf!(
                        "It takes {fields}. Use the panel that edits them (the hierarchy or the inspector) \
                         or an automation session.",
                        fields = fields.join(", ")
                    ));
            }
            Err(EditorError::ActionDisabled(_)) => {
                self.hint(ui, forge_ui::tr!("That is not available here"))
            }
            Err(e) => {
                self.handles
                    .session_mut()
                    .problem(crate::notify::Problem::coded(
                        crate::notify::Severity::Error,
                        e.code(),
                        forge_ui::tr!("The palette entry failed"),
                        &e.to_string(),
                    ));
            }
        }
    }

    // ---- chrome --------------------------------------------------------------------------

    fn item(&self, id: &str, cx: &ActionCx, km: &KeyMap) -> Option<MenuItem> {
        let a = self.actions.get(id)?;
        let m = self.handles.mirror();
        let label = match id {
            "forge.edit.undo" => m
                .undo_label()
                .map_or(forge_ui::tr!("Undo").to_string(), |l| {
                    forge_ui::trf!(
                        "Undo {step}",
                        step = crate::command_labels::history_label(l)
                    )
                }),
            "forge.edit.redo" => m
                .redo_label()
                .map_or(forge_ui::tr!("Redo").to_string(), |l| {
                    forge_ui::trf!(
                        "Redo {step}",
                        step = crate::command_labels::history_label(l)
                    )
                }),
            _ => forge_ui::l10n::tr_str(&a.title).into_owned(),
        };
        let mut item = MenuItem::action(id, &label);
        if let Some(c) = km.chords_for(id).first() {
            item = item.shortcut(&c.label());
        }
        if let ActionKind::Session(SessionOp::SetTheme(t)) = &a.kind {
            item = item.checked(self.current_theme_id() == *t);
        }
        if !a.is_enabled(cx) {
            item = item.disabled();
        }
        Some(item)
    }

    /// The menus, from the action registry (the palette and the menus cannot drift).
    fn menu_spec(&self) -> Vec<(String, Vec<MenuItem>)> {
        let cx = self.action_cx(None);
        let km_rc = self.handles.session().keymap().clone();
        let km = km_rc.borrow();
        let list = |ids: &[&str]| -> Vec<MenuItem> {
            let mut out = Vec::new();
            for id in ids {
                if *id == "-" {
                    if out.last().is_some_and(|l| *l != MenuItem::Separator) {
                        out.push(MenuItem::Separator);
                    }
                } else if let Some(i) = self.item(id, &cx, &km) {
                    out.push(i);
                }
            }
            if out.last() == Some(&MenuItem::Separator) {
                out.pop();
            }
            out
        };
        let mut panels: Vec<MenuItem> = self
            .host
            .panels()
            .iter()
            .filter_map(|(p, _)| self.item(&format!("forge.panel.open.{p}"), &cx, &km))
            .collect();
        // One key per item (a comparison sort formatting both sides allocates n·log n).
        panels.sort_by_cached_key(|m| format!("{m:?}"));
        let mut window = vec![MenuItem::submenu(forge_ui::tr!("Panels"), panels)];
        window.push(MenuItem::Separator);
        window.extend(list(&[
            "forge.dock.toggle_maximise",
            "forge.dock.float_panel",
            "forge.dock.close_panel",
            "-",
            "forge.layout.save_as",
            "forge.layout.reset",
        ]));
        vec![
            (
                forge_ui::tr!("File").into(),
                list(&[
                    "forge.file.new",
                    "forge.file.open",
                    "forge.file.save",
                    "-",
                    "forge.project.push",
                    "forge.project.pull",
                    "forge.panel.open.forge.history",
                    "-",
                    "forge.panel.open.forge.presets",
                    "forge.file.build",
                    "-",
                    "forge.panel.open.forge.settings",
                    "forge.panel.open.forge.keybindings",
                ]),
            ),
            (
                forge_ui::tr!("Edit").into(),
                list(&[
                    "forge.edit.undo",
                    "forge.edit.redo",
                    "forge.panel.open.forge.undo_history",
                    "-",
                    "forge.edit.delete",
                    "forge.edit.select_all",
                    "-",
                    "forge.palette.open",
                ]),
            ),
            (
                forge_ui::tr!("View").into(),
                list(&[
                    "forge.theme.dark",
                    "forge.theme.light",
                    "forge.theme.high_contrast",
                    "-",
                    "forge.panel.open.forge.notifications",
                    "forge.panel.open.forge.console",
                ]),
            ),
            (forge_ui::tr!("Window").into(), window),
            (
                forge_ui::tr!("Play").into(),
                list(&[
                    "forge.play.toggle",
                    "forge.play.pause",
                    "forge.play.step",
                    "forge.play.stop",
                ]),
            ),
        ]
    }

    /// Bring the menus and the status bar in line with the mirror and the session.
    fn refresh_chrome(&mut self, ui: &mut Ui) {
        let menus = self.menu_spec();
        let summary = {
            let m = self.handles.mirror();
            let n = m.len();
            let count = if n == 1 {
                forge_ui::trf!("{n} entity", n)
            } else {
                forge_ui::trf!("{n} entities", n)
            };
            match m.project().and_then(|s| s.open.as_ref()) {
                Some(o) => {
                    let name = format!("{}{}", o.name, if o.dirty { " \u{2022}" } else { "" });
                    if o.in_memory {
                        forge_ui::trf!("{name} \u{b7} {count} \u{b7} in memory", name, count)
                    } else {
                        forge_ui::trf!("{name} \u{b7} {count}", name, count)
                    }
                }
                None => forge_ui::trf!("No project \u{b7} {count}", count),
            }
        };
        let undo = self
            .handles
            .mirror()
            .undo_label()
            .map_or(String::new(), |l| {
                forge_ui::trf!(
                    "Undo: {step}",
                    step = crate::command_labels::history_label(l)
                )
            });
        let (unread, user) = {
            let s = self.handles.session();
            (
                s.notifications.unread(),
                self.emitter.client().borrow().issuer().tag(),
            )
        };
        let chord = self.pending_chord.clone();
        let Some(c) = &mut self.chrome else {
            return;
        };
        if menus != c.menus {
            if let Some(mb) = ui.widget_mut::<MenuBar>(c.menubar) {
                mb.set_menus(menus.clone());
            }
            ui.invalidate(c.menubar, Dirty::ALL);
            c.menus = menus;
            self.stats.menu_rebuilds += 1;
        }
        let rt = ui.rt_mut();
        c.status_project.set(rt, summary);
        c.status_undo.set(rt, undo);
        if let Some(ch) = chord {
            c.status_hint.set(
                rt,
                forge_ui::trf!("{ch} \u{2026} waiting for the second key", ch),
            );
        }
        c.status_notices.set(
            rt,
            if unread == 0 {
                String::new()
            } else {
                if unread == 1 {
                    forge_ui::trf!("{unread} unread notification", unread)
                } else {
                    forge_ui::trf!("{unread} unread notifications", unread)
                }
            },
        );
        c.notices_label.set(
            rt,
            if unread == 0 {
                forge_ui::tr!("Notifications").into()
            } else {
                forge_ui::trf!("Notifications ({unread})", unread)
            },
        );
        c.status_user.set(rt, user);
    }

    /// Apply the editor settings (theme, reduced motion, caret blink) to a window.
    fn apply_settings(&mut self, ui: &mut Ui, window: WindowKey) {
        let rev = self.handles.session().settings_revision();
        if self.applied_settings.get(&window) == Some(&rev) {
            return;
        }
        self.applied_settings.insert(window, rev);
        let s = self.handles.session().settings().clone();
        let theme = self.current_theme();
        if *ui.theme() != theme {
            ui.set_theme(theme);
        }
        ui.set_reduced_motion(s.reduced_motion);
        ui.set_caret_blink(s.caret_blink);
    }

    // ---- saving user config ----------------------------------------------------------------

    fn note_changes(&mut self) {
        if self.dock.take_dirty() {
            self.layout_save.changed(self.now);
        }
        let (srev, krev) = {
            let s = self.handles.session();
            (s.settings_revision(), s.keymap_revision())
        };
        if srev != self.seen_settings_rev {
            self.seen_settings_rev = srev;
            self.settings_save.changed(self.now);
        }
        if krev != self.seen_keymap_rev {
            self.seen_keymap_rev = krev;
            self.keymap_save.changed(self.now);
        }
    }

    fn due(a: &Autosave, now: Duration) -> bool {
        a.deadline().is_some_and(|d| now >= d)
    }

    fn save_due(&mut self) {
        self.note_changes();
        if Self::due(&self.layout_save, self.now) {
            self.flush_layout();
        }
        if Self::due(&self.settings_save, self.now) {
            self.flush_settings();
        }
        if Self::due(&self.keymap_save, self.now) {
            self.flush_keymap();
        }
    }

    fn report(&self, what: &str, e: &EditorError) {
        self.handles.session_mut().problem(
            crate::notify::Problem::coded(
                crate::notify::Severity::Error,
                e.code(),
                &forge_ui::trf!("{what} was not saved", what),
                &e.to_string(),
            )
            .with_actions(vec![NoticeAction::Run {
                action: "forge.layout.save_as".into(),
                label: forge_ui::tr!("Retry").into(),
            }]),
        );
    }

    fn flush_layout(&mut self) {
        let Some(store) = &self.layouts else {
            self.layout_save = Autosave::new();
            return;
        };
        match self.layout_save.flush(store, self.dock.layout()) {
            Ok(()) => self.stats.layout_writes = self.layout_save.writes,
            Err(e) => {
                self.report(forge_ui::tr!("The layout"), &e);
                self.layout_save = Autosave::new();
            }
        }
    }

    fn flush_settings(&mut self) {
        if !self.settings_save.pending() {
            return;
        }
        if let Some(d) = &self.config_dir {
            let r = self.handles.session().settings().save(d);
            match r {
                Ok(()) => self.stats.settings_writes += 1,
                Err(e) => self.report(forge_ui::tr!("Editor settings"), &e),
            }
        }
        self.settings_save.saved();
    }

    fn flush_keymap(&mut self) {
        if !self.keymap_save.pending() {
            return;
        }
        if let Some(d) = &self.config_dir {
            let layer = self.handles.session().user_keymap_layer();
            let r = layer
                .to_ron()
                .and_then(|t| write_atomic(&d.join(USER_KEYMAP_FILE), t.as_bytes()));
            match r {
                Ok(()) => self.stats.keymap_writes += 1,
                Err(e) => self.report(forge_ui::tr!("The keymap"), &e),
            }
        }
        self.keymap_save.saved();
    }
}

/// The dock panel holding keyboard focus in `ui`, from the focused widget up.
fn focused_panel(ui: &Ui) -> Option<PanelId> {
    let mut cur = ui.focused();
    while let Some(w) = cur {
        if let Some(Key::Str(s)) = ui.key_of(w)
            && let Some(p) = s.strip_prefix("panel:")
        {
            return Some(PanelId::new(p));
        }
        if let Some(strip) = ui.widget::<forge_ui::dock::DockTabStrip>(w) {
            let panels = strip.panels();
            return panels.get(strip.active()).cloned();
        }
        cur = ui.parent(w);
    }
    None
}

/// Whether an error's text names plugin `id` (the whole id, not the start of a longer one).
fn mentions(text: &str, id: &str) -> bool {
    text.match_indices(id).any(|(i, _)| {
        let rest = &text[i + id.len()..];
        let mut cs = rest.chars();
        match cs.next() {
            None => true,
            Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-' => false,
            Some('.') => !cs.next().is_some_and(|c| c.is_ascii_alphanumeric()),
            Some(_) => true,
        }
    })
}

/// Assemble a [`ShellConfig`]: define the editor's extension points, load the first-party
/// `forge.editor` actions plus `plugins` through the ordinary loader (I16), and take the
/// registries the shell runs on.
pub fn assemble(
    preset: Preset,
    plugins: &[&dyn forge_plugin::SourcePlugin],
    disabled: &[&forge_plugin::Manifest],
    config_dir: Option<PathBuf>,
) -> Result<ShellConfig, EditorError> {
    assemble_hosted(preset, plugins, disabled, config_dir, None)
}

/// [`assemble`], hosting WASM plugins (WP-21, [`crate::hosting`]): every plugin directory
/// under `hosting.dirs` is loaded on a WASM host over the core's one grant table and goes
/// through **the same load** as the source plugins (one manifest check, one conflict check,
/// one install order). A WASM plugin that fails is left out with the reason (the Plugin
/// manager lists it, a notice says why) and never stops the editor starting. The core gets
/// the plugin commands and the preset catalog of the real `Preset` registry.
///
/// Besides `plugins`, every editor loads its default plugin set here: the `forge.editor`
/// actions, the kernel's asset types (`forge.asset`), the first-party importers
/// (`forge.importers`), the built-in presets (`forge.presets`) and the 2D pipeline
/// (`forge.2d`: the sprite-sheet type and the Aseprite importer) and 3D physics
/// (`forge.phys`: the avian3d and rapier3d backends, which Play forks with) — the dependency points
/// from host to plugin, and the Plugin manager lists them like any other (I16).
pub fn assemble_hosted(
    preset: Preset,
    plugins: &[&dyn forge_plugin::SourcePlugin],
    disabled: &[&forge_plugin::Manifest],
    config_dir: Option<PathBuf>,
    hosting: Option<crate::hosting::HostingSetup>,
) -> Result<ShellConfig, EditorError> {
    let fresh = || -> Result<forge_plugin::Extensions, EditorError> {
        let mut x = crate::editor_extensions()?;
        forge_asset::AssetServer::define_points(&mut x)?;
        forge_phys::plugin::define_points(&mut x)?;
        Ok(x)
    };
    let editor = crate::actions::EditorActions::new()?;
    let asset_types = forge_asset::FirstPartyAssets::new()?;
    let importers = forge_importers::Importers::new()?;
    let presets = forge_presets::BuiltinPresets::new()?;
    // The 2D pipeline's plugin (WP-U15): the sprite-sheet asset type and the Aseprite
    // importer. Loaded under every preset (I15); the 2D preset names it in its default set.
    let two_d = forge_2d::Plugin2d::new()?;
    // 3D physics (WP-60): the avian3d and rapier3d backends on `forge.phys.backend`. Loaded
    // under every preset (I15); the 3D preset names avian3d the point's default.
    let phys = forge_phys::PhysPlugin::new()?;
    let mut all: Vec<&dyn forge_plugin::SourcePlugin> =
        vec![&editor, &asset_types, &importers, &presets, &two_d, &phys];
    all.extend_from_slice(plugins);
    // The WASM plugins found, loaded on the editor's host (compiled and checked).
    let (host, dirs, mut found, mut refused, untrusted, grants) = match &hosting {
        Some(h) => {
            let grants = EditorCore::grants(&h.core);
            let host = Arc::new(crate::hosting::editor_host(grants.clone())?);
            let set = EditorCore::read(&h.core, |p| {
                crate::security::plugin_set(
                    p.settings()
                        .filter(|(k, _)| k.starts_with(crate::security::PLUGINS_PREFIX)),
                )
            });
            // **The start-up trust gate (WP-34, WP-35).** When a project is already open as
            // the editor starts (a start-up open, or a core shared with another shell), its
            // own plugins — its `plugins/` folders and the cached versions its set names —
            // load here only as the person trusted them; the others are listed not loaded and
            // added to the question the shell asks at its first turn.
            let mut dirs = h.dirs.clone();
            if dirs.project.is_none() {
                dirs.project = EditorCore::read_location(&h.core)
                    .as_deref()
                    .and_then(crate::hosting::PluginDirs::project_dir);
            }
            let d = crate::hosting::discover(&host, &dirs, &set, Some(&h.core));
            (
                Some(host),
                dirs,
                d.plugins,
                d.refused,
                d.untrusted,
                grants.snapshot(),
            )
        }
        None => (
            None,
            crate::hosting::PluginDirs::default(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            forge_plugin::Grants::new(),
        ),
    };
    // One load for source and WASM plugins. A WASM plugin the load refuses is left out
    // (named by the error) and the load runs again without it; a source plugin's error is
    // the editor's.
    let mut x = fresh()?;
    loop {
        let hosted: Vec<&dyn forge_plugin::HostedPlugin> = found
            .iter()
            .map(|(_, p)| p as &dyn forge_plugin::HostedPlugin)
            .collect();
        match forge_plugin::loader::load_hosted(&mut x, &all, &hosted, &[], &grants) {
            Ok(_) => break,
            Err(e) => {
                let text = e.to_string();
                let Some(i) = found
                    .iter()
                    .position(|(_, p)| mentions(&text, p.id().as_str()))
                else {
                    return Err(e.into());
                };
                let (dir, p) = found.remove(i);
                refused.push(crate::hosting::Refused {
                    dir,
                    id: Some(p.id().to_string()),
                    why: text,
                });
                x = fresh()?;
            }
        }
    }
    let mut loaded: Vec<&forge_plugin::Manifest> = all.iter().map(|p| p.manifest()).collect();
    let hosted_manifests: Vec<forge_plugin::Manifest> = found
        .iter()
        .map(|(_, p)| forge_plugin::HostedPlugin::manifest(p).clone())
        .collect();
    loaded.extend(hosted_manifests.iter());
    let catalog = PanelCatalog::from_manifests(&loaded, disabled);
    let take = |e: &str| EditorError::Plugin(format!("the {e} point is not defined"));
    let panels = std::mem::replace(
        x.registry_mut::<EditorPanel<PanelCx>>()
            // l10n: an extension point's identifier (error detail)
            .ok_or_else(|| take("EditorPanel"))?,
        Registry::new(),
    );
    let actions = std::mem::replace(
        // l10n: an extension point's identifier (error detail)
        x.registry_mut::<Action>().ok_or_else(|| take("Action"))?,
        Registry::new(),
    );
    let commands = crate::palette::commands_from_registry(
        x.registry::<forge_plugin::points::Command>()
            // l10n: an extension point's identifier (error detail)
            .ok_or_else(|| take("Command"))?,
    );
    let widgets = std::mem::replace(
        x.registry_mut::<forge_plugin::points::InspectorWidget<crate::inspect::InspectorCx>>()
            // l10n: an extension point's identifier (error detail)
            .ok_or_else(|| take("InspectorWidget"))?,
        Registry::new(),
    );
    let overlays = std::mem::replace(
        x.registry_mut::<EditorOverlay>()
            // l10n: an extension point's identifier (error detail)
            .ok_or_else(|| take("EditorOverlay"))?,
        Registry::new(),
    );
    let themes = std::mem::replace(
        x.registry_mut::<ThemePoint>()
            // l10n: an extension point's identifier (error detail)
            .ok_or_else(|| take("Theme"))?,
        Registry::new(),
    );
    let tools = std::mem::replace(
        x.registry_mut::<crate::viewport::tool::ViewportTool>()
            // l10n: an extension point's identifier (error detail)
            .ok_or_else(|| take("ViewportTool"))?,
        Registry::new(),
    );
    let worlds = std::mem::replace(
        x.registry_mut::<crate::viewport::world::ViewportWorldPoint>()
            // l10n: an extension point's identifier (error detail)
            .ok_or_else(|| take("ViewportWorld"))?,
        Registry::new(),
    );
    // WP-47: the links plugins add beside settings rows, and their hints on problems: the
    // services hold what the Settings window and the notification centre read.
    let setting_links = std::mem::replace(
        x.registry_mut::<crate::settings::SettingLinkPoint>()
            // l10n: an extension point's identifier (error detail)
            .ok_or_else(|| take("SettingLink"))?,
        Registry::new(),
    );
    let notice_hints = crate::notify::NoticeHints::from_registry(
        x.registry::<crate::notify::NoticeHintPoint>()
            // l10n: an extension point's identifier (error detail)
            .ok_or_else(|| take("NoticeHint"))?,
    );
    let layers = x
        .registry::<crate::viewport::layer::ViewportLayerPoint>()
        .map(crate::viewport::layer::ViewportLayers::from_registry)
        .unwrap_or_default();
    let mut services = EditorServices {
        inspector_widgets: Rc::new(widgets),
        viewport_tools: Rc::new(tools),
        viewport_worlds: Rc::new(worlds),
        setting_links: Rc::new(setting_links),
        notice_hints: Rc::new(notice_hints),
        viewport_layers: Rc::new(RefCell::new(layers)),
        promotion_rules: Rc::new(
            x.registry::<crate::project::promote::PromotionRulePoint>()
                .map(crate::project::promote::PromotionRules::from_registry)
                .unwrap_or_default(),
        ),
        ..EditorServices::default()
    };
    // The generator backend a plugin provides, if one loaded: its stages join the graph
    // library and the graph editor lowers generator graphs through it. None loaded: none.
    if let Some(g) = x
        .registry::<crate::graph::ir::GeneratorBackendPoint>()
        .and_then(|r| r.iter().next())
        .map(|(_, g)| g.clone())
    {
        services = services.with_generator(g);
    }
    // The physics backends this load registered (the first-party ones, and whatever a plugin
    // added, replaced or chained): Play forks its simulation with them.
    if let (Some(reg), Some(play)) = (
        x.registry::<forge_phys::PhysicsBackendPoint>(),
        services.play_core.as_ref(),
    ) && let Ok(copy) = forge_phys::backend::copy_registry(reg)
    {
        play.borrow_mut().set_physics_backends(Arc::new(copy));
    }
    // Services the plugins add beside the editor's own (their panels find them in
    // `services.extensions`), in registry order.
    let providers: Vec<crate::services::ServiceProvider> = x
        .registry::<crate::services::ServiceProviderPoint>()
        .map(|r| r.iter().map(|(_, p)| p.clone()).collect())
        .unwrap_or_default();
    for p in providers {
        p(&mut services);
    }
    // The plugin manager lists what this load installed and left out (I16: first-party
    // plugins are listed like any other), the WASM plugins hosted, and the plugin
    // directories that did not load, with why.
    let sources: Vec<&forge_plugin::Manifest> = all.iter().map(|p| p.manifest()).collect();
    {
        let mut m = crate::connect::plugins::PluginManager::from_load(&sources, disabled);
        if let Some(h) = &host {
            let plugins: Vec<forge_wasm::WasmPlugin> =
                found.iter().map(|(_, p)| p.clone()).collect();
            m.note_hosted(h, &plugins);
        }
        for r in &refused {
            m.note_not_loaded(&r.dir, r.id.as_deref(), &r.why);
        }
        for d in &untrusted {
            m.note_not_loaded(d, None, crate::hosting::UNTRUSTED_WHY);
        }
        *services.connect.plugins.borrow_mut() = m;
    }
    {
        let mut log = services.log.borrow_mut();
        for r in &refused {
            log.push(
                crate::console::LogLevel::Warn,
                "forge.plugins",
                &forge_ui::trf!(
                    "The plugin in {dir} was not loaded: {why}",
                    dir = r.dir.display(),
                    why = r.why
                ),
                crate::console::LogSource::None,
            );
        }
    }
    let hosting_state =
        crate::hosting::PluginHosting::assembled(x, host, dirs, &found, &refused, &untrusted);
    *services.hosting.borrow_mut() = hosting_state;
    if let Some(h) = &hosting {
        services.hosting.borrow_mut().attach(&h.core)?;
    }
    Ok(ShellConfig {
        preset,
        panels,
        actions,
        commands,
        catalog,
        config_dir,
        overlays,
        themes,
        services,
        startup_notices: Vec::new(),
    })
}
