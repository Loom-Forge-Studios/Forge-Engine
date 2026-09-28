//! WP-40: a timed body waits for a **quiet machine** before it measures
//! (`forge_trace::timed::quiet`, ADR 0057).
//!
//! * `the_load_sampler_returns_sane_values`: the real sampler (Windows `GetSystemTimes`,
//!   Linux `/proc/stat`) gives shares in `0..=1`, and two threads spinning in this process
//!   show up as this process's load, not as load outside it — the sampler measures, it does
//!   not return a constant.
//! * `a_quiet_machine_is_not_waited_for`: a (faked) idle machine passes on the first sample.
//! * Positive control (W2): `positive_control_a_busy_machine_is_waited_for_then_reported`:
//!   a faked busy machine makes `run_timed_alone_with` wait out its bound, sample again and
//!   again, run the body anyway and report **MEASURED UNDER LOAD** — counted, and in the
//!   report line. `a_failure_under_load_says_so`: a body that fails under load carries the
//!   report in its error.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use forge_trace::timed::quiet::{
    LoadSampler, QuietOutcome, last_quiet_report, quiet_counters, wait_for_quiet,
};
use forge_trace::timed::{QuietFaults, QuietPolicy, TimedOptions, run_timed_alone_with};

fn short(max_wait_ms: u64) -> QuietPolicy {
    QuietPolicy {
        threshold: 0.35,
        window: Duration::from_millis(200),
        recheck: Duration::from_millis(300),
        max_wait: Duration::from_millis(max_wait_ms),
    }
}

fn faked(load: f64) -> QuietFaults {
    QuietFaults {
        fake_load: Some(load),
    }
}

#[test]
fn the_load_sampler_returns_sane_values() {
    let mut s = LoadSampler::new().expect("a load sampler on Windows and Linux");
    let window = Duration::from_millis(800);
    let idle = s.sample(window).expect("sample");
    for v in [idle.outside, idle.system, idle.own] {
        assert!((0.0..=1.0).contains(&v), "{idle:?}");
    }
    assert!(idle.system + 1e-9 >= idle.outside, "{idle:?}");
    // Two threads of this process spin through the next window: this process's share rises
    // by about two cores, and the load outside it is not charged for them.
    let stop = Arc::new(AtomicBool::new(false));
    let spinners: Vec<_> = (0..2)
        .map(|_| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut x = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
                }
                x
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    let busy = s.sample(window).expect("sample");
    stop.store(true, Ordering::Relaxed);
    for t in spinners {
        let _ = t.join();
    }
    let cores = f64::from(busy.cores);
    println!("{cores} cores; idle sample {idle:?}; with two spinning threads {busy:?}");
    for v in [busy.outside, busy.system, busy.own] {
        assert!((0.0..=1.0).contains(&v), "{busy:?}");
    }
    // At least one core's worth of the two (the other lane may be compiling beside us).
    assert!(
        busy.own >= 1.0 / cores,
        "two spinning threads were not seen as this process's load: {busy:?}"
    );
    assert!(
        busy.own > idle.own,
        "own load did not rise: {idle:?} -> {busy:?}"
    );
    assert!(busy.system + 1e-9 >= busy.own, "{busy:?}");
}

#[test]
fn a_quiet_machine_is_not_waited_for() {
    let r = wait_for_quiet(&short(5_000), faked(0.05));
    assert_eq!(r.outcome, QuietOutcome::Quiet);
    assert_eq!(r.samples, 1);
    assert!(r.waited < Duration::from_secs(5), "{r:?}");
    assert!(r.line("t").contains(": quiet"), "{}", r.line("t"));
}

#[test]
fn positive_control_a_busy_machine_is_waited_for_then_reported() {
    let before = quiet_counters();
    let opts = TimedOptions {
        quiet: Some(short(2_000)),
        quiet_faults: faked(0.9),
    };
    let r = run_timed_alone_with(opts, || Ok("the body ran".into()));
    assert_eq!(r.as_deref(), Ok("the body ran"), "the body runs anyway");
    let q = last_quiet_report().expect("the wait is reported");
    let line = q.line("control");
    println!("{line}");
    assert!(q.under_load(), "a busy machine was not reported: {q:?}");
    assert!(
        q.samples >= 3,
        "the gate did not keep re-checking a busy machine: {q:?}"
    );
    assert!(
        q.waited >= Duration::from_millis(1_000),
        "the gate did not wait for a busy machine: {q:?}"
    );
    assert!(
        q.waited <= Duration::from_secs(30),
        "the bound did not hold: {q:?}"
    );
    assert_eq!(q.peak, Some(0.9));
    assert!(
        line.contains("MEASURED UNDER LOAD") && line.contains("FAKED"),
        "{line}"
    );
    // Process-wide counters: a sibling test of this binary may wait meanwhile (cargo test).
    let after = quiet_counters();
    assert!(after.waits > before.waits, "{before:?} -> {after:?}");
    assert!(
        after.under_load > before.under_load,
        "{before:?} -> {after:?}"
    );
    assert!(after.waited_ms >= before.waited_ms + 1_000);
}

#[test]
fn a_failure_under_load_says_so() {
    let opts = TimedOptions {
        quiet: Some(short(700)),
        quiet_faults: faked(1.0),
    };
    let e = run_timed_alone_with(opts, || Err("frame 12 took 40 ms".into()))
        .expect_err("the body failed");
    assert!(
        e.contains("frame 12 took 40 ms") && e.contains("MEASURED UNDER LOAD"),
        "a failure under load does not say it was measured under load: {e}"
    );
}
