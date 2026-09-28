//! `test_timeline_preview_sandboxed` (gate row `C-timeline-preview-sandboxed`; DoD M2-63):
//! scrubbing and playing a clip preview it through the play core and **never write the
//! project** — the core's state hash is identical before and after, while the preview's
//! values move.
//!
//! Positive control (W2): `positive_control_a_preview_that_writes_the_project_fails` — a
//! preview that sets the sampled values as properties
//! (`PanelFaults::timeline_preview_writes_project`) changes the hash and fails.

mod common;

use std::time::Duration;

use common::part;
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;
use forge_ui::{KeyCode, Modifiers};

/// Scrub across the clip and play half a second; `Err` when the project changed.
fn preview_leaves_the_project(rig: &mut Rig) -> Result<u64, String> {
    let before = rig.state_hash();
    let tl = part(rig, "forge.sequencer", &["body", "timeline"]);
    rig.h.ui.set_focus(Some(tl), true);
    rig.chord(KeyCode::Home, Modifiers::NONE);
    for _ in 0..20 {
        rig.chord(KeyCode::Right, Modifiers::NONE);
    }
    rig.settle();
    let play = part(rig, "forge.sequencer", &["bar", "play"]);
    common::press(rig, play);
    rig.advance(Duration::from_millis(500));
    common::press(rig, play);
    rig.settle();
    let revision = rig
        .shell
        .handles()
        .services()
        .anim_preview
        .borrow()
        .revision();
    if rig.state_hash() == before {
        Ok(revision)
    } else {
        Err("the timeline preview changed the project".into())
    }
}

#[test]
fn scrubbing_and_playing_never_write_the_project() {
    let (mut rig, e) = common::sequencer_with_keys(PanelFaults::default());
    let rev = preview_leaves_the_project(&mut rig).unwrap_or_else(|e| panic!("{e}"));
    assert!(rev > 20, "the preview really ran (revision {rev})");
    let sv = rig.shell.handles().services_rc();
    let pv = sv.anim_preview.borrow();
    assert!(
        pv.values()
            .iter()
            .any(|(k, p, _)| *k == e && p == "light.intensity")
    );
}

#[test]
fn positive_control_a_preview_that_writes_the_project_fails() {
    let faults = PanelFaults {
        timeline_preview_writes_project: true,
        ..PanelFaults::default()
    };
    let (mut rig, _) = common::sequencer_with_keys(faults);
    assert!(preview_leaves_the_project(&mut rig).is_err());
}
