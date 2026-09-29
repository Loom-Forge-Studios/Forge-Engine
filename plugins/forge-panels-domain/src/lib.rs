//! `forge-panels-domain` — the first-party domain editors (Ch.21 §21.21), a source plugin
//! with the id `forge.panels.domain`:
//!
//! | Panel | Id | DoD | Backend (D-4) |
//! |---|---|---|---|
//! | 2D editors (tile palette, sprite sheet, 2D rig) | `forge.editors_2d` | M2-66 | `Scene2d`, in-memory until `forge-2d` (M4-11) |
//! | Audio mixer | `forge.audio_mixer` | M2-67 | `AudioBuses`, in-memory until `forge-audio` |
//! | Input action map | `forge.input_map` | M2-68 | `InputActions`: `forge-input` (`DeviceInput`, WP-65) |
//! | Input debugger | `forge.input_debugger` | M7-12 | `InputActions::debug_snapshot`: the `forge-input` runtime |
//!
//! Registered through the ordinary `EditorPanel` point, exactly as a third-party plugin
//! would be (I16), and available under every preset (I15); the 2D preset opens the 2D
//! editors in its default layout. The panels reach the project only through what
//! `PanelCx` hands them — the mirror (read), the command emitter (write) and the read-side
//! services — so **every project edit is a command** (I7): the models are project settings
//! (`forge_editor::domain`), and a paint stroke, a fader drag or a bone rotation is one
//! gesture, one undo entry. Selections, the brush, test tones and the spatial preview's
//! source position are session state.

#![forbid(unsafe_code)]

mod common;
pub mod editors_2d;
pub mod input_debugger;
pub mod input_map;
pub mod mixer;
pub mod widgets;

use forge_editor::panels::PanelCx;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

/// The panels this plugin provides: `(id, title, default dock, first-run tip)`. The title
/// and the tip are localisation keys (M2-31).
pub const PANELS: &[(&str, &str, Dock, &str)] = &[
    (
        "forge.editors_2d",
        forge_ui::tr_key!("2D editors"),
        Dock::Right,
        forge_ui::tr_key!("Pick a tile set, a sprite sheet or a rig to edit it."),
    ),
    (
        "forge.audio_mixer",
        forge_ui::tr_key!("Audio mixer"),
        Dock::Bottom,
        forge_ui::tr_key!("Drag a bus onto another to re-route it."),
    ),
    (
        "forge.input_map",
        forge_ui::tr_key!("Input map"),
        Dock::Center,
        forge_ui::tr_key!("These are the game's input actions, not the editor's shortcuts."),
    ),
    (
        "forge.input_debugger",
        forge_ui::tr_key!("Input debugger"),
        Dock::Bottom,
        forge_ui::tr_key!("Turn Live on and press a button to watch the actions it drives."),
    ),
];

/// The panel ids (what the stand-in set leaves out when this plugin is loaded).
pub fn panel_ids() -> Vec<&'static str> {
    PANELS.iter().map(|(id, ..)| *id).collect()
}

/// The plugin (see the crate docs).
pub struct PanelsDomain {
    manifest: Manifest,
}

impl PanelsDomain {
    pub fn new() -> Result<Self, PluginError> {
        let provides: Vec<String> = PANELS
            .iter()
            .map(|(id, ..)| format!("EditorPanel(\"{id}\")"))
            .collect();
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.panels.domain\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
        })
    }
}

impl SourcePlugin for PanelsDomain {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, title, dock, tip) in PANELS {
            let build: fn(&mut PanelCx) = match *id {
                "forge.editors_2d" => editors_2d::build,
                "forge.audio_mixer" => mixer::build,
                "forge.input_debugger" => input_debugger::build,
                _ => input_map::build,
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
