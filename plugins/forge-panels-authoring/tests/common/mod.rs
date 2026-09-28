//! Shared set-up: the editor with the core, scene and authoring panels (stand-ins for the
//! rest) in a headless `Rig`.
#![allow(dead_code)]

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::presets::builtin_preset;
use forge_editor::services::{EditorServices, PanelFaults};
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_authoring::PanelsAuthoring;
use forge_panels_core::PanelsCore;
use forge_panels_scene::PanelsScene;
use forge_plugin::SourcePlugin;
use forge_ui::{Key, WidgetId};

pub fn config() -> ShellConfig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let scene = PanelsScene::new().unwrap_or_else(|e| panic!("{e}"));
    let authoring = PanelsAuthoring::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_scene::panel_ids());
    real.extend(forge_panels_authoring::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&core, &scene, &authoring, &stand_in];
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &all, &[], None).unwrap_or_else(|e| panic!("{e}"))
}

/// A rig showing `panels`, with `faults` and a services hook.
pub fn rig_with(
    panels: &[&str],
    faults: PanelFaults,
    services: impl FnOnce(&mut EditorServices),
) -> Rig {
    let mut cfg = config();
    cfg.services.faults = faults;
    services(&mut cfg.services);
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(panels).unwrap_or_else(|e| panic!("{e}"));
    rig.settle();
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

pub fn settings_under(rig: &Rig, prefix: &str) -> Vec<(String, Value)> {
    rig.shell
        .mirror()
        .settings_under(prefix)
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
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

/// Type into a text field (replacing its text).
pub fn type_into(rig: &mut Rig, field: WidgetId, text: &str) {
    rig.h.ui.set_focus(Some(field), true);
    rig.chord(forge_ui::KeyCode::Char('a'), forge_ui::Modifiers::CTRL);
    for c in text.chars() {
        let mut ev = forge_ui::KeyEvent::press(
            forge_ui::KeyCode::Char(c.to_ascii_lowercase()),
            forge_ui::Modifiers::NONE,
        );
        ev.text = Some(c.to_string());
        rig.h.ui.handle(forge_ui::InputEvent::Key(ev));
    }
    rig.settle();
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

/// Activate a row (Enter) in a list.
pub fn activate_row(rig: &mut Rig, list: WidgetId, key: u64) {
    raise(
        rig,
        list,
        forge_ui::widgets::RowActivated { view: list, key },
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

/// Spawn through another client (a script), returning the new entity's key.
pub fn spawn(rig: &mut Rig, name: &str) -> EntityKey {
    let mut script = rig.connect(Issuer::Script {
        path: "test.fscript".into(),
    });
    let key = forge_editor::core::EditorCore::read(&rig.core, |p| p.next_key());
    script.apply(
        EditorCommand::Spawn {
            name: name.into(),
            parent: None,
        },
        None,
    );
    let _ = script.pump();
    rig.settle();
    key
}

/// Set a property through the editor's own emitter (a human edit, undoable here).
pub fn set_prop(rig: &mut Rig, e: EntityKey, path: &str, v: Value) {
    rig.shell.emitter().emit(EditorCommand::SetProperty {
        entity: e,
        path: path.into(),
        value: v,
    });
    rig.settle();
}

/// The sequencer with an entity ("Crate": a float and a vector property) selected and a
/// clip "Walk" whose float track has keys at 0 s and 1 s (values 1 and 5).
pub fn sequencer_with_keys(faults: PanelFaults) -> (Rig, EntityKey) {
    const P: &str = "forge.sequencer";
    let mut rig = rig_with(&[P], faults, |_| {});
    let e = spawn(&mut rig, "Crate");
    set_prop(
        &mut rig,
        e,
        "transform.position.local",
        Value::Vec3([0.0, 0.0, 0.0]),
    );
    set_prop(&mut rig, e, "light.intensity", Value::Float(1.0));
    rig.shell.session_mut().set_selection(vec![e]);
    rig.settle();
    let name = part(&rig, P, &["bar", "name"]);
    type_into(&mut rig, name, "Walk");
    let b = part(&rig, P, &["bar", "new_clip"]);
    press(&mut rig, b);
    let props = part(&rig, P, &["body", "side", "props"]);
    let row = rows_with(&mut rig, props, "light.intensity");
    activate_row(&mut rig, props, row[0]);
    let tl = part(&rig, P, &["body", "timeline"]);
    rig.h.ui.set_focus(Some(tl), true);
    rig.chord(forge_ui::KeyCode::Home, forge_ui::Modifiers::NONE);
    rig.chord(forge_ui::KeyCode::Right, forge_ui::Modifiers::SHIFT);
    rig.settle();
    set_prop(&mut rig, e, "light.intensity", Value::Float(5.0));
    let k = part(&rig, P, &["bar", "key"]);
    press(&mut rig, k);
    assert_eq!(
        setting(&rig, "seq.clip.walk.track.crate_light_intensity.key.k1.v"),
        Some(Value::Float(5.0)),
        "set-up: the second key"
    );
    (rig, e)
}

/// A label's text.
pub fn text_of(rig: &Rig, id: WidgetId) -> String {
    rig.h
        .ui
        .widget::<forge_ui::widgets::Label>(id)
        .map(|l| l.text(rig.h.ui.rt()))
        .unwrap_or_default()
}
