//! Shared set-up: the editor as the binary assembles it (core, scene and asset panels, the
//! stand-ins for the rest) in a headless `Rig`.
#![allow(dead_code)]

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::presets::builtin_preset;
use forge_editor::services::{EditorServices, PanelFaults};
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_core::PanelsCore;
use forge_panels_scene::PanelsScene;
use forge_plugin::SourcePlugin;
use forge_ui::{Key, WidgetId};

pub fn config(extra: &[&dyn SourcePlugin]) -> ShellConfig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let scene = PanelsScene::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_scene::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let mut all: Vec<&dyn SourcePlugin> = vec![&core, &scene, &stand_in];
    all.extend_from_slice(extra);
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &all, &[], None).unwrap_or_else(|e| panic!("{e}"))
}

/// A rig showing `panels`, with `faults` and a hook to change the services.
pub fn rig_with(
    panels: &[&str],
    faults: PanelFaults,
    services: impl FnOnce(&mut EditorServices),
    extra: &[&dyn SourcePlugin],
) -> Rig {
    let mut cfg = config(extra);
    cfg.services.faults = faults;
    services(&mut cfg.services);
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(panels).unwrap_or_else(|e| panic!("{e}"));
    rig
}

pub fn rig(panels: &[&str]) -> Rig {
    rig_with(panels, PanelFaults::default(), |_| {}, &[])
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

/// The widget at `path` if it exists.
pub fn try_part(rig: &Rig, panel: &str, path: &[&str]) -> Option<WidgetId> {
    let mut id = rig.panel_frame(panel)?;
    for k in path {
        id = id.child(&Key::Str((*k).into()));
    }
    rig.h.ui.contains(id).then_some(id)
}

/// Spawn through another client (a script), returning the new entity's key.
pub fn spawn(rig: &mut Rig, name: &str, parent: Option<EntityKey>) -> EntityKey {
    let mut script = rig.connect(Issuer::Script {
        path: "test.fscript".into(),
    });
    let key = forge_editor::core::EditorCore::read(&rig.core, |p| p.next_key());
    script.apply(
        EditorCommand::Spawn {
            name: name.into(),
            parent,
        },
        None,
    );
    let _ = script.pump();
    rig.turn();
    key
}

/// Set a property through a script client.
pub fn set_prop(rig: &mut Rig, e: EntityKey, path: &str, v: Value) {
    let mut script = rig.connect(Issuer::Script {
        path: "test.fscript".into(),
    });
    script.apply(
        EditorCommand::SetProperty {
            entity: e,
            path: path.into(),
            value: v,
        },
        None,
    );
    let _ = script.pump();
    rig.turn();
}

/// The newest undo-history entry: (label, issuer tag).
pub fn last_entry(rig: &Rig) -> (String, String) {
    let m = rig.shell.mirror();
    let h = m.history().last().unwrap_or_else(|| panic!("an entry"));
    (h.label.clone(), h.issuer_tag())
}

pub fn history_len(rig: &Rig) -> usize {
    rig.shell.mirror().history().len()
}
