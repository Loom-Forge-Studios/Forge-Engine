//! `forge_trace::timed::run_timed_alone` — the one helper every wall-clock test of the
//! workspace times through (WP-19): the timed body runs in a child process running only its
//! test, at the High priority class, **holding the machine-wide timed lock**, so no two timed
//! bodies (of any test binary, lane or runner) measure at once and GPU pass times and frame
//! times are not inflated by another timed test.
//!
//! `a_timed_body_holds_the_machine_lock`: inside the body, another handle cannot take the
//! lock. `a_timed_body_runs_alone_at_high_priority`: the body ran in another process than
//! the test's, whose arguments run only this test, at High (Windows). Positive control (W2):
//! `positive_control_an_in_process_body_is_seen`: the in-process path (no child, no lock)
//! is reported as running in the test's own process — the check above is not blind.
//! (`forge-ui`'s `test_ui_timed_priority` checks the same helper through its re-export.)

use std::fs::{OpenOptions, TryLockError};

use forge_trace::timed::{LOCK_FILE, priority_class, run_timed, run_timed_alone};

fn lock_state() -> Result<String, String> {
    let path = std::env::temp_dir().join(LOCK_FILE);
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(match f.try_lock() {
        Err(TryLockError::WouldBlock) => "held".into(),
        Ok(()) => "free".into(),
        Err(TryLockError::Error(e)) => format!("error {e}"),
    })
}

#[test]
fn a_timed_body_holds_the_machine_lock() {
    let s = run_timed_alone(lock_state).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        s, "held",
        "the timed lock was free while a timed body ran: another timed test could measure \
         beside it"
    );
}

/// (process id, arguments, priority class) of where a body ran.
fn where_it_ran(fault_in_process: bool) -> Result<(u32, String, String), String> {
    let s = run_timed(fault_in_process, || {
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
    Ok((
        pid,
        it.next().unwrap_or_default().to_string(),
        it.next().unwrap_or_default().to_string(),
    ))
}

#[test]
fn a_timed_body_runs_alone_at_high_priority() {
    let (pid, args, class) = where_it_ran(false).unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(pid, std::process::id(), "ran in the test's own process");
    assert!(
        args.contains("a_timed_body_runs_alone_at_high_priority")
            && args.contains("--exact")
            && args.contains("--test-threads=1"),
        "the child runs more than this one test: {args:?}"
    );
    if cfg!(windows) {
        assert_eq!(class, "High");
    }
}

#[test]
fn positive_control_an_in_process_body_is_seen() {
    let (pid, _, _) = where_it_ran(true).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        pid,
        std::process::id(),
        "the in-process path must run here: the process check above would be blind"
    );
}
