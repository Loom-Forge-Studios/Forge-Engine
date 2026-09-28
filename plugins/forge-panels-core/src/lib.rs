//! `forge-panels-core` — the first-party core panels (Ch.21 §21.21), a source plugin with
//! the id `forge.panels.core`:
//!
//! | Panel | Id | DoD |
//! |---|---|---|
//! | Undo history | `forge.undo_history` | M2-41 |
//! | Notifications | `forge.notifications` | M2-42 |
//! | Settings | `forge.settings` | M2-43 |
//! | Keybindings | `forge.keybindings` | M2-44 |
//! | Command palette | `forge.command_palette` | M2-45 (the palette itself is the shell's) |
//! | Console | `forge.console` | M2-39 |
//! | Profiler | `forge.profiler` | M2-40 |
//!
//! It is registered through the ordinary `EditorPanel` point, exactly as a third-party
//! plugin would be (I16): it uses no capability a third-party plugin lacks. Its panels
//! reach the project only through what `PanelCx` hands them — the mirror (read) and the
//! command emitter (write) — so every project edit is a command (I7); settings and
//! keybindings that are user config change the session directly.

#![forbid(unsafe_code)]

mod capture;
mod console;
mod history;
mod keybindings;
mod notices;
mod palette_panel;
pub mod profiler;
mod rows;
mod settings_panel;

use forge_editor::panels::PanelCx;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

pub use capture::{ChordCapture, ChordCaptured};
pub use history::row_label as history_row_label;

/// The panels this plugin provides: `(id, title, default dock, first-run tip)`. The title
/// and the tip are localisation keys (M2-31).
pub const PANELS: &[(&str, &str, Dock, &str)] = &[
    (
        "forge.undo_history",
        forge_ui::tr_key!("Undo history"),
        Dock::Right,
        forge_ui::tr_key!(
            "Double-click an entry to go back to it; nothing is lost until you make a new edit."
        ),
    ),
    (
        "forge.notifications",
        forge_ui::tr_key!("Notifications"),
        Dock::Right,
        forge_ui::tr_key!("Every refusal and error stays here with its code and what to do next."),
    ),
    (
        "forge.settings",
        forge_ui::tr_key!("Settings"),
        Dock::Floating,
        forge_ui::tr_key!(
            "Your settings stay on this computer; project settings are shared with the team."
        ),
    ),
    (
        "forge.keybindings",
        forge_ui::tr_key!("Keybindings"),
        Dock::Floating,
        forge_ui::tr_key!("Select an action, choose Change binding, then press the new keys."),
    ),
    (
        "forge.command_palette",
        forge_ui::tr_key!("Command palette"),
        Dock::Floating,
        forge_ui::tr_key!(
            "Ctrl+Shift+P opens the palette anywhere: type a few letters of any action."
        ),
    ),
    (
        "forge.console",
        forge_ui::tr_key!("Console"),
        Dock::Bottom,
        forge_ui::tr_key!("Click a line's link to jump to the entity, command or node it names."),
    ),
    (
        "forge.profiler",
        forge_ui::tr_key!("Profiler"),
        Dock::Bottom,
        forge_ui::tr_key!(
            "A budget turns red when it is exceeded; frames appear while the engine runs."
        ),
    ),
];

/// The panel ids (what the stand-in set leaves out when this plugin is loaded).
pub fn panel_ids() -> Vec<&'static str> {
    PANELS.iter().map(|(id, ..)| *id).collect()
}

/// The plugin (see the crate docs).
pub struct PanelsCore {
    manifest: Manifest,
}

impl PanelsCore {
    pub fn new() -> Result<Self, PluginError> {
        let provides: Vec<String> = PANELS
            .iter()
            .map(|(id, ..)| format!("EditorPanel(\"{id}\")"))
            .collect();
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.panels.core\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
        })
    }
}

fn descriptor(
    title: &'static str,
    dock: Dock,
    build: fn(&mut PanelCx),
    tip: &'static str,
) -> PanelDescriptor<PanelCx> {
    PanelDescriptor {
        title: forge_ui::l10n::tr(title).to_string(),
        icon: None,
        default_dock: dock,
        build: with_tip(build, tip),
    }
}

impl SourcePlugin for PanelsCore {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, title, dock, tip) in PANELS {
            let build: fn(&mut PanelCx) = match *id {
                "forge.undo_history" => history::build,
                "forge.notifications" => notices::build,
                "forge.settings" => settings_panel::build,
                "forge.keybindings" => keybindings::build,
                "forge.console" => console::build,
                "forge.profiler" => profiler::build,
                _ => palette_panel::build,
            };
            cx.add::<EditorPanel<PanelCx>>(id, descriptor(title, *dock, build, tip), Order::Last)?;
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
