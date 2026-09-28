//! The wall-clock UI gates (§21.22, D-5: `ui_virtual_*_100k`, `ui_hierarchy_100k`,
//! `ui_graph_2k_nodes`) time their frames at the High priority class, so a concurrent
//! workspace build on the same cores cannot time-slice the one UI thread (WP-U8; the
//! scheduling fix for the flaky filtered-rename gate, W5: no budget widened).
//!
//! `timed_gates_run_at_high_priority`: after `run_ahead_of_background_work` the process
//! reads back as High (Windows; elsewhere the class is left alone and there is nothing to
//! read).
//!
//! Positive control (W2): `positive_control_a_class_that_did_not_take_is_refused` — asking
//! for a class Windows does not have leaves the process where it was, and the read-back
//! check refuses it instead of reporting success. That is the check the gates rely on.
//!
//! `timed_body_runs_alone_in_its_own_process`: `run_timed_alone` runs a gate's timed body in
//! a child process that runs only that test (`--exact --test-threads=1`), at High, so under
//! `cargo test` its sibling tests (threads of the parent) cannot share its turn. Positive
//! control (W2): `positive_control_timed_body_in_process_is_refused` — the body run in the
//! test's own process is refused. `timed_body_error_reaches_the_parent`: a failing gate's
//! message survives the trip (the gates' positive controls read it).

use std::sync::{Mutex, MutexGuard, PoisonError};

use forge_ui::testing::{priority_class, run_ahead_of_background_work, set_priority_class};

/// The tests here that read or change this process's class take turns: under `cargo test`
/// they are threads of one process, and one raising the class to High between another's
/// read and read-back would look like a refused class that changed the process.
fn class_lock() -> MutexGuard<'static, ()> {
    static CLASS: Mutex<()> = Mutex::new(());
    CLASS.lock().unwrap_or_else(PoisonError::into_inner)
}

#[test]
fn timed_gates_run_at_high_priority() {
    let _class = class_lock();
    run_ahead_of_background_work().unwrap_or_else(|e| panic!("{e}"));
    if cfg!(windows) {
        assert_eq!(priority_class().as_deref(), Some("High"));
        // Once per process: a second call is free and still Ok.
        assert_eq!(run_ahead_of_background_work(), Ok(()));
    }
}

#[test]
fn positive_control_a_class_that_did_not_take_is_refused() {
    if !cfg!(windows) {
        return;
    }
    let _class = class_lock();
    let before = priority_class();
    let e = set_priority_class("Hihg").err().unwrap_or_default();
    assert!(
        e.contains("not \"Hihg\""),
        "a priority class that did not take passed the read-back: {e:?}"
    );
    assert_eq!(
        priority_class(),
        before,
        "a refused class changed the process"
    );
}

/// Positive control (W2): a test calling `run_timed_alone` a second time panics with
/// the documented message instead of silently returning the first body's result.
#[test]
fn positive_control_reentrant_run_timed_alone_panics() {
    use std::panic;
    forge_ui::testing::run_timed_alone(|| Ok("first".into()))
        .unwrap_or_else(|e| panic!("first call failed: {e}"));
    let result = panic::catch_unwind(|| forge_ui::testing::run_timed_alone(|| Ok("second".into())));
    match result {
        Ok(_) => panic!(
            "a second run_timed_alone in the same test should panic to prevent \
             silently returning the first body's result"
        ),
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                s
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.as_str()
            } else {
                "unknown panic"
            };
            assert!(
                msg.contains("run_timed_alone called a second time"),
                "expected the documented panic message, got: {msg}"
            );
        }
    }
}

// ---- run_timed_alone: the timed body has a process to itself ----------------------------

/// Where a timed body ran: (its process id, the arguments of that process, its priority).
fn where_it_ran(fault_in_process: bool) -> Result<(u32, String), String> {
    let s = forge_ui::testing::run_timed(fault_in_process, || {
        let args: Vec<String> = std::env::args().skip(1).collect();
        Ok(format!(
            "{}|{}|{}",
            std::process::id(),
            args.join(" "),
            priority_class().unwrap_or_default()
        ))
    })?;
    let mut it = s.splitn(3, '|');
    let pid = it
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .ok_or_else(|| format!("no process id in {s:?}"))?;
    let args = it.next().unwrap_or_default().to_string();
    let class = it.next().unwrap_or_default();
    if cfg!(windows) && class != "High" {
        return Err(format!(
            "the timed body ran at the {class:?} priority class"
        ));
    }
    if pid == std::process::id() {
        return Err(format!(
            "the timed body ran in the test's own process ({pid}) beside its sibling tests"
        ));
    }
    if !(args.contains("--exact") && args.contains("--test-threads=1")) {
        return Err(format!(
            "the timed body's process runs more than its one test: {args:?}"
        ));
    }
    Ok((pid, args))
}

/// `cargo test` runs a binary's tests as threads of one process, so a timed gate's frames
/// shared the cores with its own positive controls' 100k-row builds (WP-U8 verifier: the
/// hierarchy's filtered rename read 2.7-3.0 ms p95 beside a workspace build, 0.63-0.70 ms
/// alone). `run_timed_alone` gives the body a process of its own, running only its test,
/// at the High class.
#[test]
fn timed_body_runs_alone_in_its_own_process() {
    let _class = class_lock();
    let (pid, args) = where_it_ran(false).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        args.contains("timed_body_runs_alone_in_its_own_process"),
        "{args:?}"
    );
    println!("timed body ran alone in process {pid}: {args}");
}

/// Positive control (W2): a body run in the test's own process is refused.
#[test]
fn positive_control_timed_body_in_process_is_refused() {
    let _class = class_lock();
    let e = where_it_ran(true).err().unwrap_or_default();
    assert!(
        e.contains("in the test's own process"),
        "a timed body beside its sibling tests passed: {e:?}"
    );
}

/// A timed body's `Err` (a gate that failed) comes back to the parent intact, newlines and
/// all: the positive controls of every gate read it.
#[test]
fn timed_body_error_reaches_the_parent() {
    let e = forge_ui::testing::run_timed_alone(|| Err("line one\nline \\two\\".into()))
        .err()
        .unwrap_or_default();
    assert_eq!(e, "line one\nline \\two\\");
}
