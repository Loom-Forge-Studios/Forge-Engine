//! `test_pie` — the 2D sample plays in the editor (DoD M4-12, "runs from the editor").
//!
//! The editor's own play controls — the play-controls panel's buttons, which send the
//! `forge.play.control` session command on the bus — run the sample's level through
//! [`SamplePlay`], its `PlayBackend`:
//!
//! * `the_play_button_runs_the_sample_in_the_editor`: Play from the panel starts the level,
//!   the editor clock advances it at 60 Hz, Pause holds it, Stop discards it, and the project
//!   is untouched (its state hash before Play equals the hash after Stop).
//! * `a_step_in_the_editor_plays_the_exported_bits`: `Step(900)` with the scripted pad reaches
//!   the level-clear state with exactly the fingerprint the shipped game pins
//!   ([`SCRIPT_FINGERPRINT`]): the editor plays the same game as the export.
//!   Control: `positive_control_a_pie_run_without_the_script_is_caught`.

use std::time::Duration;

use forge_2d_game::pie::{BACKEND, SamplePlay};
use forge_2d_game::{SCRIPT_FINGERPRINT, SCRIPT_STEPS, demo_script};
use forge_editor::play::{PlayBackend, PlayCommand, PlayState};
use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_core::PanelsCore;
use forge_panels_scene::PanelsScene;
use forge_plugin::SourcePlugin;
use forge_ui::Key;
use forge_ui::widgets::Pressed;

const PLAY: &str = "forge.play_controls";

/// A 2D-preset editor with the real play controls, playing the sample.
fn editor() -> (Rig, std::rc::Rc<std::cell::RefCell<SamplePlay>>) {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let scene = PanelsScene::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_scene::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&core, &scene, &stand_in];
    let preset = builtin_preset("2d").unwrap_or_else(|e| panic!("{e}"));
    let mut cfg = assemble(preset, &all, &[], None).unwrap_or_else(|e| panic!("{e}"));
    let play = SamplePlay::install(&mut cfg.services);
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&[PLAY]).unwrap_or_else(|e| panic!("{e}"));
    (rig, play)
}

fn press(rig: &mut Rig, name: &str) {
    let mut b = rig
        .panel_frame(PLAY)
        .unwrap_or_else(|| panic!("{PLAY} is not open"));
    for k in ["bar", name] {
        b = b.child(&Key::Str(k.into()));
    }
    assert!(rig.h.ui.contains(b), "no {name} button");
    rig.h.ui.raise(b, Pressed(b));
    rig.settle();
}

#[test]
fn the_play_button_runs_the_sample_in_the_editor() {
    let (mut rig, play) = editor();
    let hash = rig.state_hash();
    assert_eq!(
        rig.shell.handles().services_rc().play.borrow().backend(),
        BACKEND
    );
    play.borrow_mut()
        .set_script(demo_script(SCRIPT_STEPS as u64));
    press(&mut rig, "play");
    assert_eq!(play.borrow().state(), PlayState::Playing);
    rig.advance(Duration::from_secs(1));
    let steps = play.borrow().game().map_or(0, |g| g.steps);
    assert!(
        (55..=61).contains(&steps),
        "one second of editor time is ~60 fixed steps: {steps}"
    );
    press(&mut rig, "pause");
    rig.advance(Duration::from_secs(1));
    assert_eq!(
        play.borrow().game().map_or(0, |g| g.steps),
        steps,
        "a paused level does not move"
    );
    let hero = play
        .borrow()
        .game()
        .and_then(|g| g.hero().ok())
        .map(|p| p.local.x);
    assert!(
        hero.is_some_and(|x| x > 0.0),
        "the hero ran right: {hero:?}"
    );
    press(&mut rig, "stop");
    assert_eq!(play.borrow().state(), PlayState::Stopped);
    assert!(play.borrow().game().is_none());
    assert_eq!(rig.state_hash(), hash, "playing did not change the project");
    let log: Vec<PlayCommand> = play.borrow().log().iter().map(|e| e.command).collect();
    assert_eq!(
        log,
        [PlayCommand::Play, PlayCommand::Pause, PlayCommand::Stop]
    );
}

fn stepped(script: bool) -> SamplePlay {
    let (mut rig, play) = editor();
    if script {
        play.borrow_mut()
            .set_script(demo_script(SCRIPT_STEPS as u64));
    }
    rig.shell.play(PlayCommand::Step(SCRIPT_STEPS as u32));
    rig.settle();
    std::mem::take(&mut *play.borrow_mut())
}

#[test]
fn a_step_in_the_editor_plays_the_exported_bits() {
    let p = stepped(true);
    let g = p.game().unwrap_or_else(|| panic!("the step forked a run"));
    assert_eq!(g.steps, SCRIPT_STEPS as u64);
    assert!(g.won, "the scripted run clears the level in the editor");
    assert_eq!(
        p.fingerprint(),
        SCRIPT_FINGERPRINT,
        "the editor plays the same bits as the shipped game"
    );
}

#[test]
fn positive_control_a_pie_run_without_the_script_is_caught() {
    let p = stepped(false);
    assert_ne!(p.fingerprint(), SCRIPT_FINGERPRINT);
    assert!(!p.game().is_some_and(|g| g.won));
}
