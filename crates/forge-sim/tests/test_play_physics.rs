//! `test_play_physics` — **Play runs physics** (WP-60, M4-3; gate `C-phys-play`).
//!
//! An edit world with `physics.*` properties, played headless through the same
//! `PlaySession` the editor's play controls drive (`forge_editor::sim_bridge::SimPlay`) and
//! `forge --headless` runs: a box dropped onto a static floor comes to rest on it, on every
//! first-party backend the project's `physics.backend` setting names; an input kicks a
//! physics body; the session records and replays bit for bit; the edit world is never
//! changed; a backend no plugin registered is refused with `SIM-0010`.
//!
//! Positive control (W2): `positive_control_without_physics_the_box_falls_through` — the same
//! box with the same motion but no `physics.body` goes through the floor.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use forge_cmd::{EntityKey, Value};
use forge_sim::edit::{
    P_ACCELERATION, P_BODY, P_FRAME, P_LOCAL, P_SHAPE, P_SIZE, P_VELOCITY, S_BACKEND,
};
use forge_sim::{
    EditEntity, EditSnapshot, InputAction, PlayCommand, PlaySession, SimError, SimInput, replay,
};
use forge_trace::Tracer;

fn tracer() -> &'static Tracer {
    Box::leak(Box::new(Tracer::new()))
}

fn ent(key: u64, props: Vec<(&str, Value)>) -> EditEntity {
    let mut p: BTreeMap<String, Value> =
        props.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    p.entry(P_FRAME.into()).or_insert(Value::Int(0));
    EditEntity {
        key: EntityKey(key),
        name: format!("e{key}"),
        parent: None,
        properties: p,
    }
}

/// A 20 x 1 x 20 static floor with its top at y = 0 and a 1 m dynamic box above it, in
/// frame 5; `physics` false leaves the box a plain accelerating body (the control).
fn scene(backend: Option<&str>, physics: bool) -> EditSnapshot {
    let floor = ent(
        1,
        vec![
            (P_FRAME, Value::Int(5)),
            (P_LOCAL, Value::Vec3([0.0, -0.5, 0.0])),
            (P_BODY, Value::Text("static".into())),
            (P_SIZE, Value::Vec3([20.0, 1.0, 20.0])),
        ],
    );
    let mut boxy = vec![
        (P_FRAME, Value::Int(5)),
        (P_LOCAL, Value::Vec3([0.0, 3.0, 0.0])),
        (P_VELOCITY, Value::Vec3([0.5, 0.0, 0.0])),
    ];
    if physics {
        boxy.push((P_BODY, Value::Text("dynamic".into())));
        boxy.push((P_SHAPE, Value::Text("box".into())));
    } else {
        boxy.push((P_ACCELERATION, Value::Vec3([0.0, -9.81, 0.0])));
    }
    let snap = EditSnapshot::from_entities([floor, ent(2, boxy), ent(3, vec![])]);
    let settings: BTreeMap<String, Value> = backend
        .map(|b| (S_BACKEND.to_owned(), Value::Text(b.into())))
        .into_iter()
        .collect();
    snap.with_settings(settings.iter().map(|(k, v)| (k.as_str(), v)))
}

fn box_y(s: &PlaySession) -> f64 {
    s.world()
        .unwrap()
        .transform_of(EntityKey(2))
        .unwrap()
        .0
        .local
        .y
}

#[test]
fn a_box_falls_onto_the_floor_and_rests_on_every_backend() {
    for b in [None, Some("avian3d"), Some("rapier3d")] {
        let edit = scene(b, true);
        let before = edit.hash();
        let mut s = PlaySession::with_tracer(tracer());
        s.control(PlayCommand::Play, &edit).unwrap();
        s.run_steps(180).unwrap();
        let y = box_y(&s);
        assert!(
            (y - 0.5).abs() < 0.03,
            "{b:?}: the box rests at {y}, not 0.5"
        );
        // The viewport's moving set holds the box and not the static floor.
        let moving = s.world().unwrap().moving_transforms();
        assert!(moving.contains_key(&EntityKey(2)), "{b:?}");
        assert!(!moving.contains_key(&EntityKey(1)), "{b:?}");
        assert_eq!(edit.hash(), before, "{b:?}: Play changed the edit world");
        s.control(PlayCommand::Stop, &edit).unwrap();
        assert!(s.world().is_none());
    }
}

#[test]
fn positive_control_without_physics_the_box_falls_through() {
    let edit = scene(None, false);
    let mut s = PlaySession::with_tracer(tracer());
    s.control(PlayCommand::Play, &edit).unwrap();
    s.run_steps(180).unwrap();
    let y = box_y(&s);
    assert!(y < -5.0, "without physics the box stopped at {y}");
}

#[test]
fn inputs_kick_physics_bodies_and_a_physics_session_replays_bit_for_bit() {
    for b in ["avian3d", "rapier3d"] {
        let edit = scene(Some(b), true);
        let mut s = PlaySession::with_tracer(tracer());
        s.set_embed_edit(true);
        s.control(PlayCommand::Play, &edit).unwrap();
        s.run_steps(120).unwrap();
        let rest = box_y(&s);
        s.input(SimInput {
            entity: EntityKey(2),
            action: InputAction::Impulse([0.0, 6.0, 0.0]),
        })
        .unwrap();
        s.run_steps(20).unwrap();
        let up = box_y(&s);
        assert!(up > rest + 0.5, "{b}: the kick lifted the box only to {up}");
        s.run_steps(100).unwrap();
        let hash = s.state_hash().unwrap();
        s.control(PlayCommand::Stop, &edit).unwrap();
        let rec = s.take_recording().unwrap();
        let report = replay(&rec, None, PlaySession::with_tracer(tracer())).unwrap();
        assert_eq!(report.final_hash, hash, "{b}: the replay diverged");
    }
}

#[test]
fn an_unknown_backend_is_refused_with_a_code() {
    let edit = scene(Some("no-such-backend"), true);
    let mut s = PlaySession::with_tracer(tracer());
    let e = s.control(PlayCommand::Play, &edit).unwrap_err();
    assert!(matches!(e, SimError::Physics(_)), "{e}");
    let text = e.to_string();
    assert!(
        text.starts_with("SIM-0010") && text.contains("PHYS-0003"),
        "{text}"
    );
}

#[test]
fn the_backend_setting_is_part_of_the_edit_hash() {
    // Two projects that differ only in their physics backend are different edit worlds (a
    // replay against the other one is refused as a scene mismatch), and a project without
    // physics settings hashes as it did before WP-60.
    let a = scene(Some("avian3d"), true);
    let r = scene(Some("rapier3d"), true);
    assert_ne!(a.hash(), r.hash());
    let plain = scene(None, true);
    assert_eq!(
        plain.hash(),
        EditSnapshot::from_entities(plain.entities.clone()).hash()
    );
}
