//! The input map and the input debugger over the **real input layer** (WP-65, DoD M7-12;
//! Ch.28 §28.9, §28.18): `forge_editor::domain::input::DeviceInput`, a `forge-input` runtime,
//! with a virtual gamepad standing in for a pad on the desk (gilrs is not started: the test
//! connects its own device).
//!
//! * `C-input-backend`: "press to bind" captures a gamepad button through the runtime's
//!   device poll, and what the panel authored (bindings, triggers, modifiers) is exactly
//!   what the runtime plays: the project's settings compile into a runtime whose Hold
//!   trigger and deadzone behave as authored. Control: a backend whose capture never pumps
//!   the devices binds nothing.
//! * `C-input-debugger`: the debugger shows devices, players and action phases from the
//!   live runtime; opening it reads no device, Refresh reads once, Live refreshes while on,
//!   and with Live off the editor sleeps (0 frames, 0 wakeups). Control: a Live toggle that
//!   keeps its timer after Off.

mod common;

use std::rc::Rc;
use std::time::Duration;

use common::{last_notice, part, press, rows_with, select_row, setting};
use forge_cmd::Value;
use forge_editor::domain::input::{DeviceInput, DeviceInputFaults, InputMapDef, runtime_settings};
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;
use forge_input::{Device, DeviceDesc, DeviceId, InputRuntime, PadFamily, PlayerId};
use forge_ui::{InputEvent, KeyCode, KeyEvent, Modifiers, WidgetId};

const MAP: &str = "forge.input_map";
const DBG: &str = "forge.input_debugger";

fn pad(input: &DeviceInput) -> DeviceId {
    input.runtime().connect(
        DeviceDesc::new(Device::Gamepad, "Virtual pad", "vpad")
            .family(PadFamily::PlayStation)
            .virtual_device(),
    )
}

fn type_into(rig: &mut Rig, field: WidgetId, text: &str) {
    rig.h.ui.set_focus(Some(field), true);
    rig.chord(KeyCode::Char('a'), Modifiers::CTRL);
    for c in text.chars() {
        let code = if c == ' ' {
            KeyCode::Space
        } else {
            KeyCode::Char(c.to_ascii_lowercase())
        };
        let mut ev = KeyEvent::press(code, Modifiers::NONE);
        ev.text = Some(c.to_string());
        rig.h.ui.handle(InputEvent::Key(ev));
    }
    rig.settle();
}

fn tap(rig: &mut Rig, panel: &str, path: &[&str]) {
    let b = part(rig, panel, path);
    press(rig, b);
}

fn select_action(rig: &mut Rig, name: &str) {
    let list = part(rig, MAP, &["body", "actions"]);
    let k = rows_with(rig, list, name);
    select_row(
        rig,
        list,
        *k.first().unwrap_or_else(|| panic!("no row {name:?}")),
    );
}

/// A Gameplay map with a Jump action, then press-to-bind with the pad's North.
fn bind_jump_by_pad(faults: DeviceInputFaults) -> (Rig, Rc<DeviceInput>, DeviceId) {
    let input = Rc::new(DeviceInput::virtual_only().with_faults(faults));
    let i2 = input.clone();
    let mut rig = common::rig_with(&[MAP], PanelFaults::default(), move |sv| sv.input = i2);
    let p = pad(&input);
    let name = part(&rig, MAP, &["bar", "name"]);
    type_into(&mut rig, name, "Gameplay");
    tap(&mut rig, MAP, &["bar", "new_map"]);
    type_into(&mut rig, name, "Jump");
    tap(&mut rig, MAP, &["bar", "new_action"]);
    select_action(&mut rig, "Jump");
    tap(&mut rig, MAP, &["bar", "press"]);
    // The player presses North on the pad.
    input.runtime().send(p, "North", [1.0, 0.0]);
    rig.advance(Duration::from_millis(200));
    input.runtime().send(p, "North", [0.0, 0.0]);
    rig.advance(Duration::from_millis(200));
    (rig, input, p)
}

#[test]
fn a_gamepad_press_binds_through_the_input_runtime() {
    let (rig, input, _) = bind_jump_by_pad(DeviceInputFaults::default());
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.jump.bind.0"),
        Some(Value::Text("Gamepad/North".into()))
    );
    let b = forge_editor::domain::input::InputActions::backend(&*input);
    assert!(!b.in_memory, "the real backend is labelled in-memory");
}

#[test]
fn positive_control_a_backend_that_never_pumps_binds_nothing() {
    let (rig, _, _) = bind_jump_by_pad(DeviceInputFaults { no_pump: true });
    assert_eq!(setting(&rig, "input.map.gameplay.action.jump.bind.0"), None);
}

#[test]
fn what_the_panel_authors_is_what_the_runtime_plays() {
    let (mut rig, _, _) = bind_jump_by_pad(DeviceInputFaults::default());
    // A trigger naming an action that does not exist is refused, by name.
    select_action(&mut rig, "Jump");
    let text = part(&rig, MAP, &["rules", "text"]);
    type_into(&mut rig, text, "Chord(gameplay/aim)");
    tap(&mut rig, MAP, &["rules", "set_triggers"]);
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.jump.triggers"),
        None
    );
    let (_, why) = last_notice(&rig).unwrap_or_default();
    assert!(why.contains("gameplay/aim"), "{why}");
    // A hold, and a deadzone on the binding.
    type_into(&mut rig, text, "Hold(0.5)");
    tap(&mut rig, MAP, &["rules", "set_triggers"]);
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.jump.triggers"),
        Some(Value::Text("Hold(0.5)".into()))
    );
    let list = part(&rig, MAP, &["body", "actions"]);
    let b = rows_with(&mut rig, list, "Gamepad/North");
    select_row(&mut rig, list, b[0]);
    type_into(&mut rig, text, "Deadzone(Axial, 0.2, 1)");
    tap(&mut rig, MAP, &["rules", "set_modifiers"]);
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.jump.bindmod.0"),
        Some(Value::Text("Deadzone(Axial, 0.2, 1)".into()))
    );
    assert!(
        !rows_with(&mut rig, list, "Deadzone").is_empty(),
        "the row shows its modifier"
    );
    // The game's runtime compiles the project's settings and plays them.
    let settings = runtime_settings(&rig.shell.mirror());
    let (def, problems) =
        InputMapDef::from_settings(settings.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    assert!(problems.is_empty(), "{problems:?}");
    let mut rt = InputRuntime::new();
    assert!(rt.set_map(&def).is_empty());
    let p = rt.connect(DeviceDesc::new(Device::Gamepad, "pad", "p").virtual_device());
    let jump = rt
        .handle("gameplay/jump")
        .unwrap_or_else(|| panic!("no jump"));
    rt.send(p, "North", [1.0, 0.0]);
    let mut fired_at = None;
    for f in 0..60 {
        rt.update(1.0 / 60.0);
        if rt.action(PlayerId(0), jump).triggered {
            fired_at.get_or_insert(f);
        }
    }
    assert_eq!(
        fired_at,
        Some(30),
        "the authored 0.5 s hold fired at frame {fired_at:?}"
    );
}

fn debugger(faults: PanelFaults) -> (Rig, Rc<DeviceInput>, DeviceId) {
    let input = Rc::new(DeviceInput::virtual_only());
    let i2 = input.clone();
    let mut rig = common::rig_with(&[MAP, DBG], faults, move |sv| sv.input = i2);
    let p = pad(&input);
    // Author a map through the input-map panel (commands), as a user would.
    let name = part(&rig, MAP, &["bar", "name"]);
    type_into(&mut rig, name, "Gameplay");
    tap(&mut rig, MAP, &["bar", "new_map"]);
    type_into(&mut rig, name, "Jump");
    tap(&mut rig, MAP, &["bar", "new_action"]);
    select_action(&mut rig, "Jump");
    let controls = part(&rig, MAP, &["body", "controls"]);
    let south = rows_with(&mut rig, controls, "South (Button)");
    common::raise(
        &mut rig,
        controls,
        forge_ui::widgets::RowActivated {
            view: controls,
            key: south[0],
        },
    );
    rig.settle();
    (rig, input, p)
}

#[test]
fn the_debugger_shows_the_live_runtime_and_sleeps_when_live_is_off() {
    let (mut rig, input, p) = debugger(PanelFaults::default());
    let frame_before = input.runtime().frame();
    rig.settle();
    // Opening (and the map change) read no device.
    assert_eq!(
        input.runtime().frame(),
        frame_before,
        "opening the debugger pumped the devices"
    );
    let actions = part(&rig, DBG, &["body", "actions"]);
    assert!(
        !rows_with(&mut rig, actions, "Jump").is_empty(),
        "the project's action is not shown"
    );
    // Refresh reads the pad: the device row and the triggered action appear.
    input.runtime().send(p, "South", [1.0, 0.0]);
    tap(&mut rig, DBG, &["bar", "refresh"]);
    let devices = part(&rig, DBG, &["body", "devices"]);
    assert!(!rows_with(&mut rig, devices, "Virtual pad").is_empty());
    assert!(!rows_with(&mut rig, devices, "South 1.00").is_empty());
    assert!(
        !rows_with(&mut rig, actions, "Triggered").is_empty(),
        "the pressed action is not Triggered"
    );
    let events = part(&rig, DBG, &["body", "events"]);
    assert!(!rows_with(&mut rig, events, "Gamepad/South").is_empty());
    // Live refreshes ~20 times a second while on.
    let live = part(&rig, DBG, &["bar", "live"]);
    rig.h.ui.set_focus(Some(live), true);
    rig.chord(KeyCode::Enter, Modifiers::NONE);
    let f0 = input.runtime().frame();
    rig.advance(Duration::from_secs(1));
    let n = input.runtime().frame() - f0;
    assert!((15..=25).contains(&n), "{n} refreshes in a second of Live");
    input.runtime().send(p, "South", [0.0, 0.0]);
    rig.advance(Duration::from_millis(200));
    assert!(
        !rows_with(&mut rig, actions, "Jump: Idle").is_empty(),
        "Live did not show the release"
    );
    // Off: nothing is scheduled, the editor sleeps.
    rig.h.ui.set_focus(Some(live), true);
    rig.chord(KeyCode::Enter, Modifiers::NONE);
    rig.advance(Duration::from_secs(1));
    let (frames, wakeups) = rig.advance(Duration::from_secs(5));
    assert_eq!(
        (frames, wakeups),
        (0, 0),
        "the debugger kept the editor awake with Live off"
    );
}

#[test]
fn positive_control_a_live_toggle_that_keeps_its_timer_fails_the_idle_check() {
    let (mut rig, _, _) = debugger(PanelFaults {
        input_debugger_keeps_polling: true,
        ..PanelFaults::default()
    });
    let live = part(&rig, DBG, &["bar", "live"]);
    rig.h.ui.set_focus(Some(live), true);
    rig.chord(KeyCode::Enter, Modifiers::NONE);
    rig.advance(Duration::from_millis(500));
    rig.h.ui.set_focus(Some(live), true);
    rig.chord(KeyCode::Enter, Modifiers::NONE);
    rig.advance(Duration::from_secs(1));
    let (_, wakeups) = rig.advance(Duration::from_secs(5));
    assert!(wakeups > 0, "the control did not keep the editor awake");
}
