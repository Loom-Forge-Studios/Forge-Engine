//! Gesture hot-path probe: the per-frame cost of a gizmo drag (three properties a frame,
//! one stream subscriber pumped every frame, the audit folding on). Run with
//! `cargo run -p forge-cmd --release --example gesture_bench`. Numbers are recorded in
//! ADR 0010; this is a measuring tool, not a gate.

use std::time::Instant;

use forge_cmd::{Bus, Change, CommandSink, EditorCommand, Issuer, Value};

const FRAMES: u32 = 200_000;

fn main() {
    let me = Issuer::Human { user: "ada".into() };
    let mut best = f64::INFINITY;
    for _round in 0..5 {
        let mut bus = Bus::new();
        let feed = bus.subscribe();
        let spawn = bus.envelope(
            me.clone(),
            EditorCommand::Spawn {
                name: "Cube".into(),
                parent: None,
            },
        );
        let cube = match bus.apply(spawn).map(|a| a.diff.changes.first().cloned()) {
            Ok(Some(Change::Created { entity, .. })) => entity,
            other => panic!("spawn failed: {other:?}"),
        };
        let _ = feed.drain();
        let drag = bus.begin("Drag position", me.clone());
        let paths = [
            "transform.position.x",
            "transform.position.y",
            "transform.position.z",
        ];
        let t0 = Instant::now();
        let mut seen = 0usize;
        for f in 0..FRAMES {
            for path in paths {
                let cmd = EditorCommand::SetProperty {
                    entity: cube,
                    path: path.to_string(),
                    value: Value::Float(f64::from(f) * 0.5),
                };
                let e = bus.envelope_in(drag, me.clone(), cmd);
                if bus.apply(e).is_err() {
                    panic!("frame refused");
                }
            }
            seen += feed.drain().len();
        }
        let dt = t0.elapsed();
        bus.commit(drag).expect("commits");
        assert_eq!(seen, FRAMES as usize * paths.len());
        let ns = dt.as_nanos() as f64 / f64::from(FRAMES);
        best = best.min(ns);
    }
    println!(
        "gesture frame (3 SetProperty + 1 subscriber drain): best {best:.0} ns/frame over 5 rounds of {FRAMES} frames"
    );
}
