//! `test_timeline_edits_undoable` (gate row `C-timeline-edits-undoable`; DoD M2-63, I7):
//! every timeline edit is a command with exactly one undo entry — a 30-frame key drag is
//! one gesture (one transaction) — and undo puts the model back.
//!
//! Positive control (W2): `positive_control_a_drag_without_a_gesture_fails` — the
//! sequencer sending each drag frame as its own transaction
//! (`PanelFaults::timeline_drag_without_gesture`) must fail the single-entry check.

mod common;

use common::{history_len, part, raise, setting, undo};
use forge_cmd::Value;
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;
use forge_panels_authoring::widgets::{DragPhase, KeysDragged};

const T: &str = "seq.clip.walk.track.crate_light_intensity";

/// Drag the key at 1 s by +0.5 s over 30 frames; the undo entries it made.
fn drag_entries(rig: &mut Rig) -> usize {
    let tl = part(rig, "forge.sequencer", &["body", "timeline"]);
    let before = history_len(rig);
    for i in 0..=30 {
        let phase = match i {
            0 => DragPhase::Begin,
            30 => DragPhase::End,
            _ => DragPhase::Move,
        };
        let dt = (f64::from(i) / 30.0 * 15.0).round() / 30.0;
        raise(
            rig,
            tl,
            KeysDragged {
                track: "crate_light_intensity".into(),
                keys: vec!["k1".into()],
                dt,
                phase,
            },
        );
    }
    history_len(rig) - before
}

fn check(entries: usize) -> Result<(), String> {
    if entries == 1 {
        Ok(())
    } else {
        Err(format!("a key drag made {entries} undo entries, not 1"))
    }
}

#[test]
fn a_key_drag_is_one_undo_entry_and_undo_restores_it() {
    let (mut rig, _) = common::sequencer_with_keys(PanelFaults::default());
    let n = drag_entries(&mut rig);
    check(n).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        setting(&rig, &format!("{T}.key.k1.t")),
        Some(Value::Float(1.5))
    );
    undo(&mut rig);
    assert_eq!(
        setting(&rig, &format!("{T}.key.k1.t")),
        Some(Value::Float(1.0))
    );
}

#[test]
fn positive_control_a_drag_without_a_gesture_fails() {
    let faults = PanelFaults {
        timeline_drag_without_gesture: true,
        ..PanelFaults::default()
    };
    let (mut rig, _) = common::sequencer_with_keys(faults);
    let n = drag_entries(&mut rig);
    let r = check(n);
    assert!(
        r.is_err(),
        "a gestureless drag must fail the check ({n} entries)"
    );
    assert!(r.err().is_some_and(|e| e.contains("undo entries")));
}
