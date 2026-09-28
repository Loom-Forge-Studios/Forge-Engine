//! Guard `C-sim-input-command` (WP-19; Ch.34.4, I7): **a simulation input is a bus command,
//! whichever client sends it.** `forge.play.input` (`forge_editor::play::input_command`) is a
//! session command like the play controls: the bus validates and audits it and delivers it,
//! in stream order, to the client hosting the play core — the shell or `forge --headless` —
//! which queues it for the next step boundary. So a headless `input` line, the editor's
//! `Shell::play_input`, a script and an automation session feed the play core by the one route, and
//! what a recording holds is exactly what went over the bus.
//!
//! * `headless_inputs_are_bus_commands`: a headless run's every recorded input arrived at an
//!   independent client following the core's session commands as a `forge.play.input` issued
//!   by the script, in the same order.
//! * `gui_and_agent_inputs_reach_the_shells_play_core_over_the_bus`: in a running shell, an
//!   input sent by the shell and one sent by an automation client both reach the shell's play core
//!   (its recording) as the same command, and a follower sees both with their issuers.
//! * `a_refused_input_is_reported_not_dropped`: an input naming a body the simulation does not
//!   have passes the bus (it is well formed) and is refused by the play core, and the headless
//!   run says so.
//!
//! Positive control (W2): `positive_control_direct_headless_inputs_fail` — the pre-WP-19 route
//! (the runner hands `input` lines to the play core directly, a hidden option) records inputs
//! no client ever saw on the bus, and the check refuses it.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_editor::headless::{HeadlessOptions, run};
use forge_editor::play::{
    PLAY_INPUT_CMD, PlayBackend, PlayCommand, PlayState, decode_input, input_command,
};
use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble;
use forge_editor::sim_bridge::SimPlay;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_sim::{InputAction, RecEvent, Recording, SimInput};
use forge_trace::Tracer;

fn ball() -> Vec<EditorCommand> {
    let set = |path: &str, value: Value| EditorCommand::SetProperty {
        entity: EntityKey(0),
        path: path.into(),
        value,
    };
    vec![
        EditorCommand::Spawn {
            name: "Ball".into(),
            parent: None,
        },
        set("transform.position.local", Value::Vec3([0.0, 1.0, 0.0])),
        set("motion.velocity", Value::Vec3([1.0, 0.0, 0.0])),
    ]
}

const INPUTS: [SimInput; 3] = [
    SimInput {
        entity: EntityKey(0),
        action: InputAction::Impulse([0.0, 5.0, 0.0]),
    },
    SimInput {
        entity: EntityKey(0),
        action: InputAction::SetSpin(45.0),
    },
    SimInput {
        entity: EntityKey(0),
        action: InputAction::SetAcceleration([0.0, -1.62, 0.0]),
    },
];

/// The inputs a recording holds, in order.
fn recorded(rec: &Recording) -> Vec<SimInput> {
    rec.events
        .iter()
        .filter_map(|e| match e {
            RecEvent::Input { input, .. } => Some(*input),
            _ => None,
        })
        .collect()
}

/// The simulation inputs a follower saw on the bus, with their issuers.
fn seen_on_the_bus(follower: &mut impl BusClient) -> Vec<(Issuer, SimInput)> {
    follower
        .pump()
        .session
        .iter()
        .filter(|sc| sc.target == PLAY_INPUT_CMD)
        .map(|sc| {
            let i = decode_input(&sc.target, &sc.args)
                .expect("an input")
                .expect("a well-formed input");
            (sc.issuer.clone(), i)
        })
        .collect()
}

/// A headless run of the ball scene with the three inputs; `Err` when what the play core
/// recorded is not exactly what went over the bus.
fn check_headless(direct_inputs: bool) -> Result<(), String> {
    let core = EditorCore::new();
    let mut follower = EditorCore::connect(&core, Issuer::Test);
    follower.follow_session(true);
    let mut script = String::new();
    for c in ball() {
        script += &serde_json::to_string(&c).unwrap();
        script.push('\n');
    }
    script += "play\n";
    for (k, i) in INPUTS.iter().enumerate() {
        script += &format!(
            "input {}\nrun {}\n",
            serde_json::to_string(i).unwrap(),
            k + 2
        );
    }
    script += "stop\n";
    let dir = std::env::temp_dir().join(format!(
        "forge-sim-input-{}-{direct_inputs}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let rec_path = dir.join("run.forgereplay");
    let opts = HeadlessOptions {
        record: Some(rec_path.clone()),
        direct_inputs_for_tests: direct_inputs,
        ..HeadlessOptions::default()
    };
    let mut out = Vec::new();
    let sum = run(
        &core,
        "inputs.forge",
        Some(script.as_bytes()),
        &opts,
        &mut out,
    )
    .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out).to_string();
    if sum.refused != 0 {
        return Err(format!("the run refused a line:\n{text}"));
    }
    let rec = Recording::read(&rec_path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dir);
    let recorded = recorded(&rec);
    if recorded != INPUTS {
        return Err(format!(
            "the play core recorded {recorded:?}, not the script's inputs:\n{text}"
        ));
    }
    let seen = seen_on_the_bus(&mut follower);
    let script_issuer = Issuer::Script {
        path: "inputs.forge".into(),
    };
    let from_script: Vec<SimInput> = seen
        .iter()
        .filter(|(who, _)| *who == script_issuer)
        .map(|(_, i)| *i)
        .collect();
    if from_script != recorded {
        return Err(format!(
            "the play core recorded {} input(s) but the bus carried {} from the script \
             ({seen:?}): an input reached the play core without being a bus command",
            recorded.len(),
            from_script.len()
        ));
    }
    Ok(())
}

#[test]
fn headless_inputs_are_bus_commands() {
    check_headless(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_direct_headless_inputs_fail() {
    let e = check_headless(true)
        .expect_err("inputs handed to the play core directly passed the bus check");
    assert!(e.contains("without being a bus command"), "{e}");
}

#[test]
fn gui_and_automation_inputs_reach_the_shells_play_core_over_the_bus() {
    let tracer: &'static Tracer = Box::leak(Box::new(Tracer::new()));
    let stand_in = StandInPanels::without(&[]).unwrap();
    let mut cfg = assemble(builtin_preset("3d").unwrap(), &[&stand_in], &[], None).unwrap();
    cfg.services = std::mem::take(&mut cfg.services).with_play_core(SimPlay::with_tracer(tracer));
    let mut rig = Rig::new(cfg).unwrap();
    let mut follower = EditorCore::connect(&rig.core, Issuer::Test);
    follower.follow_session(true);
    let em = rig.shell.emitter().clone();
    for c in ball() {
        em.emit(c);
        rig.turn();
    }
    rig.settle();
    rig.shell.play(PlayCommand::Play);
    let services = rig.shell.handles().services_rc();
    let core = services.play_core.clone().unwrap();
    assert_eq!(core.borrow().session().state(), PlayState::Playing);
    // The editor's own input.
    rig.shell.play_input(INPUTS[0]);
    // An automation session's input, from another client of the same core.
    let auto = Issuer::Automation {
        session: "s1".into(),
        tool: "simulate".into(),
    };
    let mut automation_client = EditorCore::connect(&rig.core, auto.clone());
    automation_client.apply(input_command(INPUTS[1]), None);
    rig.h.set_now(rig.h.now() + Duration::from_millis(50));
    rig.turn();
    rig.settle();
    rig.shell.play(PlayCommand::Stop);
    assert_eq!(core.borrow_mut().take_error(), None);
    let rec = core
        .borrow_mut()
        .session_mut()
        .take_recording()
        .expect("a recording");
    assert_eq!(
        recorded(&rec),
        INPUTS[..2].to_vec(),
        "both inputs reached the shell's play core"
    );
    let seen = seen_on_the_bus(&mut follower);
    assert_eq!(
        seen,
        vec![
            (
                Issuer::Human {
                    user: "tester".into()
                },
                INPUTS[0]
            ),
            (auto, INPUTS[1]),
        ],
        "each input is the same bus command, with its issuer"
    );
}

#[test]
fn a_refused_input_is_reported_not_dropped() {
    let core = EditorCore::new();
    let mut script = String::new();
    for c in ball() {
        script += &serde_json::to_string(&c).unwrap();
        script.push('\n');
    }
    script +=
        "play\ninput {\"entity\":99,\"action\":{\"Impulse\":[0,1,0]}}\ninput {\"entity\":0}\n";
    let mut out = Vec::new();
    let sum = run(
        &core,
        "refuse.forge",
        Some(script.as_bytes()),
        &HeadlessOptions::default(),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out);
    assert_eq!(sum.refused, 2, "{text}");
    assert!(text.contains("line 5: refused"), "{text}");
    assert!(text.contains("line 6: refused"), "{text}");
}
