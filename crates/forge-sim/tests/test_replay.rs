//! Guard (M2-5): **a recorded play session replays bit-for-bit.**
//!
//! A session is played the way the editor plays it — the editor's clock with jittered frame
//! times and a stall driving `advance`, inputs arriving between frames, pause, single steps,
//! resume, stop — and recorded. The recording is written, read back and replayed with no
//! clock at all; the replaying session's own recording must be **byte-identical** to the
//! file (every control, input and state hash at the same step), and its final state hash
//! equal to the one recorded at Stop.
//!
//! Also: a committed recording (`tests/data/golden_session.forgereplay`) replays on every
//! build and platform — the cross-build half of determinism (`FORGE_BLESS=1` rewrites it after
//! a deliberate change to the simulation); a replay against another scene is refused
//! (`SIM-0006`); a one-ulp change to one recorded input is caught at its step (`SIM-0007`); a
//! crash-truncated file replays up to where it ends.
//!
//! Positive control (W2): `positive_control_a_frame_rate_dependent_step_fails_replay` records
//! with the variable-timestep fault (each step integrates the editor frame's measured time)
//! and requires the replay to diverge.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use forge_cmd::{EntityKey, Value};
use forge_sim::edit::{P_ACCELERATION, P_FRAME, P_LOCAL, P_SPIN, P_VELOCITY, P_YAW};
use forge_sim::{
    EditEntity, EditSnapshot, InputAction, PlayCommand, PlaySession, PlayState, RecEvent,
    Recording, SimError, SimFaults, SimInput, replay,
};
use forge_trace::Tracer;

fn tracer() -> &'static Tracer {
    Box::leak(Box::new(Tracer::new()))
}

/// A small deterministic generator for the scene and the session script.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
    /// Uniform in [-1, 1).
    fn f(&mut self) -> f64 {
        (self.next() as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn scene(seed: u64, n: u64) -> EditSnapshot {
    let mut r = Lcg(seed);
    EditSnapshot::from_entities((1..=n).map(|k| {
        let mut p = std::collections::BTreeMap::new();
        p.insert(
            P_LOCAL.to_owned(),
            Value::Vec3([r.f() * 1e6, r.f() * 100.0, r.f() * 1e3]),
        );
        p.insert(P_FRAME.to_owned(), Value::Int((k % 3) as i64));
        p.insert(P_YAW.to_owned(), Value::Float(r.f() * 180.0));
        match k % 4 {
            0 => {}
            1 => {
                p.insert(
                    P_VELOCITY.to_owned(),
                    Value::Vec3([r.f() * 30.0, r.f() * 30.0, r.f()]),
                );
            }
            2 => {
                p.insert(
                    P_ACCELERATION.to_owned(),
                    Value::Vec3([0.0, -9.81, r.f() * 0.1]),
                );
                p.insert(P_SPIN.to_owned(), Value::Float(r.f() * 720.0));
            }
            _ => {
                p.insert(P_SPIN.to_owned(), Value::Float(r.f() * 45.0));
            }
        }
        EditEntity {
            key: EntityKey(k),
            name: format!("body {k}"),
            parent: None,
            properties: p,
        }
    }))
}

/// Play `edit` as the editor would: jittered frames (8–33 ms, one 700 ms stall), inputs
/// between frames, a pause with single steps, resume, stop.
fn play_session(edit: &EditSnapshot, seed: u64, faults: SimFaults) -> Recording {
    let mut r = Lcg(seed);
    let mut s = PlaySession::with_tracer(tracer()).with_faults(faults);
    s.set_check_every(30);
    let mut now = Duration::from_secs(100);
    s.control(PlayCommand::Play, edit).unwrap();
    let n = edit.entities.len() as u64;
    for frame in 0..240 {
        let ms = if frame == 90 { 700 } else { 8 + r.below(26) };
        now += Duration::from_micros(ms * 1000 + r.below(1000));
        s.advance(now).unwrap();
        if r.below(4) == 0 {
            let entity = EntityKey(1 + r.below(n));
            let action = match r.below(4) {
                0 => InputAction::Impulse([r.f(), r.f() * 5.0, r.f()]),
                1 => InputAction::SetVelocity([r.f() * 10.0, 0.0, r.f() * 10.0]),
                2 => InputAction::SetAcceleration([0.0, -1.62, 0.0]),
                _ => InputAction::SetSpin(r.f() * 360.0),
            };
            s.input(SimInput { entity, action }).unwrap();
        }
        if frame == 150 {
            s.control(PlayCommand::Pause, edit).unwrap();
            s.control(PlayCommand::Step(1), edit).unwrap();
            s.input(SimInput {
                entity: EntityKey(2),
                action: InputAction::Impulse([0.0, 3.0, 0.0]),
            })
            .unwrap();
            s.control(PlayCommand::Step(7), edit).unwrap();
            s.control(PlayCommand::Play, edit).unwrap();
        }
    }
    s.control(PlayCommand::Stop, edit).unwrap();
    assert_eq!(s.state(), PlayState::Stopped);
    s.take_recording().unwrap()
}

fn replay_of(rec: &Recording, scene: Option<&EditSnapshot>) -> Result<Recording, SimError> {
    // Replay with a session whose recording we can read back afterwards.
    let r = replay(rec, scene, PlaySession::with_tracer(tracer()))?;
    assert_eq!(Some(r.final_hash.as_str()), rec.final_hash());
    // `replay` checked event equality; rebuild the replayed file to compare bytes as well.
    let mut s = PlaySession::with_tracer(tracer());
    s.set_check_every(rec.header.check_every);
    let edit = rec.header.edit.clone().unwrap_or_default();
    let edit = scene.unwrap_or(&edit);
    for ev in &rec.events {
        while s.step() < ev.at() && s.is_running() {
            s.run_steps(1)?;
        }
        match ev {
            RecEvent::Control { cmd, .. } => s.control(*cmd, edit)?,
            RecEvent::Input { input, .. } => s.input(*input)?,
            _ => {}
        }
    }
    Ok(s.take_recording().unwrap_or_else(|| rec.clone()))
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("forge-sim-replay-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn a_recorded_session_replays_bit_for_bit() {
    let edit = scene(7, 300);
    let rec = play_session(&edit, 11, SimFaults::default());
    assert!(rec.is_complete());
    let inputs = rec
        .events
        .iter()
        .filter(|e| matches!(e, RecEvent::Input { .. }))
        .count();
    let checks = rec
        .events
        .iter()
        .filter(|e| matches!(e, RecEvent::Check { .. }))
        .count();
    assert!(
        inputs > 30 && checks > 5,
        "{inputs} inputs, {checks} checks"
    );
    assert!(rec.steps() > 200, "{} steps", rec.steps());

    let path = tmp("session.forgereplay");
    rec.write(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let back = Recording::read(&path).unwrap();
    assert_eq!(back, rec, "the file reads back to the same recording");

    let replayed = replay_of(&back, None).unwrap();
    assert_eq!(
        replayed.to_jsonl().as_bytes(),
        &bytes[..],
        "the replay's own recording is byte-identical to the file"
    );
    // The scene given explicitly (what `forge --headless --script --replay` does) is checked
    // by hash and replays the same.
    let r = replay(&back, Some(&edit), PlaySession::with_tracer(tracer())).unwrap();
    assert_eq!(r.steps, rec.steps());
    assert_eq!(r.checks, checks + 1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn another_scene_is_refused_and_a_one_ulp_input_change_is_caught_at_its_step() {
    let edit = scene(3, 60);
    let rec = play_session(&edit, 5, SimFaults::default());
    let mut other = edit.clone();
    if let Some(Value::Vec3(v)) = other.entities[0].properties.get_mut(P_LOCAL) {
        v[0] = f64::from_bits(v[0].to_bits() + 1);
    }
    assert!(matches!(
        replay(&rec, Some(&other), PlaySession::with_tracer(tracer())),
        Err(SimError::SceneMismatch { .. })
    ));

    let mut tampered = rec.clone();
    let (i, at) = tampered
        .events
        .iter()
        .enumerate()
        .find_map(|(i, e)| match e {
            RecEvent::Input { at, .. } if *at > 40 => Some((i, *at)),
            _ => None,
        })
        .unwrap();
    if let RecEvent::Input { input, .. } = &mut tampered.events[i] {
        input.action = match input.action {
            InputAction::Impulse(v) => {
                InputAction::Impulse([f64::from_bits(v[0].to_bits() ^ 1), v[1], v[2]])
            }
            InputAction::SetVelocity(v) => {
                InputAction::SetVelocity([f64::from_bits(v[0].to_bits() ^ 1), v[1], v[2]])
            }
            InputAction::SetAcceleration(v) => {
                InputAction::SetAcceleration([v[0], f64::from_bits(v[1].to_bits() ^ 1), v[2]])
            }
            InputAction::SetSpin(s) => InputAction::SetSpin(f64::from_bits(s.to_bits() ^ 1)),
        };
    }
    // The tampered input is applied as written, so the first mismatch is the next hash after
    // it: a checkpoint (or the end), never earlier than the input's step.
    match replay(&tampered, None, PlaySession::with_tracer(tracer())) {
        Err(SimError::Diverged { step, expected, .. }) => {
            assert!(step > at, "diverged at {step}, input at {at}");
            assert!(
                expected.contains("check") || expected.contains("end"),
                "{expected}"
            );
        }
        other => panic!("a one-ulp input change was not caught: {other:?}"),
    }
}

#[test]
fn a_crash_truncated_recording_replays_up_to_where_it_ends() {
    let edit = scene(9, 40);
    let rec = play_session(&edit, 2, SimFaults::default());
    let text = rec.to_jsonl();
    let lines: Vec<&str> = text.lines().collect();
    let cut = lines[..lines.len() * 2 / 3].join("\n");
    let partial = Recording::from_jsonl(&cut).unwrap();
    assert!(!partial.is_complete());
    let r = replay(&partial, None, PlaySession::with_tracer(tracer())).unwrap();
    assert_eq!(r.steps, partial.steps());
    assert!(Recording::from_jsonl("").is_err());
    assert!(matches!(
        Recording::from_jsonl(&text.replacen("\"forge_replay\":1", "\"forge_replay\":9", 1)),
        Err(SimError::BadRecording { line: 1, .. })
    ));
}

#[test]
fn the_committed_golden_session_replays_on_this_build() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/golden_session.forgereplay");
    if std::env::var_os("FORGE_BLESS").is_some() {
        let edit = scene(42, 24);
        let rec = play_session(&edit, 42, SimFaults::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        rec.write(&path).unwrap();
    }
    let rec = Recording::read(&path).unwrap_or_else(|e| {
        panic!("{e}: run with FORGE_BLESS=1 once to record the golden session")
    });
    let r = replay(&rec, None, PlaySession::with_tracer(tracer())).unwrap();
    assert!(r.checks >= 5, "{r:?}");
    // The committed file is also what this build records for the same session today.
    let fresh = play_session(&scene(42, 24), 42, SimFaults::default());
    assert_eq!(
        fresh.to_jsonl(),
        std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n")
    );
}

#[test]
fn positive_control_a_frame_rate_dependent_step_fails_replay() {
    let edit = scene(7, 300);
    let rec = play_session(
        &edit,
        11,
        SimFaults {
            frame_dt: true,
            ..SimFaults::default()
        },
    );
    match replay(&rec, None, PlaySession::with_tracer(tracer())) {
        Err(SimError::Diverged { step, .. }) => assert!(step > 0),
        other => panic!("a frame-rate-dependent step replayed as if exact: {other:?}"),
    }
}
