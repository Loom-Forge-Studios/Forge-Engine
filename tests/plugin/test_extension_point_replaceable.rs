//! Ch.32.2 — `test_extension_point_replaceable`: every extension point supports `add`,
//! `replace`, `remove` and `chain`, exercised, with each item observed by **using** it —
//! a panel is built, a command is dry-run on a real bus, a store backend is opened — so a
//! registry cannot pass by bookkeeping alone.
//!
//! Coverage: every point the host defines is checked, and every catalog point whose item
//! type lives in `forge-plugin`, `forge-store`, `forge-asset` or `forge-phys` is defined by the
//! host (a new point defined there without a kit fails here). The asset points are observed by
//! use: an importer imports a file from an in-memory project, an exporter exports, a type
//! decodes; a physics backend (WP-60) builds a world that steps a falling body.
//!
//! It also carries Ch.21.19's `test_panel_extension_replaceable`: through the ordinary
//! loader, a test plugin replaces the hierarchy panel, chains the inspector and removes the
//! console that a first-party panels plugin provides.
//!
//! Positive control (W2): `positive_control_every_broken_registry_is_caught` runs the check
//! on registries that each break one operation (ignored order, a replace that moves or is
//! ignored, a remove that takes the wrong item, a chain that drops its wrapper, a silent
//! last-wins replace); each must be caught, by the clause that names it, for every point,
//! and a faithful model of the contract must pass.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use forge_asset::{
    AssetError, AssetId, AssetTypeItem, AssetTypePoint, ExportArtefact, ExportSource, Exporter,
    ExporterPoint, ImportCx, Importer, ImporterPoint, Settings, Vfs, dry_import,
};
use forge_cmd::{
    Bus, Change, CmdError, CommandHandler, CommandSink, DiffBuilder, EditorCommand, Issuer, Value,
};
use forge_editor::actions::{Action, ActionCx, ActionItem, ActionKind};
use forge_editor::graph::ir::{
    Diagnostic, GeneratorBackend, GeneratorBackendPoint, LoweredGenerator,
};
use forge_editor::graph::library::NodeLibrary;
use forge_editor::notify::{
    HintMatch, NoticeHint, NoticeHintPoint, NoticeHints, Problem, Severity,
};
use forge_editor::overlay::{EditorOverlay, OverlayDescriptor, ThemeItem, ThemePoint};
use forge_editor::services::{EditorServices, ServiceProvider, ServiceProviderPoint};
use forge_editor::settings::{SettingLink, SettingLinkPoint};
use forge_editor::viewport::tool::{
    ToolBehavior, ToolCategory, ToolCx, ToolInput, ToolReply, ToolView, ViewportTool,
    ViewportToolItem,
};
use forge_editor::viewport::world::{
    CellCameras, ViewportWorld, ViewportWorldFactory, ViewportWorldPoint, WorldCell, WorldLaunch,
    WorldStep,
};
use forge_graph::Graph;
use forge_plugin::conformance::{Kit, RegistryOps, Rule, Violation, check_replaceable};
use forge_plugin::points::{
    Command, CommandItem, Dock, EditorPanel, InspectorWidget, InspectorWidgetDescriptor,
    PanelDescriptor, Preset, PresetDescriptor, PresetKind, SEED_POINTS, WidgetTarget,
    install_commands,
};
use forge_plugin::{
    ExtensionPoint, Extensions, Grants, InstallCx, ItemId, Manifest, Order, PluginError, PluginId,
    Registry, Replaced, SourcePlugin, loader,
};
use forge_store::{
    BackendDescriptor, Bytes, MemoryStore, ProjectStore, StoreBackend, StoreError, StorePath,
};

/// The editor's panel context, as a test stand-in: panels append what they build.
#[derive(Default)]
struct Cx(Vec<String>);

// ---- kits: make an item tagged `t`, observe it by using it, wrap it ------------------------

fn panel(t: &str) -> PanelDescriptor<Cx> {
    let t = t.to_string();
    PanelDescriptor {
        title: format!("title-{t}"),
        icon: None,
        default_dock: Dock::Left,
        build: Arc::new(move |cx: &mut Cx| cx.0.push(t.clone())),
    }
}

fn built(build: &(dyn Fn(&mut Cx) + Send + Sync)) -> String {
    let mut cx = Cx::default();
    build(&mut cx);
    cx.0.concat()
}

fn wrap_build(inner: Arc<dyn Fn(&mut Cx) + Send + Sync>) -> Arc<dyn Fn(&mut Cx) + Send + Sync> {
    Arc::new(move |cx: &mut Cx| {
        cx.0.push("wrap(".into());
        inner(cx);
        cx.0.push(")".into());
    })
}

fn panel_kit() -> Kit<EditorPanel<Cx>> {
    Kit {
        make: panel,
        probe: |p| built(&*p.build),
        wrap: |p| PanelDescriptor {
            build: wrap_build(p.build),
            ..p
        },
    }
}

fn widget_kit() -> Kit<InspectorWidget<Cx>> {
    Kit {
        make: |t| {
            let t = t.to_string();
            InspectorWidgetDescriptor {
                applies_to: WidgetTarget::Attribute("color".into()),
                build: Arc::new(move |cx: &mut Cx| cx.0.push(t.clone())),
            }
        },
        probe: |w| built(&*w.build),
        wrap: |w| InspectorWidgetDescriptor {
            build: wrap_build(w.build),
            ..w
        },
    }
}

struct Handler(Arc<dyn CommandHandler>);
impl CommandHandler for Handler {
    fn plan(&self, b: &mut DiffBuilder<'_>, a: &serde_json::Value) -> Result<(), CmdError> {
        self.0.plan(b, a)
    }
}

fn command_kit() -> Kit<Command> {
    Kit {
        make: |t| {
            let t = t.to_string();
            CommandItem::new(move |b, _| b.set_setting("probe", Some(Value::Text(t.clone()))))
        },
        // Dry-run the command on a real bus and read what it would set.
        probe: |item| {
            let mut bus = Bus::new();
            if bus
                .register_handler("probe.cmd", item.policy, Handler(Arc::clone(&item.handler)))
                .is_err()
            {
                return "<register failed>".into();
            }
            let e = bus.envelope(
                Issuer::Test,
                EditorCommand::Invoke {
                    target: "probe.cmd".into(),
                    args: "{}".into(),
                },
            );
            match bus.dry_run(&e) {
                Ok(d) => match d.changes.last() {
                    Some(Change::Setting {
                        after: Some(Value::Text(s)),
                        ..
                    }) => s.clone(),
                    other => format!("<{other:?}>"),
                },
                Err(r) => format!("<{r}>"),
            }
        },
        wrap: |item| {
            item.wrap(|inner, b, args| {
                inner.plan(b, args)?;
                let seen = match b.setting("probe") {
                    Some(Value::Text(s)) => s,
                    _ => String::new(),
                };
                b.set_setting("probe", Some(Value::Text(format!("wrap({seen})"))))
            })
        },
    }
}

fn preset_kit() -> Kit<Preset> {
    Kit {
        make: |t| PresetDescriptor {
            label: t.into(),
            kind: PresetKind::TwoD,
            defaults: BTreeMap::new(),
            files: BTreeMap::new(),
        },
        probe: |p| p.label.clone(),
        wrap: |p| PresetDescriptor {
            label: format!("wrap({})", p.label),
            ..p
        },
    }
}

/// Ch.21.19's `Action` point (defined in forge-editor): an action is observed by running
/// its command builder and reading the command it builds.
fn action_kit() -> Kit<Action> {
    fn probe_text(cmds: &[EditorCommand]) -> String {
        match cmds.first() {
            Some(EditorCommand::SetSetting {
                value: Some(Value::Text(s)),
                ..
            }) => s.clone(),
            other => format!("<{other:?}>"),
        }
    }
    Kit {
        make: |t| {
            let t = t.to_string();
            ActionItem::new(
                "probe",
                "Test",
                ActionKind::Command(Arc::new(move |_cx: &ActionCx| {
                    vec![EditorCommand::SetSetting {
                        key: "probe".into(),
                        value: Some(Value::Text(t.clone())),
                    }]
                })),
            )
        },
        probe: |a| match &a.kind {
            ActionKind::Command(build) => probe_text(&build(&ActionCx::default())),
            other => format!("<{other:?}>"),
        },
        wrap: |a| {
            let inner = a.kind.clone();
            ActionItem {
                kind: ActionKind::Command(Arc::new(move |cx: &ActionCx| {
                    let seen = match &inner {
                        ActionKind::Command(b) => probe_text(&b(cx)),
                        other => format!("<{other:?}>"),
                    };
                    vec![EditorCommand::SetSetting {
                        key: "probe".into(),
                        value: Some(Value::Text(format!("wrap({seen})"))),
                    }]
                })),
                ..a
            }
        },
    }
}

// ---- Ch.21.19's EditorOverlay, Theme and ViewportTool points (WP-U6) -----------------------

/// An overlay is observed by building it (into the test context).
fn overlay_kit() -> Kit<EditorOverlay<Cx>> {
    Kit {
        make: |t| {
            let t = t.to_string();
            OverlayDescriptor {
                title: "probe".into(),
                order: 0,
                build: Arc::new(move |cx: &mut Cx| cx.0.push(t.clone())),
            }
        },
        probe: |o| built(&*o.build),
        wrap: |o| OverlayDescriptor {
            build: wrap_build(o.build),
            ..o
        },
    }
}

/// A theme is observed by the token set it hands the UI (its name is a token).
fn theme_kit() -> Kit<ThemePoint> {
    Kit {
        make: |t| {
            let mut tokens = forge_ui::Theme::dark();
            tokens.name = t.to_string();
            ThemeItem::new("probe", tokens)
        },
        probe: |t| t.tokens.name.clone(),
        wrap: |t| {
            let mut tokens = (*t.tokens).clone();
            tokens.name = format!("wrap({})", tokens.name);
            ThemeItem {
                tokens: Arc::new(tokens),
                ..t
            }
        },
    }
}

/// A tool whose status line is its tag.
struct TagTool(String);
impl ToolBehavior for TagTool {
    fn input(&mut self, _cx: &mut ToolCx<'_>, _ev: &ToolInput) -> ToolReply {
        ToolReply::Ignored
    }
    fn status(&self, _view: &ToolView<'_>) -> Option<String> {
        Some(self.0.clone())
    }
}

/// Middleware: a tool that forwards to the wrapped one and marks its status.
struct WrapTool(Box<dyn ToolBehavior>);
impl ToolBehavior for WrapTool {
    fn input(&mut self, cx: &mut ToolCx<'_>, ev: &ToolInput) -> ToolReply {
        self.0.input(cx, ev)
    }
    fn status(&self, view: &ToolView<'_>) -> Option<String> {
        Some(format!("wrap({})", self.0.status(view).unwrap_or_default()))
    }
}

/// A viewport tool is observed by making a behaviour from its factory and asking it for its
/// status over a real (empty) viewport.
fn tool_kit() -> Kit<ViewportTool> {
    Kit {
        make: |t| {
            let t = t.to_string();
            ViewportToolItem::new("probe", ToolCategory::Measure, "P", move || {
                Box::new(TagTool(t.clone()))
            })
        },
        probe: |item| {
            let tool = (item.make)();
            let cam = forge_editor::viewport::camera::EditorCamera::new(forge_frames::FrameId(0));
            let scene = forge_editor::viewport::scene::ViewportScene::new();
            let mirror = forge_editor::mirror::ProjectMirror::new();
            let frames = forge_editor::services::default_frames();
            let settings = forge_editor::viewport::tool::ToolSettings::default();
            let layers = forge_editor::viewport::layer::ViewportLayers::new();
            let view = ToolView {
                camera: &cam,
                size: (640.0, 480.0),
                scene: &scene,
                frames: frames.as_ref(),
                tick: forge_frames::Tick(0),
                selection: &[],
                settings: &settings,
                layers: &layers,
                mirror: &mirror,
            };
            tool.status(&view).unwrap_or_default()
        },
        wrap: |item| {
            let inner = item.make;
            ViewportToolItem {
                make: Arc::new(move || Box::new(WrapTool(inner())) as Box<dyn ToolBehavior>),
                ..item
            }
        },
    }
}

/// A viewport world that shows nothing, named by its tag (the `ViewportWorld` point: a
/// plugin's world factory). Observed by making one and reading its name back.
struct TagWorld(String);

/// Middleware: a world that wraps another, everything delegated.
struct WrapWorld(Box<dyn ViewportWorld>);

fn world_name(w: &dyn ViewportWorld) -> String {
    let any = w.as_any();
    if let Some(t) = any.downcast_ref::<TagWorld>() {
        return t.0.clone();
    }
    match any.downcast_ref::<WrapWorld>() {
        Some(wr) => format!("wrap({})", world_name(wr.0.as_ref())),
        None => "?".into(),
    }
}

/// The world a test world delegates to (none for a tag).
trait Inner {
    fn inner(&self) -> Option<&dyn ViewportWorld>;
}

impl Inner for TagWorld {
    fn inner(&self) -> Option<&dyn ViewportWorld> {
        None
    }
}

impl Inner for WrapWorld {
    fn inner(&self) -> Option<&dyn ViewportWorld> {
        Some(self.0.as_ref())
    }
}

macro_rules! delegate_world {
    ($t:ty) => {
        impl ViewportWorld for $t {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn shown(&self) -> bool {
                self.inner().map_or(false, |w| w.shown())
            }
            fn frames(&self) -> Option<Arc<dyn forge_frames::FrameResolver>> {
                self.inner().and_then(|w| w.frames())
            }
            fn tick(&self) -> forge_frames::Tick {
                self.inner().map_or(forge_frames::Tick(0), |w| w.tick())
            }
            fn focus(
                &mut self,
                _cams: &mut dyn CellCameras,
                _cell: usize,
                _seed_path: &str,
                _name: &str,
            ) -> WorldLaunch {
                WorldLaunch::Nothing
            }
            fn poll(&mut self, _cams: &mut dyn CellCameras) -> Option<usize> {
                None
            }
            fn moving(&self, _cell: usize) -> bool {
                false
            }
            fn pending(&self, _cell: usize) -> bool {
                false
            }
            fn step(&mut self, _cams: &mut dyn CellCameras, _dt: f64) -> WorldStep {
                WorldStep {
                    done: true,
                    ..WorldStep::default()
                }
            }
            fn cancel(&mut self) -> bool {
                false
            }
            fn leave(&mut self) {}
            fn cell_hidden(&mut self, _cell: usize) {}
            fn paint(&self, _cx: &mut forge_ui::widget::PaintCx, _cell: &WorldCell) {}
            fn feed(&self) -> Arc<forge_ui::LiveCell> {
                forge_ui::LiveCell::new()
            }
            fn busy(&self) -> bool {
                false
            }
        }
    };
}

delegate_world!(TagWorld);
delegate_world!(WrapWorld);

/// A viewport world factory is observed by making a world and reading its name.
fn world_kit() -> Kit<ViewportWorldPoint> {
    Kit {
        make: |t| {
            let t = t.to_string();
            Arc::new(move |_s: &Rc<EditorServices>| {
                Box::new(TagWorld(t.clone())) as Box<dyn ViewportWorld>
            }) as ViewportWorldFactory
        },
        probe: |f| world_name(f(&Rc::new(EditorServices::default())).as_ref()),
        wrap: |f| {
            Arc::new(move |s: &Rc<EditorServices>| {
                Box::new(WrapWorld(f(s))) as Box<dyn ViewportWorld>
            }) as ViewportWorldFactory
        },
    }
}

/// What a test service provider leaves in the services: its tag.
struct ProvidedTag(String);

/// A service provider is observed by running it over fresh services and reading what it
/// added.
fn provider_kit() -> Kit<ServiceProviderPoint> {
    Kit {
        make: |t| {
            let t = t.to_string();
            Arc::new(move |s: &mut EditorServices| {
                s.extensions.insert(Rc::new(ProvidedTag(t.clone())));
            }) as ServiceProvider
        },
        probe: |p| {
            let mut s = EditorServices::default();
            p(&mut s);
            s.extensions
                .get::<ProvidedTag>()
                .map_or_else(|| "?".into(), |t| t.0.clone())
        },
        wrap: |p| {
            Arc::new(move |s: &mut EditorServices| {
                p(s);
                let inner = s
                    .extensions
                    .get::<ProvidedTag>()
                    .map_or_else(|| "?".into(), |t| t.0.clone());
                s.extensions
                    .insert(Rc::new(ProvidedTag(format!("wrap({inner})"))));
            }) as ServiceProvider
        },
    }
}

/// A generator backend whose name is its tag (WP-23 x WP-U15's `GeneratorBackend` point: a
/// plugin's backend, keyed by its name). It has no stages and lowers nothing.
struct TagGen(String);

impl GeneratorBackend for TagGen {
    fn name(&self) -> &str {
        &self.0
    }
    fn registry(&self) -> Result<forge_reflect::ForgeRegistry, String> {
        Err(format!("{} has no stages", self.0))
    }
    fn lower(&self, _g: &Graph, _lib: &NodeLibrary) -> Result<LoweredGenerator, Vec<Diagnostic>> {
        Err(Vec::new())
    }
}

/// Middleware: a backend whose name wraps the inner one's, everything else delegated.
struct WrapGen(Arc<dyn GeneratorBackend>, String);

impl GeneratorBackend for WrapGen {
    fn name(&self) -> &str {
        &self.1
    }
    fn registry(&self) -> Result<forge_reflect::ForgeRegistry, String> {
        self.0.registry()
    }
    fn lower(&self, g: &Graph, lib: &NodeLibrary) -> Result<LoweredGenerator, Vec<Diagnostic>> {
        self.0.lower(g, lib)
    }
}

/// A generator backend is observed by its name.
fn generator_kit() -> Kit<GeneratorBackendPoint> {
    Kit {
        make: |t| Arc::new(TagGen(t.to_string())) as Arc<dyn GeneratorBackend>,
        probe: |b| b.name().to_string(),
        wrap: |b| {
            let name = format!("wrap({})", b.name());
            Arc::new(WrapGen(b, name)) as Arc<dyn GeneratorBackend>
        },
    }
}

/// A settings-row link whose panel is its tag, observed by using it: the Settings window's
/// resolution ([`row_link`](forge_editor::settings::row_link)) of a row the editor has no dialog
/// for (WP-47).
fn setting_link_kit() -> Kit<SettingLinkPoint> {
    Kit {
        make: |t| SettingLink {
            panel: t.to_string(),
            label: "Open".into(),
        },
        probe: |l| {
            let mut reg = Registry::<SettingLinkPoint>::new();
            let owner = PluginId::new("forge.probe").unwrap_or_else(|e| panic!("{e}"));
            let _ = reg.add(owner, "project.probe", l.clone(), Order::Last);
            forge_editor::settings::row_link("project.probe", &reg)
                .map(|(panel, _)| panel)
                .unwrap_or_default()
        },
        wrap: |l| SettingLink {
            panel: format!("wrap({})", l.panel),
            ..l
        },
    }
}

/// A viewport layer that says which layer it is (`ViewportLayer` point, WP-44): its tag.
struct TagLayer(String);

impl forge_editor::viewport::layer::ViewportLayer for TagLayer {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn sync(&mut self, _m: &forge_editor::mirror::ProjectMirror) -> bool {
        false
    }
    fn draw(
        &self,
        _view: &forge_editor::viewport::tool::ToolView<'_>,
        _sink: &mut forge_editor::viewport::scene::LineSink<'_>,
    ) {
    }
}

/// A layer factory is observed by making a layer and reading its tag.
fn layer_kit() -> Kit<forge_editor::viewport::layer::ViewportLayerPoint> {
    use forge_editor::viewport::layer::{ViewportLayer, ViewportLayerFactory};
    fn tag(l: &dyn ViewportLayer) -> String {
        l.as_any()
            .downcast_ref::<TagLayer>()
            .map_or_else(|| "?".into(), |t| t.0.clone())
    }
    Kit {
        make: |t| {
            let t = t.to_string();
            Arc::new(move || Box::new(TagLayer(t.clone())) as Box<dyn ViewportLayer>)
                as ViewportLayerFactory
        },
        probe: |f| tag(f().as_ref()),
        wrap: |f| {
            Arc::new(move || {
                let inner = tag(f().as_ref());
                Box::new(TagLayer(format!("wrap({inner})"))) as Box<dyn ViewportLayer>
            }) as ViewportLayerFactory
        },
    }
}

/// A problem hint whose next step is its tag, observed by using it: applied to a problem with
/// a generic next step for its code (WP-47).
fn notice_hint_kit() -> Kit<NoticeHintPoint> {
    Kit {
        make: |t| NoticeHint {
            applies_to: HintMatch::Prefix("PROBE".into()),
            next: Some(t.to_string()),
            open: None,
        },
        probe: |h| {
            let hints = NoticeHints::default().with(h.clone());
            hints
                .apply(Problem::coded(Severity::Error, "PROBE-0001", "x", "y"))
                .next
        },
        wrap: |h| NoticeHint {
            next: h.next.map(|n| format!("wrap({n})")),
            ..h
        },
    }
}

/// A promotion rule whose preset's label is its tag (`PromotionRule` point, WP-44).
struct TagRule(String);

impl forge_editor::project::promote::PromotionRule for TagRule {
    fn preset(&self) -> Option<forge_editor::project::promote::RulePreset> {
        use forge_editor::project::promote::{Cost, RulePreset};
        Some(RulePreset {
            dir: "tagged".into(),
            label: self.0.clone(),
            family: forge_editor::presets::Family::ThreeD,
            enter: (Cost::NonDestructive, String::new()),
            leave: (Cost::NonDestructive, String::new()),
        })
    }
}

/// A promotion rule is observed by its preset's label.
fn rule_kit() -> Kit<forge_editor::project::promote::PromotionRulePoint> {
    use forge_editor::project::promote::PromotionRule;
    fn tag(r: &dyn PromotionRule) -> String {
        r.preset().map_or_else(|| "?".into(), |p| p.label)
    }
    Kit {
        make: |t| Arc::new(TagRule(t.to_string())) as Arc<dyn PromotionRule>,
        probe: |r| tag(r.as_ref()),
        wrap: |r| Arc::new(TagRule(format!("wrap({})", tag(r.as_ref())))) as Arc<dyn PromotionRule>,
    }
}

fn marker() -> StorePath {
    StorePath::new("backend.txt").unwrap_or_else(|e| panic!("{e}"))
}

fn backend_kit() -> Kit<StoreBackend> {
    Kit {
        // A backend whose stores say which backend opened them.
        make: |t| {
            let t = t.to_string();
            BackendDescriptor {
                label: "test".into(),
                open: Arc::new(move |_: &str, who: &str| {
                    let mut s = MemoryStore::new(who);
                    s.write(&marker(), Bytes::from(t.clone().into_bytes()))?;
                    Ok(Box::new(s) as Box<dyn ProjectStore>)
                }),
            }
        },
        // Open a store through it and read the marker.
        probe: |b| match (b.open)("loc", "tester").and_then(|s| s.read(&marker())) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) => format!("<{e}>"),
        },
        // Middleware: a store opened through the chain is the inner store, re-marked.
        wrap: |b| {
            let inner = b.open;
            BackendDescriptor {
                open: Arc::new(move |loc: &str, who: &str| {
                    let mut s = inner(loc, who)?;
                    let seen = String::from_utf8_lossy(&s.read(&marker())?).into_owned();
                    s.write(&marker(), Bytes::from(format!("wrap({seen})").into_bytes()))?;
                    Ok::<_, StoreError>(s)
                }),
                ..b
            }
        },
    }
}

// ---- forge-asset points (Ch.8): observed by importing, exporting and decoding ------------

/// An importer whose main artefact is named `tag`.
struct TagImporter(String);

impl Importer for TagImporter {
    fn version(&self) -> u32 {
        1
    }
    fn extensions(&self) -> &[&str] {
        &["probe"]
    }
    fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError> {
        cx.emit("", "probe", &self.0, cx.source().clone());
        Ok(())
    }
}

/// Middleware: runs the wrapped importer, then renames what it produced.
struct WrapImporter(Arc<dyn Importer>);

impl Importer for WrapImporter {
    fn version(&self) -> u32 {
        self.0.version()
    }
    fn extensions(&self) -> &[&str] {
        self.0.extensions()
    }
    fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError> {
        self.0.import(cx)?;
        for a in cx.artefacts_mut() {
            a.name = format!("wrap({})", a.name);
        }
        Ok(())
    }
}

fn importer_kit() -> Kit<ImporterPoint> {
    Kit {
        make: |t| Arc::new(TagImporter(t.to_string())),
        // Import a real file from a real (in-memory) project and read what came out.
        probe: |i| {
            let path = StorePath::new("probe/x.probe").unwrap_or_else(|e| panic!("{e}"));
            let vfs = Vfs::new(Box::new(MemoryStore::new("tester")));
            if let Err(e) = vfs.write(&path, Bytes::from_static(b"source")) {
                return format!("<{e}>");
            }
            match dry_import(i.as_ref(), &vfs, &path, &Settings::new()) {
                Ok(a) => a.first().map(|a| a.name.clone()).unwrap_or_default(),
                Err(e) => format!("<{e}>"),
            }
        },
        wrap: |i| Arc::new(WrapImporter(i)),
    }
}

struct TagExporter(String);

impl Exporter for TagExporter {
    fn kinds(&self) -> &[&str] {
        &["probe"]
    }
    fn export(
        &self,
        _: &dyn ExportSource,
        _: AssetId,
        _: &str,
    ) -> Result<Vec<(String, Bytes)>, AssetError> {
        Ok(vec![(self.0.clone(), Bytes::new())])
    }
}

struct WrapExporter(Arc<dyn Exporter>);

impl Exporter for WrapExporter {
    fn kinds(&self) -> &[&str] {
        self.0.kinds()
    }
    fn export(
        &self,
        src: &dyn ExportSource,
        root: AssetId,
        name: &str,
    ) -> Result<Vec<(String, Bytes)>, AssetError> {
        let mut out = self.0.export(src, root, name)?;
        for f in &mut out {
            f.0 = format!("wrap({})", f.0);
        }
        Ok(out)
    }
}

struct NoSource;

impl ExportSource for NoSource {
    fn artefact(&self, id: AssetId) -> Result<ExportArtefact, AssetError> {
        Err(AssetError::UnknownAsset(id.to_string()))
    }
}

fn exporter_kit() -> Kit<ExporterPoint> {
    Kit {
        make: |t| Arc::new(TagExporter(t.to_string())),
        probe: |e| match e.export(&NoSource, AssetId(1), "x") {
            Ok(files) => files.first().map(|f| f.0.clone()).unwrap_or_default(),
            Err(err) => format!("<{err}>"),
        },
        wrap: |e| Arc::new(WrapExporter(e)),
    }
}

fn type_kit() -> Kit<AssetTypePoint> {
    Kit {
        // A type whose decoder yields its tag.
        make: |t| {
            let t = t.to_string();
            AssetTypeItem {
                type_name: "String",
                type_id: std::any::TypeId::of::<String>(),
                decode: Arc::new(move |_: &Bytes| {
                    Ok(Arc::new(t.clone()) as Arc<dyn std::any::Any + Send + Sync>)
                }),
            }
        },
        probe: |item| match (item.decode)(&Bytes::new()) {
            Ok(v) => v
                .downcast::<String>()
                .map(|s| (*s).clone())
                .unwrap_or_else(|_| "<not a String>".into()),
            Err(e) => format!("<{e}>"),
        },
        wrap: |item| {
            let inner = item.decode;
            AssetTypeItem {
                decode: Arc::new(move |b: &Bytes| {
                    let v = inner(b)?;
                    let s = v
                        .downcast::<String>()
                        .map_err(|_| "not a String".to_string())?;
                    Ok(Arc::new(format!("wrap({s})")) as Arc<dyn std::any::Any + Send + Sync>)
                }),
                ..item
            }
        },
    }
}

/// A physics backend that is `inner` in everything but its id: the tag the kit observes
/// after building a world through it and stepping a body (the item is used, not inspected).
struct Tagged {
    tag: String,
    inner: Box<dyn forge_phys::PhysicsBackend>,
}

impl forge_phys::PhysicsBackend for Tagged {
    fn id(&self) -> &str {
        &self.tag
    }
    fn set_gravity(&mut self, g: forge_frames::DVec3) {
        self.inner.set_gravity(g);
    }
    fn add_body(&mut self, id: forge_phys::BodyId, d: &forge_phys::BodyDesc) -> Result<(), forge_phys::PhysError> {
        self.inner.add_body(id, d)
    }
    fn remove_body(&mut self, id: forge_phys::BodyId) -> Result<(), forge_phys::PhysError> {
        self.inner.remove_body(id)
    }
    fn add_collider(
        &mut self,
        id: forge_phys::ColliderId,
        b: forge_phys::BodyId,
        d: &forge_phys::ColliderDesc,
    ) -> Result<(), forge_phys::PhysError> {
        self.inner.add_collider(id, b, d)
    }
    fn remove_collider(&mut self, id: forge_phys::ColliderId) -> Result<(), forge_phys::PhysError> {
        self.inner.remove_collider(id)
    }
    fn add_joint(
        &mut self,
        id: forge_phys::JointId,
        d: &forge_phys::JointDesc,
        f: &forge_phys::JointFrames,
    ) -> Result<(), forge_phys::PhysError> {
        self.inner.add_joint(id, d, f)
    }
    fn remove_joint(&mut self, id: forge_phys::JointId) -> Result<(), forge_phys::PhysError> {
        self.inner.remove_joint(id)
    }
    fn state(&self, id: forge_phys::BodyId) -> Result<forge_phys::BodyState, forge_phys::PhysError> {
        self.inner.state(id)
    }
    fn set_state(&mut self, id: forge_phys::BodyId, s: &forge_phys::BodyState) -> Result<(), forge_phys::PhysError> {
        self.inner.set_state(id, s)
    }
    fn set_velocity(
        &mut self,
        id: forge_phys::BodyId,
        l: forge_frames::DVec3,
        a: forge_frames::DVec3,
    ) -> Result<(), forge_phys::PhysError> {
        self.inner.set_velocity(id, l, a)
    }
    fn apply_impulse(
        &mut self,
        id: forge_phys::BodyId,
        l: forge_frames::DVec3,
        a: forge_frames::DVec3,
    ) -> Result<(), forge_phys::PhysError> {
        self.inner.apply_impulse(id, l, a)
    }
    fn set_kinematic_target(
        &mut self,
        id: forge_phys::BodyId,
        t: forge_frames::FramePos,
        r: forge_frames::DQuat,
    ) -> Result<(), forge_phys::PhysError> {
        self.inner.set_kinematic_target(id, t, r)
    }
    fn mass(&self, id: forge_phys::BodyId) -> Result<f64, forge_phys::PhysError> {
        self.inner.mass(id)
    }
    fn is_sleeping(&self, id: forge_phys::BodyId) -> Result<bool, forge_phys::PhysError> {
        self.inner.is_sleeping(id)
    }
    fn step(&mut self, dt: f64, e: &mut Vec<forge_phys::PhysEvent>) -> Result<(), forge_phys::PhysError> {
        self.inner.step(dt, e)
    }
    fn cast_ray(
        &self,
        r: &forge_phys::Ray,
        f: &forge_phys::QueryFilter,
    ) -> Result<Option<forge_phys::Hit>, forge_phys::PhysError> {
        self.inner.cast_ray(r, f)
    }
    fn cast_shape(
        &self,
        c: &forge_phys::ShapeCast,
        f: &forge_phys::QueryFilter,
    ) -> Result<Option<forge_phys::Hit>, forge_phys::PhysError> {
        self.inner.cast_shape(c, f)
    }
    fn overlap(
        &self,
        o: &forge_phys::Overlap,
        f: &forge_phys::QueryFilter,
        out: &mut Vec<forge_phys::ColliderId>,
    ) -> Result<(), forge_phys::PhysError> {
        self.inner.overlap(o, f, out)
    }
    fn contact_count(&self) -> usize {
        self.inner.contact_count()
    }
}

/// The `PhysicsBackend` point (WP-60): a factory whose backend's id is its tag, observed by
/// building a physics world through it and stepping a falling body.
fn phys_backend_kit() -> Kit<forge_phys::PhysicsBackendPoint> {
    Kit {
        make: |t| {
            let tag = t.to_string();
            Arc::new(move |frame: forge_frames::FrameId, s: &forge_phys::PhysicsSettings| {
                let inner = forge_phys::backend::first_party_factory(forge_phys::RAPIER)
                    .expect("this build has rapier3d")(frame, s)?;
                Ok(Box::new(Tagged {
                    tag: tag.clone(),
                    inner,
                }) as Box<dyn forge_phys::PhysicsBackend>)
            }) as forge_phys::BackendFactory
        },
        probe: |f| {
            let frame = forge_frames::FrameId(0);
            let s = forge_phys::PhysicsSettings::default();
            let Ok(b) = f(frame, &s) else {
                return "?".into();
            };
            let mut w = forge_phys::PhysicsWorld::with_backend(frame, s, b);
            let at = forge_frames::FramePos::new(frame, forge_frames::DVec3::new(0.0, 5.0, 0.0));
            let ball = forge_phys::ColliderDesc::new(forge_phys::Shape::Sphere { radius: 0.5 });
            let fell = w
                .add_body(&forge_phys::BodyDesc::dynamic(at))
                .and_then(|id| w.add_collider(id, &ball).map(|_| id))
                .and_then(|id| w.step().map(|()| id))
                .and_then(|id| w.position(id))
                .is_ok_and(|p| p.local.y < 5.0);
            if fell {
                w.backend_id().to_owned()
            } else {
                "?".into()
            }
        },
        wrap: |f| {
            Arc::new(move |frame: forge_frames::FrameId, s: &forge_phys::PhysicsSettings| {
                let inner = f(frame, s)?;
                Ok(Box::new(Tagged {
                    tag: format!("wrap({})", inner.id()),
                    inner,
                }) as Box<dyn forge_phys::PhysicsBackend>)
            }) as forge_phys::BackendFactory
        },
    }
}

fn host() -> Extensions {
    let mut x = Extensions::new();
    for r in [
        x.define::<EditorPanel<Cx>>(),
        x.define::<InspectorWidget<Cx>>(),
        x.define::<Command>(),
        x.define::<Preset>(),
        x.define::<StoreBackend>(),
        x.define::<Action>(),
        x.define::<ImporterPoint>(),
        x.define::<ExporterPoint>(),
        x.define::<AssetTypePoint>(),
        x.define::<EditorOverlay<Cx>>(),
        x.define::<ThemePoint>(),
        x.define::<ViewportTool>(),
        x.define::<ViewportWorldPoint>(),
        x.define::<ServiceProviderPoint>(),
        x.define::<GeneratorBackendPoint>(),
        x.define::<SettingLinkPoint>(),
        x.define::<NoticeHintPoint>(),
        x.define::<forge_editor::viewport::layer::ViewportLayerPoint>(),
        x.define::<forge_editor::project::promote::PromotionRulePoint>(),
        x.define::<forge_phys::PhysicsBackendPoint>(),
    ] {
        r.expect("define");
    }
    x
}

fn all_points(run: &mut dyn FnMut(&'static str, Vec<Violation>)) {
    run(
        EditorPanel::<Cx>::ID,
        check_replaceable(Registry::new, &panel_kit()),
    );
    run(
        InspectorWidget::<Cx>::ID,
        check_replaceable(Registry::new, &widget_kit()),
    );
    run(
        Command::ID,
        check_replaceable(Registry::new, &command_kit()),
    );
    run(Preset::ID, check_replaceable(Registry::new, &preset_kit()));
    run(
        StoreBackend::ID,
        check_replaceable(Registry::new, &backend_kit()),
    );
    run(Action::ID, check_replaceable(Registry::new, &action_kit()));
    run(
        ImporterPoint::ID,
        check_replaceable(Registry::new, &importer_kit()),
    );
    run(
        ExporterPoint::ID,
        check_replaceable(Registry::new, &exporter_kit()),
    );
    run(
        AssetTypePoint::ID,
        check_replaceable(Registry::new, &type_kit()),
    );
    run(
        EditorOverlay::<Cx>::ID,
        check_replaceable(Registry::new, &overlay_kit()),
    );
    run(
        ThemePoint::ID,
        check_replaceable(Registry::new, &theme_kit()),
    );
    run(
        ViewportTool::ID,
        check_replaceable(Registry::new, &tool_kit()),
    );
    run(
        ViewportWorldPoint::ID,
        check_replaceable(Registry::new, &world_kit()),
    );
    run(
        ServiceProviderPoint::ID,
        check_replaceable(Registry::new, &provider_kit()),
    );
    run(
        GeneratorBackendPoint::ID,
        check_replaceable(Registry::new, &generator_kit()),
    );
    run(
        SettingLinkPoint::ID,
        check_replaceable(Registry::new, &setting_link_kit()),
    );
    run(
        NoticeHintPoint::ID,
        check_replaceable(Registry::new, &notice_hint_kit()),
    );
    run(
        forge_editor::viewport::layer::ViewportLayerPoint::ID,
        check_replaceable(Registry::new, &layer_kit()),
    );
    run(
        forge_editor::project::promote::PromotionRulePoint::ID,
        check_replaceable(Registry::new, &rule_kit()),
    );
    run(
        forge_phys::PhysicsBackendPoint::ID,
        check_replaceable(Registry::new, &phys_backend_kit()),
    );
}

#[test]
fn every_extension_point_supports_add_replace_remove_and_chain() {
    let mut checked = BTreeSet::new();
    all_points(&mut |id, v| {
        assert!(
            v.is_empty(),
            "{id}:\n{}",
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
        checked.insert(id);
    });
    // Every point the host defines was checked...
    let defined: BTreeSet<&str> = host().points().map(|p| p.id).collect();
    assert_eq!(checked, defined);
    // ...and the host defines every catalog point whose item type lives in these crates.
    // (forge-editor's DEFINED_POINTS: Action, EditorOverlay, ViewportTool and Theme — every
    // Ch.21.19 point now has a kit.)
    let here: BTreeSet<&str> = SEED_POINTS
        .iter()
        .filter(|p| {
            matches!(
                p.defined_in,
                "forge-plugin" | "forge-store" | "forge-asset" | "forge-phys"
            )
                || forge_editor::DEFINED_POINTS.contains(&p.id)
        })
        .map(|p| p.id)
        .collect();
    assert_eq!(here, defined, "a point defined here has no conformance kit");
}

#[test]
fn chained_commands_reach_the_bus_through_install_commands() {
    let core = PluginId::new("forge.core").expect("id");
    let tagger = PluginId::new("com.example.tagger").expect("id");
    let mut reg = Registry::<Command>::new();
    let kit = command_kit();
    let id = reg
        .add(core, "demo.tag", (kit.make)("A"), Order::Last)
        .expect("add");
    reg.chain(&tagger, &id, kit.wrap).expect("chain");
    let mut bus = Bus::new();
    assert_eq!(install_commands(&reg, &mut bus).expect("installs"), 1);
    let e = bus.envelope(
        Issuer::Human { user: "ada".into() },
        EditorCommand::Invoke {
            target: "demo.tag".into(),
            args: "{}".into(),
        },
    );
    let txn = e.txn;
    bus.apply(e).expect("applies");
    assert_eq!(
        bus.project().setting("probe"),
        Some(&Value::Text("wrap(A)".into()))
    );
    // A plugin command is an ordinary undoable command (I7, I8).
    bus.undo(txn).expect("undo");
    assert_eq!(bus.project().setting("probe"), None);
}

// ---- Ch.21.19: replace the hierarchy, chain the inspector, remove the console --------------

struct Plugin {
    manifest: Manifest,
    install: fn(&mut InstallCx) -> Result<(), PluginError>,
}

impl SourcePlugin for Plugin {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        (self.install)(cx)
    }
}

#[test]
fn test_panel_extension_replaceable() {
    let first_party = Plugin {
        manifest: Manifest::parse(
            r#"Plugin(id: "forge.panels", version: "0.1.0", engine: "^0.1", kind: Source,
                provides: [ EditorPanel("forge.hierarchy"), EditorPanel("forge.inspector"),
                            EditorPanel("forge.console") ])"#,
        )
        .expect("manifest"),
        install: |cx| {
            for key in ["forge.hierarchy", "forge.inspector", "forge.console"] {
                cx.add::<EditorPanel<Cx>>(key, panel(key), Order::Last)?;
            }
            Ok(())
        },
    };
    let test_plugin = Plugin {
        manifest: Manifest::parse(
            r#"Plugin(id: "com.test.panels", version: "1.0.0", engine: "^0.1", kind: Source,
                replaces: [ EditorPanel("forge.hierarchy") ],
                chains: [ EditorPanel("forge.inspector") ],
                removes: [ EditorPanel("forge.console") ])"#,
        )
        .expect("manifest"),
        install: |cx| {
            cx.replace::<EditorPanel<Cx>>("forge.hierarchy", panel("my.hierarchy"))?;
            cx.chain::<EditorPanel<Cx>>("forge.inspector", |p| PanelDescriptor {
                build: wrap_build(p.build),
                ..p
            })?;
            cx.remove::<EditorPanel<Cx>>("forge.console")
        },
    };
    let mut x = host();
    // The test plugin sorts before the first-party one; the loader's phases still apply.
    loader::load(&mut x, &[&test_plugin, &first_party], &[], &Grants::new()).expect("loads");
    let reg = x.registry::<EditorPanel<Cx>>().expect("defined");
    let shown: Vec<(String, String)> = reg
        .iter()
        .map(|(k, p)| (k.to_string(), built(&*p.build)))
        .collect();
    assert_eq!(
        shown,
        [
            ("forge.hierarchy".to_string(), "my.hierarchy".to_string()),
            (
                "forge.inspector".to_string(),
                "wrap(forge.inspector)".to_string()
            ),
        ]
    );
    let prov = reg.provenance("forge.hierarchy").expect("provenance");
    assert_eq!(prov.owner.as_str(), "forge.panels");
    assert_eq!(
        prov.replaced_by.as_ref().map(PluginId::as_str),
        Some("com.test.panels")
    );
}

// ---- positive control: broken registries -----------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// A faithful model: must pass (the check is not stricter than the contract).
    None,
    /// `add` ignores `Order` (everything goes last).
    OrderIgnored,
    /// `replace` swaps the item but moves it to the end.
    ReplaceMovesToEnd,
    /// `replace` reports success and changes nothing.
    ReplaceIgnored,
    /// `remove` removes the neighbour instead.
    RemoveOffByOne,
    /// `chain` drops the wrapper.
    ChainIgnored,
    /// A second plugin's replace silently wins.
    LastWins,
}

type Slot<T> = (String, PluginId, Option<PluginId>, T);

/// An independent, deliberately simple registry, so faults can be injected anywhere.
struct Naive<P: ExtensionPoint> {
    items: Vec<Slot<P::Item>>,
    fault: Fault,
}

impl<P: ExtensionPoint> Naive<P> {
    fn new(fault: Fault) -> Self {
        Self {
            items: Vec::new(),
            fault,
        }
    }

    fn find(&self, key: &str) -> Result<usize, PluginError> {
        self.items
            .iter()
            .position(|s| s.0 == key)
            .ok_or_else(|| PluginError::UnknownItem(format!("{}/{key}", P::ID)))
    }
}

impl<P: ExtensionPoint> RegistryOps<P> for Naive<P> {
    fn add(
        &mut self,
        owner: PluginId,
        key: &str,
        item: P::Item,
        order: Order,
    ) -> Result<ItemId, PluginError> {
        if self.find(key).is_ok() {
            return Err(PluginError::DuplicateItem {
                item: key.into(),
                first: String::new(),
                second: String::new(),
            });
        }
        let at = match (self.fault, order) {
            (Fault::OrderIgnored, _) | (_, Order::Last) => self.items.len(),
            (_, Order::First) => 0,
            (_, Order::Before(k)) => self.find(&k)?,
            (_, Order::After(k)) => self.find(&k)? + 1,
        };
        self.items.insert(at, (key.into(), owner, None, item));
        ItemId::of::<P>(key)
    }

    fn replace(
        &mut self,
        by: &PluginId,
        target: &ItemId,
        item: P::Item,
    ) -> Result<Replaced<P::Item>, PluginError> {
        let i = self.find(target.key())?;
        if let Some(prev) = &self.items[i].2
            && prev != by
            && self.fault != Fault::LastWins
        {
            return Err(PluginError::Conflict {
                item: target.to_string(),
                first: prev.to_string(),
                first_op: "replaces",
                second: by.to_string(),
                second_op: "replaces",
            });
        }
        let previous = self.items[i]
            .2
            .clone()
            .unwrap_or_else(|| self.items[i].1.clone());
        if self.fault == Fault::ReplaceIgnored {
            return Ok(Replaced {
                old: item,
                previous,
            });
        }
        let old = std::mem::replace(&mut self.items[i].3, item);
        self.items[i].2 = Some(by.clone());
        if self.fault == Fault::ReplaceMovesToEnd {
            let s = self.items.remove(i);
            self.items.push(s);
        }
        Ok(Replaced { old, previous })
    }

    fn remove(&mut self, _by: &PluginId, target: &ItemId) -> Result<P::Item, PluginError> {
        let mut i = self.find(target.key())?;
        if self.fault == Fault::RemoveOffByOne && i + 1 < self.items.len() {
            i += 1;
        }
        Ok(self.items.remove(i).3)
    }

    fn chain(
        &mut self,
        _by: &PluginId,
        target: &ItemId,
        wrap: Box<dyn FnOnce(P::Item) -> P::Item>,
    ) -> Result<(), PluginError> {
        let i = self.find(target.key())?;
        if self.fault == Fault::ChainIgnored {
            return Ok(());
        }
        let s = self.items.remove(i);
        self.items.insert(i, (s.0, s.1, s.2, wrap(s.3)));
        Ok(())
    }

    fn items(&self) -> Vec<&P::Item> {
        self.items.iter().map(|s| &s.3).collect()
    }
}

fn faults_on<P: ExtensionPoint>(kit: &Kit<P>) {
    for (fault, rule) in [
        (Fault::OrderIgnored, Rule::Add),
        (Fault::ReplaceMovesToEnd, Rule::Replace),
        (Fault::ReplaceIgnored, Rule::Replace),
        (Fault::RemoveOffByOne, Rule::Remove),
        (Fault::ChainIgnored, Rule::Chain),
        (Fault::LastWins, Rule::Conflict),
    ] {
        let v = check_replaceable(|| Naive::<P>::new(fault), kit);
        let rules: Vec<Rule> = v.iter().map(|x| x.rule).collect();
        assert!(
            rules.contains(&rule),
            "{}: {fault:?} was not caught as {rule:?}; got {rules:?}",
            P::ID
        );
    }
    let v = check_replaceable(|| Naive::<P>::new(Fault::None), kit);
    assert!(v.is_empty(), "{}: the faithful model fails: {v:#?}", P::ID);
}

#[test]
fn positive_control_every_broken_registry_is_caught() {
    faults_on(&panel_kit());
    faults_on(&widget_kit());
    faults_on(&command_kit());
    faults_on(&preset_kit());
    faults_on(&backend_kit());
    faults_on(&action_kit());
    faults_on(&importer_kit());
    faults_on(&exporter_kit());
    faults_on(&type_kit());
    faults_on(&overlay_kit());
    faults_on(&theme_kit());
    faults_on(&tool_kit());

    // A kit that cannot tell items apart would make every clause pass vacuously: refused.
    let blind: Kit<Preset> = Kit {
        make: |_| (preset_kit().make)("same"),
        probe: |p| p.label.clone(),
        wrap: |p| p,
    };
    let v = check_replaceable(Registry::new, &blind);
    assert_eq!(
        v.iter().map(|x| x.rule).collect::<Vec<_>>(),
        [Rule::VacuousKit]
    );
}
