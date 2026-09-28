// timed-gates: exempt(step costs are printed for the record; the gate on the disabled zone is test_trace_overhead)
//! For the record (M2-11 "trace overhead ~0 when disabled", ADR 0030): what one simulation
//! step of 10,000 moving bodies costs with every trace sink off, with the live counters on,
//! and with Perfetto recording — and so what share of a step the disabled `sim.step` zone
//! is. Printed, not gated: the gate on the disabled path is forge-trace's
//! `test_trace_overhead` (a measured nanosecond budget with a positive control).
//!
//! It does assert what must hold whatever the machine: tracing never changes the simulation
//! (the state hash is identical with every sink combination).

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::Instant;

use forge_cmd::{EntityKey, Value};
use forge_sim::edit::{P_ACCELERATION, P_LOCAL, P_SPIN, P_VELOCITY};
use forge_sim::{EditEntity, EditSnapshot, SimWorld};
use forge_trace::{Sinks, Tracer};

fn scene(n: u64) -> EditSnapshot {
    EditSnapshot::from_entities((0..n).map(|k| {
        let x = k as f64;
        let mut p = BTreeMap::new();
        p.insert(P_LOCAL.to_owned(), Value::Vec3([x, 0.5 * x, -x]));
        p.insert(P_VELOCITY.to_owned(), Value::Vec3([1.0, 2.0, 0.5]));
        p.insert(P_ACCELERATION.to_owned(), Value::Vec3([0.0, -9.81, 0.0]));
        p.insert(P_SPIN.to_owned(), Value::Float(15.0 + x * 1e-3));
        EditEntity {
            key: EntityKey(k),
            name: String::new(),
            parent: None,
            properties: p,
        }
    }))
}

/// Median ms per step over `steps`, and the final state hash.
fn run(edit: &EditSnapshot, tracer: &Tracer, steps: usize) -> (f64, String) {
    let mut w = SimWorld::fork(edit).unwrap();
    let mut t = Vec::with_capacity(steps);
    for _ in 0..steps {
        let t0 = Instant::now();
        w.step(tracer).unwrap();
        t.push(t0.elapsed().as_secs_f64() * 1e3);
        tracer.frame_mark();
    }
    t.sort_by(f64::total_cmp);
    (t[t.len() / 2], w.state_hash())
}

#[test]
fn step_cost_with_and_without_tracing_for_the_record() {
    let edit = scene(10_000);
    let off = Tracer::new();
    let (ms_off, h_off) = run(&edit, &off, 60);
    let counters = Tracer::new();
    counters.enable(Sinks::COUNTERS).unwrap();
    let (ms_counters, h_counters) = run(&edit, &counters, 60);
    let perfetto = Tracer::new();
    perfetto.enable(Sinks::PERFETTO).unwrap();
    let (ms_perfetto, h_perfetto) = run(&edit, &perfetto, 60);
    assert_eq!(h_off, h_counters, "tracing changed the simulation");
    assert_eq!(h_off, h_perfetto, "tracing changed the simulation");
    // One zone per step; forge-trace measures a disabled zone at well under 2 ns.
    let share = 2.0e-6 / ms_off;
    println!(
        "10k bodies, one step: {ms_off:.3} ms tracing off, {ms_counters:.3} ms live counters, {ms_perfetto:.3} ms Perfetto; a disabled zone is < {:.6} % of a step",
        share * 100.0
    );
    assert!(counters.snapshot().frames.len() >= 59);
}
