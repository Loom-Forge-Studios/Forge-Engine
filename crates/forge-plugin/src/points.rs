//! The seed extension points (Ch.32.2) this crate defines, and the catalog of every seed
//! point's stable id.
//!
//! A point is defined where its item type lives: `StoreBackend` in `forge-store`, render
//! passes in `forge-render`, and so on. This crate defines the ones it can without
//! depending upward: [`Command`], [`Preset`], and the editor's [`EditorPanel`] and
//! [`InspectorWidget`] — generic over the editor's context type, so `forge-editor`
//! instantiates them with its own `PanelCx` and keeps full typing without this crate
//! knowing the editor exists.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;

use forge_cmd::{Bus, CmdError, CommandHandler, CommandPolicy, DiffBuilder};

use crate::{ExtensionPoint, Registry};

/// One row of the seed catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeedPoint {
    /// Stable id.
    pub id: &'static str,
    /// Manifest name.
    pub name: &'static str,
    /// The crate that defines its item type (it may not exist yet).
    pub defined_in: &'static str,
    /// The governing chapter.
    pub chapter: &'static str,
}

const fn seed(
    id: &'static str,
    name: &'static str,
    defined_in: &'static str,
    chapter: &'static str,
) -> SeedPoint {
    SeedPoint {
        id,
        name,
        defined_in,
        chapter,
    }
}

/// Ch.32.2's seed list plus Ch.21.19's editor points: the ids are the contract, fixed now so
/// manifests written today keep resolving when the defining crate lands.
pub const SEED_POINTS: &[SeedPoint] = &[
    seed("forge.asset.importer", "Importer", "forge-asset", "Ch.8"),
    seed("forge.asset.exporter", "Exporter", "forge-asset", "Ch.8"),
    seed("forge.asset.type", "AssetType", "forge-asset", "Ch.8"),
    seed("forge.render.pass", "RenderPass", "forge-render", "Ch.10"),
    seed(
        "forge.render.material_node",
        "MaterialNode",
        "forge-render",
        "Ch.10",
    ),
    seed("forge.graph.node", "GraphNode", "forge-graph", "Ch.24"),
    seed("forge.pcg.rule", "PcgRule", "forge-pcg", "Ch.14"),
    seed(
        "forge.phys.backend",
        "PhysicsBackend",
        "forge-phys",
        "Ch.17",
    ),
    seed("forge.nav.builder", "NavmeshBuilder", "forge-nav", "Ch.18"),
    seed(
        "forge.editor.inspector_widget",
        "InspectorWidget",
        "forge-plugin",
        "Ch.21",
    ),
    seed("forge.editor.panel", "EditorPanel", "forge-plugin", "Ch.21"),
    seed(
        "forge.editor.overlay",
        "EditorOverlay",
        "forge-editor",
        "Ch.21",
    ),
    seed(
        "forge.editor.viewport_tool",
        "ViewportTool",
        "forge-editor",
        "Ch.21",
    ),
    seed("forge.editor.action", "Action", "forge-editor", "Ch.21"),
    // The item is forge_ui::Theme; the point is defined in forge-editor because forge-ui
    // must not depend on the plugin kernel (games link forge-ui without forge-cmd; ADR 0029).
    seed("forge.ui.theme", "Theme", "forge-editor", "Ch.21"),
    seed("forge.cmd.command", "Command", "forge-plugin", "Ch.7"),
    seed(
        "forge.store.backend",
        "StoreBackend",
        "forge-store",
        "Ch.33",
    ),
    seed("forge.preset", "Preset", "forge-plugin", "Ch.31"),
    seed("forge.script.host", "ScriptHost", "forge-script", "Ch.23"),
    seed(
        "forge.hal.platform",
        "PlatformBackend",
        "forge-hal",
        "Ch.27",
    ),
    seed("forge.play.localiser", "Localiser", "forge-play", "Ch.28"),
    seed(
        "forge.play.input_device",
        "InputDevice",
        "forge-play",
        "Ch.28",
    ),
    seed("forge.trace.profiler", "Profiler", "forge-trace", "Ch.29"),
    seed(
        "forge.editor.viewport_world",
        "ViewportWorld",
        "forge-editor",
        "Ch.21",
    ),
    seed(
        "forge.editor.service_provider",
        "ServiceProvider",
        "forge-editor",
        "Ch.21",
    ),
    seed(
        "forge.editor.generator_backend",
        "GeneratorBackend",
        "forge-editor",
        "Ch.24",
    ),
    // WP-47: a plugin's link beside a settings row, and its hints on the problems it knows.
    seed(
        "forge.editor.setting_link",
        "SettingLink",
        "forge-editor",
        "Ch.21",
    ),
    seed(
        "forge.editor.notice_hint",
        "NoticeHint",
        "forge-editor",
        "Ch.21",
    ),
    seed(
        "forge.editor.viewport_layer",
        "ViewportLayer",
        "forge-editor",
        "Ch.21",
    ),
    seed(
        "forge.editor.promotion_rule",
        "PromotionRule",
        "forge-editor",
        "Ch.31",
    ),
];

/// The catalog row for `id`.
#[must_use]
pub fn seed_point(id: &str) -> Option<&'static SeedPoint> {
    SEED_POINTS.iter().find(|p| p.id == id)
}

// ---- editor points ----------------------------------------------------------------------

/// Where a panel docks by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    /// Left side.
    Left,
    /// Right side.
    Right,
    /// Bottom.
    Bottom,
    /// The centre (tabbed with the viewport).
    Center,
    /// A floating window.
    Floating,
}

/// A panel's build function: called with the editor's panel context.
pub type BuildFn<Cx> = Arc<dyn Fn(&mut Cx) + Send + Sync>;

/// An editor panel (Ch.21.19). The registry key is the panel id (`forge.hierarchy`).
pub struct PanelDescriptor<Cx> {
    /// Title shown on its tab.
    pub title: String,
    /// Icon name, if any.
    pub icon: Option<String>,
    /// Default dock position.
    pub default_dock: Dock,
    /// Builds the panel's widgets.
    pub build: BuildFn<Cx>,
}

impl<Cx> Clone for PanelDescriptor<Cx> {
    fn clone(&self) -> Self {
        Self {
            title: self.title.clone(),
            icon: self.icon.clone(),
            default_dock: self.default_dock,
            build: Arc::clone(&self.build),
        }
    }
}

/// The `EditorPanel` point (`forge.editor.panel`), for an editor whose panel context is `Cx`.
pub struct EditorPanel<Cx>(PhantomData<fn(&mut Cx)>);

impl<Cx: 'static> ExtensionPoint for EditorPanel<Cx> {
    type Item = PanelDescriptor<Cx>;
    const ID: &'static str = "forge.editor.panel";
    const NAME: &'static str = "EditorPanel";
}

/// What an inspector widget edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WidgetTarget {
    /// Values of a reflected type (its type path).
    Type(String),
    /// Fields carrying a `#[forge(...)]` attribute.
    Attribute(String),
}

/// A custom inspector editor (Ch.21.19).
pub struct InspectorWidgetDescriptor<Cx> {
    /// What it edits.
    pub applies_to: WidgetTarget,
    /// Builds the widget.
    pub build: BuildFn<Cx>,
}

impl<Cx> Clone for InspectorWidgetDescriptor<Cx> {
    fn clone(&self) -> Self {
        Self {
            applies_to: self.applies_to.clone(),
            build: Arc::clone(&self.build),
        }
    }
}

/// The `InspectorWidget` point (`forge.editor.inspector_widget`).
pub struct InspectorWidget<Cx>(PhantomData<fn(&mut Cx)>);

impl<Cx: 'static> ExtensionPoint for InspectorWidget<Cx> {
    type Item = InspectorWidgetDescriptor<Cx>;
    const ID: &'static str = "forge.editor.inspector_widget";
    const NAME: &'static str = "InspectorWidget";
}

// ---- commands -----------------------------------------------------------------------------

/// A plugin command: an `Invoke` handler on the bus (Ch.7). The registry key is the
/// `Invoke` target. It plans through a `DiffBuilder` like any built-in, so a plugin command
/// is undoable, dry-runnable and provenance-tagged by construction (I7, I8).
#[derive(Clone)]
pub struct CommandItem {
    /// Its policy (human-only, undo/redo rules).
    pub policy: CommandPolicy,
    /// Its planner.
    pub handler: Arc<dyn CommandHandler>,
}

impl CommandItem {
    /// An ordinary command planned by `f`.
    pub fn new(
        f: impl Fn(&mut DiffBuilder<'_>, &serde_json::Value) -> Result<(), CmdError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            policy: CommandPolicy::ORDINARY,
            handler: Arc::new(f),
        }
    }

    /// Middleware for `chain`: `f` receives the wrapped command's handler and runs before,
    /// after or around it.
    pub fn wrap(
        self,
        f: impl Fn(
            &dyn CommandHandler,
            &mut DiffBuilder<'_>,
            &serde_json::Value,
        ) -> Result<(), CmdError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let inner = self.handler;
        Self {
            policy: self.policy,
            handler: Arc::new(move |b: &mut DiffBuilder<'_>, args: &serde_json::Value| {
                f(&*inner, b, args)
            }),
        }
    }
}

/// The `Command` point (`forge.cmd.command`).
pub struct Command;

impl ExtensionPoint for Command {
    type Item = CommandItem;
    const ID: &'static str = "forge.cmd.command";
    const NAME: &'static str = "Command";
}

struct Shared(Arc<dyn CommandHandler>);

impl CommandHandler for Shared {
    fn plan(&self, b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
        self.0.plan(b, args)
    }
}

/// Register every command in `reg` (after replaces, removes and chains) on `bus` as an
/// `Invoke` handler. Returns how many were registered.
pub fn install_commands(reg: &Registry<Command>, bus: &mut Bus) -> Result<usize, CmdError> {
    let mut n = 0;
    for (target, item) in reg.iter() {
        bus.register_handler(target, item.policy, Shared(Arc::clone(&item.handler)))?;
        n += 1;
    }
    Ok(n)
}

// ---- presets ------------------------------------------------------------------------------

/// Which workspace preset (Ch.31).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetKind {
    /// 2D.
    TwoD,
    /// 3D.
    ThreeD,
}

/// A workspace preset: defaults only, never a capability gate (I15). Registry key: its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetDescriptor {
    /// Shown in the new-project dialog.
    pub label: String,
    /// Which preset family.
    pub kind: PresetKind,
    /// Setting defaults (`key` -> RON value text).
    pub defaults: BTreeMap<String, String>,
    /// The preset's files by name (`workspace.ron`, its layout and keymap layer; Ch.31.3), for
    /// hosts that build a full workspace from the registry. Empty for a defaults-only preset.
    pub files: BTreeMap<String, String>,
}

/// The `Preset` point (`forge.preset`).
pub struct Preset;

impl ExtensionPoint for Preset {
    type Item = PresetDescriptor;
    const ID: &'static str = "forge.preset";
    const NAME: &'static str = "Preset";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<P: ExtensionPoint>() -> Option<(&'static str, &'static str)> {
        seed_point(P::ID).map(|r| (r.name, r.defined_in))
    }

    #[test]
    fn defined_points_match_their_catalog_rows() {
        assert_eq!(row::<Command>(), Some(("Command", "forge-plugin")));
        assert_eq!(row::<Preset>(), Some(("Preset", "forge-plugin")));
        assert_eq!(
            row::<EditorPanel<()>>(),
            Some(("EditorPanel", "forge-plugin"))
        );
        assert_eq!(
            row::<InspectorWidget<()>>(),
            Some(("InspectorWidget", "forge-plugin"))
        );
        let mut ids: Vec<&str> = SEED_POINTS.iter().map(|p| p.id).collect();
        let mut names: Vec<&str> = SEED_POINTS.iter().map(|p| p.name).collect();
        ids.sort_unstable();
        names.sort_unstable();
        ids.dedup();
        names.dedup();
        assert_eq!(ids.len(), SEED_POINTS.len(), "ids are unique");
        assert_eq!(names.len(), SEED_POINTS.len(), "manifest names are unique");
    }
}
