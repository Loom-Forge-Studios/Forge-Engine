//! `test_sandbox_row_follows_its_txn` (Ch.37 I19; WP-U14, owner rule 1): **the sandbox row
//! teammates read waits only for the transaction that changed it.**
//!
//! A gesture's frames do not each send the sandbox row (`test_sandbox_deltas`); the row goes
//! once the gesture is over. "Over" must mean *that* gesture's transaction closed — not that
//! no transaction is open anywhere on the bus. An automation session's preview transaction waits
//! for a person's review for as long as it takes; while it waits, a drag the person commits must
//! still reach the teammates' view of this sandbox, or they read a stale summary for the whole
//! review.
//!
//! The check: an automation opens a transaction and leaves it open. The person drags a lamp: the
//! first frame writes `x` (a new transaction: one row), later frames write `x` and `y` (the
//! summary moves to two properties, held while the drag is open), then the drag commits.
//! The server's row for this sandbox must say two properties at once, with the session's
//! transaction still open; and the session's later frames inside its own open transaction send
//! no row.
//!
//! Positive control (W2): `positive_control_a_row_waiting_for_every_txn_fails` — a row that
//! waits until no transaction is open anywhere (`CollabFaults::row_waits_for_every_txn`, the
//! defect as found) leaves the committed drag unseen, and the check fails.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_cmd::{EditorCommand, EntityKey, Issuer, TxnState, Value};
use forge_editor::client::BusClient;
use forge_editor::collab::licence::MemoryEntitlement;
use forge_editor::collab::{self as c, CollabFaults};
use forge_editor::core::{CollabAttach, EditorCore, LocalBus, SharedCore};
use forge_project::collab::{CollabView, MemoryCollab, MemoryIdentity};

const NOW: u64 = 20_000 * 86_400_000;

struct Editor {
    core: SharedCore,
    ui: LocalBus,
    server: Arc<MemoryCollab>,
}

fn editor(faults: CollabFaults) -> Editor {
    static N: AtomicU64 = AtomicU64::new(0);
    let identity = Arc::new(MemoryIdentity::new());
    let name = format!(
        "sandbox-row-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let store = forge_project::memory::open_named(&name, "forge-server");
    let server = Arc::new(MemoryCollab::new(store, identity.clone()));
    let licence = Arc::new(MemoryEntitlement::team_for_a_year(NOW));
    licence.set_now(Some(NOW));
    let core = EditorCore::new();
    EditorCore::attach_collab(
        &core,
        CollabAttach {
            identity,
            server: server.clone(),
            licence,
            owner: "ada".into(),
            email: Some("ada@studio.example".into()),
        },
    );
    EditorCore::set_collab_faults(&core, faults);
    let ui = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    Editor { core, ui, server }
}

fn refused(ui: &mut LocalBus, what: &str) -> Result<(), String> {
    match ui.pump().refused.first() {
        Some(r) => Err(format!("{what} refused: {}", r.rejection.error)),
        None => Ok(()),
    }
}

fn set(entity: EntityKey, path: &str, v: f64) -> EditorCommand {
    EditorCommand::SetProperty {
        entity,
        path: path.into(),
        value: Value::Float(v),
    }
}

/// The summary teammates read for ada's sandbox.
fn row_summary(e: &Editor) -> Vec<String> {
    e.server
        .sandboxes("ada")
        .into_iter()
        .find(|r| r.owner == "ada")
        .map(|r| r.summary)
        .unwrap_or_default()
}

fn rows_sent(e: &Editor) -> Result<u64, String> {
    EditorCore::collab_counters(&e.core)
        .map(|c| c.rows_sent)
        .ok_or_else(|| "not attached".to_string())
}

fn check(faults: CollabFaults) -> Result<(), String> {
    let mut e = editor(faults);
    let lamp: EntityKey = EditorCore::read(&e.core, |p| p.next_key());
    e.ui.apply(
        EditorCommand::Spawn {
            name: "Lamp".into(),
            parent: None,
        },
        None,
    );
    refused(&mut e.ui, "spawn")?;
    let crate_key: EntityKey = EditorCore::read(&e.core, |p| p.next_key());
    e.ui.apply(
        EditorCommand::Spawn {
            name: "Crate".into(),
            parent: None,
        },
        None,
    );
    refused(&mut e.ui, "spawn")?;
    e.ui.apply(c::create_command("Studio"), None);
    refused(&mut e.ui, "create")?;

    // An automation session's preview transaction, left open (awaiting the person's review).
    let mut auto = EditorCore::connect(
        &e.core,
        Issuer::Automation {
            session: "auto-1".into(),
            tool: "apply".into(),
        },
    );
    let plan = auto.begin("Automation session plan");
    auto.apply(set(crate_key, "mass", 5.0), Some(plan));
    refused(&mut auto, "the session's edit")?;
    e.ui.pump();

    // The person's drag: frame 1 writes x (a new transaction, one row); later frames write
    // x and y (the summary moves to two properties while the drag is open).
    let drag = e.ui.begin("Drag");
    e.ui.apply(set(lamp, "x", 0.0), Some(drag));
    refused(&mut e.ui, "frame 0")?;
    let at_first_frame = rows_sent(&e)?;
    for f in 1..30u32 {
        e.ui.apply(set(lamp, "x", f64::from(f)), Some(drag));
        e.ui.apply(set(lamp, "y", f64::from(f)), Some(drag));
        refused(&mut e.ui, "a frame")?;
    }
    if rows_sent(&e)? != at_first_frame {
        return Err("the drag's frames sent sandbox rows while it was open".into());
    }
    e.ui.commit(drag);
    refused(&mut e.ui, "commit")?;
    e.ui.pump();
    if e.ui.txn_state(plan) != Some(TxnState::Open) {
        return Err("the session's transaction is no longer open".into());
    }
    let s = row_summary(&e);
    if !s
        .iter()
        .any(|l| l.starts_with("human:ada") && l.contains("2 properties on 1 entity"))
    {
        return Err(format!(
            "with the session's transaction still open, teammates read a stale summary after the drag committed: {s:?}"
        ));
    }
    // The session's own frames, inside its open transaction, send no row.
    let before = rows_sent(&e)?;
    for f in 0..20u32 {
        auto.apply(set(crate_key, "friction", f64::from(f)), Some(plan));
        refused(&mut auto, "an automation session frame")?;
        e.ui.pump();
    }
    if rows_sent(&e)? != before {
        return Err(format!(
            "the session's frames in its open transaction sent {} row(s)",
            rows_sent(&e)? - before
        ));
    }
    // Its commit (the approval) sends the row with its frames.
    auto.commit(plan);
    refused(&mut auto, "the session's commit")?;
    e.ui.pump();
    if rows_sent(&e)? != before + 1 {
        return Err("the session's commit did not send one row".into());
    }
    let s = row_summary(&e);
    if !s
        .iter()
        .any(|l| l.starts_with("automation:auto-1") && l.contains("2 properties"))
    {
        return Err(format!("the row does not read the session's work: {s:?}"));
    }
    Ok(())
}

#[test]
fn a_committed_drag_reaches_teammates_while_an_automation_txn_stays_open() {
    check(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_row_waiting_for_every_txn_fails() {
    let e = check(CollabFaults {
        row_waits_for_every_txn: true,
        ..CollabFaults::default()
    })
    .expect_err("a row that waits for every open transaction must fail");
    assert!(e.contains("stale summary"), "{e}");
}
