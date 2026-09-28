//! `test_play_controls` (Ch.21 §21.21, DoD M2-33; Ch.37 §37.5): play, pause, step and stop,
//! and play-in-editor runs against **your own sandbox**.
//!
//! * Play forks the simulation: an entity with a velocity moves in what the viewport shows,
//!   the play banner overlay says the run is a sandbox, and the loop wakes at the display
//!   rate only while playing.
//! * Pause holds the tick; Step advances exactly one fixed tick; Stop discards the run.
//! * **The project is untouched**: its state hash is identical before Play and after Stop,
//!   and the undo history has not grown. The controls are recorded in the play session's
//!   log (what replay consumes), not on the project's undo stack.
//! * Stopped, the editor is idle again: 0 frames and 0 wakeups.
//! * **The controls are commands on the bus** (`forge.play.control`, a session command): an
//!   automation session plays, pauses, steps and stops exactly as the panel does, with no project
//!   change and no undo entry; a malformed control is refused by the bus; the panel's own Play
//!   reaches another client as the same command.
//! * **The GPU host is asked for every simulated step**: each play tick requests a frame whose
//!   drawables are where the simulation has them (the frame cache keys on the play revision).
//!
//! Positive control (W2): `positive_control_a_play_that_writes_the_project_fails` — a play
//! panel that "keeps the simulation's results" on Stop — must fail the sandbox check.

mod common;

use std::time::Duration;

use common::{history_len, part, rig_with, set_prop, spawn};
use forge_cmd::Value;
use forge_editor::play::{
    P_VELOCITY, PLAY_CMD, PlayCommand, PlayState, SIM_DT, play_command, ticks_after,
};
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;
use forge_frames::Tick;
use forge_ui::widgets::Pressed;

const PLAY: &str = "forge.play_controls";
const VP: &str = "forge.viewport";

fn press(rig: &mut Rig, name: &str) {
    let b = part(rig, PLAY, &["bar", name]);
    rig.h.ui.raise(b, Pressed(b));
    rig.settle();
}

fn state(rig: &Rig) -> (PlayState, Tick) {
    let s = rig.shell.handles().services_rc();
    let p = s.play.borrow();
    (p.state(), p.tick())
}

fn banner_shown(rig: &Rig) -> bool {
    let root = rig.h.ui.root();
    let id = root
        .child(&forge_ui::Key::Static("shell"))
        .child(&forge_ui::Key::Static("overlays"))
        .child(&forge_ui::Key::Str(
            "overlay:forge.overlay.play_banner".into(),
        ))
        .child(&forge_ui::Key::Static("play_banner"));
    rig.h.ui.contains(id) && rig.h.ui.rect(id).is_some_and(|r| r.w > 0.0) && rig.h.ui.is_visible(id)
}

fn check_sandbox(faults: PanelFaults) -> Result<(), String> {
    let mut rig = rig_with(&[VP, PLAY], faults, |_| {}, &[]);
    let e = spawn(&mut rig, "Ball", None);
    set_prop(
        &mut rig,
        e,
        "transform.position.local",
        Value::Vec3([0.0; 3]),
    );
    set_prop(&mut rig, e, P_VELOCITY, Value::Vec3([2.0, 0.0, 0.0]));
    rig.advance(Duration::from_secs(3));
    let hash = rig.state_hash();
    let entries = history_len(&rig);
    if banner_shown(&rig) {
        return Err("the play banner shows while editing".into());
    }

    press(&mut rig, "play");
    if state(&rig).0 != PlayState::Playing {
        return Err(format!("Play left the state {:?}", state(&rig).0));
    }
    // One second of play at the display rate.
    let (frames, _) = rig.advance(Duration::from_secs(1));
    if frames < 30 {
        return Err(format!("playing drew {frames} frames in 1 s"));
    }
    let (_, tick) = state(&rig);
    if tick.0 < 50 * SIM_DT {
        return Err(format!("the simulation reached tick {} in 1 s", tick.0));
    }
    if !banner_shown(&rig) {
        return Err("no play banner while playing".into());
    }
    // The sandbox moved the ball; the project did not.
    let services = rig.shell.handles().services_rc();
    let sim_x = services
        .play
        .borrow()
        .transforms()
        .get(&e)
        .map_or(0.0, |(p, _, _)| p.local.x);
    if sim_x < 1.5 {
        return Err(format!("the simulated ball is at x = {sim_x}"));
    }
    if rig.state_hash() != hash {
        return Err("playing changed the project".into());
    }

    press(&mut rig, "pause");
    let paused = state(&rig);
    rig.advance(Duration::from_secs(1));
    if state(&rig) != paused || paused.0 != PlayState::Paused {
        return Err(format!(
            "pause did not hold: {paused:?} -> {:?}",
            state(&rig)
        ));
    }
    press(&mut rig, "step");
    if state(&rig).1 != ticks_after(paused.1, 1) {
        return Err(format!(
            "step went from {} to {}",
            paused.1.0,
            state(&rig).1.0
        ));
    }

    press(&mut rig, "stop");
    if state(&rig).0 != PlayState::Stopped {
        return Err("Stop did not stop".into());
    }
    if rig.state_hash() != hash || history_len(&rig) != entries {
        return Err(format!(
            "the project changed across a play session ({} history entries added)",
            history_len(&rig) - entries
        ));
    }
    let log: Vec<PlayCommand> = services
        .play
        .borrow()
        .log()
        .iter()
        .map(|l| l.command)
        .collect();
    if log
        != vec![
            PlayCommand::Play,
            PlayCommand::Pause,
            PlayCommand::Step(1),
            PlayCommand::Stop,
        ]
    {
        return Err(format!("the session log is {log:?}"));
    }
    rig.settle();
    if banner_shown(&rig) {
        return Err("the banner outlived the session".into());
    }
    let idle = rig.advance(Duration::from_secs(10));
    if idle != (0, 0) {
        return Err(format!("stopped, the editor is not idle: {idle:?}"));
    }
    Ok(())
}

#[test]
fn play_runs_in_a_sandbox_and_never_writes_the_project() {
    check_sandbox(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_play_that_writes_the_project_fails() {
    let e = check_sandbox(PanelFaults {
        play_writes_project: true,
        ..PanelFaults::default()
    })
    .expect_err("a play session that writes the project must fail");
    assert!(e.contains("changed across a play session"), "{e}");
}

#[test]
fn the_play_menu_and_chord_drive_the_same_core() {
    let mut rig = rig_with(&[VP, PLAY], PanelFaults::default(), |_| {}, &[]);
    rig.run("forge.play.toggle");
    assert_eq!(state(&rig).0, PlayState::Playing);
    rig.run("forge.play.pause");
    assert_eq!(state(&rig).0, PlayState::Paused);
    rig.run("forge.play.step");
    assert_eq!(state(&rig).0, PlayState::Paused);
    rig.run("forge.play.toggle");
    assert_eq!(state(&rig).0, PlayState::Stopped);
}

// ---- the play controls are commands on the bus (M2-33: "issued as commands") ----------

fn auto(rig: &Rig) -> forge_editor::core::LocalBus {
    rig.connect(forge_cmd::Issuer::Automation {
        session: "player".into(),
        tool: "apply".into(),
    })
}

/// Send `cmd` as an automation session and let the editor take it in.
fn automation_sends(rig: &mut Rig, a: &mut forge_editor::core::LocalBus, cmd: PlayCommand) {
    use forge_editor::client::BusClient;
    a.apply(play_command(cmd), None);
    let p = a.pump();
    assert!(p.refused.is_empty(), "{:?}", p.refused);
    rig.settle();
}

#[test]
fn an_automation_plays_pauses_steps_and_stops_through_the_bus() {
    use forge_editor::client::BusClient;
    let mut rig = rig_with(&[VP, PLAY], PanelFaults::default(), |_| {}, &[]);
    let e = spawn(&mut rig, "Ball", None);
    set_prop(&mut rig, e, P_VELOCITY, Value::Vec3([2.0, 0.0, 0.0]));
    rig.settle();
    let hash = rig.state_hash();
    let entries = history_len(&rig);
    let mut a = auto(&rig);

    automation_sends(&mut rig, &mut a, PlayCommand::Play);
    assert_eq!(state(&rig).0, PlayState::Playing, "the session's Play ran");
    rig.advance(Duration::from_millis(500));
    assert!(state(&rig).1.0 > 0, "the session's simulation runs");
    automation_sends(&mut rig, &mut a, PlayCommand::Pause);
    let paused = state(&rig);
    assert_eq!(paused.0, PlayState::Paused);
    automation_sends(&mut rig, &mut a, PlayCommand::Step(3));
    assert_eq!(state(&rig).1, ticks_after(paused.1, 3));
    automation_sends(&mut rig, &mut a, PlayCommand::Stop);
    assert_eq!(state(&rig).0, PlayState::Stopped);

    // Session commands: no project change, no undo entries, and the play session's log
    // has exactly what the session sent.
    assert_eq!(rig.state_hash(), hash);
    assert_eq!(history_len(&rig), entries);
    let log: Vec<PlayCommand> = rig
        .shell
        .handles()
        .services_rc()
        .play
        .borrow()
        .log()
        .iter()
        .map(|l| l.command)
        .collect();
    assert_eq!(
        log,
        vec![
            PlayCommand::Play,
            PlayCommand::Pause,
            PlayCommand::Step(3),
            PlayCommand::Stop
        ]
    );

    // A malformed control is refused by the bus with its code, and the play core never
    // sees it.
    a.apply(
        forge_cmd::EditorCommand::Invoke {
            target: PLAY_CMD.into(),
            args: "{\"control\": \"rewind\"}".into(),
        },
        None,
    );
    let p = a.pump();
    assert_eq!(p.refused.len(), 1, "a bad control is refused");
    rig.settle();
    assert_eq!(state(&rig).0, PlayState::Stopped);
}

#[test]
fn the_panel_sends_play_as_a_bus_command_too() {
    // The panel's Play reaches another client of the core as the same session command:
    // the panel has no privileged path to the play core (I7's rule, applied to the
    // session commands).
    use forge_editor::client::BusClient;
    let mut rig = rig_with(&[VP, PLAY], PanelFaults::default(), |_| {}, &[]);
    let mut watcher = auto(&rig);
    watcher.follow_session(true);
    press(&mut rig, "play");
    press(&mut rig, "stop");
    let seen: Vec<String> = watcher
        .pump()
        .session
        .iter()
        .map(|s| format!("{} {} by {}", s.target, s.args, s.issuer.tag()))
        .collect();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(
        seen[0].starts_with(PLAY_CMD) && seen[0].contains("\"play\""),
        "{seen:?}"
    );
    assert!(seen[1].contains("\"stop\""), "{seen:?}");
    assert!(seen.iter().all(|s| s.contains("by human:")), "{seen:?}");
}

// ---- what the GPU host is asked to draw follows the simulation --------------------------

/// The ball's x in each frame the viewport asked the GPU host to render since last time.
fn gpu_ball_x(rig: &Rig, ball: forge_cmd::EntityKey) -> Vec<f64> {
    let s = rig.shell.handles().services_rc();
    let dirty = s.viewport_surfaces.borrow_mut().take_dirty();
    dirty
        .iter()
        .filter_map(|(_, f)| f.drawables.iter().find(|d| d.key == ball))
        .map(|d| d.pos.local.x)
        .collect()
}

#[test]
fn the_frame_sent_to_the_gpu_moves_with_every_play_tick() {
    let mut rig = rig_with(
        &[VP, PLAY],
        PanelFaults::default(),
        |s| s.viewport_surfaces.borrow_mut().attach_host("test host"),
        &[],
    );
    let e = spawn(&mut rig, "Ball", None);
    set_prop(
        &mut rig,
        e,
        "transform.position.local",
        Value::Vec3([0.0; 3]),
    );
    set_prop(&mut rig, e, P_VELOCITY, Value::Vec3([2.0, 0.0, 0.0]));
    rig.settle();
    let _ = gpu_ball_x(&rig, e);

    press(&mut rig, "play");
    let mut xs = Vec::new();
    for _ in 0..4 {
        rig.advance(Duration::from_millis(100));
        let frame = gpu_ball_x(&rig, e);
        assert!(
            !frame.is_empty(),
            "a playing tick asked the GPU for no frame"
        );
        xs.push(frame[frame.len() - 1]);
    }
    for w in xs.windows(2) {
        assert!(
            w[1] > w[0],
            "the frame sent to the GPU froze while playing: ball x {xs:?}"
        );
    }
    // And it is where the simulation has the ball.
    let sim_x = rig
        .shell
        .handles()
        .services_rc()
        .play
        .borrow()
        .transforms()
        .get(&e)
        .map_or(f64::NAN, |(p, _, _)| p.local.x);
    assert!((xs[3] - sim_x).abs() < 1e-9, "GPU {} vs sim {sim_x}", xs[3]);
}
