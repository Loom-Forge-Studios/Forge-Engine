//! The read-side services the shell hands every panel next to the mirror (Ch.21 §21.18):
//! the component catalogue and the `InspectorWidget` registry (the inspector), the console
//! log, the asset catalogue (the asset browser), and the 2D, audio and input backends of the
//! domain editors (WP-U13). None of them is a way to change project state: the asset
//! catalogue follows the project, and every project edit is still a command through the
//! emitter (I7).
//!
//! Each backend that is not built yet is a labelled in-memory implementation (D-4): the
//! asset database over an in-memory store until the launcher opens a project folder.
//!
//! **Plugins add services** beside these: a `ServiceProvider` ([`ServiceProviderPoint`]) is
//! run by `shell::assemble_hosted` once the services are built, and puts what its panels read
//! into [`EditorServices::extensions`] (typed, one per type); a `ViewportWorld` provider
//! ([`crate::viewport::world::ViewportWorldPoint`]) is kept in
//! [`EditorServices::viewport_worlds`] for the viewport panels.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use forge_frames::{FrameResolver, WorldFrame};
use forge_plugin::Registry;

use forge_reflect::PinKind;

use crate::assets::AssetCatalog;
use crate::console::ConsoleLog;
use crate::domain::audio::{AudioBuses, MemoryAudio};
use crate::domain::input::{InputActions, MemoryInput};
use crate::domain::scene2d::{Forge2dScene, Scene2d};
use crate::inspect::{ComponentCatalog, InspectorWidgets};
use crate::play::PlayBackend;
use crate::profile::{CounterSource, SelfUiCounters};
use crate::sim_bridge::{SimPlay, TraceCounters};
use crate::viewport::surface::ViewportSurfaces;
use crate::viewport::tool::{ToolSettings, ViewportTool};
use crate::viewport::world::ViewportWorldPoint;

/// The frame resolver the viewport, the gizmos and the play backend resolve positions through:
/// the world frame ([`WorldFrame`]). A viewport showing a plugin's world uses that world's
/// frames instead ([`crate::viewport::world::ViewportWorld::frames`]).
pub fn default_frames() -> Arc<dyn FrameResolver> {
    Arc::new(WorldFrame)
}

/// State every viewport shares (session state): the active tool and the tool settings
/// the toolbar and the brush panel edit.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewportShared {
    pub tool: String,
    pub settings: ToolSettings,
    revision: u64,
}

impl Default for ViewportShared {
    fn default() -> Self {
        Self {
            tool: "forge.tool.move".into(),
            settings: ToolSettings::default(),
            revision: 0,
        }
    }
}

impl ViewportShared {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Change something; bumps the revision when it changed.
    pub fn update(&mut self, f: impl FnOnce(&mut Self)) {
        let before = self.clone();
        f(self);
        if *self != before {
            self.revision = before.revision + 1;
        }
    }
}

/// The named budgets the profiler shows (the perf gate's file, Ch.29).
pub const BUDGETS_RON: &str = include_str!("../../../tests/perf/budgets.ron");
/// The device class whose allowances the profiler shows (the dev box's).
pub const BUDGET_CLASS: &str = "rtx3080";

forge_trace::control_switches! {
    /// W2 positive-control switches for the WP-U5 guards. Never set outside those tests.
    #[doc(hidden)]
    #[derive(Clone, Debug, Default)]
    pub struct PanelFaults {
        /// The inspector builds no editor for rows of this kind.
        pub inspector_skip_kind: Option<PinKind>,
        /// The hierarchy realises every row (bypasses virtualisation).
        pub hierarchy_no_virtualisation: bool,
        /// A dismissed first-run tip keeps animating (the `C-first-run-tips` control).
        pub tip_keeps_timer: bool,
        /// The hierarchy rebuilds every row on each mirror change while a filter is active.
        pub hierarchy_filter_rebuilds: bool,
        /// The hierarchy applies a filter in one loop turn instead of bounded slices.
        pub hierarchy_filter_unbounded: bool,
        /// The hierarchy's filter job snapshots every row / every root at a phase change (an
        /// O(n) step no slice deadline can interrupt) instead of walking them by cursor.
        pub hierarchy_filter_snapshots: bool,
        /// The hierarchy edits its tree (a repaint and an accessibility rebuild) on every mirror
        /// change, even one no row shows (`test_hierarchy_hidden_edits_no_damage`'s control).
        pub hierarchy_edit_every_change: bool,
        /// A probe, not a fault: when set, the hierarchy's filter job records here the most
        /// items it touched between two deadline checks (`test_hierarchy_filter_bounded`).
        pub hierarchy_step_probe: Option<Rc<std::cell::Cell<usize>>>,
        /// The inspector passes integer edits through `f64` (loses bits above 2^53).
        pub inspector_int_through_f64: bool,
        /// The console rebuilds every row on each new log line.
        pub console_rebuild_every_line: bool,
        /// The tile map paints each cell as its own command (no gesture): a stroke is many
        /// undo entries.
        pub tile_paint_without_gesture: bool,
        /// The tile-map canvas paints every cell of the map, not only the visible ones.
        pub tilemap_paint_every_cell: bool,
        /// The 2D editors re-read the whole `d2.` namespace (every chunk of every map) on each
        /// change instead of applying the changed keys.
        pub d2_full_reread: bool,
        /// Each step of a tile paint stroke re-sends every chunk the stroke touched so far
        /// instead of the chunks that step dirtied (O(n²) commands over a long stroke).
        pub d2_stroke_resends_all: bool,
        /// The preset switcher (and a plugin's lossy dialog) send a lossy change without showing
        /// what is lost and asking first (`test_lossy_warnings_present`'s control).
        pub lossy_warning_skipped: bool,
        /// The launcher lists the built-in templates only, as before WP-21 (a preset a plugin
        /// provides never reaches the new-project flow): `test_plugin_presets`' control.
        pub launcher_builtin_templates_only: bool,
        /// A probe, not a fault: when set, the 2D editors add here the chunks they decode or
        /// copy while bringing their model up to date (`test_tile_stroke_bounded`).
        pub d2_model_probe: Option<Rc<std::cell::Cell<usize>>>,
        /// The viewport redraws every display frame (a render loop instead of redraw on
        /// change): `test_viewport`'s idle control.
        pub viewport_redraw_always: bool,
        /// Stop writes the simulation's transforms back into the project (a play-in-editor
        /// that is not a sandbox): `test_play_controls`' control.
        pub play_writes_project: bool,
        /// The profiler refreshes at 60 Hz instead of at most 10: `test_profiler_panel`'s
        /// control.
        pub profiler_uncapped: bool,
        /// The preset switcher (and a plugin's dialog on the same view) rebuild their view of the project
        /// (every entity, every rule setting) on each mirror change instead of applying the
        /// change logs: `test_project_panels_idle`'s edit-cost control.
        pub project_view_full_rebuild: bool,
        /// A probe, not a fault: when set, the preset switcher (and a plugin's dialog) add
        /// here the settings and entity properties they read, and the entities they plan over,
        /// while following the project (`test_project_panels_idle`).
        pub project_view_probe: Option<Rc<std::cell::Cell<usize>>>,
        /// The graph editor connects pins without checking their types and units:
        /// `test_graph_editor`'s unit-check control.
        pub graph_skips_unit_check: bool,
        /// The graph editor sends a multi-command edit (a paste, a delete, a reroute) as
        /// separate transactions: `test_graph_editor`'s single-undo control.
        pub graph_edits_not_grouped: bool,
        /// The sequencer sends each frame of a key drag as its own transaction (no gesture):
        /// `test_timeline_edits_undoable`'s control (WP-U11).
        pub timeline_drag_without_gesture: bool,
        /// The sequencer's preview writes the sampled values into the project (a preview that
        /// is not a sandbox): `test_timeline_preview_sandboxed`'s control (WP-U11).
        pub timeline_preview_writes_project: bool,
        /// The timeline asks for animation frames while nothing plays (it never lets the editor
        /// idle): `test_authoring_panels_idle`'s control (WP-U11).
        pub timeline_always_animating: bool,
        /// Pausing the sequencer leaves its timeline marked as playing (`set_playing` forces
        /// `playing = true`): `test_authoring_panels_idle`'s play-then-pause control (WP-U14).
        pub timeline_pause_keeps_playing: bool,
        /// The sequencer re-shows every track (and the clip list) on each change instead of the
        /// tracks the change log touched: `test_sequencer_drag_cost`'s control (WP-U14).
        pub sequencer_full_show: bool,
        /// The timeline paints every key of every visible track, in view or not and however
        /// dense: `test_sequencer_drag_cost`'s paint control (WP-U14).
        pub timeline_paint_every_key: bool,
        /// A probe, not a fault: when set, the sequencer adds here the timeline rows it builds
        /// (`test_sequencer_drag_cost`).
        pub sequencer_rows_probe: Option<Rc<std::cell::Cell<usize>>>,
        /// The revision history panel's sync step asks for another loop turn every turn,
        /// changed or not (`the_lifecycle_panels_are_idle_when_nothing_changes`'s positive
        /// control: an idle editor that never sleeps; L-02).
        pub history_always_want_turn: bool,
    }
}

/// The graph library over the built-in nodes, a generator backend's stages (if one is loaded)
/// and a component catalogue's types (empty if the built-ins fail to register, which a unit
/// test rules out; a backend whose registry fails adds no stages).
fn graph_library(
    c: &ComponentCatalog,
    generator: Option<&dyn crate::graph::ir::GeneratorBackend>,
) -> crate::graph::library::NodeLibrary {
    let stages = generator.and_then(|g| g.registry().ok());
    let mut extra = vec![c.registry()];
    extra.extend(stages.as_ref());
    crate::graph::library::NodeLibrary::builtin(&extra).unwrap_or_default()
}

/// Services a plugin adds beside the editor's own, one per type: what a plugin that hosts
/// something over the core (a protocol server and the state its panel reads) gives its
/// panels, which find it by its type. The editor never names what is here; a panel that
/// finds nothing says so. Filled by whoever assembles the editor (the binary), before the
/// shell is built; panels read it on the UI thread.
#[derive(Default)]
pub struct ServiceExtensions {
    map: RefCell<std::collections::BTreeMap<std::any::TypeId, Rc<dyn std::any::Any>>>,
}

impl ServiceExtensions {
    /// Add `value` (replacing one of the same type, which is returned).
    pub fn insert<T: std::any::Any>(&self, value: Rc<T>) -> Option<Rc<T>> {
        self.map
            .borrow_mut()
            .insert(std::any::TypeId::of::<T>(), value)
            .and_then(|old| old.downcast::<T>().ok())
    }

    /// The service of type `T`, if one was added.
    #[must_use]
    pub fn get<T: std::any::Any>(&self) -> Option<Rc<T>> {
        let v = self
            .map
            .borrow()
            .get(&std::any::TypeId::of::<T>())
            .cloned()?;
        v.downcast::<T>().ok()
    }

    /// How many services were added.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.borrow().len()
    }

    /// True when none was added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for ServiceExtensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceExtensions")
            .field("len", &self.len())
            .finish()
    }
}

/// Adds a plugin's services beside the editor's own: run once by `shell::assemble_hosted`
/// after the services are built (in registry order), it typically puts what the plugin's
/// panels read into [`EditorServices::extensions`].
pub type ServiceProvider = Arc<dyn Fn(&mut EditorServices) + Send + Sync>;

/// The `ServiceProvider` extension point (`forge.editor.service_provider`): how a plugin
/// gives its panels services the editor itself does not name (see [`ServiceProvider`]).
pub struct ServiceProviderPoint;

impl forge_plugin::ExtensionPoint for ServiceProviderPoint {
    type Item = ServiceProvider;
    const ID: &'static str = "forge.editor.service_provider";
    const NAME: &'static str = "ServiceProvider";
}

/// See the module docs.
pub struct EditorServices {
    pub components: Rc<ComponentCatalog>,
    pub inspector_widgets: Rc<InspectorWidgets>,
    pub log: Rc<RefCell<ConsoleLog>>,
    /// `None`: no asset database (the browser shows why). Attach one with
    /// [`EditorServices::set_assets`] so the 2D editors read source images through it.
    pub assets: Option<Rc<RefCell<dyn AssetCatalog>>>,
    /// The 2D pipeline the 2D editors ask: `forge-2d` ([`Forge2dScene`], M4-11).
    pub scene2d: Rc<dyn Scene2d>,
    /// The audio engine the mixer meters from (in-memory until `forge-audio`).
    pub audio: Rc<dyn AudioBuses>,
    /// The input layer the input-map editor asks (in-memory until `forge-play`, M5-8).
    pub input: Rc<dyn InputActions>,
    /// The animation library the sequencer and the state machine editor ask (WP-U11;
    /// in-memory until `forge-anim`).
    pub anim: Rc<dyn crate::authoring::anim::AnimSource>,
    /// The string runtime the localisation editor compiles for (in-memory until
    /// `forge-play`, M5-8).
    pub strings: Rc<dyn crate::authoring::strings::StringTables>,
    /// The sequencer's preview on the play core (session state): the viewport draws its
    /// overrides while no play session runs.
    pub anim_preview: Rc<RefCell<crate::authoring::preview::TimelinePreview>>,
    /// The frame resolver positions are resolved through (the world frame by default).
    pub frames: Arc<dyn FrameResolver>,
    /// The play core the shell runs the play controls on: WP-13's `forge-sim` session
    /// ([`SimPlay`]) by default.
    pub play: Rc<RefCell<dyn PlayBackend>>,
    /// The same play core as [`SimPlay`], for what the trait does not carry (the session's
    /// recording, simulation inputs, the checkpoint cadence); `None` when `play` is another
    /// backend.
    pub play_core: Option<Rc<RefCell<SimPlay>>>,
    /// What the profiler reads: `forge-trace`'s live feed ([`TraceCounters`] over `tracer`)
    /// by default.
    pub profiler: Arc<dyn CounterSource>,
    /// The tracer producers report to (zones, counters, frame marks, adapter lanes): the
    /// process-wide one, which the default `profiler` reads and `play` reports to.
    pub tracer: &'static forge_trace::Tracer,
    /// The UI's own counters (`ui.self`): shown, never waking the loop.
    pub ui_self: Arc<SelfUiCounters>,
    /// The `ViewportTool` registry (filled by the loader in `assemble`).
    pub viewport_tools: Rc<Registry<ViewportTool>>,
    /// The `ViewportWorld` registry (filled by the loader in `assemble`): the viewport panels
    /// make their world from its first item (none: the project scene only).
    pub viewport_worlds: Rc<Registry<ViewportWorldPoint>>,
    /// Render surfaces a GPU scene renderer attaches to (the binary's host).
    pub viewport_surfaces: Rc<RefCell<ViewportSurfaces>>,
    /// The active tool and tool settings every viewport shares.
    pub viewport: Rc<RefCell<ViewportShared>>,
    /// The promotion rules plugins add (`PromotionRule`, filled by `assemble`): the preset
    /// switcher offers their presets and plans with them.
    pub promotion_rules: Rc<crate::project::promote::PromotionRules>,
    /// The layers plugins keep over the scene (one set for every viewport; filled by
    /// `assemble` from the `ViewportLayer` registry).
    pub viewport_layers: Rc<RefCell<crate::viewport::layer::ViewportLayers>>,
    /// The launcher's recent projects and default projects folder (user config, WP-U7).
    pub launcher: Rc<RefCell<crate::project::recent::LauncherState>>,
    /// The graph IR the graph editor compiles through: `forge-graph`
    /// ([`ForgeGraphIr`](crate::graph::ir::ForgeGraphIr), WP-23).
    pub graph: Rc<dyn crate::graph::ir::GraphIr>,
    /// The graph editor's nodes: every `#[forge_api]` item the editor knows (the built-in
    /// nodes and the component catalogue's types).
    pub graph_library: Rc<crate::graph::library::NodeLibrary>,
    /// The generator backend (Ch.13) a plugin provides through
    /// [`GeneratorBackendPoint`](crate::graph::ir::GeneratorBackendPoint), if one loaded: its
    /// stages are in [`Self::graph_library`] and the graph editor lowers generator graphs
    /// through it. `None`: generator graphs compile but do not lower.
    pub generator: Option<Arc<dyn crate::graph::ir::GeneratorBackend>>,
    /// The connect panels' services (WP-U9): plugin manager, audit, remote,
    /// compute. `assemble` fills the plugin manager from the load; a shell over a core
    /// follows it with [`ConnectServices::attach`](crate::connect::ConnectServices::attach).
    pub connect: crate::connect::ConnectServices,
    /// The collaboration panels' services (WP-U10): the identity database, the team server
    /// and the licence, read-only plus session state; attached with
    /// [`CollabServices::attach`](crate::collab::CollabServices::attach) after
    /// `EditorCore::attach_collab`.
    pub collab: crate::collab::CollabServices,
    /// The running editor's WASM plugins (WP-21): the plugin directories, hot reload, and
    /// the registries the shell does not hold (commands, presets, importers). `assemble`
    /// fills it; the shell polls it.
    pub hosting: Rc<RefCell<crate::hosting::PluginHosting>>,
    /// Services plugins added beside these ([`ServiceExtensions`]).
    pub extensions: ServiceExtensions,
    /// The links plugins added beside settings rows (the `SettingLink` point, WP-47; filled
    /// by the loader in `assemble`): the Settings window shows them next to their rows.
    pub setting_links: Rc<crate::settings::SettingLinks>,
    /// The hints plugins added to problems (the `NoticeHint` point, WP-47; filled by the
    /// loader in `assemble`): the notification centre applies them to what it posts.
    pub notice_hints: Rc<crate::notify::NoticeHints>,
    #[doc(hidden)]
    pub faults: PanelFaults,
}

impl std::fmt::Debug for EditorServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorServices")
            .field("components", &self.components)
            .field("inspector_widgets", &self.inspector_widgets.len())
            .field("assets", &self.assets.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for EditorServices {
    /// The built-in components, no inspector widgets, an empty log and no asset database.
    fn default() -> Self {
        let tracer = forge_trace::global();
        // The committed budget file always parses (a unit test holds it); were it ever
        // broken, the profiler would show no allowances rather than fail the editor.
        let _ = tracer.declare_budgets_ron(BUDGETS_RON, BUDGET_CLASS);
        let play = Rc::new(RefCell::new(SimPlay::with_tracer(tracer)));
        let components = Rc::new(ComponentCatalog::editor().unwrap_or_default());
        Self {
            components: components.clone(),
            inspector_widgets: Rc::new(InspectorWidgets::new()),
            log: Rc::new(RefCell::new(ConsoleLog::new())),
            assets: None,
            scene2d: Rc::new(Forge2dScene::new()),
            audio: Rc::new(MemoryAudio::new()),
            input: Rc::new(MemoryInput::new()),
            anim: Rc::new(crate::authoring::anim::MemoryAnim::new()),
            strings: Rc::new(crate::authoring::strings::MemoryStrings),
            anim_preview: Rc::new(RefCell::new(
                crate::authoring::preview::TimelinePreview::new(),
            )),
            frames: default_frames(),
            play: play.clone(),
            play_core: Some(play),
            profiler: Arc::new(TraceCounters::of(tracer)),
            tracer,
            ui_self: Arc::new(SelfUiCounters::new()),
            viewport_tools: Rc::new(Registry::new()),
            viewport_worlds: Rc::new(Registry::new()),
            viewport_surfaces: Rc::new(RefCell::new(ViewportSurfaces::new())),
            viewport: Rc::new(RefCell::new(ViewportShared::default())),
            viewport_layers: Rc::new(RefCell::new(crate::viewport::layer::ViewportLayers::new())),
            promotion_rules: Rc::default(),
            launcher: Rc::new(RefCell::new(
                crate::project::recent::LauncherState::default(),
            )),
            graph: Rc::new(crate::graph::ir::ForgeGraphIr),
            graph_library: Rc::new(graph_library(&components, None)),
            generator: None,
            connect: crate::connect::ConnectServices::default(),
            collab: crate::collab::CollabServices::default(),
            hosting: Rc::new(RefCell::new(crate::hosting::PluginHosting::default())),
            extensions: ServiceExtensions::default(),
            setting_links: Rc::new(Registry::new()),
            notice_hints: Rc::new(crate::notify::NoticeHints::default()),
            faults: PanelFaults::default(),
        }
    }
}

impl EditorServices {
    /// The core the open project's trust is read from and answered on (WP-34,
    /// [`crate::trust`]): the one the connect services follow, else the one the plugin
    /// hosting runs over. `None` for an editor that is a device of another's core.
    #[must_use]
    pub fn trust_core(&self) -> Option<crate::core::SharedCore> {
        self.connect
            .core
            .clone()
            .or_else(|| self.hosting.borrow().core().cloned())
    }

    /// With an asset catalogue (see [`EditorServices::set_assets`]).
    pub fn with_assets(mut self, a: Rc<RefCell<dyn AssetCatalog>>) -> Self {
        self.set_assets(a);
        self
    }

    /// Attach the asset catalogue, and give the 2D pipeline the project's files through it:
    /// image sizes in the 2D editors then come from the PNG / Aseprite headers of the files
    /// in the project (Ch.35 §35.5), not a guess. Replaces [`EditorServices::scene2d`] with a
    /// [`Forge2dScene`] reading through `a`.
    pub fn set_assets(&mut self, a: Rc<RefCell<dyn AssetCatalog>>) {
        let catalog = Rc::downgrade(&a);
        let read: crate::domain::scene2d::SourceReader = Rc::new(move |path: &str| {
            // A catalogue busy with a write (staging, following) answers nothing this time;
            // the editor falls back as it does for an unreadable file, never panics.
            let c = catalog.upgrade()?;
            let c = c.try_borrow().ok()?;
            c.source(path)
        });
        self.scene2d = Rc::new(Forge2dScene::with_source(read));
        self.assets = Some(a);
    }
    /// With a generator backend (its stages join the graph library).
    pub fn with_generator(mut self, g: Arc<dyn crate::graph::ir::GeneratorBackend>) -> Self {
        self.graph_library = Rc::new(graph_library(&self.components, Some(g.as_ref())));
        self.generator = Some(g);
        self
    }
    /// With a component catalogue (its types join the graph library).
    pub fn with_components(mut self, c: ComponentCatalog) -> Self {
        self.graph_library = Rc::new(graph_library(&c, self.generator.as_deref()));
        self.components = Rc::new(c);
        self
    }
    /// Run the play controls on `core` (a play core with its own tracer or fault switches).
    pub fn with_play_core(mut self, core: SimPlay) -> Self {
        let core = Rc::new(RefCell::new(core));
        self.play = core.clone();
        self.play_core = Some(core);
        self
    }
}
