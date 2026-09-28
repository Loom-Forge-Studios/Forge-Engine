//! `forge-panels-assets` — the first-party asset panels (Ch.21 §21.21), a source plugin with
//! the id `forge.panels.assets`:
//!
//! | Panel | Id | DoD |
//! |---|---|---|
//! | Asset browser | `forge.assets` | M2-38 |
//! | Graph editor | `forge.graph` | M2-46 |
//!
//! Registered through the ordinary `EditorPanel` point (I16). The browser reads the asset
//! catalogue the shell hands every panel and changes the project only through
//! `forge.asset.*` commands on the emitter (I7). The graph editor (blueprint, material,
//! generator and PCG graphs are assets of the project) edits `graph.*` project settings
//! through the emitter and compiles through the shell's `GraphIr` (I7).

#![forbid(unsafe_code)]

pub mod browser;
pub mod graph;
pub mod index;

pub use index::CatalogIndex;

use forge_editor::panels::PanelCx;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

/// The panels this plugin provides: `(id, title, default dock, first-run tip)`. The title
/// and the tip are localisation keys (M2-31).
pub const PANELS: &[(&str, &str, Dock, &str)] = &[
    (
        "forge.assets",
        forge_ui::tr_key!("Assets"),
        Dock::Bottom,
        forge_ui::tr_key!("Drop files from your computer onto the grid to import them."),
    ),
    (
        "forge.graph",
        forge_ui::tr_key!("Graph editor"),
        Dock::Center,
        forge_ui::tr_key!("Press Space on the canvas to add a node; drag from a pin to wire it."),
    ),
];

/// The panel ids (what the stand-in set leaves out when this plugin is loaded).
pub fn panel_ids() -> Vec<&'static str> {
    PANELS.iter().map(|(id, ..)| *id).collect()
}

/// The plugin (see the crate docs).
pub struct PanelsAssets {
    manifest: Manifest,
}

impl PanelsAssets {
    pub fn new() -> Result<Self, PluginError> {
        let provides: Vec<String> = PANELS
            .iter()
            .map(|(id, ..)| format!("EditorPanel(\"{id}\")"))
            .collect();
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.panels.assets\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
        })
    }
}

impl SourcePlugin for PanelsAssets {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, title, dock, tip) in PANELS {
            cx.add::<EditorPanel<PanelCx>>(
                id,
                PanelDescriptor {
                    title: forge_ui::l10n::tr(title).to_string(),
                    icon: None,
                    default_dock: *dock,
                    build: with_tip(
                        if *id == "forge.graph" {
                            graph::build
                        } else {
                            browser::build
                        },
                        tip,
                    ),
                },
                Order::Last,
            )?;
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
