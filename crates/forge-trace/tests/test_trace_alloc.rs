//! Guard (WP-19, owner rule 2): **with the live counters on, a steady frame allocates
//! nothing** — measured with a counting allocator (thread-local, so sibling tests cannot
//! pollute it), not asserted by reading the code.
//!
//! A steady frame is what the editor and `forge --headless` do sixty times a second while a
//! simulation plays: the same zones open and close, the same counters are reported, and the
//! frame is marked. Before WP-19 each counter report copied its name into a fresh `String`
//! and each frame mark allocated a `String` per zone part plus a new timeline sample; now
//! counters are keyed by their `&'static str`, zone totals are zeroed in place, and the
//! timeline's oldest sample is recycled once it is full.
//!
//! Positive control (W2): `positive_control_a_new_counter_name_allocates` — the identical
//! measurement over frames that report counters under names the tracer has not seen
//! allocates (the counter table grows), so the counter is not blind.

use allocation_counter::measure;
use forge_trace::{Sinks, TIMELINE_FRAMES, Tracer};

/// One steady frame: two zones (one nested, one twice), two counters, a frame mark.
fn frame(t: &Tracer, extra_counter: Option<&'static str>) {
    {
        let _a = t.zone("sim.step");
        let _b = t.zone("sim.integrate");
    }
    {
        let _a = t.zone("sim.step");
    }
    t.counter("sim.bodies", 1000.0);
    t.counter("render.frame.draw_calls", 12.0);
    if let Some(name) = extra_counter {
        t.counter(name, 1.0);
    }
    t.frame_mark();
}

/// A tracer with the live counters on and a budget per zone, run until the timeline is full
/// (the warm-up: a fresh timeline grows until it holds `TIMELINE_FRAMES` frames).
fn warmed() -> Tracer {
    let t = Tracer::new();
    t.enable(Sinks::COUNTERS).expect("counters");
    t.declare_budget("sim.step", 4.0, "ms");
    t.declare_budget("sim.bodies", 10_000.0, "");
    for _ in 0..(TIMELINE_FRAMES + 16) {
        frame(&t, None);
    }
    t
}

#[test]
fn a_steady_frame_allocates_nothing() {
    let t = warmed();
    let info = measure(|| {
        for _ in 0..100 {
            frame(&t, None);
        }
    });
    println!(
        "100 steady frames with the live counters on: {} allocation(s), {} B",
        info.count_total, info.bytes_total
    );
    assert_eq!(
        info.count_total, 0,
        "a steady frame allocated ({info:?}): counters and frame marks must reuse their storage"
    );
    // The feed still records what happened.
    let s = t.snapshot();
    assert_eq!(s.frames.len(), TIMELINE_FRAMES);
    let last = s.frames.last().expect("a frame");
    assert!(last.parts.iter().any(|(n, _)| n == "sim.step"));
    assert!(last.parts.iter().any(|(n, _)| n == "sim.integrate"));
    assert!(s.counters.contains(&("sim.bodies".to_owned(), 1000.0)));
}

#[test]
fn positive_control_a_new_counter_name_allocates() {
    let t = warmed();
    // More new names than one table node holds: the counter table must grow.
    let names: Vec<&'static str> = (0..40)
        .map(|i| &*Box::leak(format!("x.counter_{i}").into_boxed_str()))
        .collect();
    let info = measure(|| {
        for n in &names {
            frame(&t, Some(n));
        }
    });
    assert!(
        info.count_total > 0,
        "a frame reporting new counter names allocated nothing: the measurement is blind"
    );
}

#[test]
fn a_zone_that_skips_a_frame_is_not_a_part_of_it() {
    let t = warmed();
    {
        let _z = t.zone("sim.rare");
    }
    t.frame_mark();
    t.frame_mark();
    let s = t.snapshot();
    let n = s.frames.len();
    assert!(s.frames[n - 2].parts.iter().any(|(p, _)| p == "sim.rare"));
    assert!(
        s.frames[n - 1].parts.is_empty(),
        "a frame with no zones has no parts: {:?}",
        s.frames[n - 1]
    );
}
