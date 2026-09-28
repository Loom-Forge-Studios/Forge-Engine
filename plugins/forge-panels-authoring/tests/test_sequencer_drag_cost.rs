//! `test_sequencer_drag_cost` (D-5, owner rule 2; WP-U14, DoD M2-63): **a key drag costs one
//! track, and a paint costs the keys that can be seen.**
//!
//! The timeline document already re-reads one track per key-drag frame
//! (`TimelineDoc::apply_changes`). The panel must not then re-show everything — rebuild
//! every track's row, clone every key id, rebuild the clip list — on each frame; and the
//! timeline must not walk every key of every visible track when it paints, only those in
//! the visible time range, one mark per pixel column.
//!
//! The check: a clip of 21 tracks (20 of them 400 keys each, a hundred of those past the
//! clip's end) and a 20-frame drag of one key on the first track. Every frame rebuilds
//! **one** timeline row (measured through `PanelFaults::sequencer_rows_probe`), the drag
//! still lands (one undo entry, the key at its new time), and the timeline's last paint drew
//! no more key marks than the lanes have pixel columns per track, never the 8,000 keys.
//!
//! Positive controls (W2):
//! * `positive_control_a_full_reshow_per_frame_fails` — the panel re-showing everything on
//!   each change (`PanelFaults::sequencer_full_show`, the code as found) rebuilds every row a
//!   frame, and the check fails.
//! * `positive_control_painting_every_key_fails` — a timeline painting every key of every
//!   visible track (`PanelFaults::timeline_paint_every_key`) draws them all, and the check
//!   fails.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use common::{history_len, part, raise, setting};
use forge_cmd::{EditorCommand, Value};
use forge_editor::services::PanelFaults;
use forge_panels_authoring::widgets::{DragPhase, KeysDragged, TimelineView};

const P: &str = "forge.sequencer";
const EXTRA: usize = 20;
const KEYS: usize = 400;
const FRAMES: usize = 20;

fn set(key: String, v: Value) -> EditorCommand {
    EditorCommand::SetSetting {
        key,
        value: Some(v),
    }
}

fn check(mut faults: PanelFaults) -> Result<(), String> {
    let probe = Rc::new(Cell::new(0usize));
    faults.sequencer_rows_probe = Some(probe.clone());
    let (mut rig, e) = common::sequencer_with_keys(faults);
    // Twenty dense tracks on the clip (a quarter of their keys past its 5 s end).
    let mut cmds = Vec::new();
    for i in 0..EXTRA {
        let t = format!("seq.clip.walk.track.dense{i:02}");
        cmds.push(set(format!("{t}.kind"), Value::Text("Property".into())));
        cmds.push(set(format!("{t}.entity"), Value::Entity(e)));
        cmds.push(set(
            format!("{t}.path"),
            Value::Text("light.intensity".into()),
        ));
        for j in 0..KEYS {
            let at = j as f64 * 0.01666;
            cmds.push(set(format!("{t}.key.k{j:04}.t"), Value::Float(at)));
            cmds.push(set(format!("{t}.key.k{j:04}.v"), Value::Float(j as f64)));
        }
    }
    rig.shell.emitter().emit_all("Dense tracks", cmds);
    rig.settle();
    let tl = part(&rig, P, &["body", "timeline"]);
    let rows = rig
        .h
        .ui
        .widget::<TimelineView>(tl)
        .map_or(0, TimelineView::painted_rows);
    if rows < 2 {
        return Err(format!("set-up: the timeline painted {rows} row(s)"));
    }
    // The drag: key k1 of the first track, 20 frames.
    let before = history_len(&rig);
    let drag = |rig: &mut forge_editor::testing::Rig, dt: f64, phase| {
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
    };
    drag(&mut rig, 0.1, DragPhase::Begin);
    probe.set(0);
    for i in 0..FRAMES {
        drag(
            &mut rig,
            0.5 * (i as f64 + 1.0) / FRAMES as f64,
            DragPhase::Move,
        );
    }
    let built = probe.get();
    drag(&mut rig, 0.5, DragPhase::End);
    if history_len(&rig) != before + 1 {
        return Err("the drag is not one undo entry".into());
    }
    if setting(&rig, "seq.clip.walk.track.crate_light_intensity.key.k1.t")
        != Some(Value::Float(1.5))
    {
        return Err("the drag did not land".into());
    }
    if built > FRAMES {
        return Err(format!(
            "{FRAMES} key-drag frames rebuilt {built} timeline rows ({:.1} per frame, {} tracks)",
            built as f64 / FRAMES as f64,
            EXTRA + 1
        ));
    }
    // The last paint: only the keys that can be seen.
    rig.h.ui.damage_all();
    rig.settle();
    let (painted, w) = rig
        .h
        .ui
        .widget::<TimelineView>(tl)
        .map(|t| (t.painted_keys(), rig.h.ui.rect(tl).map_or(0.0, |r| r.w)))
        .unwrap_or_default();
    let rows = rig
        .h
        .ui
        .widget::<TimelineView>(tl)
        .map_or(0, TimelineView::painted_rows);
    // What can be seen: per row, the keys inside the clip's time range (a key past the end
    // is off the lanes), and never more than one mark per pixel column.
    let in_range = (0..KEYS).filter(|j| *j as f64 * 0.01666 <= 5.1).count();
    let by_time = 2 + rows.saturating_sub(1) * in_range;
    let by_pixels = rows * (w.max(0.0) as usize + 2);
    let bound = by_time.min(by_pixels);
    if painted == 0 || painted > bound {
        return Err(format!(
            "the timeline painted {painted} key marks over {rows} row(s) {w:.0} px wide (at most {bound} can be seen; the rows hold {} keys)",
            rows.saturating_sub(1) * KEYS + 2
        ));
    }
    eprintln!(
        "measured: {built} timeline row(s) rebuilt over {FRAMES} drag frames; {painted} key marks painted of {} keys in {rows} row(s)",
        rows.saturating_sub(1) * KEYS + 2
    );
    Ok(())
}

#[test]
fn a_key_drag_rebuilds_one_track_and_paint_draws_what_is_seen() {
    check(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_full_reshow_per_frame_fails() {
    let e = check(PanelFaults {
        sequencer_full_show: true,
        ..PanelFaults::default()
    })
    .expect_err("re-showing every track per frame must fail");
    assert!(e.contains("timeline rows"), "{e}");
    eprintln!("control (the code as found): {e}");
}

#[test]
fn positive_control_painting_every_key_fails() {
    let e = check(PanelFaults {
        timeline_paint_every_key: true,
        ..PanelFaults::default()
    })
    .expect_err("painting every key must fail");
    assert!(e.contains("key marks"), "{e}");
    eprintln!("control: {e}");
}
