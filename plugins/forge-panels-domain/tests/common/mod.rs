//! Shared set-up: the editor with the core and domain panels (stand-ins for the rest) in a
//! headless `Rig`.
#![allow(dead_code)]

use forge_cmd::Value;
use forge_editor::presets::builtin_preset;
use forge_editor::services::{EditorServices, PanelFaults};
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_core::PanelsCore;
use forge_panels_domain::PanelsDomain;
use forge_plugin::SourcePlugin;
use forge_ui::{Key, WidgetId};

pub fn config(preset: &str) -> ShellConfig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let domain = PanelsDomain::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_domain::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&core, &domain, &stand_in];
    let preset = builtin_preset(preset).unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &all, &[], None).unwrap_or_else(|e| panic!("{e}"))
}

/// A rig showing `panels` under the 3D preset, with `faults` and a services hook.
pub fn rig_with(
    panels: &[&str],
    faults: PanelFaults,
    services: impl FnOnce(&mut EditorServices),
) -> Rig {
    let mut cfg = config("3d");
    cfg.services.faults = faults;
    services(&mut cfg.services);
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(panels).unwrap_or_else(|e| panic!("{e}"));
    rig
}

pub fn rig(panels: &[&str]) -> Rig {
    rig_with(panels, PanelFaults::default(), |_| {})
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

pub fn setting(rig: &Rig, key: &str) -> Option<Value> {
    rig.shell.mirror().setting(key).cloned()
}

pub fn history_len(rig: &Rig) -> usize {
    rig.shell.mirror().history().len()
}

/// Undo once and let the mirror follow.
pub fn undo(rig: &mut Rig) {
    rig.shell.emitter().undo();
    rig.settle();
}

/// Raise an action from a widget as if the widget had, then run a loop turn.
pub fn raise<A: 'static>(rig: &mut Rig, source: WidgetId, a: A) {
    rig.h.ui.raise(source, a);
    rig.turn();
    rig.settle();
}

/// Press a button.
pub fn press(rig: &mut Rig, button: WidgetId) {
    raise(rig, button, forge_ui::widgets::Pressed(button));
}

/// Select a row (by key) in a list and tell the panel.
pub fn select_row(rig: &mut Rig, list: WidgetId, key: u64) {
    forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| t.select(&[key]));
    raise(
        rig,
        list,
        forge_ui::widgets::SelectionChanged {
            view: list,
            keys: vec![key],
        },
    );
}

/// The row keys of a list whose label contains `text`.
pub fn rows_with(rig: &mut Rig, list: WidgetId, text: &str) -> Vec<u64> {
    forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| {
        let mut v: Vec<(usize, u64)> = t
            .keys()
            .filter(|k| t.label_of(*k).is_some_and(|l| l.contains(text)))
            .map(|k| (t.row_of(k).unwrap_or(usize::MAX), k))
            .collect();
        v.sort_unstable();
        v.into_iter().map(|(_, k)| k).collect()
    })
    .unwrap_or_default()
}

/// The labels of a list, in row order (expanded rows).
pub fn labels(rig: &mut Rig, list: WidgetId) -> Vec<String> {
    forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r).and_then(|k| t.label_of(k)).map(str::to_string))
            .collect()
    })
    .unwrap_or_default()
}

/// The newest notification's title and detail (a refusal names why).
pub fn last_notice(rig: &Rig) -> Option<(String, String)> {
    let s = rig.shell.session();
    s.notifications
        .history()
        .next_back()
        .map(|n| (n.title.clone(), n.detail.clone()))
}
