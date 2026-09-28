//! `test_connect_panels_idle` (D-5, Ch.21 §21.11 / §21.22): with all four connect panels
//! showing — over a core another client edits (an automation session through the bus) — an
//! idle editor draws **0 frames, wakes 0 times and runs 0 sync passes** over 10 s. The other
//! client's edit wakes it, it redraws, and it is idle again.
//!
//! Positive control (W2): `positive_control_a_panel_polling_its_backend_fails` — the audit
//! panel polls its sources every loop turn instead of following their live feeds; the idle
//! check must fail (sync passes grow while nothing happens).

mod common;

use std::time::Duration;

use forge_cmd::EditorCommand;
use forge_editor::client::BusClient;

use common::feed;

const ALL: [&str; 4] = [
    "forge.plugins",
    "forge.audit_log",
    "forge.remote",
    "forge.compute",
];

fn spawn(c: &mut forge_editor::core::LocalBus, name: &str) -> Result<(), String> {
    c.apply(
        EditorCommand::Spawn {
            name: name.into(),
            parent: None,
        },
        None,
    );
    match c.pump().refused.first() {
        Some(r) => Err(format!("{name}: {}", r.rejection)),
        None => Ok(()),
    }
}

fn check(poll: bool) -> Result<(u64, u64, u64), String> {
    let mut rig = common::rig_with(&ALL, None, |cfg, _| {
        cfg.services.connect.faults.poll_backends = poll;
    });
    let mut other = common::automation(&rig, "auto-1");
    spawn(&mut other, "Seed")?;
    feed(&mut rig);
    // Past the shell's debounced layout save.
    rig.advance(Duration::from_secs(6));
    let p0 = rig.shell.stats().sync_passes;
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    let passes = rig.shell.stats().sync_passes - p0;
    if (frames, wakeups, passes) != (0, 0, 0) {
        return Err(format!(
            "idle with the connect panels open: {frames} frames, {wakeups} wakeups, {passes} sync passes in 10 s"
        ));
    }
    // The other client's edit wakes the editor; then it is idle again.
    spawn(&mut other, "Sprout")?;
    let (frames, _) = rig.advance(Duration::from_secs(2));
    if frames == 0 {
        return Err("the other client's edit did not reach the editor".into());
    }
    rig.advance(Duration::from_secs(6));
    let (f2, w2) = rig.advance(Duration::from_secs(10));
    if (f2, w2) != (0, 0) {
        return Err(format!("not idle again: {f2} frames, {w2} wakeups"));
    }
    Ok((frames, wakeups, passes))
}

#[test]
fn idle_connect_panels_cost_nothing() {
    check(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_panel_polling_its_backend_fails() {
    let e = check(true).expect_err("a polling panel must fail the idle check");
    assert!(e.contains("sync passes"), "{e}");
}
