//! Shared set-up: the editor as the binary assembles it (the core panels plus stand-ins for
//! the panels later work packages build), in a headless `Rig`.
#![allow(dead_code)]

use forge_editor::presets::builtin_preset;
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_core::{PanelsCore, panel_ids};
use forge_plugin::SourcePlugin;
use forge_ui::{Key, WidgetId};

pub fn config(extra: &[&dyn SourcePlugin], config_dir: Option<std::path::PathBuf>) -> ShellConfig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let stand_in = StandInPanels::without(&panel_ids()).unwrap_or_else(|e| panic!("{e}"));
    let mut all: Vec<&dyn SourcePlugin> = vec![&core, &stand_in];
    all.extend_from_slice(extra);
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &all, &[], config_dir).unwrap_or_else(|e| panic!("{e}"))
}

pub fn rig() -> Rig {
    Rig::new(config(&[], None)).unwrap_or_else(|e| panic!("{e}"))
}

/// The widget at `path` (keys) under a panel's frame.
pub fn part(rig: &Rig, panel: &str, path: &[&str]) -> WidgetId {
    let mut id = rig
        .panel_frame(panel)
        .unwrap_or_else(|| panic!("{panel} is not open"));
    for k in path {
        id = id.child(&Key::Str((*k).into()));
    }
    assert!(rig.h.ui.contains(id), "{panel}: no widget at {path:?}");
    id
}

/// A fresh temporary directory for user config.
pub fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "forge-panels-core-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}
