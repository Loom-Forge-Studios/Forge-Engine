//! `forge-panels-authoring` — the first-party authoring editors (Ch.21 §21.21), a source
//! plugin with the id `forge.panels.authoring`:
//!
//! | Panel | Id | DoD | Backend (D-4) |
//! |---|---|---|---|
//! | Sequencer & timeline | `forge.sequencer` | M2-63 | `AnimSource`, in-memory until `forge-anim`; the preview runs on the play core (`forge-sim`) |
//! | Animation state machine | `forge.anim_graph` | M2-64 | `AnimSource` (in-memory) |
//! | Localisation | `forge.localisation` | M2-65 | `StringTables`, in-memory until `forge-play` (M5-8) |
//!
//! Registered through the ordinary `EditorPanel` point, exactly as a third-party plugin
//! would be (I16), and available under every preset (I15). The panels reach the project
//! only through what `PanelCx` hands them — the mirror (read), the command emitter (write)
//! and the read-side services — so **every project edit is a command** (I7): the models
//! are project settings (`forge_editor::authoring`), and a key drag, a state move or a
//! sample drag is one gesture, one undo entry. The playhead, the preview, the state
//! machine's preview parameters and the pseudo-locale toggle are session state.

#![forbid(unsafe_code)]

pub mod anim_graph;
mod common;
pub mod localisation;
pub mod sequencer;
pub mod widgets;

use forge_editor::panels::PanelCx;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

/// The panels this plugin provides: `(id, title, default dock, first-run tip)`. The title
/// and the tip are localisation keys (M2-31).
pub const PANELS: &[(&str, &str, Dock, &str)] = &[
    (
        "forge.sequencer",
        forge_ui::tr_key!("Sequencer"),
        Dock::Bottom,
        forge_ui::tr_key!(
            "Select an entity, pick a property and press Key to record it at the playhead."
        ),
    ),
    (
        "forge.anim_graph",
        forge_ui::tr_key!("Animation state machine"),
        Dock::Center,
        forge_ui::tr_key!("States are nodes: drag from one state to another to add a transition."),
    ),
    (
        "forge.localisation",
        forge_ui::tr_key!("Localisation"),
        Dock::Center,
        forge_ui::tr_key!(
            "Preview the pseudo-locale to find strings that are not translated or do not fit."
        ),
    ),
];

/// The panel ids (what the stand-in set leaves out when this plugin is loaded).
pub fn panel_ids() -> Vec<&'static str> {
    PANELS.iter().map(|(id, ..)| *id).collect()
}

/// The plugin (see the crate docs).
pub struct PanelsAuthoring {
    manifest: Manifest,
}

impl PanelsAuthoring {
    pub fn new() -> Result<Self, PluginError> {
        let provides: Vec<String> = PANELS
            .iter()
            .map(|(id, ..)| format!("EditorPanel(\"{id}\")"))
            .collect();
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.panels.authoring\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
        })
    }
}

impl SourcePlugin for PanelsAuthoring {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, title, dock, tip) in PANELS {
            let build: fn(&mut PanelCx) = match *id {
                "forge.sequencer" => sequencer::build,
                "forge.anim_graph" => anim_graph::build,
                _ => localisation::build,
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
