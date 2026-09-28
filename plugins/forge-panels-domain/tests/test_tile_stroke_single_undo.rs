//! `test_tile_stroke_single_undo` (Ch.21 §21.21 "2D editors", §21.18 gestures, gate row
//! `C-tile-stroke-single-undo`; DoD M2-66): a paint stroke over 40 cells is **one**
//! transaction, one undo entry, and one undo takes every cell back — like a slider drag
//! (`test_gesture_single_undo`). The stroke's commands carry every chunk touched so far,
//! so a gesture step replaced within a frame loses nothing.
//!
//! Positive control (W2): `positive_control_stroke_without_a_gesture_fails` — each step is
//! its own transaction; the stroke becomes many undo entries.

mod common;

use forge_cmd::Value;
use forge_editor::services::PanelFaults;
use forge_panels_domain::widgets::{PaintCells, StrokePhase};

const P: &str = "forge.editors_2d";

fn run(faults: PanelFaults) -> Result<usize, String> {
    let mut rig = common::rig_with(&[P], faults, |_| {});
    let new_ts = common::part(&rig, P, &["tiles", "bar", "new_tileset"]);
    common::press(&mut rig, new_ts);
    let new_map = common::part(&rig, P, &["tiles", "bar", "new_map"]);
    common::press(&mut rig, new_map);
    let maps = common::part(&rig, P, &["tiles", "body", "side", "maps"]);
    let layer = common::rows_with(&mut rig, maps, "Layer 0");
    common::select_row(&mut rig, maps, *layer.first().ok_or("no layer row")?);
    let tiles = common::part(&rig, P, &["tiles", "body", "side", "tilesets"]);
    let ground = common::rows_with(&mut rig, tiles, "Ground");
    common::select_row(&mut rig, tiles, *ground.first().ok_or("no terrain row")?);
    let before_hash = rig.state_hash();
    let before = common::history_len(&rig);
    let canvas = common::part(&rig, P, &["tiles", "body", "main", "canvas"]);
    // A diagonal-ish stroke across four chunks, several steps per loop turn.
    rig.h.ui.raise(
        canvas,
        PaintCells {
            cells: vec![(0, 0)],
            erase: false,
            phase: StrokePhase::Begin,
        },
    );
    for i in 1..40i64 {
        rig.h.ui.raise(
            canvas,
            PaintCells {
                cells: vec![(i, i / 2)],
                erase: false,
                phase: StrokePhase::Move,
            },
        );
        if i % 3 == 0 {
            rig.turn();
        }
    }
    rig.h.ui.raise(
        canvas,
        PaintCells {
            cells: vec![],
            erase: false,
            phase: StrokePhase::End,
        },
    );
    rig.settle();
    let entries = common::history_len(&rig) - before;
    let painted: usize = rig
        .shell
        .mirror()
        .settings_under("d2.tilemap.tile_map.layer.0.chunk.")
        .map(|(_, v)| match v {
            Value::Text(t) => t.matches("aground").count(),
            _ => 0,
        })
        .sum();
    if painted != 40 {
        return Err(format!(
            "{painted} cells painted, expected 40 (a replaced step lost cells)"
        ));
    }
    if entries != 1 {
        return Err(format!(
            "the 40-cell stroke made {entries} undo entries (expected 1)"
        ));
    }
    common::undo(&mut rig);
    if rig.state_hash() != before_hash {
        return Err("one undo did not take the whole stroke back".into());
    }
    Ok(entries)
}

#[test]
fn test_tile_stroke_single_undo() {
    run(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_stroke_without_a_gesture_fails() {
    let e = run(PanelFaults {
        tile_paint_without_gesture: true,
        ..PanelFaults::default()
    })
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("undo entries"),
        "a stroke without a gesture passed: {e:?}"
    );
}
