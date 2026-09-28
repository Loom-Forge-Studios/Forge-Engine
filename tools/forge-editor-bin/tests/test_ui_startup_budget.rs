//! `ui_startup_budget` (Ch.21 §21.22, D-5; gate row `C-ui-startup`, DoD M2-30): the real
//! `forge-editor` binary, cold-started as a fresh process (warm file cache), reaches its first
//! interactive frame — every window drawn and nothing left to do — within **1.5 s** on the
//! reference machine (RTX 3080, 12 logical CPUs).
//!
//! The parent measures from `spawn` to the line the editor prints when it settles
//! (`--report-startup`), so process creation, the plugin load, the GPU device, the window and
//! the first frame are all inside the figure. One untimed warm-up launch first (the linker
//! just wrote the executable), then three timed launches; the gate reads their median. The
//! launches run **alone** (`forge_trace::timed`: the machine-wide timed lock, High priority),
//! and the editor itself starts at the High class; nextest runs this binary alone (the
//! `wall-clock` group, `.config/nextest.toml`), so another lane's build or a sibling test does
//! not share the measurement (scheduling, not a tolerance: W5).
//!
//! Millisecond budgets gate on the reference machine only. Elsewhere (WARP on the Windows CI
//! leg, lavapipe in the Linux container) the test prints the measured value and
//! `AWAITING(reference machine)` — it never passes silently (W9).
//!
//! Positive control (W2): an injected 2 s stall in startup (`FORGE_STARTUP_STALL_MS`, read
//! only by a test build's `controls` feature, ADR 0046) must be measured over the budget, and
//! on the reference machine the gate must fail.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BUDGET_MS: f64 = 1500.0;
const STALL_MS: u64 = 2000;
const RUNS: usize = 3;

/// One launch: (spawn to interactive in ms, the adapter the editor rendered on).
fn launch(stall_ms: Option<u64>) -> Result<(f64, String), String> {
    let exe = env!("CARGO_BIN_EXE_forge-editor");
    let mut cmd = Command::new(exe);
    cmd.args([
        "--no-user-config",
        "--no-remote-host",
        "--exit-after",
        "0",
        "--report-startup",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    if let Some(ms) = stall_ms {
        cmd.env("FORGE_STARTUP_STALL_MS", ms.to_string());
    }
    // The editor starts at the priority class the timed body runs at (High, read back by
    // forge_trace::timed), as a user's foreground launch is scheduled ahead of background
    // builds: without it the child would start at Normal beside another lane's rustc.
    // Scheduling, not a tolerance (W5).
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const HIGH_PRIORITY_CLASS: u32 = 0x0000_0080;
        cmd.creation_flags(HIGH_PRIORITY_CLASS);
    }
    let t0 = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("spawn {exe}: {e}"))?;
    let out = child.stdout.take().ok_or("no stdout")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if line.starts_with("forge-editor: interactive") {
                let _ = tx.send((t0.elapsed(), line));
            }
        }
    });
    let got = rx.recv_timeout(Duration::from_secs(120));
    // `--exit-after 0` ends the run as soon as it settled; a launch that never settles is
    // killed (by the PID this test started, never by image name).
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut e, &mut err);
    }
    let (at, line) = got.map_err(|_| {
        format!("the editor never reported an interactive frame (exit {status:?}): {err}")
    })?;
    if !status.is_some_and(|s| s.success()) {
        return Err(format!("the editor exited with {status:?}: {err}"));
    }
    let adapter = line
        .split("| adapter ")
        .nth(1)
        .unwrap_or("unknown")
        .trim()
        .to_string();
    Ok((at.as_secs_f64() * 1000.0, adapter))
}

/// The reference machine (Ch.21 §21.22): the development workstation's RTX 3080 with 12
/// logical CPUs.
fn is_reference(adapter: &str) -> bool {
    adapter.contains("RTX 3080")
        && std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .ok()
            == Some(12)
}

/// Whether this machine can open a window at all (a Linux leg without a display cannot).
fn no_display() -> Option<&'static str> {
    if cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        return Some("no display on this Linux leg (DISPLAY/WAYLAND_DISPLAY unset)");
    }
    None
}

struct Measured {
    median_ms: f64,
    runs: Vec<f64>,
    adapter: String,
}

/// The warm-up launch and the timed launches, run alone.
fn measure(stall_ms: Option<u64>) -> Result<Measured, String> {
    let s = forge_ui::testing::run_timed_alone(|| {
        launch(stall_ms)?; // warm-up: not gated
        let mut runs = Vec::with_capacity(RUNS);
        let mut adapter = String::new();
        for _ in 0..RUNS {
            let (ms, a) = launch(stall_ms)?;
            runs.push(ms);
            adapter = a;
        }
        Ok(format!(
            "{}|{adapter}",
            runs.iter()
                .map(|r| format!("{r:.1}"))
                .collect::<Vec<_>>()
                .join(",")
        ))
    })?;
    let (runs, adapter) = s.split_once('|').ok_or("malformed timed result")?;
    let mut runs: Vec<f64> = runs
        .split(',')
        .map(|r| r.parse::<f64>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let mut sorted = runs.clone();
    sorted.sort_by(f64::total_cmp);
    let median_ms = sorted[sorted.len() / 2];
    runs.shrink_to_fit();
    Ok(Measured {
        median_ms,
        runs,
        adapter: adapter.to_string(),
    })
}

/// The gate: `Err` over budget on the reference machine; elsewhere the figure is printed
/// under `AWAITING(reference machine)`.
fn verdict(m: &Measured) -> Result<String, String> {
    let line = format!(
        "ui_startup_budget: median {:.0} ms (runs {:?} ms) cold start to first interactive frame, budget {BUDGET_MS} ms, adapter {}",
        m.median_ms, m.runs, m.adapter
    );
    if !is_reference(&m.adapter) {
        return Ok(format!(
            "{line} — AWAITING(reference machine: RTX 3080, 12 logical CPUs; not gated here)"
        ));
    }
    if m.median_ms > BUDGET_MS {
        return Err(format!("{line} — over budget"));
    }
    Ok(format!("{line} — within budget on the reference machine"))
}

#[test]
fn ui_startup_budget() {
    if let Some(why) = no_display() {
        println!("ui_startup_budget: AWAITING({why})");
        return;
    }
    let m = measure(None).unwrap_or_else(|e| panic!("{e}"));
    let v = verdict(&m).unwrap_or_else(|e| panic!("{e}"));
    println!("{v}");
}

#[test]
fn positive_control_a_2s_stall_in_startup_fails() {
    if let Some(why) = no_display() {
        println!("positive_control_a_2s_stall_in_startup_fails: AWAITING({why})");
        return;
    }
    let m = measure(Some(STALL_MS)).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        m.median_ms > BUDGET_MS,
        "a {STALL_MS} ms stall measured {:.0} ms: the measurement cannot see startup",
        m.median_ms
    );
    let v = verdict(&m);
    if is_reference(&m.adapter) {
        let e = v.expect_err("a 2 s stall passed the budget on the reference machine");
        assert!(e.contains("over budget"), "{e}");
    } else {
        println!("{}", v.unwrap_or_else(|e| e));
    }
}
