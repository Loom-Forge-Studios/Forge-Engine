//! `test_tile_stroke_bounded` (Ch.21 §21.21 "2D editors", D-5 one-frame drag latency, gate
//! row `C-tile-stroke-bounded`; DoD M2-66): a paint stroke's step costs the cells it
//! painted, not the stroke so far and not the map. On a 512 × 512 map of terrain cells
//! (32 × 32 = 1,024 chunks) every step of a 40-cell stroke
//!
//! * **sends** only the chunks that step dirtied (one here: each step paints one cell) —
//!   not every chunk the stroke touched so far, which is O(n²) commands over a long stroke;
//! * brings the editor's model up to date by decoding or copying at most the stroke's
//!   chunks — the mirror's echo is applied key by key, the canvas shares the layer
//!   instead of copying it;
//! * takes a loop turn about as long as a step on a 128 × 128 map (8 × 8 = 64 chunks).
//!
//! Renaming the layer afterwards decodes no chunk at all: a non-chunk `d2.` key is applied
//! in place, not by re-reading the namespace.
//!
//! Positive controls (W2): `positive_control_full_reread_per_step_fails` — the editor
//! re-reads the whole `d2.` namespace on each change: the model, rename and time bounds
//! fail; `positive_control_resending_the_stroke_fails` — each step re-sends every chunk the
//! stroke touched so far: the send bound fails.

mod common;

use std::cell::Cell as StdCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::domain::scene2d::{Cell, Chunk, chunk_key};
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;
use forge_panels_domain::widgets::{PaintCells, StrokePhase};

const P: &str = "forge.editors_2d";
/// The stroke: cells (0, 0) .. (39, 0), one per step — chunks (0, 0), (1, 0), (2, 0).
const STEPS: i64 = 40;
const STROKE_CHUNKS: usize = 3;

/// Which positive control, if any.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    FullReread,
    ResendStroke,
}

struct Map {
    rig: Rig,
    probe: Rc<StdCell<usize>>,
    canvas: forge_ui::WidgetId,
}

/// A rig showing a `side` × `side`-chunk map of terrain cells, its layer selected and the
/// ground terrain as the brush.
fn map(side: i64, fault: Fault) -> Result<Map, String> {
    let probe = Rc::new(StdCell::new(0));
    let faults = PanelFaults {
        d2_full_reread: fault == Fault::FullReread,
        d2_stroke_resends_all: fault == Fault::ResendStroke,
        d2_model_probe: Some(probe.clone()),
        ..PanelFaults::default()
    };
    let mut rig = common::rig_with(&[P], faults, |_| {});
    let new_ts = common::part(&rig, P, &["tiles", "bar", "new_tileset"]);
    common::press(&mut rig, new_ts);
    let new_map = common::part(&rig, P, &["tiles", "bar", "new_map"]);
    common::press(&mut rig, new_map);
    let mut chunk = Chunk::default();
    // Another terrain (the heavier cell to decode), so every step of the stroke changes a cell.
    for c in &mut chunk.cells {
        *c = Cell::Terrain("rock".into());
    }
    let text = chunk.encode();
    let mut script = rig.connect(Issuer::Script {
        path: "big_map.fscript".into(),
    });
    for cy in 0..side {
        for cx in 0..side {
            script.apply(
                EditorCommand::SetSetting {
                    key: chunk_key("tile_map", 0, (cx, cy)),
                    value: Some(Value::Text(text.clone())),
                },
                None,
            );
        }
    }
    let _ = script.pump();
    rig.settle();
    let maps = common::part(&rig, P, &["tiles", "body", "side", "maps"]);
    let layer = common::rows_with(&mut rig, maps, "Layer 0");
    common::select_row(&mut rig, maps, *layer.first().ok_or("no layer row")?);
    let tiles = common::part(&rig, P, &["tiles", "body", "side", "tilesets"]);
    let ground = common::rows_with(&mut rig, tiles, "Ground");
    common::select_row(&mut rig, tiles, *ground.first().ok_or("no terrain row")?);
    let canvas = common::part(&rig, P, &["tiles", "body", "main", "canvas"]);
    Ok(Map { rig, probe, canvas })
}

/// One stroke step on `m`: its loop turn's time, the model work it caused and the commands
/// it sent.
fn step(m: &mut Map, i: i64) -> (Duration, usize, u64) {
    let phase = match i {
        0 => StrokePhase::Begin,
        STEPS => StrokePhase::End,
        _ => StrokePhase::Move,
    };
    let cells = if phase == StrokePhase::End {
        Vec::new()
    } else {
        vec![(i, 0)]
    };
    m.probe.set(0);
    let sent0 = m.rig.shell.emitter().emitted();
    let t = Instant::now();
    m.rig.h.ui.raise(
        m.canvas,
        PaintCells {
            cells,
            erase: false,
            phase,
        },
    );
    m.rig.turn();
    // The mirror's echo of the step is applied on the next turn.
    m.rig.turn();
    let elapsed = t.elapsed();
    let sent = m.rig.shell.emitter().emitted() - sent0;
    (elapsed, m.probe.get(), sent)
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or_default()
}

fn run(fault: Fault) -> Result<String, String> {
    // 32 chunks a side = 512 cells; 8 chunks a side = 128 cells (still more than the
    // canvas shows, so both paint a full screen).
    let mut big = map(32, fault)?;
    let mut small = map(8, fault)?;
    let (mut tb, mut ts) = (Vec::new(), Vec::new());
    let (mut worst, mut worst_sent, mut total_sent) = (0usize, 0u64, 0u64);
    // Interleaved, so load on the machine lands on both alike.
    for i in 0..=STEPS {
        let (d, w, sent) = step(&mut big, i);
        tb.push(d);
        worst = worst.max(w);
        // The last step only commits the gesture (no chunk).
        if i < STEPS {
            worst_sent = worst_sent.max(sent);
            total_sent += sent;
        }
        let (d, _, _) = step(&mut small, i);
        ts.push(d);
    }
    big.rig.settle();
    // The stroke landed (the bound is not met by painting nothing).
    let painted = (0..STEPS)
        .filter(|x| {
            let (c, i) = forge_editor::domain::scene2d::chunk_of(*x, 0);
            common::setting(&big.rig, &chunk_key("tile_map", 0, c)).is_some_and(|v| match v {
                Value::Text(t) => t.split(',').nth(i) == Some("aground"),
                _ => false,
            })
        })
        .count();
    if painted != STEPS as usize {
        return Err(format!("{painted} of {STEPS} stroke cells painted"));
    }
    // Rename the layer: a non-chunk key, applied in place.
    big.probe.set(0);
    let mut script = big.rig.connect(Issuer::Script {
        path: "rename.fscript".into(),
    });
    script.apply(
        EditorCommand::SetSetting {
            key: "d2.tilemap.tile_map.layer.0.name".into(),
            value: Some(Value::Text("Ground layer".into())),
        },
        None,
    );
    let _ = script.pump();
    big.rig.settle();
    let rename_work = big.probe.get();
    let maps = common::part(&big.rig, P, &["tiles", "body", "side", "maps"]);
    if common::rows_with(&mut big.rig, maps, "Ground layer").is_empty() {
        return Err("the renamed layer is not listed".into());
    }
    let total = (32 * 32) as usize;
    let (mb, ms) = (median(tb), median(ts));
    let report = format!(
        "worst step: {worst} chunks decoded or copied (map {total}, stroke {STROKE_CHUNKS}); \
         worst step sent {worst_sent} command(s), {total_sent} over {STEPS} steps; \
         a layer rename decoded {rename_work}; \
         median step turn {:.2} ms on 512x512 vs {:.2} ms on 128x128",
        mb.as_secs_f64() * 1e3,
        ms.as_secs_f64() * 1e3
    );
    let mut failed = Vec::new();
    if worst > STROKE_CHUNKS {
        failed.push("model work unbounded");
    }
    // Each step paints one cell: one chunk dirtied, one command.
    if worst_sent > 1 {
        failed.push("stroke steps re-send the stroke");
    }
    if rename_work > 0 {
        failed.push("a layer rename re-reads the map");
    }
    // A step on the big map may cost at most three times a step on the small one (an
    // O(map) step has 16 times the chunks to go through; measured ~1.05x when bounded,
    // ~14x for a full re-read per step).
    if mb > ms * 3 {
        failed.push("step time scales with the map");
    }
    if failed.is_empty() {
        Ok(report)
    } else {
        Err(format!("{}: {report}", failed.join(", ")))
    }
}

#[test]
fn test_tile_stroke_bounded() {
    // A wall-clock gate (a step on 512x512 against one on 128x128): timed alone, in a process
    // of its own, on a quiet machine (WP-40).
    let r =
        forge_trace::timed::run_timed_alone(|| run(Fault::None)).unwrap_or_else(|e| panic!("{e}"));
    println!("test_tile_stroke_bounded: {r}");
}

#[test]
fn positive_control_full_reread_per_step_fails() {
    let e = run(Fault::FullReread).err().unwrap_or_default();
    println!("positive control: {e}");
    assert!(
        e.contains("model work unbounded")
            && e.contains("a layer rename re-reads the map")
            && e.contains("step time scales with the map"),
        "a full re-read per stroke step passed: {e:?}"
    );
}

#[test]
fn positive_control_resending_the_stroke_fails() {
    let e = run(Fault::ResendStroke).err().unwrap_or_default();
    println!("positive control: {e}");
    assert!(
        e.contains("stroke steps re-send the stroke"),
        "re-sending the stroke on every step passed: {e:?}"
    );
}
