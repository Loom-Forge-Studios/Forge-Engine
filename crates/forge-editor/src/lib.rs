//! `forge-editor` — the editor shell (Ch.21 §21.17-§21.19), a client of `forge-cmd` and
//! nothing more (I7).
//!
//! **The split (Ch.7.1, Ch.34).** The editor is a headless core plus clients:
//!
//! * [`core`] — the headless core (the command bus that owns the project) and
//!   [`core::LocalBus`], its in-process client. The only module that names the bus.
//! * [`client`] — the [`client::BusClient`] trait: the one door between the UI and the
//!   core. The split editor's remote transport (M2-16) implements it; nothing above it
//!   changes.
//! * [`mirror`], [`emitter`], [`session`], [`panel_rt`] — what panels get (§21.18): the
//!   project read-only, the command emitter (the only way to change it), session state.
//! * [`shell`] — the main window (menu bar, toolbar, dock, status bar), the palette host,
//!   notifications, the stream pump and the user-config savers.
//! * [`settings`], [`keybind`], [`notify`] — the models behind the settings window, the
//!   keybindings editor and the notification centre (their panels are in
//!   `plugins/forge-panels-core`).
//! * [`headless`] — `forge --headless` / `forge-editor --headless`: the same core, commands
//!   and play controls from a script, replay files recorded and replayed.
//! * [`sim_bridge`] — the real backends behind [`play::PlayBackend`] (`SimPlay`, WP-13's
//!   play core forked from the mirror) and [`profile::CounterSource`] (`TraceCounters`,
//!   `forge-trace`'s live feed).
//!
//! And what WP-U3 built for the shell:
//!
//! * [`panels`] — the `EditorPanel` extension point host: the dock builds every panel from
//!   the plugin registry, so a plugin can add, replace, remove or chain one (I16).
//! * [`presets`] — workspace presets as data (`presets/<preset>/workspace.ron`, Ch.31).
//! * [`layout_store`] — user layouts saved and restored by name, and the crash-safe
//!   debounced autosave. Layouts are user config, not project state.
//! * [`keymap`] and [`keys`] — rebindable, per-context chords with conflict detection, and
//!   the widget that resolves them along the focus path.
//! * [`actions`] and [`palette`] — the action registry (the `Action` extension point) and
//!   the command palette over every action, panel and command.
//!
//! Docking itself — the layout model, the dock area widget, floating windows — is
//! `forge_ui::dock`, the widget layer's.

#![forbid(unsafe_code)]

pub mod actions;
pub mod assets;
pub mod authoring;
pub mod client;
pub mod collab;
pub mod command_labels;
pub mod composition;
pub mod connect;
pub mod console;
pub mod controls;
pub mod core;
pub mod domain;
pub mod emitter;
mod error;
pub mod feed;
pub mod graph;
pub mod headless;
pub mod hosting;
pub mod inspect;
pub mod keybind;
pub mod keymap;
pub mod keys;
pub mod layout_store;
pub mod mirror;
pub mod notify;
pub mod overlay;
pub mod palette;
pub mod panel_rt;
pub mod panels;
pub mod play;
pub mod presets;
pub mod profile;
pub mod project;
pub mod security;
pub mod services;
pub mod session;
pub mod settings;
pub mod shell;
pub mod sim_bridge;
pub mod stand_in;
pub mod testing;
pub mod tips;
pub mod trust;
pub mod user_config;
pub mod viewport;
pub mod viewspec;

pub use error::EditorError;

use forge_plugin::points::{Command, EditorPanel, InspectorWidget, Preset};
use forge_plugin::{Extensions, PluginError};

/// The editor's extension-point host: every point the editor defines, empty, ready for
/// the loader. First-party and third-party plugins fill it the same way (I16).
pub fn editor_extensions() -> Result<Extensions, PluginError> {
    let mut x = Extensions::new();
    x.define::<EditorPanel<panels::PanelCx>>()?;
    x.define::<InspectorWidget<inspect::InspectorCx>>()?;
    x.define::<actions::Action>()?;
    x.define::<Command>()?;
    x.define::<Preset>()?;
    x.define::<overlay::EditorOverlay>()?;
    x.define::<overlay::ThemePoint>()?;
    x.define::<viewport::tool::ViewportTool>()?;
    x.define::<viewport::world::ViewportWorldPoint>()?;
    x.define::<viewport::layer::ViewportLayerPoint>()?;
    x.define::<project::promote::PromotionRulePoint>()?;
    x.define::<services::ServiceProviderPoint>()?;
    x.define::<graph::ir::GeneratorBackendPoint>()?;
    x.define::<settings::SettingLinkPoint>()?;
    x.define::<notify::NoticeHintPoint>()?;
    Ok(x)
}

/// The extension points `forge-editor` itself defines (their item types live here).
pub const DEFINED_POINTS: &[&str] = &[
    "forge.editor.action",
    "forge.editor.overlay",
    "forge.editor.viewport_tool",
    "forge.ui.theme",
    "forge.editor.viewport_world",
    "forge.editor.service_provider",
    "forge.editor.generator_backend",
    "forge.editor.setting_link",
    "forge.editor.notice_hint",
    "forge.editor.viewport_layer",
    "forge.editor.promotion_rule",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_editor_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in EditorError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-editor |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code));
        }
    }

    #[test]
    fn the_editor_defines_its_points() {
        let x = editor_extensions().unwrap_or_else(|e| panic!("{e}"));
        let ids: Vec<&str> = x.points().map(|p| p.id).collect();
        for id in [
            "forge.editor.panel",
            "forge.editor.inspector_widget",
            "forge.editor.action",
            "forge.cmd.command",
            "forge.preset",
            "forge.editor.overlay",
            "forge.editor.viewport_tool",
            "forge.ui.theme",
            "forge.editor.viewport_world",
            "forge.editor.service_provider",
            "forge.editor.generator_backend",
            "forge.editor.setting_link",
            "forge.editor.notice_hint",
        ] {
            assert!(ids.contains(&id), "{id} not defined: {ids:?}");
        }
        for id in DEFINED_POINTS {
            let row = forge_plugin::points::seed_point(id).map(|p| p.defined_in);
            assert_eq!(row, Some("forge-editor"), "{id}");
        }
    }
}
