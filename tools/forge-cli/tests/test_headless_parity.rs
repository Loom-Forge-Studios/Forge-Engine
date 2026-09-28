//! Guard `test_headless_parity` (Ch.34.4, M2-17): **headless and GUI runs of the same scene
//! produce identical simulation hashes.**
//!
//! The GUI run is the editor as a user drives it: the scene is built through the shell's
//! command emitter (so the GUI's mirror is what it sees); the play controls are the shell's
//! own — `forge.play.control` commands on the bus, run by the shell on its play core
//! (`EditorServices::play`, a `forge_editor::sim_bridge::SimPlay`), which forks the
//! simulation from the mirror; and the editor's loop — frames of jittered length, one long
//! stall — advances that core by the clock while inputs arrive between frames, with a pause
//! and single steps in the middle. The session is recorded.
//!
//! The headless run is the real `forge` binary, `forge --headless --script <scene>
//! --replay <gui recording>`: it builds the same scene through its own core (the project, not
//! a mirror), forks from it — refusing if the scene's hash differs from the one the GUI forked
//! (`SIM-0006`) — and re-runs the recorded session by step count with no clock, checking every
//! checkpoint hash and the final one (`SIM-0007` on the first difference). It must exit
//! cleanly, print the GUI's final simulation hash, and the project state hash the GUI's core
//! holds.
//!
//! Positive controls (W2), each a run whose GUI half has one real-world bug and which must
//! make the headless binary fail:
//! * `positive_control_a_frame_rate_dependent_gui_step_fails_parity` — the GUI steps with the
//!   frame's measured time (the variable-timestep bug): `SIM-0007`.
//! * `positive_control_a_gui_fork_without_hidden_bodies_fails_parity` — the GUI forks from what
//!   the viewport draws, leaving out a body hidden in the hierarchy: `SIM-0006`.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::play::PlayBackend;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::sim_bridge::{BridgeFaults, SimPlay};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_sim::{InputAction, PlayCommand, PlayState, SimFaults, SimInput};
use forge_trace::Tracer;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
    fn f(&mut self) -> f64 {
        (self.next() as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const BODIES: u64 = 16;

/// The scene, as editor commands: bodies in three frames with transforms and motion, one of
/// them hidden in the hierarchy, a parented child, a setting.
fn scene() -> Vec<EditorCommand> {
    let mut r = Lcg(2026);
    let mut cmds = Vec::new();
    let set = |e: u64, path: &str, value: Value| EditorCommand::SetProperty {
        entity: EntityKey(e),
        path: path.into(),
        value,
    };
    for k in 0..BODIES {
        cmds.push(EditorCommand::Spawn {
            name: format!("Body {k}"),
            parent: (k == 5).then_some(EntityKey(4)),
        });
        cmds.push(set(
            k,
            "transform.position.local",
            Value::Vec3([r.f() * 6.371e6, r.f() * 50.0, r.f() * 1e4]),
        ));
        cmds.push(set(
            k,
            "transform.position.frame",
            Value::Int((k % 3) as i64),
        ));
        cmds.push(set(k, "transform.yaw", Value::Float(r.f() * 180.0)));
        cmds.push(set(k, "transform.pitch", Value::Float(r.f() * 30.0)));
        match k % 3 {
            0 => cmds.push(set(
                k,
                "motion.velocity",
                Value::Vec3([r.f() * 20.0, r.f() * 5.0, r.f() * 20.0]),
            )),
            1 => {
                cmds.push(set(
                    k,
                    "motion.acceleration",
                    Value::Vec3([0.0, -9.81, 0.0]),
                ));
                cmds.push(set(k, "motion.spin_dps", Value::Float(r.f() * 400.0)));
            }
            _ => cmds.push(set(k, "motion.spin_dps", Value::Float(r.f() * 30.0))),
        }
    }
    cmds.push(set(3, "editor.hidden", Value::Bool(true)));
    cmds.push(EditorCommand::SetSetting {
        key: "editor.grid_size".into(),
        value: Some(Value::Float(0.25)),
    });
    cmds
}

fn gui_config() -> ShellConfig {
    let stand_in = StandInPanels::without(&[]).unwrap();
    assemble(builtin_preset("3d").unwrap(), &[&stand_in], &[], None).unwrap()
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "forge-headless-parity-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// What the GUI run produced.
struct Gui {
    recording: PathBuf,
    sim_hash: String,
    state_hash: u64,
    steps: u64,
}

/// Send an input for a body the session simulates (a user can only act on what exists) the
/// way the editor sends one: the `forge.play.input` command on the bus, which the shell hands
/// to its play core when the stream delivers it (WP-19, gate C-sim-input-command).
fn give(rig: &mut Rig, core: &RefCell<SimPlay>, i: SimInput) {
    let simulated = core
        .borrow()
        .session()
        .world()
        .is_some_and(|w| w.contains(i.entity));
    if simulated {
        rig.shell.play_input(i);
    }
}

/// The GUI run (see the module docs).
fn gui_run(out: &Path, sim: SimFaults, bridge: BridgeFaults) -> Gui {
    let tracer: &'static Tracer = Box::leak(Box::new(Tracer::new()));
    let mut play = SimPlay::with_tracer(tracer).with_faults(sim, bridge);
    play.session_mut().set_check_every(20);
    let mut cfg = gui_config();
    cfg.services = std::mem::take(&mut cfg.services).with_play_core(play);
    let mut rig = Rig::new(cfg).unwrap();
    let em = rig.shell.emitter().clone();
    for c in scene() {
        em.emit(c);
        rig.turn();
    }
    rig.settle();
    assert!(rig.mirror_matches(), "the GUI's mirror is the project");
    assert_eq!(rig.shell.mirror().len(), BODIES as usize);

    // The shell's own play core: the one its play controls (bus commands) run on.
    let services = rig.shell.handles().services_rc();
    let core = services.play_core.clone().unwrap();
    let state = || services.play.borrow().state();
    rig.shell.play(PlayCommand::Play);
    assert_eq!(state(), PlayState::Playing, "Play went through the bus");
    let mut r = Lcg(77);
    for frame in 0..220 {
        // Frames of jittered length and one long stall: the shell advances the play core
        // by its clock every turn.
        let ms = if frame == 60 { 480 } else { 7 + r.below(34) };
        let now = rig.h.now() + Duration::from_micros(ms * 1000 + r.below(1000));
        rig.h.set_now(now);
        rig.turn();
        if r.below(3) == 0 {
            let entity = EntityKey(r.below(BODIES));
            let action = match r.below(3) {
                0 => InputAction::Impulse([r.f() * 2.0, r.f() * 8.0, r.f() * 2.0]),
                1 => InputAction::SetAcceleration([0.0, -1.62, r.f()]),
                _ => InputAction::SetSpin(r.f() * 90.0),
            };
            give(&mut rig, &core, SimInput { entity, action });
        }
        if frame == 120 {
            rig.shell.play(PlayCommand::Pause);
            rig.shell.play(PlayCommand::Step(4));
            give(
                &mut rig,
                &core,
                SimInput {
                    entity: EntityKey(3),
                    action: InputAction::Impulse([0.0, 12.0, 0.0]),
                },
            );
            rig.shell.play(PlayCommand::Step(2));
            rig.shell.play(PlayCommand::Play);
            assert_eq!(state(), PlayState::Playing);
        }
    }
    rig.shell.play(PlayCommand::Stop);
    assert_eq!(state(), PlayState::Stopped);
    assert_eq!(core.borrow_mut().take_error(), None);
    let log: Vec<PlayCommand> = core.borrow().log().iter().map(|l| l.command).collect();
    assert_eq!(
        log,
        [
            PlayCommand::Play,
            PlayCommand::Pause,
            PlayCommand::Step(4),
            PlayCommand::Step(2),
            PlayCommand::Play,
            PlayCommand::Stop
        ],
        "every control reached the play core through the bus"
    );
    let rec = core.borrow_mut().session_mut().take_recording().unwrap();
    let recording = out.join("gui.forgereplay");
    rec.write(&recording).unwrap();
    Gui {
        recording,
        sim_hash: rec.final_hash().unwrap().to_owned(),
        state_hash: rig.state_hash(),
        steps: rec.steps(),
    }
}

fn write_scene(out: &Path) -> PathBuf {
    let p = out.join("scene.fscript");
    let mut text = String::from("# the parity scene, as editor commands\n");
    for c in scene() {
        text.push_str(&serde_json::to_string(&c).unwrap());
        text.push('\n');
    }
    std::fs::write(&p, text).unwrap();
    p
}

/// `forge --headless --script <scene> --replay <recording>`: (success, stdout + stderr).
fn headless(script: &Path, recording: &Path) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_forge"))
        .arg("--headless")
        .arg("--script")
        .arg(script)
        .arg("--replay")
        .arg(recording)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    (o.status.success(), text)
}

#[test]
fn headless_and_gui_runs_produce_identical_simulation_hashes() {
    let d = dir();
    let gui = gui_run(&d, SimFaults::default(), BridgeFaults::default());
    assert!(gui.steps > 150, "{} steps", gui.steps);
    let script = write_scene(&d);
    let (ok, text) = headless(&script, &gui.recording);
    assert!(ok, "forge --headless failed:\n{text}");
    assert!(
        text.contains("bit-for-bit (against the script's project)"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "sim hash {} after {} step(s)",
            gui.sim_hash, gui.steps
        )),
        "the headless run's simulation hash differs from the GUI's {}:\n{text}",
        gui.sim_hash
    );
    assert!(
        text.contains(&format!("state hash {:016x}", gui.state_hash)),
        "the headless project differs from the GUI's:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn positive_control_a_frame_rate_dependent_gui_step_fails_parity() {
    let d = dir();
    let gui = gui_run(
        &d,
        SimFaults {
            frame_dt: true,
            ..SimFaults::default()
        },
        BridgeFaults::default(),
    );
    let (ok, text) = headless(&write_scene(&d), &gui.recording);
    assert!(
        !ok,
        "a frame-rate-dependent GUI step passed parity:\n{text}"
    );
    assert!(text.contains("SIM-0007"), "{text}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn positive_control_a_gui_fork_without_hidden_bodies_fails_parity() {
    let d = dir();
    let gui = gui_run(&d, SimFaults::default(), BridgeFaults { skip_hidden: true });
    let (ok, text) = headless(&write_scene(&d), &gui.recording);
    assert!(
        !ok,
        "a GUI fork missing a hidden body passed parity:\n{text}"
    );
    assert!(text.contains("SIM-0006"), "{text}");
    let _ = std::fs::remove_dir_all(&d);
}
