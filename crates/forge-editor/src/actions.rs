//! The action registry (Ch.21 §21.17, §21.19): every user-invokable thing is an action,
//! registered through the `Action` extension point (`forge.editor.action`), so menus, the
//! toolbar, the keymap and the command palette all read one registry and cannot drift.
//!
//! An action is either a **command** — it builds [`EditorCommand`]s the shell sends on the
//! bus (I7: the registry never touches project state itself) — undo/redo on the bus, or a
//! **session** operation (open a panel, change the layout or theme: user config and
//! session state, not project state).

use std::sync::Arc;

use forge_cmd::{EditorCommand, EntityKey};
use forge_plugin::{ExtensionPoint, Order, PluginError, PluginId, Registry};
use forge_ui::dock::PanelId;

use crate::EditorError;
use crate::keymap::{Chord, KeyContext};

/// What an action's predicate and command builder see.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActionCx {
    /// The selected entities.
    pub selection: Vec<EntityKey>,
    /// The panel holding keyboard focus.
    pub focused_panel: Option<PanelId>,
    /// Panels in the layout now.
    pub open_panels: Vec<PanelId>,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// Builds the commands an action sends (the shell wraps several in one transaction).
pub type CommandBuilder = Arc<dyn Fn(&ActionCx) -> Vec<EditorCommand> + Send + Sync>;
/// Whether an action is available in a context.
pub type Predicate = Arc<dyn Fn(&ActionCx) -> bool + Send + Sync>;

/// A session operation: session state or user config, never project state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionOp {
    OpenPalette,
    /// Open (or focus) a panel.
    OpenPanel(PanelId),
    /// Close the focused panel.
    ClosePanel,
    /// Tear the focused panel off into a floating window.
    FloatPanel,
    /// Maximise the focused panel, or restore.
    ToggleMaximise,
    SaveLayoutAs,
    ResetLayout,
    /// Switch to a theme by id.
    SetTheme(String),
    /// A plugin-defined session operation, by name (the plugin's own handler runs it).
    Custom(String),
}

/// What running an action does.
#[derive(Clone)]
pub enum ActionKind {
    /// Send these commands on the bus.
    Command(CommandBuilder),
    /// Undo the last transaction (on the bus).
    Undo,
    /// Redo (on the bus).
    Redo,
    /// A session operation.
    Session(SessionOp),
}

impl std::fmt::Debug for ActionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionKind::Command(_) => f.write_str("Command(..)"),
            ActionKind::Undo => f.write_str("Undo"),
            ActionKind::Redo => f.write_str("Redo"),
            ActionKind::Session(s) => write!(f, "Session({s:?})"),
        }
    }
}

/// One registered action (the `Action` point's item). Its key is its `ActionId`.
#[derive(Clone, Debug)]
pub struct ActionItem {
    pub title: String,
    pub category: String,
    /// Chords the action suggests; the keymap data decides (Ch.21 §21.17).
    pub default_chords: Vec<(Chord, KeyContext)>,
    pub kind: ActionKind,
    /// `None`: always enabled.
    pub enabled: Option<PredicateBox>,
}

/// A predicate with a `Debug` impl.
#[derive(Clone)]
pub struct PredicateBox(pub Predicate);

impl std::fmt::Debug for PredicateBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Predicate(..)")
    }
}

impl ActionItem {
    pub fn new(title: &str, category: &str, kind: ActionKind) -> Self {
        Self {
            title: title.to_string(),
            category: category.to_string(),
            default_chords: Vec::new(),
            kind,
            enabled: None,
        }
    }

    /// Only available when `f` says so.
    pub fn when(mut self, f: impl Fn(&ActionCx) -> bool + Send + Sync + 'static) -> Self {
        self.enabled = Some(PredicateBox(Arc::new(f)));
        self
    }

    pub fn is_enabled(&self, cx: &ActionCx) -> bool {
        self.enabled.as_ref().is_none_or(|p| (p.0)(cx))
    }
}

/// The `Action` extension point (`forge.editor.action`).
pub struct Action;

impl ExtensionPoint for Action {
    type Item = ActionItem;
    const ID: &'static str = "forge.editor.action";
    // l10n: the extension point's identifier
    const NAME: &'static str = "Action";
}

/// What invoking an action asks the shell to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Invocation {
    /// Send these on the bus, as one transaction labelled with the action's title.
    Commands {
        label: String,
        commands: Vec<EditorCommand>,
    },
    Undo,
    Redo,
    Session(SessionOp),
}

/// Look up and run an action against `cx`. Never touches project state: commands come
/// back for the shell to send (I7).
pub fn invoke(reg: &Registry<Action>, id: &str, cx: &ActionCx) -> Result<Invocation, EditorError> {
    let a = reg
        .get(id)
        .ok_or_else(|| EditorError::UnknownAction(id.to_string()))?;
    if !a.is_enabled(cx) {
        return Err(EditorError::ActionDisabled(id.to_string()));
    }
    Ok(match &a.kind {
        ActionKind::Command(build) => Invocation::Commands {
            label: a.title.clone(),
            commands: build(cx),
        },
        ActionKind::Undo => Invocation::Undo,
        ActionKind::Redo => Invocation::Redo,
        ActionKind::Session(s) => Invocation::Session(s.clone()),
    })
}

fn sel_any(cx: &ActionCx) -> bool {
    !cx.selection.is_empty()
}

fn focused(cx: &ActionCx) -> bool {
    cx.focused_panel.is_some()
}

/// The shell's built-in actions, as the first-party `forge.editor` plugin registers them
/// (through the same point a third-party plugin uses, I16).
pub fn builtin_actions() -> Vec<(&'static str, ActionItem)> {
    use ActionKind::*;
    use SessionOp::*;
    vec![
        (
            "forge.palette.open",
            ActionItem::new(
                forge_ui::tr_key!("Show command palette"),
                forge_ui::tr_key!("View"),
                Session(OpenPalette),
            ),
        ),
        (
            "forge.edit.undo",
            ActionItem::new(forge_ui::tr_key!("Undo"), forge_ui::tr_key!("Edit"), Undo)
                .when(|cx| cx.can_undo),
        ),
        (
            "forge.edit.redo",
            ActionItem::new(forge_ui::tr_key!("Redo"), forge_ui::tr_key!("Edit"), Redo)
                .when(|cx| cx.can_redo),
        ),
        (
            "forge.edit.delete",
            ActionItem::new(
                forge_ui::tr_key!("Delete selection"),
                forge_ui::tr_key!("Edit"),
                Command(Arc::new(|cx: &ActionCx| {
                    cx.selection
                        .iter()
                        .map(|e| EditorCommand::Despawn { entity: *e })
                        .collect()
                })),
            )
            .when(sel_any),
        ),
        (
            "forge.edit.select_all",
            ActionItem::new(
                forge_ui::tr_key!("Select all"),
                forge_ui::tr_key!("Edit"),
                Session(Custom("select_all".into())),
            ),
        ),
        (
            "forge.text.select_all",
            ActionItem::new(
                forge_ui::tr_key!("Select all text"),
                forge_ui::tr_key!("Text"),
                Session(Custom("text.select_all".into())),
            ),
        ),
        (
            "forge.text.delete",
            ActionItem::new(
                forge_ui::tr_key!("Delete character"),
                forge_ui::tr_key!("Text"),
                Session(Custom("text.delete".into())),
            ),
        ),
        (
            "forge.file.save",
            // WP-U7: the shell sends `forge.project.save`, a session command the core
            // performs (E-36) — not an undoable edit, like the play controls.
            ActionItem::new(
                forge_ui::tr_key!("Save project"),
                forge_ui::tr_key!("File"),
                Session(Custom("save".into())),
            ),
        ),
        (
            "forge.file.new",
            ActionItem::new(
                forge_ui::tr_key!("New project\u{2026}"),
                forge_ui::tr_key!("File"),
                Session(OpenPanel(PanelId::new("forge.launcher"))),
            ),
        ),
        (
            "forge.file.open",
            ActionItem::new(
                forge_ui::tr_key!("Open project\u{2026}"),
                forge_ui::tr_key!("File"),
                Session(OpenPanel(PanelId::new("forge.launcher"))),
            ),
        ),
        (
            "forge.project.push",
            ActionItem::new(
                forge_ui::tr_key!("Push to remote"),
                forge_ui::tr_key!("File"),
                Session(Custom("project.push".into())),
            ),
        ),
        (
            "forge.project.pull",
            ActionItem::new(
                forge_ui::tr_key!("Pull from remote"),
                forge_ui::tr_key!("File"),
                Session(Custom("project.pull".into())),
            ),
        ),
        (
            "forge.file.build",
            ActionItem::new(
                forge_ui::tr_key!("Build and export\u{2026}"),
                forge_ui::tr_key!("File"),
                Session(OpenPanel(PanelId::new("forge.export"))),
            ),
        ),
        (
            "forge.dock.toggle_maximise",
            ActionItem::new(
                forge_ui::tr_key!("Maximise or restore panel"),
                forge_ui::tr_key!("Window"),
                Session(ToggleMaximise),
            )
            .when(focused),
        ),
        (
            "forge.dock.float_panel",
            ActionItem::new(
                forge_ui::tr_key!("Float panel in its own window"),
                forge_ui::tr_key!("Window"),
                Session(FloatPanel),
            )
            .when(focused),
        ),
        (
            "forge.dock.close_panel",
            ActionItem::new(
                forge_ui::tr_key!("Close panel"),
                forge_ui::tr_key!("Window"),
                Session(ClosePanel),
            )
            .when(focused),
        ),
        (
            "forge.layout.save_as",
            ActionItem::new(
                forge_ui::tr_key!("Save layout as…"),
                forge_ui::tr_key!("Window"),
                Session(SaveLayoutAs),
            ),
        ),
        (
            "forge.layout.reset",
            ActionItem::new(
                forge_ui::tr_key!("Reset layout to the preset's"),
                forge_ui::tr_key!("Window"),
                Session(ResetLayout),
            ),
        ),
        (
            "forge.panel.open.forge.console",
            ActionItem::new(
                forge_ui::tr_key!("Show console"),
                forge_ui::tr_key!("Window"),
                Session(OpenPanel(PanelId::new("forge.console"))),
            ),
        ),
        (
            "forge.play.toggle",
            ActionItem::new(
                forge_ui::tr_key!("Play / stop in editor"),
                forge_ui::tr_key!("Play"),
                Session(Custom("play.toggle".into())),
            ),
        ),
        (
            "forge.play.pause",
            ActionItem::new(
                forge_ui::tr_key!("Pause / resume play-in-editor"),
                forge_ui::tr_key!("Play"),
                Session(Custom("play.pause".into())),
            ),
        ),
        (
            "forge.play.step",
            ActionItem::new(
                forge_ui::tr_key!("Step one simulation tick"),
                forge_ui::tr_key!("Play"),
                Session(Custom("play.step".into())),
            ),
        ),
        (
            "forge.play.stop",
            ActionItem::new(
                forge_ui::tr_key!("Stop play-in-editor"),
                forge_ui::tr_key!("Play"),
                Session(Custom("play.stop".into())),
            ),
        ),
        (
            "forge.hierarchy.rename",
            ActionItem::new(
                forge_ui::tr_key!("Rename selected entity"),
                forge_ui::tr_key!("Hierarchy"),
                Session(Custom("hierarchy.rename".into())),
            )
            .when(|cx| cx.selection.len() == 1),
        ),
        (
            "forge.assets.rename",
            ActionItem::new(
                forge_ui::tr_key!("Rename asset"),
                forge_ui::tr_key!("Assets"),
                Session(Custom("assets.rename".into())),
            ),
        ),
        (
            "forge.console.clear",
            ActionItem::new(
                forge_ui::tr_key!("Clear console"),
                forge_ui::tr_key!("Console"),
                Session(Custom("console.clear".into())),
            ),
        ),
        (
            "forge.theme.dark",
            ActionItem::new(
                forge_ui::tr_key!("Theme: Dark"),
                forge_ui::tr_key!("Preferences"),
                Session(SetTheme("forge.dark".into())),
            ),
        ),
        (
            "forge.theme.light",
            ActionItem::new(
                forge_ui::tr_key!("Theme: Light"),
                forge_ui::tr_key!("Preferences"),
                Session(SetTheme("forge.light".into())),
            ),
        ),
        (
            "forge.theme.high_contrast",
            ActionItem::new(
                forge_ui::tr_key!("Theme: High contrast"),
                forge_ui::tr_key!("Preferences"),
                Session(SetTheme("forge.high_contrast".into())),
            ),
        ),
    ]
}

/// The first-party `forge.editor` source plugin: registers the built-in actions through
/// the ordinary `Action` point, as any plugin would (I16).
pub struct EditorActions {
    manifest: forge_plugin::Manifest,
}

impl EditorActions {
    pub fn new() -> Result<Self, PluginError> {
        let mut provides: Vec<String> = builtin_actions()
            .iter()
            .map(|(id, _)| format!("Action(\"{id}\")"))
            .collect();
        // The built-in themes and overlays, through their points like any plugin's (I16).
        provides.extend(
            crate::overlay::builtin_themes()
                .iter()
                .map(|(id, _)| format!("Theme(\"{id}\")")),
        );
        provides.extend(
            crate::overlay::builtin_overlays()
                .iter()
                .map(|(id, _)| format!("EditorOverlay(\"{id}\")")),
        );
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.editor\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: forge_plugin::Manifest::parse(&text)?,
        })
    }
}

impl forge_plugin::SourcePlugin for EditorActions {
    fn manifest(&self) -> &forge_plugin::Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut forge_plugin::InstallCx) -> Result<(), PluginError> {
        for (id, a) in builtin_actions() {
            cx.add::<Action>(id, a, Order::Last)?;
        }
        for (id, t) in crate::overlay::builtin_themes() {
            cx.add::<crate::overlay::ThemePoint>(id, t, Order::Last)?;
        }
        for (id, o) in crate::overlay::builtin_overlays() {
            cx.add::<crate::overlay::EditorOverlay>(id, o, Order::Last)?;
        }
        Ok(())
    }
}

/// A registry holding the built-in actions, owned by the first-party `forge.editor`.
pub fn builtin_registry() -> Result<Registry<Action>, PluginError> {
    let mut r = Registry::<Action>::new();
    let owner = PluginId::new("forge.editor")?;
    for (id, a) in builtin_actions() {
        r.add(owner.clone(), id, a, Order::Last)?;
    }
    Ok(r)
}
