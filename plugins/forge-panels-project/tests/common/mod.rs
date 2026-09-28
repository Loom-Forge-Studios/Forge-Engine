//! Shared set-up: the editor with the core and project panels (stand-ins for the rest) in a
//! headless `Rig`, over a core whose projects live in a per-test temporary folder.
#![allow(dead_code)]

use std::path::PathBuf;

use forge_cmd::{Issuer, Value};
use forge_editor::core::{EditorCore, LocalBus};
use forge_editor::presets::builtin_preset;
use forge_editor::project::ProjectStatus;
use forge_editor::services::{EditorServices, PanelFaults};
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_core::PanelsCore;
use forge_panels_project::PanelsProject;
use forge_plugin::SourcePlugin;
use forge_ui::widgets::Label;
use forge_ui::{Key, WidgetId};

pub fn config(preset: &str) -> ShellConfig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let project = PanelsProject::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_project::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&core, &project, &stand_in];
    let preset = builtin_preset(preset).unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &all, &[], None).unwrap_or_else(|e| panic!("{e}"))
}

/// A fresh temporary folder for one test (removed first if a previous run left it).
pub fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "forge-panels-project-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A rig showing `panels`, new projects going under `projects`, with `faults`.
pub fn rig_in(
    panels: &[&str],
    projects: &std::path::Path,
    faults: PanelFaults,
    services: impl FnOnce(&mut EditorServices),
) -> Rig {
    let mut cfg = config("3d");
    cfg.services.faults = faults;
    cfg.services.launcher.borrow_mut().projects_dir = projects.to_path_buf();
    services(&mut cfg.services);
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(panels).unwrap_or_else(|e| panic!("{e}"));
    rig
}

pub fn rig(panels: &[&str], projects: &std::path::Path) -> Rig {
    rig_in(panels, projects, PanelFaults::default(), |_| {})
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

/// Click a widget (a real pointer click) and let the editor settle.
pub fn click(rig: &mut Rig, id: WidgetId) {
    rig.h.click(id);
    rig.turn();
    rig.settle();
}

/// Select a row (by key) in a list and tell the panel (a click on the row).
pub fn select_row(rig: &mut Rig, list: WidgetId, key: u64) {
    forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| t.select(&[key]));
    rig.h.ui.raise(
        list,
        forge_ui::widgets::SelectionChanged {
            view: list,
            keys: vec![key],
        },
    );
    rig.turn();
    rig.settle();
}

/// Type into a text field (focus it, select what is there, type).
pub fn type_into(rig: &mut Rig, field: WidgetId, text: &str) {
    rig.h.ui.set_focus(Some(field), true);
    rig.turn();
    rig.chord(forge_ui::KeyCode::Char('a'), forge_ui::Modifiers::CTRL);
    rig.h.type_text(text);
    rig.turn();
    rig.settle();
}

/// A label's text.
pub fn label(rig: &Rig, id: WidgetId) -> String {
    rig.h
        .ui
        .widget::<Label>(id)
        .map(|l| l.text(rig.h.ui.rt()))
        .unwrap_or_default()
}

pub fn visible(rig: &Rig, id: WidgetId) -> bool {
    rig.h.ui.contains(id) && !rig.h.ui.is_hidden(id)
}

pub fn setting(rig: &Rig, key: &str) -> Option<Value> {
    rig.shell.mirror().setting(key).cloned()
}

/// The core's lifecycle status.
pub fn status(rig: &Rig) -> std::sync::Arc<ProjectStatus> {
    EditorCore::project_status(&rig.core)
}

/// The newest notification's title and detail.
pub fn last_notice(rig: &Rig) -> Option<(String, String)> {
    let s = rig.shell.session();
    s.notifications
        .history()
        .next_back()
        .map(|n| (n.title.clone(), n.detail.clone()))
}

/// A script client on the rig's core.
pub fn script(rig: &Rig) -> LocalBus {
    rig.connect(Issuer::Script {
        path: "setup.fscript".into(),
    })
}

/// Entity names in the mirror, sorted.
pub fn names(rig: &Rig) -> Vec<String> {
    let m = rig.shell.mirror();
    let mut v: Vec<String> = m.entities().map(|(_, e)| e.name.clone()).collect();
    v.sort();
    v
}
