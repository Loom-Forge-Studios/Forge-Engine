//! The `EditorPanel` extension point in the editor (Ch.21 §21.19, I16): the dock builds
//! every panel from the plugin registry, so a plugin that adds, replaces, removes or chains
//! a panel changes what the user sees — exactly as a first-party panel is built.
//!
//! * [`PanelCx`] is the context a panel's build function gets. It is `'static` data (the
//!   registry's build functions are `Fn(&mut PanelCx)`), so a panel describes its widgets
//!   as build steps that run against the dock's frame; a chained panel wraps the steps of
//!   the panel it chains. It carries the panel's only handles into the project (Ch.21
//!   §21.18): the mirror (read) and the command emitter (write), plus session state —
//!   see [`crate::panel_rt`].
//! * [`EditorPanelHost`] is the dock's [`PanelHost`] over the registry. A panel that is not
//!   in the registry (its plugin is disabled, or a plugin removed it) keeps its place in the
//!   layout and shows a placeholder naming that plugin.

use std::collections::BTreeMap;

use forge_plugin::points::{EditorPanel, PanelDescriptor, PresetKind};
use forge_plugin::{Manifest, Registry};
use forge_ui::dock::{PanelHost, PanelId};
use forge_ui::widgets::{Build, EmptyState};
use forge_ui::{NodeStyle, UiError, WidgetId};

use crate::emitter::CommandEmitter;
use crate::mirror::ProjectMirror;
use crate::panel_rt::{PanelBuilder, ShellHandles, WindowKey};
use crate::session::SessionState;

/// One build step: adds widgets under the panel's frame.
pub type PanelStep = Box<dyn FnOnce(&mut dyn Build, WidgetId) -> Result<(), UiError>>;
/// A build step that also wires handlers and sync steps (see [`crate::panel_rt`]).
pub type LiveStep = Box<dyn FnOnce(&mut PanelBuilder) -> Result<(), UiError>>;

enum Step {
    Plain(PanelStep),
    Live(LiveStep),
}

/// The context an `EditorPanel` build function gets.
pub struct PanelCx {
    panel: PanelId,
    preset: PresetKind,
    steps: Vec<Step>,
    empty_state: Option<String>,
    never_empty: Option<String>,
    tip: Option<String>,
    shell: ShellHandles,
    window: WindowKey,
}

impl PanelCx {
    /// A context over detached handles (an empty in-memory core): for building a panel
    /// outside a running shell.
    pub fn new(panel: PanelId, preset: PresetKind) -> Self {
        Self::with_shell(panel, preset, ShellHandles::detached(), None)
    }

    /// A context over the running shell's handles, building into `window`.
    pub fn with_shell(
        panel: PanelId,
        preset: PresetKind,
        shell: ShellHandles,
        window: WindowKey,
    ) -> Self {
        Self {
            panel,
            preset,
            steps: Vec::new(),
            empty_state: None,
            never_empty: None,
            tip: None,
            shell,
            window,
        }
    }

    /// The project, read-only (Ch.21 §21.18).
    pub fn mirror(&self) -> std::cell::Ref<'_, ProjectMirror> {
        self.shell.mirror()
    }

    /// The only way to change the project: emit commands (Ch.21 §21.18, I7).
    pub fn cmd(&self) -> &CommandEmitter {
        self.shell.cmd()
    }

    /// Session state and user config (selection, notifications, settings, keymap).
    pub fn session(&self) -> std::cell::Ref<'_, SessionState> {
        self.shell.session()
    }

    /// Add widgets that talk back: handlers for their actions and sync steps that follow
    /// the mirror (see [`crate::panel_rt`]).
    pub fn add_live(
        &mut self,
        step: impl FnOnce(&mut PanelBuilder) -> Result<(), UiError> + 'static,
    ) {
        self.steps.push(Step::Live(Box::new(step)));
    }

    /// The panel being built (its extension-point id).
    pub fn panel(&self) -> &PanelId {
        &self.panel
    }

    /// The workspace preset. Presets choose defaults and never gate panels (I15): a panel
    /// uses this only to pick sensible defaults, and must build under every preset.
    pub fn preset(&self) -> PresetKind {
        self.preset
    }

    /// Add widgets under the panel's frame (a flex column filling the panel).
    pub fn add(
        &mut self,
        step: impl FnOnce(&mut dyn Build, WidgetId) -> Result<(), UiError> + 'static,
    ) {
        self.steps.push(Step::Plain(Box::new(step)));
    }

    /// The empty state to show when the panel has nothing to show (text, at most one action).
    pub fn empty_state(&mut self, message: &str) {
        self.empty_state = Some(message.to_string());
    }

    /// Declare that this panel always has content — a form, a status line, a list that is
    /// never empty — and so has no empty state; `why` says what it shows (M2-70: every
    /// panel either shows its catalogue empty state when it has nothing, or declares this).
    pub fn never_empty(&mut self, why: &str) {
        self.never_empty = Some(why.to_string());
    }

    /// What [`PanelCx::never_empty`] declared.
    pub fn never_empty_reason(&self) -> Option<&str> {
        self.never_empty.as_deref()
    }

    /// The panel's first-run tip: one short sentence, a localisation key (see
    /// [`crate::tips`]).
    pub fn first_run_tip(&mut self, text: &str) {
        self.tip = Some(text.to_string());
    }

    /// What [`PanelCx::first_run_tip`] declared.
    pub fn tip(&self) -> Option<&str> {
        self.tip.as_deref()
    }

    /// How many build steps the panel queued (a panel with none built nothing).
    pub fn step_count(&self) -> usize {
        self.steps.len() + usize::from(self.empty_state.is_some())
    }

    /// Run the queued steps under `parent`.
    pub fn run(self, b: &mut dyn Build, parent: WidgetId) -> Result<(), UiError> {
        // The first-run tip, above the panel's content (never focused, never modal).
        if let Some(tip) = &self.tip
            && crate::tips::should_show(&self.shell.session(), &self.panel)
        {
            let mut pb = PanelBuilder::new(b, parent, &self.shell, self.window);
            crate::tips::build(&mut pb, &self.panel, tip)?;
        }
        for s in self.steps {
            match s {
                Step::Plain(s) => s(b, parent)?,
                Step::Live(s) => {
                    let mut pb = PanelBuilder::new(b, parent, &self.shell, self.window);
                    s(&mut pb)?;
                }
            }
        }
        if let Some(m) = self.empty_state {
            b.add(
                parent,
                "empty",
                NodeStyle::leaf().grow(1.0),
                EmptyState::new(&m),
            )?;
        }
        Ok(())
    }
}

/// Why panels the registry lacks are missing, from the plugin manifests: which plugin
/// removed a panel, or which disabled plugin provides it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelCatalog {
    reasons: BTreeMap<PanelId, String>,
}

impl PanelCatalog {
    /// From the manifests of the loaded plugins and of the disabled ones.
    pub fn from_manifests(loaded: &[&Manifest], disabled: &[&Manifest]) -> Self {
        let mut reasons = BTreeMap::new();
        for m in disabled {
            for r in m.provides.iter().filter(|r| r.point == "EditorPanel") {
                reasons.insert(
                    PanelId::new(&r.key),
                    forge_ui::trf!(
                        "\u{26a0} \u{201c}{panel}\u{201d} comes from the plugin {plugin}, which is disabled. \
                         Enable it in the plugin manager to bring the panel back; its place \
                         in the layout is kept.",
                        panel = r.key,
                        plugin = m.id
                    ),
                );
            }
        }
        for m in loaded {
            for r in m.removes.iter().filter(|r| r.point == "EditorPanel") {
                reasons.insert(
                    PanelId::new(&r.key),
                    forge_ui::trf!(
                        "\u{26a0} \u{201c}{panel}\u{201d} was removed by the plugin {plugin}. Its place in \
                         the layout is kept for when that plugin is disabled.",
                        panel = r.key,
                        plugin = m.id
                    ),
                );
            }
        }
        Self { reasons }
    }

    pub fn reason(&self, p: &PanelId) -> Option<&str> {
        self.reasons.get(p).map(String::as_str)
    }
}

/// The dock's panel host over the `EditorPanel` registry.
#[derive(Clone)]
pub struct EditorPanelHost {
    /// Shared by every window's copy of the host: a panel a plugin adds while the editor
    /// runs (a WASM plugin dropped in, WP-21) is buildable in every window at once.
    panels: std::rc::Rc<std::cell::RefCell<BTreeMap<PanelId, PanelDescriptor<PanelCx>>>>,
    order: std::rc::Rc<std::cell::RefCell<Vec<PanelId>>>,
    preset: PresetKind,
    catalog: PanelCatalog,
    shell: Option<ShellHandles>,
    window: WindowKey,
}

impl EditorPanelHost {
    /// A host over `reg` whose panels get detached handles (tests, audits).
    pub fn new(
        reg: &Registry<EditorPanel<PanelCx>>,
        preset: PresetKind,
        catalog: PanelCatalog,
    ) -> Self {
        Self::build_host(reg, preset, catalog, None, None)
    }

    /// A host for the running shell's dock area in `window`: its panels get the shell's
    /// handles.
    pub fn with_shell(
        reg: &Registry<EditorPanel<PanelCx>>,
        preset: PresetKind,
        catalog: PanelCatalog,
        shell: ShellHandles,
        window: WindowKey,
    ) -> Self {
        Self::build_host(reg, preset, catalog, Some(shell), window)
    }

    fn build_host(
        reg: &Registry<EditorPanel<PanelCx>>,
        preset: PresetKind,
        catalog: PanelCatalog,
        shell: Option<ShellHandles>,
        window: WindowKey,
    ) -> Self {
        let mut panels = BTreeMap::new();
        let mut order = Vec::new();
        for (k, d) in reg.iter() {
            panels.insert(PanelId::new(k), d.clone());
            order.push(PanelId::new(k));
        }
        Self {
            panels: std::rc::Rc::new(std::cell::RefCell::new(panels)),
            order: std::rc::Rc::new(std::cell::RefCell::new(order)),
            preset,
            catalog,
            shell,
            window,
        }
    }

    /// The same host building into `window` (the dock's host factory, one per area).
    pub fn for_window(mut self, window: WindowKey) -> Self {
        self.window = window;
        self
    }

    /// A registered panel's default dock (where "Open" puts it).
    pub fn default_dock(&self, p: &PanelId) -> Option<forge_plugin::points::Dock> {
        self.panels.borrow().get(p).map(|d| d.default_dock)
    }

    /// Whether a panel of this id is registered (a live install stages its panels on it).
    #[must_use]
    pub fn has_panel(&self, id: &PanelId) -> bool {
        self.panels.borrow().contains_key(id)
    }

    /// Add a panel a plugin installed while the editor runs (every window's host sees it).
    /// `false` when a panel of that id is registered already (nothing changes).
    pub fn add_panel(&self, id: PanelId, d: PanelDescriptor<PanelCx>) -> bool {
        let mut panels = self.panels.borrow_mut();
        if panels.contains_key(&id) {
            return false;
        }
        panels.insert(id.clone(), d);
        self.order.borrow_mut().push(id);
        true
    }

    /// Every registered panel with its title, in registry order (the Window menu, the
    /// palette's "Open:" entries).
    pub fn panels(&self) -> Vec<(PanelId, String)> {
        let panels = self.panels.borrow();
        self.order
            .borrow()
            .iter()
            .filter_map(|p| panels.get(p).map(|d| (p.clone(), d.title.clone())))
            .collect()
    }

    /// Run a panel's build function into a fresh context (tests, the empty-state audit).
    pub fn describe(&self, p: &PanelId) -> Option<PanelCx> {
        let d = self.panels.borrow().get(p)?.clone();
        let mut cx = match &self.shell {
            Some(s) => PanelCx::with_shell(p.clone(), self.preset, s.clone(), self.window),
            None => PanelCx::new(p.clone(), self.preset),
        };
        (d.build)(&mut cx);
        Some(cx)
    }
}

impl PanelHost for EditorPanelHost {
    fn title(&self, panel: &PanelId) -> Option<String> {
        self.panels.borrow().get(panel).map(|d| d.title.clone())
    }

    fn build(
        &mut self,
        b: &mut dyn Build,
        parent: WidgetId,
        panel: &PanelId,
    ) -> Result<bool, UiError> {
        let Some(cx) = self.describe(panel) else {
            return Ok(false);
        };
        cx.run(b, parent)?;
        Ok(true)
    }

    fn missing_reason(&self, panel: &PanelId) -> String {
        match self.catalog.reason(panel) {
            Some(r) => r.to_string(),
            None => forge_ui::trf!(
                "\u{26a0} The panel \u{201c}{panel}\u{201d} is not available: no loaded plugin \
                 provides it. Its place in the layout is kept.",
                panel
            ),
        }
    }
}

/// Open `panel` in `layout` the way the Window menu and the palette do: if it is there,
/// show its tab; otherwise place it by its descriptor's default dock (a side of the main
/// area, a tab beside the centre panels, or its own floating window).
pub fn open_panel(
    layout: &mut forge_ui::dock::Layout,
    panel: &PanelId,
    dock: forge_plugin::points::Dock,
) -> Result<(), crate::EditorError> {
    use forge_plugin::points::Dock;
    use forge_ui::dock::{AreaId, DropTarget, DropZone, Side};
    if layout.contains(panel) {
        layout.activate(panel);
        return Ok(());
    }
    let main_panels = layout.main.panels();
    let target = match (dock, main_panels.first()) {
        (_, None) => DropTarget::EmptyArea(AreaId::Main),
        (Dock::Left, _) => DropTarget::AreaEdge {
            area: AreaId::Main,
            side: Side::Left,
        },
        (Dock::Right, _) => DropTarget::AreaEdge {
            area: AreaId::Main,
            side: Side::Right,
        },
        (Dock::Bottom, _) => DropTarget::AreaEdge {
            area: AreaId::Main,
            side: Side::Bottom,
        },
        (Dock::Center | Dock::Floating, Some(first)) => {
            let anchor = main_panels
                .iter()
                .find(|p| p.as_str() == "forge.viewport")
                .unwrap_or(first)
                .clone();
            DropTarget::Group {
                area: AreaId::Main,
                anchor,
                zone: DropZone::Tab(None),
            }
        }
    };
    layout.insert(panel.clone(), &target)?;
    if dock == Dock::Floating {
        let r = forge_ui::Rect::new(120.0, 120.0, 520.0, 380.0);
        layout.float(panel, r, None)?;
    }
    Ok(())
}

/// I15 (Ch.21 §21.19): open every panel of `reg` under every preset, in a real dock, and
/// list what failed. A panel fails if it did not build (the dock shows a placeholder) or
/// built nothing — a panel that hides itself under some preset is exactly that. Empty
/// means every panel opens under every preset.
// l10n-block: the I15 guard's findings, read by tests and CI, never shown in the editor
pub fn check_panels_under_presets(
    reg: &Registry<EditorPanel<PanelCx>>,
    presets: &[crate::presets::Preset],
) -> Vec<String> {
    use forge_ui::dock::{AreaId, DockTree, dock_area, panel_frame_id};
    use forge_ui::{Ui, UiConfig};
    let mut problems = Vec::new();
    for preset in presets {
        let name = &preset.workspace.id;
        let kind = preset.workspace.family.kind();
        let mut layout = preset.layout.clone();
        for (k, d) in reg.iter() {
            if let Err(e) = open_panel(&mut layout, &PanelId::new(k), d.default_dock) {
                problems.push(format!("{name}: {k} could not be opened: {e}"));
            }
        }
        let mut ui = match Ui::new(UiConfig::default()) {
            Ok(u) => u,
            Err(e) => {
                problems.push(format!("{name}: {e}"));
                continue;
            }
        };
        let root = ui.root();
        let host = EditorPanelHost::new(reg, kind, PanelCatalog::default());
        // Every area of the layout is a dock (floating windows are their own `Ui`s in the
        // editor; one `Ui` per area here builds exactly the same frames).
        let mut areas = vec![(AreaId::Main, layout.main.clone())];
        areas.extend(
            layout
                .floating
                .iter()
                .map(|w| (AreaId::Floating(w.id), w.root.clone())),
        );
        for (i, (area, root_node)) in areas.into_iter().enumerate() {
            let sig = ui.rt_mut().signal(DockTree::new(root_node.clone()));
            let key = forge_ui::Key::Index(i as u32);
            let dock = match dock_area(
                &mut ui,
                root,
                key,
                NodeStyle::default().fill(),
                area,
                sig,
                Box::new(EditorPanelHost::new(reg, kind, PanelCatalog::default())),
            ) {
                Ok(d) => d,
                Err(e) => {
                    problems.push(format!("{name}: dock: {e}"));
                    continue;
                }
            };
            ui.frame(std::time::Duration::ZERO);
            for p in root_node.panels() {
                let f = panel_frame_id(dock, &p);
                if !ui.contains(f) {
                    problems.push(format!("{name}: {p} has no frame"));
                    continue;
                }
                if ui.contains(f.child(&forge_ui::Key::Static("missing"))) {
                    problems.push(format!("{name}: {p} did not build (placeholder shown)"));
                    continue;
                }
                if ui.children(f).is_empty() {
                    problems.push(format!("{name}: {p} built nothing under this preset"));
                }
            }
        }
        for (k, _) in reg.iter() {
            if host
                .describe(&PanelId::new(k))
                .is_none_or(|cx| cx.step_count() == 0)
            {
                problems.push(format!("{name}: {k} queued no content under this preset"));
            }
        }
    }
    problems
}
