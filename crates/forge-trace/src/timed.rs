//! Wall-clock measurements **run alone** (Ch.29 budgets, §21.22 UI budgets, D-5; W5).
//!
//! Every test that asserts a time — a perf-gate row, the streaming walk's frame budget, the
//! UI gates' milliseconds, a nanosecond zone budget, a GPU pass time — runs its timed body
//! through [`run_timed_alone`]. What that buys, and why each part is needed:
//!
//! * **A process of its own.** `cargo test` runs a binary's tests as threads of one process:
//!   a gate's timed turns would share the cores with its own positive controls and siblings
//!   (process priority cannot separate threads of one process, and nextest's `wall-clock`
//!   group does not apply to `cargo test`). So the timed body re-runs its own test as a child
//!   of the test binary (`<name> --exact --test-threads=1`), which runs nothing else, prints
//!   its result and exits; the parent returns that result. Under nextest the same happens
//!   (a spawn costs milliseconds), so both runners measure the same way.
//! * **The High priority class** (Windows, read back): another lane's `cargo build` on the
//!   same cores is normal priority, so the timed thread is scheduled ahead of it — the class
//!   a game's or an editor's foreground frame effectively gets. At normal priority a
//!   0.4 ms UI turn measured 2-3 ms beside a dozen `rustc`s, and the perf gate's rows and
//!   the streaming walk's frames failed under another lane's build.
//! * **One at a time on the machine** (WP-19). Priority orders the timed process against
//!   *background* work, not against another timed process — and the GPU is scheduled per
//!   device, not by process priority, so two timed bodies measuring at once (the perf gate
//!   in one lane, the streaming walk or a UI gate in the other; a full-workspace `cargo
//!   test` in both) inflate each other's GPU pass times and frames (WP-17 saw the walk at
//!   43.9 ms and the atmosphere precompute 2 % over its row that way). Every timed body
//!   therefore holds a machine-wide lock ([`LOCK_FILE`] in the temp directory, an OS file
//!   lock released if the process dies) while it runs: timed bodies of every test binary,
//!   of every lane and of both runners take their turns.
//!
//! This is scheduling, not a tolerance: no budget changes and nothing is retried (W5).
//! Elsewhere than Windows the class is left alone (a negative nice needs root) and `Ok` is
//! returned; the file lock and the child process apply everywhere.
//!
//! * **No untimed GPU work beside it either** (WP-19 verification): a golden render or a UI
//!   test that times nothing still shares the GPU, and it inflated the perf gate's
//!   atmosphere row to 72 and 100 ms beside another lane's `cargo nextest`. So the lock is a
//!   **reader-writer lock**: every GPU-using test process holds a *shared* [`gpu_turn`]
//!   while it has a device (`forge_gpu::AdapterPool` takes one when built with its
//!   `test-gpu-turn` feature, which every crate with GPU tests enables as a dev-dependency;
//!   never in a shipped build), and a timed body holds it *exclusively*. A second file, the
//!   turnstile ([`TURN_FILE`]), gives the timed body precedence: it takes the turnstile
//!   first, so new GPU turns queue behind it while the ones already running finish, and it
//!   is not starved by a stream of overlapping tests. Turns are counted per process (a test
//!   that makes a second pool while holding one does not queue behind a waiting timed body,
//!   which would deadlock), and a thread that holds a turn cannot take the exclusive lock
//!   (it would wait for itself): that is an error, not a hang. The timed child process
//!   itself (and anything it launches) takes no turn — its parent holds the lock for it.
//!
//! What it does not cover, said plainly: a process built without the turn (another program
//! on the desktop, or a lane whose branch predates this) still shares the GPU. The GPU rows
//! are therefore measured with **timestamps** around their own work, never the wall clock,
//! so GPU work scheduled between them is not counted either (`test_perf_gate`).
//!
//! * **A quiet machine** (WP-40, [`quiet`]): neither the lock nor the priority class sees
//!   another worktree's compiler, and a dozen `rustc`s on the same cores still inflated the
//!   streaming walk, the perf gate and the UI gates. So once the parent holds the lock
//!   exclusively it waits — bounded, sampled every few seconds — until the share of all cores
//!   busy outside this process is at most [`quiet::QUIET_THRESHOLD`]; if the bound expires
//!   the body runs anyway and the report says it was **measured under load** (printed, logged
//!   in the temp directory, and appended to a failure). The wait is one line per timed body.
//!
//! Every file of the workspace's tests that reads the wall clock either times through this
//! helper (and its test binary is in nextest's serial `wall-clock` group) or declares why it
//! is not a gate: `cargo xtask timed-gates` checks both (WP-40).

pub mod quiet;

use std::io::Write;

pub use quiet::{QuietFaults, QuietPolicy, QuietReport};
use std::sync::atomic::{AtomicU32, Ordering};

thread_local! {
    #[allow(unused_doc_comments)]
    static CALLED: AtomicU32 = const { AtomicU32::new(0) };
}

/// Guards that `run_timed` / `run_timed_alone` is called at most once per test thread.
/// If the counter is already positive on enter, the test is calling it a second time → panic
/// with the documented message. The counter is monotonic (never decrements) so that even
/// after the first call's guard drops, a second call from the same test is caught.
///
/// Private: only [`run_timed_alone`] constructs one, on entry, and lets it drop at the end
/// of the call. Nothing outside this module needs to name the type.
struct TimedReentrancyGuard;

impl TimedReentrancyGuard {
    fn new(test: &str) -> Self {
        CALLED.with(|c| {
            let prev = c.fetch_add(1, Ordering::SeqCst);
            if prev != 0 {
                panic!(
                    "run_timed_alone called a second time from the \"{test}\" test; \
                     only one timed body may run per test"
                );
            }
            prev
        });
        TimedReentrancyGuard
    }
}

impl Drop for TimedReentrancyGuard {
    fn drop(&mut self) {
        // intentionally a no-op: the counter is monotonic so the second-call
        // guard in the same test is triggered by the first call having set it
    }
}

/// The environment variable naming the one test a timed child process was launched for.
pub const TIMED_ALONE_ENV: &str = "FORGE_TIMED_ALONE";

/// The machine-wide lock every timed body holds exclusively while it runs, and every
/// GPU-using test process holds shared ([`gpu_turn`]) while it has a device (in
/// `std::env::temp_dir()`).
pub const LOCK_FILE: &str = "forge-timed-alone.lock";

/// The turnstile in front of [`LOCK_FILE`]: a timed body holds it while it waits for and
/// holds the lock, a GPU turn passes through it only to take its shared hold — so GPU turns
/// that arrive after a waiting timed body queue behind it.
pub const TURN_FILE: &str = "forge-timed-alone.turn";

/// The line prefixes a timed child prints its result under (newlines escaped).
const TIMED_MARK_OK: &str = "forge-timed-alone:ok:";
const TIMED_MARK_ERR: &str = "forge-timed-alone:err:";

/// Run this process ahead of normal-priority background work: the High priority class on
/// Windows, read back (`Err` says why it could not be set). Once per process.
pub fn run_ahead_of_background_work() -> Result<(), String> {
    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| set_priority_class("High")).clone()
}

/// Run a wall-clock test's timed body **alone** (see the module docs): in a child process
/// that runs only this test, at the High priority class, holding the machine-wide timed
/// lock. Returns the body's result as the child reported it; a child that panicked (an
/// `assert!` in the body) comes back as `Err` with the tail of its output.
///
/// Call it from the test's own thread (libtest names the thread after the test) before any
/// setup the timed body needs: everything that builds the measured state goes in `body`,
/// which runs only in the child.
pub fn run_timed_alone(body: impl FnOnce() -> Result<String, String>) -> Result<String, String> {
    run_timed_alone_with(TimedOptions::default(), body)
}

/// How a timed body waits for a quiet machine ([`run_timed_alone_with`]).
#[derive(Clone, Debug, Default)]
pub struct TimedOptions {
    /// The wait's policy; `None` is [`QuietPolicy::for_timed_body`] (the default bound, the
    /// `FORGE_TIMED_QUIET_MAX_S` override, shortened after a recent expiry).
    pub quiet: Option<QuietPolicy>,
    /// W2 fault switches of the wait (positive controls only).
    pub quiet_faults: QuietFaults,
}

/// [`run_timed_alone`] with the quiet wait's policy and fault switches given (the positive
/// controls of `test_timed_quiet`: a faked busy machine and a short bound).
pub fn run_timed_alone_with(
    opts: TimedOptions,
    body: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    let thread = std::thread::current();
    let Some(test) = thread.name().filter(|n| *n != "main") else {
        return Err("a timed test must run on its libtest thread (named after the test)".into());
    };
    let _guard = TimedReentrancyGuard::new(test);
    run_timed_inner(false, &opts, body)
}

/// [`run_timed_alone`], or with `fault_in_process` the body runs in this process beside
/// whatever else it runs, at High priority but without a process or the machine lock of
/// its own, and without waiting for a quiet machine (the positive control of `forge-ui`'s
/// `test_ui_timed_priority`).
pub fn run_timed(
    fault_in_process: bool,
    body: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    run_timed_inner(fault_in_process, &TimedOptions::default(), body)
}

fn run_timed_inner(
    fault_in_process: bool,
    opts: &TimedOptions,
    body: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    let thread = std::thread::current();
    let Some(test) = thread.name().filter(|n| *n != "main") else {
        return Err("a timed test must run on its libtest thread (named after the test)".into());
    };
    if fault_in_process {
        run_ahead_of_background_work()?;
        return body();
    }
    if std::env::var(TIMED_ALONE_ENV).is_ok_and(|t| t == test) {
        // The child: nothing else runs in this process. Report and exit, so nothing after
        // the timed body (in this copy of the test) runs; the parent checks the result.
        let r = run_ahead_of_background_work().and_then(|()| body());
        let line = match &r {
            Ok(s) => format!("{TIMED_MARK_OK}{}", escape_line(s)),
            Err(e) => format!("{TIMED_MARK_ERR}{}", escape_line(e)),
        };
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "\n{line}");
        let _ = out.flush();
        std::process::exit(0);
    }
    // The parent: one timed child at a time from this process, and one timed body at a time
    // on the machine.
    static ONE_CHILD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE_CHILD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _machine = machine_lock()?;
    // Inside the exclusive lock: wait (bounded) until nothing else loads the cores.
    let policy = opts
        .quiet
        .clone()
        .unwrap_or_else(QuietPolicy::for_timed_body);
    let quiet = quiet::wait_for_quiet_reported(test, &policy, opts.quiet_faults);
    let under_load = |r: Result<String, String>| match r {
        Err(e) if quiet.under_load() => Err(format!("{e}\n[{}]", quiet.line(test))),
        r => r,
    };
    let exe = std::env::current_exe().map_err(|e| format!("the test binary's path: {e}"))?;
    let out = std::process::Command::new(exe)
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(TIMED_ALONE_ENV, test)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not start {test} in a process of its own: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    // What the body printed reaches this test's output (libtest captures it like any print):
    // the child's lines less libtest's own header and the result line.
    for l in stdout.lines() {
        let libtest =
            l.is_empty() || l.starts_with("running ") || l.starts_with("forge-timed-alone:");
        if !libtest {
            println!("{l}");
        }
    }
    for l in stdout.lines().rev() {
        if let Some(s) = l.strip_prefix(TIMED_MARK_OK) {
            return Ok(unescape_line(s));
        }
        if let Some(e) = l.strip_prefix(TIMED_MARK_ERR) {
            return under_load(Err(unescape_line(e)));
        }
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    under_load(Err(format!(
        "{test} run alone reported no result ({}): {}{}",
        out.status,
        tail(&stdout),
        tail(&stderr)
    )))
}

/// Whether this process is a timed child (running one test's timed body alone).
#[must_use]
pub fn is_timed_child() -> bool {
    std::env::var_os(TIMED_ALONE_ENV).is_some()
}

fn open_lock_file(name: &str) -> Result<std::fs::File, String> {
    let path = std::env::temp_dir().join(name);
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("the timed lock {}: {e}", path.display()))
}

/// The machine-wide timed lock, held exclusively (see [`machine_lock`]). Released on drop,
/// or when the process ends however it ends (the OS's file locks).
#[derive(Debug)]
pub struct TimedLock {
    _main: std::fs::File,
    _turn: std::fs::File,
}

/// Take the machine-wide timed lock **exclusively**: blocks while another process's timed
/// body runs, and while any GPU turn ([`gpu_turn`]) is held anywhere on the machine; GPU
/// turns asked for meanwhile wait behind it (the turnstile). `Err` if this thread holds a
/// GPU turn itself (it would wait for itself forever).
pub fn machine_lock() -> Result<TimedLock, String> {
    let me = std::thread::current().id();
    if lock_turns()
        .by_thread
        .iter()
        .any(|(t, n)| *t == me && *n > 0)
    {
        return Err(
            "this thread holds a GPU turn (a live AdapterPool): a timed body cannot run alone \
             while its own test holds the GPU — create devices inside the timed body"
                .into(),
        );
    }
    let turn = open_lock_file(TURN_FILE)?;
    turn.lock()
        .map_err(|e| format!("could not take the timed turnstile: {e}"))?;
    let main = open_lock_file(LOCK_FILE)?;
    main.lock()
        .map_err(|e| format!("could not take the timed lock: {e}"))?;
    Ok(TimedLock {
        _main: main,
        _turn: turn,
    })
}

/// This process's GPU turns: one shared hold of [`LOCK_FILE`] while any is alive, and the
/// count per thread (so [`machine_lock`] refuses a thread that holds one).
struct Turns {
    held: usize,
    shared: Option<std::fs::File>,
    by_thread: Vec<(std::thread::ThreadId, usize)>,
}

static TURNS: std::sync::Mutex<Turns> = std::sync::Mutex::new(Turns {
    held: 0,
    shared: None,
    by_thread: Vec::new(),
});

fn lock_turns() -> std::sync::MutexGuard<'static, Turns> {
    TURNS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A GPU turn: while it lives, no timed body runs anywhere on the machine (see the module
/// docs). Dropping the last one of a process releases the process's shared hold.
#[derive(Debug)]
pub struct GpuTurn {
    /// `None` in a timed child: its parent holds the lock exclusively for it.
    thread: Option<std::thread::ThreadId>,
}

/// Take a GPU turn for this process (shared; see the module docs). Blocks while a timed body
/// runs or waits for its turn. In a timed child process (and anything it launches) it takes
/// nothing: the child's parent holds the lock exclusively on its behalf.
pub fn gpu_turn() -> Result<GpuTurn, String> {
    if is_timed_child() {
        return Ok(GpuTurn { thread: None });
    }
    let me = std::thread::current().id();
    let mut t = lock_turns();
    if t.held == 0 {
        // Through the turnstile (queued behind a waiting timed body), then the shared hold;
        // the turnstile is let go at once so other processes' turns pass too.
        let turn = open_lock_file(TURN_FILE)?;
        turn.lock()
            .map_err(|e| format!("could not pass the timed turnstile: {e}"))?;
        let main = open_lock_file(LOCK_FILE)?;
        main.lock_shared()
            .map_err(|e| format!("could not take a shared GPU turn: {e}"))?;
        drop(turn);
        t.shared = Some(main);
    }
    t.held += 1;
    match t.by_thread.iter_mut().find(|(id, _)| *id == me) {
        Some((_, n)) => *n += 1,
        None => t.by_thread.push((me, 1)),
    }
    Ok(GpuTurn { thread: Some(me) })
}

impl Drop for GpuTurn {
    fn drop(&mut self) {
        let Some(me) = self.thread else {
            return;
        };
        let mut t = lock_turns();
        if let Some(i) = t.by_thread.iter().position(|(id, _)| *id == me) {
            t.by_thread[i].1 -= 1;
            if t.by_thread[i].1 == 0 {
                t.by_thread.swap_remove(i);
            }
        }
        t.held = t.held.saturating_sub(1);
        if t.held == 0 {
            t.shared = None;
        }
    }
}

/// Whether this process holds a GPU turn right now.
#[must_use]
pub fn holds_gpu_turn() -> bool {
    lock_turns().held > 0
}

/// A control-only GPU turn that tracks the thread (so [`machine_lock`] refuses this thread)
/// but acquires **no** shared file lock — used by the positive control
/// `positive_control_without_a_real_lock_timed_body_grants` to show that without an actual
/// shared hold the timed lock is granted.
///
/// **doc(hidden)** — test-only, following the `*_for_control` naming convention.
///
/// A control turn must only be used after dropping real [`gpu_turn`] handles (so no shared
/// lock is held) — the test guarantees the `held == 0` / `shared == None` precondition
/// before calling [`turn_for_control`].
#[doc(hidden)]
#[derive(Debug)]
pub struct ControlTurn {
    thread: std::thread::ThreadId,
}

/// Return a control-only GPU turn (see [`ControlTurn`]).
///
/// **doc(hidden)** — test-only. Panics if this process still holds a real [`gpu_turn`].
#[doc(hidden)]
pub fn turn_for_control() -> ControlTurn {
    let me = std::thread::current().id();
    let mut t = lock_turns();
    assert!(
        t.held == 0 && t.shared.is_none(),
        "turn_for_control requires all real gpu_turn handles to be dropped first"
    );
    t.by_thread.push((me, 1));
    drop(t);
    ControlTurn { thread: me }
}

/// Whether this process holds a control (-only) GPU turn.
#[must_use]
pub fn holds_turn_for_control() -> bool {
    lock_turns()
        .by_thread
        .iter()
        .any(|(id, n)| id == &std::thread::current().id() && *n > 0)
}

impl Drop for ControlTurn {
    fn drop(&mut self) {
        let me = self.thread;
        let mut t = lock_turns();
        if let Some(i) = t.by_thread.iter().position(|(id, _)| *id == me) {
            t.by_thread[i].1 = t.by_thread[i].1.saturating_sub(1);
            if t.by_thread[i].1 == 0 {
                t.by_thread.swap_remove(i);
            }
        }
    }
}

fn escape_line(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn unescape_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

/// Set this process's priority class (Windows class names: `Normal`, `High`, ...) and read
/// it back. `Err` when it could not be set or reads back as anything else.
#[cfg(windows)]
pub fn set_priority_class(class: &str) -> Result<(), String> {
    let pid = std::process::id();
    let out = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$p = Get-Process -Id {pid}; $p.PriorityClass = '{class}'; \
                 (Get-Process -Id {pid}).PriorityClass"
            ),
        ])
        .output()
        .map_err(|e| format!("could not run powershell to set the priority class: {e}"))?;
    let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && got == class {
        Ok(())
    } else {
        Err(format!(
            "the process priority class is {got:?}, not {class:?} ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// See the Windows version: elsewhere the class is left alone.
#[cfg(not(windows))]
pub fn set_priority_class(_class: &str) -> Result<(), String> {
    Ok(())
}

/// This process's priority class as Windows names it (`None` elsewhere or if unreadable).
#[must_use]
pub fn priority_class() -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let pid = std::process::id();
    let out = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-Process -Id {pid}).PriorityClass"),
        ])
        .output()
        .ok()?;
    let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !got.is_empty()).then_some(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_round_trips_through_the_escape() {
        for s in [
            "",
            "a",
            "line one\nline \\two\\",
            "a\r\nb\\",
            "\\n literally",
        ] {
            assert_eq!(unescape_line(&escape_line(s)), s);
            assert!(!escape_line(s).contains('\n'));
        }
    }

    #[test]
    fn the_machine_lock_is_exclusive_across_handles() {
        let held = machine_lock().expect("lock");
        let path = std::env::temp_dir().join(LOCK_FILE);
        let other = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open");
        assert!(
            matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
            "a second handle took the timed lock while it was held"
        );
        drop(held);
        // Released: this handle can take it (blocking: another process may be timing).
        other.lock().expect("free once released");
    }
}
