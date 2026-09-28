//! `test_sandbox_deltas` (Ch.37 I19; WP-U10, owner rule 2, D-5): **a team sandbox's delta
//! list costs nothing per frame.**
//!
//! Every command applied in a team sandbox is a sandbox delta, and every client pump follows
//! the baseline (records the sandbox row, Live-pulls). A long session between publishes must
//! not make either grow with the session:
//!
//! * a gesture's frames **fold**: a 600-frame drag of two properties holds two deltas, not
//!   1200, and still publishes its last values;
//! * the sandbox row is sent when a transaction starts, ends, is undone or redone — **not per
//!   frame** inside an open gesture, and never on an idle pump;
//! * the status reads the counts and the summary kept as deltas arrive (unpublished
//!   transactions, "edited 3 properties on 1 entity"), and follows an undo and a redo.
//!
//! Positive controls (W2): a sandbox that keeps every frame (`CollabFaults::no_fold`) holds
//! 1201 deltas, and one that sends the row whenever the summary moves
//! (`CollabFaults::row_every_change`) sends one per frame; the check fails on each.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::collab::licence::MemoryEntitlement;
use forge_editor::collab::{self as c, CollabFaults};
use forge_editor::core::{CollabAttach, EditorCore, LocalBus, SharedCore};
use forge_project::collab::{CollabBackend, CollabView, MemoryCollab, MemoryIdentity};

const NOW: u64 = 20_000 * 86_400_000;
const FRAMES: u32 = 600;

struct Editor {
    core: SharedCore,
    ui: LocalBus,
    server: Arc<MemoryCollab>,
}

fn editor(faults: CollabFaults) -> Editor {
    static N: AtomicU64 = AtomicU64::new(0);
    let identity = Arc::new(MemoryIdentity::new());
    let name = format!(
        "sandbox-deltas-{}-{}",
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

fn send(e: &mut Editor, cmd: EditorCommand) -> Result<(), String> {
    e.ui.apply(cmd, None);
    match e.ui.pump().refused.first() {
        Some(r) => Err(format!(
            "{}: {}",
            r.rejection.code().as_str(),
            r.rejection.error
        )),
        None => Ok(()),
    }
}

fn counters(e: &Editor) -> Result<forge_editor::core::CollabCounters, String> {
    EditorCore::collab_counters(&e.core).ok_or_else(|| "not attached".to_string())
}

fn summary(e: &Editor) -> Vec<String> {
    EditorCore::collab_status(&e.core).map_or_else(Vec::new, |s| s.summary.clone())
}

fn unpublished(e: &Editor) -> usize {
    EditorCore::collab_status(&e.core).map_or(0, |s| s.unpublished)
}

fn check(faults: CollabFaults) -> Result<(), String> {
    let mut e = editor(faults);
    let lamp: EntityKey = EditorCore::read(&e.core, |p| p.next_key());
    send(
        &mut e,
        EditorCommand::Spawn {
            name: "Lamp".into(),
            parent: None,
        },
    )?;
    send(&mut e, c::create_command("Studio"))?;
    // A first edit after the team exists: one delta, one transaction, one row.
    send(
        &mut e,
        EditorCommand::SetProperty {
            entity: lamp,
            path: "mass".into(),
            value: Value::Float(1.0),
        },
    )?;
    let start = counters(&e)?;
    if start.deltas != 1 || start.counted != 1 {
        return Err(format!("a single edit is not one delta: {start:?}"));
    }

    // A drag: 600 frames, two properties each, a pump per frame (the editor's loop).
    let drag = e.ui.begin("Drag");
    for f in 0..FRAMES {
        for path in ["x", "y"] {
            e.ui.apply(
                EditorCommand::SetProperty {
                    entity: lamp,
                    path: path.into(),
                    value: Value::Float(f64::from(f)),
                },
                Some(drag),
            );
        }
        if let Some(r) = e.ui.pump().refused.first() {
            return Err(format!("frame {f} refused: {}", r.rejection.error));
        }
    }
    let mid = counters(&e)?;
    if mid.deltas != 3 {
        return Err(format!(
            "the drag held {} deltas, not its two properties (plus the first edit)",
            mid.deltas
        ));
    }
    // The drag's first frame starts a transaction (one row); no frame after it sends one.
    if mid.rows_sent > start.rows_sent + 1 {
        return Err(format!(
            "the drag sent {} sandbox rows during its frames",
            mid.rows_sent - start.rows_sent
        ));
    }
    e.ui.commit(drag);
    e.ui.pump();
    let done = counters(&e)?;
    if done.rows_sent > mid.rows_sent + 1 {
        return Err("the drag's commit sent more than one row".into());
    }
    if unpublished(&e) != 2 {
        return Err(format!(
            "{} unpublished transactions, not 2",
            unpublished(&e)
        ));
    }
    let s = summary(&e);
    if !s
        .iter()
        .any(|l| l.contains("edited 3 properties on 1 entity"))
    {
        return Err(format!("the summary does not read the drag: {s:?}"));
    }
    // Idle pumps send nothing and hold nothing new.
    for _ in 0..100 {
        e.ui.pump();
    }
    if counters(&e)? != done {
        return Err(format!(
            "idle pumps changed the sandbox: {:?}",
            counters(&e)?
        ));
    }
    // An undo takes the drag out of what counts; a redo puts it back.
    e.ui.undo(drag);
    e.ui.pump();
    let s = summary(&e);
    if unpublished(&e) != 1
        || !s
            .iter()
            .any(|l| l.contains("edited 1 property on 1 entity"))
    {
        return Err(format!(
            "the undone drag still counts: {} {s:?}",
            unpublished(&e)
        ));
    }
    e.ui.redo(drag);
    e.ui.pump();
    if unpublished(&e) != 2 {
        return Err("the redone drag does not count".into());
    }
    // Publishing the folded deltas publishes the drag's last values.
    send(&mut e, c::publish_command(""))?;
    let head = e
        .server
        .doc_at(e.server.head())
        .map_err(|x| x.to_string())?;
    let x = head
        .entities
        .iter()
        .find(|en| en.name == "Lamp")
        .and_then(|en| en.properties.get("x").cloned());
    if x != Some(Value::Float(f64::from(FRAMES - 1))) {
        return Err(format!("the published drag ended at {x:?}"));
    }
    let rev = e.server.revisions(None, usize::MAX);
    if rev.last().map(|r| r.commands) != Some(3) {
        return Err(format!(
            "the publish carried {:?} commands, not the 3 folded deltas",
            rev.last().map(|r| r.commands)
        ));
    }
    Ok(())
}

#[test]
fn sandbox_deltas_fold_and_rows_follow_transactions() {
    check(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_unfolded_frames_fail() {
    let e = check(CollabFaults {
        no_fold: true,
        ..CollabFaults::default()
    })
    .expect_err("a sandbox that keeps every frame must fail");
    assert!(e.contains("the drag held 1201 deltas"), "{e}");
}

#[test]
fn positive_control_a_row_per_frame_fails() {
    let e = check(CollabFaults {
        row_every_change: true,
        ..CollabFaults::default()
    })
    .expect_err("a row per frame must fail");
    assert!(e.contains("sandbox rows during its frames"), "{e}");
}
