//! `test_authoring_panels_idle` (gate row `C-authoring-panels-idle`; D-5, DoD M2-63..M2-65):
//! with the sequencer (a clip with keys, **played and then paused**), the state machine
//! editor and the localisation editor open, an idle editor draws no frame and never wakes.
//!
//! The rig really plays the clip first — the timeline's frame loop runs and the playhead
//! moves — then pauses it: a pause must stop the loop, not leave it spinning.
//!
//! Positive controls (W2):
//! * `positive_control_a_timeline_that_animates_while_stopped_fails` — a timeline asking for
//!   frames while nothing plays (`PanelFaults::timeline_always_animating`) keeps the editor
//!   awake and fails.
//! * `positive_control_a_pause_that_keeps_playing_fails` — a pause that leaves the timeline
//!   marked as playing (`PanelFaults::timeline_pause_keeps_playing`: `set_playing` forces
//!   `playing = true`) keeps the frame loop running and fails.

mod common;

use std::time::Duration;

use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;

const P: &str = "forge.sequencer";

fn idle(rig: &mut Rig) -> Result<(), String> {
    // One-shot deadlines (the shell's own) pass first.
    rig.advance(Duration::from_secs(3));
    let (frames, wakes) = rig.advance(Duration::from_secs(10));
    if (frames, wakes) == (0, 0) {
        Ok(())
    } else {
        Err(format!(
            "idle editor: {frames} frame(s), {wakes} wakeup(s) in 10 s"
        ))
    }
}

fn open_all(faults: PanelFaults) -> Rig {
    let (mut rig, _) = common::sequencer_with_keys(faults);
    rig.show_panels(&[P, "forge.anim_graph", "forge.localisation"])
        .unwrap_or_else(|e| panic!("{e}"));
    rig.settle();
    // Play the clip from the start: the preview advances on the timeline's frames.
    let tl = common::part(&rig, P, &["body", "timeline"]);
    rig.h.ui.set_focus(Some(tl), true);
    rig.chord(forge_ui::KeyCode::Home, forge_ui::Modifiers::NONE);
    rig.settle();
    let play = common::part(&rig, P, &["bar", "play"]);
    common::press(&mut rig, play);
    let (frames, _) = rig.advance(Duration::from_millis(500));
    let t = rig
        .shell
        .handles()
        .services_rc()
        .anim_preview
        .borrow()
        .time();
    assert!(
        frames > 0 && t > 0.0,
        "set-up: the clip did not play ({frames} frame(s), playhead {t})"
    );
    assert!(
        rig.shell
            .handles()
            .services_rc()
            .anim_preview
            .borrow()
            .is_playing(),
        "set-up: the preview is not playing"
    );
    // Pause it (the same button).
    common::press(&mut rig, play);
    assert!(
        !rig.shell
            .handles()
            .services_rc()
            .anim_preview
            .borrow()
            .is_playing(),
        "set-up: the pause did not pause the preview"
    );
    rig.settle();
    rig
}

#[test]
fn the_authoring_panels_are_idle_when_nothing_changes() {
    let mut rig = open_all(PanelFaults::default());
    idle(&mut rig).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_timeline_that_animates_while_stopped_fails() {
    let faults = PanelFaults {
        timeline_always_animating: true,
        ..PanelFaults::default()
    };
    let mut rig = open_all(faults);
    let r = idle(&mut rig);
    assert!(
        r.is_err(),
        "an always-animating timeline must fail the idle check"
    );
}

#[test]
fn positive_control_a_pause_that_keeps_playing_fails() {
    let faults = PanelFaults {
        timeline_pause_keeps_playing: true,
        ..PanelFaults::default()
    };
    let mut rig = open_all(faults);
    let r = idle(&mut rig);
    assert!(
        r.is_err(),
        "a pause that leaves the timeline playing must fail the idle check"
    );
}
