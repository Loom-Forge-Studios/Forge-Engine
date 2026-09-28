//! The input action map (`forge.input_map`, DoD M2-68) through the running shell: maps
//! and actions as commands; press-to-bind from the keyboard, the mouse and (through the
//! input backend) a gamepad; composites for axes; picking a control; refusals that name
//! both actions of a conflict; and no effect on the editor keymap.

mod common;

use std::rc::Rc;
use std::time::Duration;

use common::{history_len, last_notice, part, press, raise, rows_with, select_row, setting, undo};
use forge_cmd::Value;
use forge_editor::domain::input::{Control, Device, MemoryInput, Shape};
use forge_editor::testing::Rig;
use forge_ui::widgets::{RowActivated, RowRenamed};
use forge_ui::{InputEvent, KeyCode, KeyEvent, Modifiers, WidgetId};

const P: &str = "forge.input_map";

fn w(rig: &Rig, path: &[&str]) -> WidgetId {
    part(rig, P, path)
}

fn tap(rig: &mut Rig, path: &[&str]) {
    let b = w(rig, path);
    press(rig, b);
}

fn type_name(rig: &mut Rig, name: &str) {
    let f = w(rig, &["bar", "name"]);
    rig.h.ui.set_focus(Some(f), true);
    rig.chord(KeyCode::Char('a'), Modifiers::CTRL);
    for c in name.chars() {
        let mut ev = KeyEvent::press(KeyCode::Char(c.to_ascii_lowercase()), Modifiers::NONE);
        ev.text = Some(c.to_string());
        rig.h.ui.handle(InputEvent::Key(ev));
    }
    rig.settle();
}

fn key(rig: &mut Rig, c: char) {
    rig.chord(KeyCode::Char(c), Modifiers::NONE);
}

fn select_action(rig: &mut Rig, name: &str) {
    let list = w(rig, &["body", "actions"]);
    let k = rows_with(rig, list, name);
    let k = *k
        .first()
        .unwrap_or_else(|| panic!("no action row {name:?}"));
    select_row(rig, list, k);
}

fn setup(rig: &mut Rig) {
    type_name(rig, "Gameplay");
    tap(rig, &["bar", "new_map"]);
    type_name(rig, "Jump");
    tap(rig, &["bar", "new_action"]);
}

#[test]
fn actions_and_press_to_bind_are_commands_and_undo() {
    let mut rig = common::rig(&[P]);
    let keymap_before = format!("{:?}", rig.shell.session().keymap().borrow().bindings());
    let start = rig.state_hash();
    setup(&mut rig);
    assert_eq!(
        setting(&rig, "input.map.gameplay.name"),
        Some(Value::Text("Gameplay".into()))
    );
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.jump.kind"),
        Some(Value::Text("Button".into()))
    );
    // Press to bind: the next key.
    select_action(&mut rig, "Jump");
    tap(&mut rig, &["bar", "press"]);
    let cap = w(&rig, &["capture"]);
    assert!(!rig.h.ui.is_hidden(cap), "the capture shows");
    assert_eq!(rig.h.ui.focused(), Some(cap), "and takes focus");
    rig.chord(KeyCode::Space, Modifiers::NONE);
    rig.settle();
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.jump.bind.0"),
        Some(Value::Text("Keyboard/Space".into()))
    );
    assert!(rig.h.ui.is_hidden(cap));
    // Pick a control from the list: the gamepad's South button.
    let controls = w(&rig, &["body", "controls"]);
    let south = rows_with(&mut rig, controls, "South");
    select_action(&mut rig, "Jump");
    raise(
        &mut rig,
        controls,
        RowActivated {
            view: controls,
            key: south[0],
        },
    );
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.jump.bind.1"),
        Some(Value::Text("Gamepad/South".into()))
    );
    // A stick does not fit a button action: refused with the reason.
    let stick = rows_with(&mut rig, controls, "LeftStick (Axis2D)");
    let before = history_len(&rig);
    raise(
        &mut rig,
        controls,
        RowActivated {
            view: controls,
            key: stick[0],
        },
    );
    assert_eq!(history_len(&rig), before);
    let (_, why) = last_notice(&rig).unwrap_or_default();
    assert!(why.contains("Axis2D") && why.contains("Button"), "{why}");
    // Rename in place.
    let list = w(&rig, &["body", "actions"]);
    let row = rows_with(&mut rig, list, "Jump (Button)");
    raise(
        &mut rig,
        list,
        RowRenamed {
            view: list,
            key: row[0],
            name: "Leap".into(),
        },
    );
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.jump.name"),
        Some(Value::Text("Leap".into()))
    );
    // The editor keymap is untouched: game bindings are project state.
    assert_eq!(
        format!("{:?}", rig.shell.session().keymap().borrow().bindings()),
        keymap_before
    );
    // Every edit undoes.
    for _ in 0..6 {
        undo(&mut rig);
    }
    assert_eq!(rig.state_hash(), start);
}

#[test]
fn a_conflict_is_refused_naming_both_actions_and_composites_drive_axes() {
    let mut rig = common::rig(&[P]);
    setup(&mut rig);
    select_action(&mut rig, "Jump");
    tap(&mut rig, &["bar", "press"]);
    rig.chord(KeyCode::Space, Modifiers::NONE);
    rig.settle();
    type_name(&mut rig, "Fire");
    tap(&mut rig, &["bar", "new_action"]);
    select_action(&mut rig, "Fire");
    let before = history_len(&rig);
    tap(&mut rig, &["bar", "press"]);
    rig.chord(KeyCode::Space, Modifiers::NONE);
    rig.settle();
    assert_eq!(
        history_len(&rig),
        before,
        "the conflicting binding was not written"
    );
    let (title, why) = last_notice(&rig).unwrap_or_default();
    assert!(
        title.contains("refused") && why.contains("Jump") && why.contains("Fire"),
        "{title}: {why}"
    );
    // A 2D axis from four presses.
    type_name(&mut rig, "Move");
    let kind = w(&rig, &["bar", "kind"]);
    rig.h.ui.set_focus(Some(kind), true);
    rig.chord(KeyCode::Down, Modifiers::NONE);
    rig.chord(KeyCode::Down, Modifiers::NONE);
    rig.settle();
    tap(&mut rig, &["bar", "new_action"]);
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.move.kind"),
        Some(Value::Text("Axis2D".into()))
    );
    select_action(&mut rig, "Move");
    tap(&mut rig, &["bar", "composite"]);
    for c in ['w', 's', 'a', 'd'] {
        key(&mut rig, c);
    }
    rig.settle();
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.move.bind.0"),
        Some(Value::Text(
            "Composite2D(Keyboard/W, Keyboard/S, Keyboard/A, Keyboard/D)".into()
        ))
    );
    // Esc cancels a capture and writes nothing.
    let before = history_len(&rig);
    tap(&mut rig, &["bar", "press"]);
    rig.chord(KeyCode::Escape, Modifiers::NONE);
    rig.settle();
    assert_eq!(history_len(&rig), before);
    // Removing a binding row.
    let list = w(&rig, &["body", "actions"]);
    let b = rows_with(&mut rig, list, "Composite2D");
    select_row(&mut rig, list, b[0]);
    tap(&mut rig, &["bar", "remove"]);
    assert_eq!(setting(&rig, "input.map.gameplay.action.move.bind.0"), None);
}

#[test]
fn a_gamepad_press_is_captured_through_the_backend_poll() {
    let input = Rc::new(MemoryInput::new());
    let i2 = input.clone();
    let mut rig = common::rig_with(&[P], Default::default(), move |sv| sv.input = i2);
    setup(&mut rig);
    select_action(&mut rig, "Jump");
    tap(&mut rig, &["bar", "press"]);
    input.inject(Control {
        device: Device::Gamepad,
        name: "East".into(),
        shape: Shape::Button,
    });
    rig.advance(Duration::from_millis(200));
    assert_eq!(
        setting(&rig, "input.map.gameplay.action.jump.bind.0"),
        Some(Value::Text("Gamepad/East".into()))
    );
    // The poll stops with the capture: no timer keeps the editor awake afterwards.
    rig.advance(Duration::from_secs(5));
    let (frames, wakeups) = rig.advance(Duration::from_secs(5));
    assert_eq!((frames, wakeups), (0, 0), "a finished capture kept polling");
}
