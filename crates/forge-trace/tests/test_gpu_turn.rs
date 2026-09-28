//! `forge_trace::timed::gpu_turn` — the shared side of the timed lock (WP-19 verification):
//! GPU work of a test that times nothing (a golden render, a UI test) must not run while a
//! timed body measures, so every GPU-using test process holds a shared turn while it has a
//! device (`forge_gpu::AdapterPool` with `test-gpu-turn`), and a timed body holds the lock
//! exclusively.
//!
//! * `a_gpu_turn_holds_the_lock_shared`: while a turn lives, another handle cannot take the
//!   lock exclusively but can share it (other GPU tests run side by side).
//! * `a_timed_body_waits_for_gpu_turns_and_new_turns_wait_for_it`: the exclusive lock is not
//!   granted while a turn lives, and is granted once it is dropped; a turn asked for while a
//!   timed body holds the lock is not granted until the timed body lets go (the turnstile).
//! * `a_thread_holding_a_turn_cannot_take_the_timed_lock`: an error, not a self-deadlock.
//! * Positive control (W2) `positive_control_without_a_turn_the_timed_lock_is_granted`: with
//!   the process's last turn dropped, the lock is taken exclusively from another handle, so
//!   the exclusion the checks above observe is the turn's doing, not a lock that is never
//!   granted.
//!
//! The tests share this process's turn count, so they run one at a time (a file mutex).

use std::fs::{OpenOptions, TryLockError};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use forge_trace::timed::{LOCK_FILE, gpu_turn, holds_gpu_turn, machine_lock};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn other_handle() -> std::fs::File {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(std::env::temp_dir().join(LOCK_FILE))
        .expect("open the lock file")
}

#[test]
fn a_gpu_turn_holds_the_lock_shared() {
    let _s = serial();
    let turn = gpu_turn().expect("turn");
    assert!(holds_gpu_turn());
    let other = other_handle();
    assert!(
        matches!(other.try_lock(), Err(TryLockError::WouldBlock)),
        "the lock was free for exclusive use while a GPU turn was held"
    );
    other
        .try_lock_shared()
        .expect("GPU turns share the lock with each other");
    other.unlock().expect("unlock");
    drop(turn);
    assert!(!holds_gpu_turn());
}

#[test]
fn a_timed_body_waits_for_gpu_turns_and_new_turns_wait_for_it() {
    let _s = serial();
    let turn = gpu_turn().expect("turn");
    // A timed body asks for the lock while the turn lives: not granted.
    let (tx, rx) = mpsc::channel();
    let timed = std::thread::spawn(move || {
        let lock = machine_lock().expect("timed lock");
        tx.send(()).expect("send");
        lock
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "a timed body took the lock while a GPU turn was held"
    );
    drop(turn);
    rx.recv_timeout(Duration::from_secs(60))
        .expect("the timed body gets the lock once the turn is dropped");
    let lock = timed.join().expect("timed thread");
    // A GPU turn asked for while the timed body holds the lock: not granted until it lets go.
    let (tx, rx) = mpsc::channel();
    let gpu = std::thread::spawn(move || {
        let t = gpu_turn().expect("turn");
        tx.send(()).expect("send");
        drop(t);
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "a GPU turn was granted while a timed body held the lock"
    );
    drop(lock);
    rx.recv_timeout(Duration::from_secs(60))
        .expect("the GPU turn is granted once the timed body lets go");
    gpu.join().expect("gpu thread");
}

#[test]
fn a_thread_holding_a_turn_cannot_take_the_timed_lock() {
    let _s = serial();
    let turn = gpu_turn().expect("turn");
    let e = machine_lock().expect_err("a thread holding a GPU turn must not wait for itself");
    assert!(e.contains("GPU turn"), "{e}");
    drop(turn);
}

#[test]
fn positive_control_without_a_turn_the_timed_lock_is_granted() {
    let _s = serial();
    drop(gpu_turn().expect("turn"));
    assert!(!holds_gpu_turn());
    // Another process's timed body or GPU test may hold it at this moment: wait for it, as a
    // timed body would. The point is that this process's dropped turn does not keep it held.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let other = other_handle();
        other.lock().expect("lock");
        tx.send(()).expect("send");
    });
    rx.recv_timeout(Duration::from_secs(120))
        .expect("the lock is free for exclusive use once the process's turns are dropped");
}
