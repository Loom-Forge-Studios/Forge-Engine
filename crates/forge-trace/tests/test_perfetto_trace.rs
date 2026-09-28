//! The Perfetto sink writes a trace Perfetto can load: descriptors before events, one thread
//! track per recording thread, every thread's slices properly nested and balanced, events in
//! time order, counters exact, frame marks and budget overruns as instants. The trace is
//! decoded back with `forge_trace::perfetto::read` and checked structurally; the checker is
//! itself shown to reject an unbalanced trace.

use std::collections::BTreeMap;

use forge_trace::perfetto::{self, Decoded, TrackKind};
use forge_trace::{Sinks, Tracer};

/// Structural check of a decoded trace; `Err` names the first problem.
fn check(d: &Decoded) -> Result<(), String> {
    if d.first_flags & 1 == 0 {
        return Err("first packet does not clear incremental state".into());
    }
    let known: BTreeMap<u64, TrackKind> = d.tracks.iter().map(|t| (t.uuid, t.kind)).collect();
    let mut stacks: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    let mut last_ts = 0;
    for e in &d.events {
        if e.ts < last_ts {
            return Err(format!("event at {} after {last_ts}", e.ts));
        }
        last_ts = e.ts;
        if !known.contains_key(&e.track) {
            return Err(format!("event on undeclared track {}", e.track));
        }
        match e.kind {
            perfetto::SLICE_BEGIN => stacks
                .entry(e.track)
                .or_default()
                .push(e.name.clone().unwrap_or_default()),
            perfetto::SLICE_END => {
                if stacks.entry(e.track).or_default().pop().is_none() {
                    return Err(format!("unbalanced end on track {} at {}", e.track, e.ts));
                }
            }
            perfetto::COUNTER => {
                if known.get(&e.track) != Some(&TrackKind::Counter) || e.value.is_none() {
                    return Err(format!("counter sample on a non-counter track {}", e.track));
                }
            }
            perfetto::INSTANT => {}
            k => return Err(format!("unknown event type {k}")),
        }
    }
    for (t, s) in stacks {
        if !s.is_empty() {
            return Err(format!("track {t} leaves {s:?} open"));
        }
    }
    Ok(())
}

#[test]
fn a_multi_thread_recording_decodes_to_a_well_formed_trace() {
    let t: &'static Tracer = Box::leak(Box::new(Tracer::new()));
    t.enable(Sinks::PERFETTO.union(Sinks::COUNTERS)).unwrap();
    t.declare_budget("sim.step", 0.0, "ms");
    t.frame_mark();
    let workers: Vec<_> = (0..3)
        .map(|i| {
            std::thread::Builder::new()
                .name(format!("worker {i}"))
                .spawn(move || {
                    for _ in 0..50 {
                        let _outer = t.zone("sim.step");
                        {
                            let _a = t.zone("sim.integrate");
                            let _b = t.zone("sim.integrate.inner");
                        }
                        let _c = t.zone("sim.spin");
                    }
                })
                .unwrap()
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    t.counter("sim.bodies", 12.5);
    t.counter("sim.bodies", -3.0);
    t.instant("play");
    t.frame_mark();
    let bytes = t.take_perfetto("forge-test");
    let d = perfetto::read(&bytes).expect("the trace decodes");
    check(&d).unwrap();

    let threads: Vec<&str> = d
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Thread)
        .map(|t| t.name.as_str())
        .collect();
    for i in 0..3 {
        assert!(
            threads.contains(&format!("worker {i}").as_str()),
            "{threads:?}"
        );
    }
    assert!(
        d.tracks
            .iter()
            .any(|t| t.kind == TrackKind::Process && t.name == "forge-test")
    );
    let begins = d
        .events
        .iter()
        .filter(|e| e.kind == perfetto::SLICE_BEGIN)
        .count();
    assert_eq!(begins, 3 * 50 * 4);
    let values: Vec<f64> = d.events.iter().filter_map(|e| e.value).collect();
    assert_eq!(values, [12.5, -3.0]);
    let instants: Vec<&str> = d
        .events
        .iter()
        .filter(|e| e.kind == perfetto::INSTANT)
        .filter_map(|e| e.name.as_deref())
        .collect();
    assert!(instants.contains(&"play"));
    assert!(instants.contains(&"frame"));
    assert!(
        instants.contains(&"over budget: sim.step"),
        "a zero-allowance budget is over, and the trace says so: {instants:?}"
    );
    assert!(d.events.iter().all(|e| e.sequence == 1));
}

#[test]
fn the_checker_rejects_an_unbalanced_trace() {
    let t = Tracer::new();
    t.enable(Sinks::PERFETTO).unwrap();
    {
        let _z = t.zone("a.b");
    }
    let mut d = perfetto::read(&t.take_perfetto("x")).unwrap();
    check(&d).unwrap();
    // Drop the SLICE_END: the checker must see the open slice.
    d.events.retain(|e| e.kind != perfetto::SLICE_END);
    assert!(check(&d).unwrap_err().contains("open"));
}

#[test]
fn an_empty_trace_is_still_a_valid_trace() {
    let t = Tracer::new();
    let d = perfetto::read(&t.take_perfetto("empty")).unwrap();
    check(&d).unwrap();
    assert_eq!(d.tracks.len(), 1);
}
