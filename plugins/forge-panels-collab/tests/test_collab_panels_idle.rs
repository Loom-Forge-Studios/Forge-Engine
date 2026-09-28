//! `test_collab_panels_idle` (D-5, §21.11): an editor on a team, with **every
//! collaboration panel open** (and the hierarchy, inspector and viewport that draw presence),
//! teammates online and idle, **draws nothing and wakes never**. Presence, claims and the
//! queue reach the panels through live feeds, not polling.
//!
//! Positive control (W2): `positive_control_panels_that_poll_never_sleep` makes the panels
//! ask for a loop turn on every sync instead of following their feeds; the idle check must
//! fail.

mod common;

use std::time::Duration;

use common::*;
use forge_project::collab::{CollabView, Presence, Role};

const ALL: &[&str] = &[
    "forge.team",
    "forge.sandbox",
    "forge.presence",
    "forge.ownership",
    "forge.publish_queue",
    "forge.conflicts",
    "forge.licence",
    "forge.hierarchy",
    "forge.inspector",
    "forge.viewport",
];

/// `Ok` when the editor, settled, sleeps: no turn wanted and nothing drawn or woken for ten
/// seconds.
fn idle(poll: bool) -> Result<(), String> {
    let s = server();
    let mut rig = rig_with(&s, ALL, |cfg| cfg.services.collab.faults.poll = poll);
    rig.shell
        .emitter()
        .emit(forge_editor::collab::create_command("Studio"));
    rig.settle();
    let _bob = join(&s, "bob", Role::Developer);
    s.server.set_presence(Presence {
        user: "bob".into(),
        selection: vec![0],
        panel: Some("forge.viewport".into()),
        at_ms: NOW,
    });
    rig.settle();
    if !poll {
        // Let the feeds deliver the teammate once (a polling editor never gets here: it
        // would turn forever).
        feed(&mut rig);
        rig.advance(Duration::from_secs(2));
    }
    if let Some(d) = rig.shell.next_deadline() {
        return Err(format!("the loop wants another turn at {d:?}"));
    }
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    if (frames, wakeups) == (0, 0) {
        Ok(())
    } else {
        Err(format!("{frames} frames, {wakeups} wakeups while idle"))
    }
}

#[test]
fn an_idle_team_editor_draws_and_wakes_nothing() {
    idle(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_panels_that_poll_never_sleep() {
    let e = idle(true).err().unwrap_or_default();
    assert!(
        e.contains("wants another turn"),
        "the polling control must fail: {e}"
    );
}
