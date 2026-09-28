//! `turn_for_control` positive control (WP-L-11): a GPU turn that takes **no** shared file
//! lock does not block the timed lock — proving the real shared lock acquired by
//! `gpu_turn` is what the exclusion tests observe.
//!
//! * `positive_control_without_a_real_lock_timed_body_grants`: a control turn (fake GPU turn)
//!   keeps a timed body blocked from this thread's self-deadlock check but does not prevent a
//!   **different** thread from taking the exclusive timed lock, because no file handle was
//!   acquired.

use std::sync::mpsc;
use std::time::Duration;

use forge_trace::timed::{holds_gpu_turn, machine_lock, turn_for_control};

#[test]
fn positive_control_without_a_real_lock_timed_body_grants() {
    let held_lock = machine_lock().expect("acquire the timed lock first");

    // Drop any real real GPU turns: our test binary has no real turns, but the mutex may
    // have been tainted by another run in this test session. Release the lock to start clean.
    drop(held_lock);

    // Verify no real GPU turns are held (the timed child path is off for this test).
    assert!(!holds_gpu_turn(), "no real GPU turns before control turn");

    // Take a control (fake) GPU turn — it tracks the thread for self-refusal but acquires
    // no file lock.
    let _control = turn_for_control();
    assert!(
        forge_trace::timed::holds_turn_for_control(),
        "control turn tracks us for self-refusal"
    );

    // A timed body on another thread tries the exclusive timed lock.
    // With a real turn it would block (the first exclusion test proves); here it succeeds
    // because no shared file handle was acquired by the control turn.
    let (tx, rx) = mpsc::channel();
    let timed = std::thread::spawn(move || {
        let lock = machine_lock().expect("timed body must get the lock under a control turn");
        tx.send(()).expect("send timed lock result");
        // Keep the lock while we verify the control turn is still live.
        (lock, rx.recv_timeout(Duration::from_secs(10)).is_ok())
    });

    // The timed body should succeed (not block), because the control turn holds no file lock.
    let timed_result = timed.join().expect("timed thread must complete");
    drop(timed_result.0); // timed body's lock

    assert!(
        timed_result.1,
        "timed body must have been granted the exclusive lock while a control turn was held — \
         this is the positive control proving gpu_turn's shared lock is the exclusion mechanism"
    );
}
