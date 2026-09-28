//! Guard (M2-11, Ch.29): **tracing costs ~0 when disabled** — measured, not asserted by
//! reading the code.
//!
//! A disabled zone (open + close) is timed in a tight loop against the same loop with no
//! zone; the difference is the zone's cost. It must stay under [`DISABLED_BUDGET_NS`]: that
//! is only achievable by a zone that reads no clock, takes no lock and allocates nothing
//! while every sink is off (a relaxed load and a branch, ~0.3 ns here).
//!
//! Each figure is the **minimum** over several trials: preemption and a loaded machine only
//! ever add time, so the minimum is the cost of the code, and the guard does not flake under
//! a parallel test run (W5: no retry, no widened tolerance).
//!
//! Positive control (W2): `positive_control_a_zone_that_reads_the_clock_while_disabled_fails`
//! runs the identical measurement over a zone that timestamps its open and close whether or
//! not a sink is on — the mistake this guard exists for — and requires it to exceed the
//! budget.
//!
//! The enabled costs (live counters, Perfetto) are measured and printed too, for the record.
//!
//! Every measurement runs **alone** (`forge_trace::timed::run_timed_alone`, WP-19): in a
//! process of its own running only that test, at the High priority class, holding the
//! machine-wide timed lock — so plain `cargo test` (whose tests are threads of one process)
//! measures as nextest does, and another lane's build or timed test cannot share its turn.

use std::hint::black_box;
use std::time::{Duration, Instant};

use forge_trace::{Sinks, Tracer};

/// A disabled zone must cost less than this per open+close.
const DISABLED_BUDGET_NS: f64 = 2.0;
const ITERS: u32 = 2_000_000;
/// Iterations of the enabled measurements (they record every zone).
const ENABLED_ITERS: u32 = 100_000;
const TRIALS: usize = 9;

/// Run a measuring test's body alone (see the module docs) and print what it reports.
fn timed(body: impl FnOnce() -> Result<String, String>) {
    let s = forge_trace::timed::run_timed_alone(body).unwrap_or_else(|e| panic!("{e}"));
    println!("{s}");
}

fn min_ns_per_iter(iters: u32, mut f: impl FnMut()) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..TRIALS {
        let t0 = Instant::now();
        for _ in 0..iters {
            f();
        }
        let ns = t0.elapsed().as_nanos() as f64 / f64::from(iters);
        best = best.min(ns);
    }
    best
}

/// Cost of `f` over an empty iteration, ns.
fn overhead_ns(f: impl FnMut()) -> f64 {
    overhead_ns_n(ITERS, f)
}

fn overhead_ns_n(iters: u32, f: impl FnMut()) -> f64 {
    let base = min_ns_per_iter(iters, || black_box(()));
    (min_ns_per_iter(iters, f) - base).max(0.0)
}

/// The mistake: a zone that reads the clock at open and close even with every sink off.
struct ClockReadingZone<'t> {
    tracer: &'t Tracer,
    start: Instant,
}

impl<'t> ClockReadingZone<'t> {
    fn open(tracer: &'t Tracer) -> Self {
        Self {
            tracer,
            start: Instant::now(),
        }
    }
}

impl Drop for ClockReadingZone<'_> {
    fn drop(&mut self) {
        let d: Duration = self.start.elapsed();
        if !self.tracer.sinks().is_empty() {
            black_box(d);
        }
    }
}

#[test]
fn a_disabled_zone_costs_nothing_measurable() {
    timed(a_disabled_zone_body);
}

fn a_disabled_zone_body() -> Result<String, String> {
    let t = Tracer::new();
    let zone = overhead_ns(|| {
        let z = t.zone("sim.step");
        black_box(&z);
    });
    let global = overhead_ns(|| {
        let z = forge_trace::zone!("sim.step");
        black_box(&z);
    });
    let counter = overhead_ns(|| t.counter("sim.bodies", black_box(3.0)));
    let line = format!(
        "disabled: zone {zone:.3} ns, zone! (global) {global:.3} ns, counter {counter:.3} ns (budget {DISABLED_BUDGET_NS} ns)"
    );
    assert!(
        zone <= DISABLED_BUDGET_NS,
        "a disabled zone costs {zone:.2} ns > {DISABLED_BUDGET_NS} ns"
    );
    assert!(
        global <= DISABLED_BUDGET_NS,
        "a disabled zone! costs {global:.2} ns"
    );
    assert!(
        counter <= DISABLED_BUDGET_NS,
        "a disabled counter costs {counter:.2} ns"
    );
    assert_eq!(t.perfetto_len(), (0, 0));
    assert!(t.snapshot().frames.is_empty());
    Ok(line)
}

#[test]
fn positive_control_a_zone_that_reads_the_clock_while_disabled_fails() {
    timed(|| {
        let t = Tracer::new();
        let bad = overhead_ns(|| {
            let z = ClockReadingZone::open(&t);
            black_box(&z);
        });
        assert!(
            bad > DISABLED_BUDGET_NS,
            "the guard cannot see a clock read: a clock-reading zone measured {bad:.2} ns"
        );
        Ok(format!("clock-reading zone while disabled: {bad:.3} ns"))
    });
}

#[test]
fn enabled_costs_for_the_record() {
    timed(enabled_costs_body);
}

fn enabled_costs_body() -> Result<String, String> {
    let t = Tracer::new();
    t.set_perfetto_capacity(usize::MAX);
    t.enable(Sinks::COUNTERS).map_err(|e| e.to_string())?;
    let counters = overhead_ns_n(ENABLED_ITERS, || {
        let z = t.zone("sim.step");
        black_box(&z);
    });
    t.disable(Sinks::COUNTERS);
    t.enable(Sinks::PERFETTO).map_err(|e| e.to_string())?;
    let perfetto = overhead_ns_n(ENABLED_ITERS, || {
        let z = t.zone("sim.step");
        black_box(&z);
    });
    let (events, dropped) = t.perfetto_len();
    let t0 = Instant::now();
    let bytes = t.take_perfetto("bench");
    let encode = t0.elapsed();
    assert!(counters > 0.0 && perfetto > 0.0);
    Ok(format!(
        "enabled: counters {counters:.1} ns/zone, perfetto {perfetto:.1} ns/zone; encoded {events} events ({dropped} dropped) into {} bytes in {:.1} ms",
        bytes.len(),
        encode.as_secs_f64() * 1e3
    ))
}
