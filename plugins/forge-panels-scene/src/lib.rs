//! `forge-panels-scene` — the first-party scene panels (Ch.21 §21.21), a source plugin with
//! the id `forge.panels.scene`:
//!
//! | Panel | Id | DoD |
//! |---|---|---|
//! | Scene hierarchy | `forge.hierarchy` | M2-35 |
//! | Inspector | `forge.inspector` | M2-37 |
//! | Viewport | `forge.viewport` | M2-32 |
//! | Play controls | `forge.play_controls` | M2-33 |
//!
//! It also registers the first-party viewport tools — select, move, rotate, scale and
//! measure — through the ordinary `ViewportTool` point (I16).
//!
//! Registered through the ordinary `EditorPanel` point, exactly as a third-party plugin
//! would be (I16). The panels reach the project only through what `PanelCx` hands them —
//! the mirror (read), the command emitter (write) and the read-side services — so every
//! project edit is a command (I7). Selection, expansion, the camera focus and the
//! navigator's position are session state.

#![forbid(unsafe_code)]

pub mod hierarchy;
pub mod inspector;
pub mod play;
pub mod viewport;

use forge_editor::panels::PanelCx;
use forge_editor::viewport::tool::{ViewportTool, ViewportToolItem};
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

/// The panels this plugin provides: `(id, title, default dock, first-run tip)`. The title
/// and the tip are localisation keys (M2-31).
pub const PANELS: &[(&str, &str, Dock, &str)] = &[
    (
        "forge.hierarchy",
        forge_ui::tr_key!("Hierarchy"),
        Dock::Left,
        forge_ui::tr_key!(
            "Drag an entity onto another to parent it; type in the filter to find one by name."
        ),
    ),
    (
        "forge.inspector",
        forge_ui::tr_key!("Inspector"),
        Dock::Right,
        forge_ui::tr_key!(
            "Every change here is a command: Ctrl+Z undoes it and the undo history says who made it."
        ),
    ),
    (
        "forge.viewport",
        forge_ui::tr_key!("Viewport"),
        Dock::Center,
        forge_ui::tr_key!("Click to select; the toolbar picks the move, rotate and scale gizmos."),
    ),
    (
        "forge.play_controls",
        forge_ui::tr_key!("Play controls"),
        Dock::Center,
        forge_ui::tr_key!(
            "Play forks the world into your own sandbox; Stop throws the simulation away."
        ),
    ),
];

/// The viewport tools this plugin registers, `(id, item)`.
pub fn tools() -> Vec<(&'static str, ViewportToolItem)> {
    forge_editor::viewport::tool::builtin_tools()
}

/// The panel ids (what the stand-in set leaves out when this plugin is loaded).
pub fn panel_ids() -> Vec<&'static str> {
    PANELS.iter().map(|(id, ..)| *id).collect()
}

/// The plugin (see the crate docs).
pub struct PanelsScene {
    manifest: Manifest,
}

impl PanelsScene {
    pub fn new() -> Result<Self, PluginError> {
        let mut provides: Vec<String> = PANELS
            .iter()
            .map(|(id, ..)| format!("EditorPanel(\"{id}\")"))
            .collect();
        provides.extend(
            tools()
                .iter()
                .map(|(id, _)| format!("ViewportTool(\"{id}\")")),
        );
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.panels.scene\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
        })
    }
}

impl SourcePlugin for PanelsScene {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, title, dock, tip) in PANELS {
            let build: fn(&mut PanelCx) = match *id {
                "forge.hierarchy" => hierarchy::build,
                "forge.viewport" => viewport::build,
                "forge.play_controls" => play::build,
                _ => inspector::build,
            };
            cx.add::<EditorPanel<PanelCx>>(
                id,
                PanelDescriptor {
                    title: forge_ui::l10n::tr(title).to_string(),
                    icon: None,
                    default_dock: *dock,
                    build: with_tip(build, tip),
                },
                Order::Last,
            )?;
        }
        for (id, item) in tools() {
            cx.add::<ViewportTool>(id, item, Order::Last)?;
        }
        Ok(())
    }
}

/// `build`, after declaring the panel's first-run tip (a localisation key; M2-70).
fn with_tip(
    build: fn(&mut forge_editor::panels::PanelCx),
    tip: &'static str,
) -> forge_plugin::points::BuildFn<forge_editor::panels::PanelCx> {
    std::sync::Arc::new(move |cx: &mut forge_editor::panels::PanelCx| {
        cx.first_run_tip(forge_ui::l10n::tr(tip));
        build(cx);
    })
}
