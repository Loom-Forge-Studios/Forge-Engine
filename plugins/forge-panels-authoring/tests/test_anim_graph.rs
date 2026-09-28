//! The animation state machine editor (`forge.anim_graph`, DoD M2-64) through the running
//! shell: machines, parameters, states and blend spaces as commands; transitions wired on
//! the graph with their conditions (a bad condition refused with its reason); state moves
//! one transaction; bone masks per bone subtree; blend-sample drags one gesture; the preview
//! runs the machine without writing the project; validation shown on the nodes.

mod common;

use common::{
    activate_row, history_len, last_notice, part, press, raise, rows_with, select_row, setting,
    settings_under, text_of, type_into, undo,
};
use forge_cmd::Value;
use forge_editor::testing::Rig;
use forge_panels_authoring::widgets::{BlendPointMoved, DragPhase, SampleMoved};
use forge_ui::widgets::{
    CanvasSelection, ConnectRequested, NodeCanvas, NodesMoved, NumericCommitted, PinRef,
};
use forge_ui::{Point, WidgetId};

const P: &str = "forge.anim_graph";

fn w(rig: &Rig, path: &[&str]) -> WidgetId {
    part(rig, P, path)
}

fn tap(rig: &mut Rig, path: &[&str]) {
    let b = w(rig, path);
    press(rig, b);
}

fn name(rig: &mut Rig, n: &str) {
    let f = w(rig, &["bar", "name"]);
    type_into(rig, f, n);
}

/// The graph canvas (the node canvas builds under a clip container).
fn canvas(rig: &Rig) -> WidgetId {
    w(rig, &["body", "canvas", "canvas"])
}

/// The canvas key of a state, found by its node title.
fn node(rig: &Rig, title: &str) -> u64 {
    let c = canvas(rig);
    let w = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .unwrap_or_else(|| panic!("canvas"));
    let m = w.model();
    m.nodes()
        .find(|(_, n)| n.title.starts_with(title))
        .map(|(k, _)| *k)
        .unwrap_or_else(|| panic!("no node {title}"))
}

fn add_param(rig: &mut Rig, n: &str, kind: usize) {
    name(rig, n);
    let k = w(rig, &["bar", "kind"]);
    // The radio group writes its signal; select by keyboard (Home, then Down × kind).
    rig.h.ui.set_focus(Some(k), true);
    rig.chord(forge_ui::KeyCode::Home, forge_ui::Modifiers::NONE);
    for _ in 0..kind {
        rig.chord(forge_ui::KeyCode::Down, forge_ui::Modifiers::NONE);
    }
    tap(rig, &["bar", "add_param"]);
}

/// A machine "Locomotion" with speed (Float), grounded (Bool), jump (Trigger) and the
/// states Idle (entry), Walk, Jump.
fn setup() -> Rig {
    let mut rig = common::rig(&[P]);
    name(&mut rig, "Locomotion");
    tap(&mut rig, &["bar", "new_machine"]);
    add_param(&mut rig, "speed", 0);
    add_param(&mut rig, "grounded", 1);
    add_param(&mut rig, "jump", 2);
    let motions = w(&rig, &["body", "side", "motions"]);
    for m in ["Walk", "Jump"] {
        let row = rows_with(&mut rig, motions, &format!("{m} ("));
        activate_row(&mut rig, motions, row[0]);
    }
    rig
}

fn connect(rig: &mut Rig, from: &str, to: &str, conds: &str) {
    let f = w(rig, &["bar", "conds"]);
    type_into(rig, f, conds);
    let c = canvas(rig);
    let (a, b) = (node(rig, from), node(rig, to));
    raise(
        rig,
        c,
        ConnectRequested {
            canvas: c,
            from: PinRef::output(a, 0),
            to: PinRef::input(b, 0),
        },
    );
}

#[test]
fn machines_states_and_transitions_are_commands() {
    let mut rig = setup();
    let m = "anim.machine.locomotion";
    assert_eq!(
        setting(&rig, &format!("{m}.skeleton")),
        Some(Value::Text("Humanoid".into()))
    );
    assert_eq!(
        setting(&rig, &format!("{m}.param.jump.kind")),
        Some(Value::Text("Trigger".into()))
    );
    assert_eq!(
        setting(&rig, &format!("{m}.state.walk.motion")),
        Some(Value::Text("Walk".into()))
    );
    // Wire Idle -> Walk with a condition: a transition.
    let before = history_len(&rig);
    connect(&mut rig, "Idle", "Walk", "speed > 0.5");
    assert_eq!(
        history_len(&rig),
        before + 1,
        "a transition is one undo entry"
    );
    assert_eq!(
        setting(&rig, &format!("{m}.transition.idle_to_walk.cond.c0")),
        Some(Value::Text("speed > 0.5".into()))
    );
    // The wire and its labelled input pin are on the graph.
    let c = canvas(&rig);
    let walk = node(&rig, "Walk");
    let labels: Vec<String> = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .and_then(|w| {
            w.model()
                .node(walk)
                .map(|n| n.inputs.iter().map(|p| p.label.clone()).collect())
        })
        .unwrap_or_default();
    assert!(labels.contains(&"speed > 0.5".to_string()), "{labels:?}");
    // A condition that tests a float as a bool is refused with the reason; nothing sent.
    let before = history_len(&rig);
    connect(&mut rig, "Walk", "Idle", "speed");
    assert_eq!(history_len(&rig), before);
    let (_, why) = last_notice(&rig).unwrap_or_default();
    assert!(why.contains("is a float"), "{why}");
    // From any state, on the trigger.
    let any = node(&rig, "Any state");
    let jump = node(&rig, "Jump");
    let f = w(&rig, &["bar", "conds"]);
    type_into(&mut rig, f, "jump");
    raise(
        &mut rig,
        c,
        ConnectRequested {
            canvas: c,
            from: PinRef::output(any, 0),
            to: PinRef::input(jump, 0),
        },
    );
    assert_eq!(
        setting(&rig, &format!("{m}.transition.any_to_jump.from")),
        Some(Value::Text("any".into()))
    );
    // Moving two states is one transaction.
    let before = history_len(&rig);
    raise(
        &mut rig,
        c,
        NodesMoved {
            canvas: c,
            moves: vec![
                (walk, Point::new(300.0, 40.0)),
                (jump, Point::new(300.0, 200.0)),
            ],
        },
    );
    assert_eq!(history_len(&rig), before + 1);
    assert_eq!(
        setting(&rig, &format!("{m}.state.walk.y")),
        Some(Value::Float(40.0))
    );
    undo(&mut rig);
    assert_ne!(
        setting(&rig, &format!("{m}.state.walk.y")),
        Some(Value::Float(40.0))
    );
    // Deleting a state removes its transitions in the same transaction.
    raise(
        &mut rig,
        c,
        CanvasSelection {
            canvas: c,
            nodes: vec![walk],
        },
    );
    let before = history_len(&rig);
    tap(&mut rig, &["bar", "delete"]);
    assert_eq!(history_len(&rig), before + 1);
    assert!(settings_under(&rig, &format!("{m}.state.walk.")).is_empty());
    assert!(settings_under(&rig, &format!("{m}.transition.idle_to_walk.")).is_empty());
}

#[test]
fn the_preview_runs_the_machine_and_writes_nothing() {
    let mut rig = setup();
    connect(&mut rig, "Idle", "Walk", "speed > 0.5");
    connect(&mut rig, "Walk", "Idle", "speed < 0.5");
    let hash = rig.state_hash();
    // Set the preview's speed, step: the machine moves to Walk.
    let params = w(&rig, &["body", "side", "params"]);
    let row = rows_with(&mut rig, params, "speed (Float)");
    select_row(&mut rig, params, row[0]);
    let v = w(&rig, &["preview_bar", "value"]);
    raise(
        &mut rig,
        v,
        NumericCommitted {
            field: v,
            value: 1.0,
        },
    );
    tap(&mut rig, &["preview_bar", "step"]);
    let text = text_of(&rig, w(&rig, &["preview"]));
    assert!(text.contains("in Walk"), "{text}");
    assert!(text.contains("idle \u{2192} walk"), "{text}");
    // Back to idle when the speed drops, with a cross-fade (both motions weighted).
    raise(
        &mut rig,
        v,
        NumericCommitted {
            field: v,
            value: 0.0,
        },
    );
    tap(&mut rig, &["preview_bar", "step"]);
    let text = text_of(&rig, w(&rig, &["preview"]));
    assert!(text.contains("in Idle"), "{text}");
    assert!(text.contains("Walk") && text.contains("Idle"), "{text}");
    assert_eq!(rig.state_hash(), hash, "the preview is session state");
}

#[test]
fn blend_spaces_masks_and_validation() {
    let mut rig = setup();
    let m = "anim.machine.locomotion";
    // A 1D blend on speed: three samples.
    name(&mut rig, "Move");
    tap(&mut rig, &["bar", "add_1d"]);
    assert_eq!(
        setting(&rig, &format!("{m}.state.move.blend")),
        Some(Value::Text("1D".into()))
    );
    assert_eq!(
        setting(&rig, &format!("{m}.state.move.sample.s2.motion")),
        Some(Value::Text("Run".into()))
    );
    // Select it: the blend view shows; dragging a sample is one gesture.
    let c = canvas(&rig);
    let mv = node(&rig, "Move");
    raise(
        &mut rig,
        c,
        CanvasSelection {
            canvas: c,
            nodes: vec![mv],
        },
    );
    let blend = w(&rig, &["body", "right", "blend"]);
    assert!(rig.h.ui.is_visible(blend));
    let before = history_len(&rig);
    for (i, phase) in [
        DragPhase::Begin,
        DragPhase::Move,
        DragPhase::Move,
        DragPhase::End,
    ]
    .into_iter()
    .enumerate()
    {
        raise(
            &mut rig,
            blend,
            SampleMoved {
                sample: "s1".into(),
                x: 1.0 + 0.25 * i as f64,
                y: 0.0,
                phase,
            },
        );
    }
    assert_eq!(
        history_len(&rig),
        before + 1,
        "a sample drag is one undo entry"
    );
    assert_eq!(
        setting(&rig, &format!("{m}.state.move.sample.s1.x")),
        Some(Value::Float(1.75))
    );
    // The preview point sets the preview speed; the weights follow (session only).
    raise(&mut rig, blend, BlendPointMoved { x: 2.0, y: 0.0 });
    let params = w(&rig, &["body", "side", "params"]);
    assert!(!rows_with(&mut rig, params, "speed (Float) = 2.00").is_empty());
    // A bone mask: toggling the left shoulder takes its whole arm.
    name(&mut rig, "Left arm");
    tap(&mut rig, &["body", "right", "mask_bar", "new_mask"]);
    let bones = w(&rig, &["body", "right", "bones"]);
    let row = rows_with(&mut rig, bones, "LeftShoulder");
    let before = history_len(&rig);
    activate_row(&mut rig, bones, row[0]);
    assert_eq!(history_len(&rig), before + 1);
    let weights = settings_under(&rig, "anim.mask.left_arm.bone.");
    assert_eq!(
        weights.len(),
        4,
        "shoulder, arm, forearm, hand: {weights:?}"
    );
    tap(&mut rig, &["body", "right", "mask_bar", "assign"]);
    assert_eq!(
        setting(&rig, &format!("{m}.state.move.mask")),
        Some(Value::Text("left_arm".into()))
    );
    // Validation: a state no transition reaches is flagged on its node.
    let problems = text_of(&rig, w(&rig, &["problems"]));
    assert!(problems.contains("unreachable"), "{problems}");
    let err = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .and_then(|w| w.model().node(mv).and_then(|n| n.error.clone()));
    assert!(err.is_some_and(|e| e.contains("unreachable")));
}
