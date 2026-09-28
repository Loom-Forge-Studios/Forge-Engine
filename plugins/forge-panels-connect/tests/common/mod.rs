//! Shared set-up: the editor with the core and connect panels (stand-ins for the rest) in a
//! headless `Rig`, over a core the connect services follow.
#![allow(dead_code)]

use forge_cmd::Issuer;
use forge_editor::core::{EditorCore, LocalBus, SharedCore};
use forge_editor::presets::builtin_preset;
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_connect::PanelsConnect;
use forge_panels_core::PanelsCore;
use forge_plugin::SourcePlugin;
use forge_ui::widgets::{Label, SelectionChanged, VirtualTree};
use forge_ui::{Key, WidgetId};

pub fn config() -> ShellConfig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let connect = PanelsConnect::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_connect::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&core, &connect, &stand_in];
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &all, &[], None).unwrap_or_else(|e| panic!("{e}"))
}

/// A rig over a fresh core the connect services follow; `edit` adjusts the services first.
pub fn rig_with(
    panels: &[&str],
    config_dir: Option<&std::path::Path>,
    edit: impl FnOnce(&mut ShellConfig, &SharedCore),
) -> Rig {
    let core = EditorCore::new();
    let mut cfg = config();
    cfg.services.connect.attach(&core, config_dir);
    edit(&mut cfg, &core);
    let mut rig = Rig::with_core(cfg, core).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(panels).unwrap_or_else(|e| panic!("{e}"));
    rig
}

pub fn rig(panels: &[&str]) -> Rig {
    rig_with(panels, None, |_, _| {})
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

/// Press a button (as a click would) and let the editor settle.
pub fn press(rig: &mut Rig, id: WidgetId) {
    rig.h.ui.raise(id, forge_ui::widgets::Pressed(id));
    rig.turn();
    rig.settle();
}

/// Select a row (by key) in a list and tell the panel.
pub fn select_row(rig: &mut Rig, list: WidgetId, key: u64) {
    VirtualTree::edit(&mut rig.h.ui, list, |t| t.select(&[key]));
    rig.h.ui.raise(
        list,
        SelectionChanged {
            view: list,
            keys: vec![key],
        },
    );
    rig.turn();
    rig.settle();
}

/// Select the first top-level row whose label contains `needle`; its key.
pub fn select_containing(rig: &mut Rig, list: WidgetId, needle: &str) -> u64 {
    let key = VirtualTree::edit(&mut rig.h.ui, list, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r))
            .find(|k| t.label_of(*k).is_some_and(|l| l.contains(needle)))
    })
    .flatten()
    .unwrap_or_else(|| panic!("no row contains {needle:?}: {:?}", rows(rig, list)));
    select_row(rig, list, key);
    key
}

/// Every row label of a list, in order.
pub fn rows(rig: &mut Rig, list: WidgetId) -> Vec<String> {
    VirtualTree::edit(&mut rig.h.ui, list, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r).and_then(|k| t.label_of(k)).map(str::to_string))
            .collect()
    })
    .unwrap_or_default()
}

/// A label's text.
pub fn label(rig: &Rig, id: WidgetId) -> String {
    rig.h
        .ui
        .widget::<Label>(id)
        .map(|l| l.text(rig.h.ui.rt()))
        .unwrap_or_default()
}

/// Let live feeds deliver (they refresh at most twice a second).
pub fn feed(rig: &mut Rig) {
    rig.advance(std::time::Duration::from_millis(1200));
    rig.settle();
}

/// A human client of the rig's core (a second window, a test's own requests).
pub fn human(rig: &Rig) -> LocalBus {
    rig.connect(Issuer::Human {
        user: "tester".into(),
    })
}

/// An automation client of the rig's core: a non-human session issuing through the bus
/// like the UI (what a plugin that hosts sessions over the core gives each of them).
pub fn automation(rig: &Rig, session: &str) -> LocalBus {
    rig.connect(Issuer::Automation {
        session: session.into(),
        tool: "test".into(),
    })
}

/// Entity names in the core's project, sorted.
pub fn names(rig: &Rig) -> Vec<String> {
    let mut v: Vec<String> = EditorCore::read(&rig.core, |p| {
        p.entities().map(|(_, e)| e.name().to_string()).collect()
    });
    v.sort();
    v
}
