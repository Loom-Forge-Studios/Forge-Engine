//! `test_sandbox_txn_hook` (Ch.37 I19; WP-U14): **a sandbox's transaction counts follow the
//! bus, whichever path undid, redid or cancelled.**
//!
//! The sandbox keeps, as deltas arrive, how many of its transactions count (not undone or
//! cancelled) and their summary — the status's "unpublished" and the row teammates read.
//! That upkeep used to be a call at every cancel / undo / redo site in the core; a new path
//! that forgot it (an approval, an automation session's batch, a future tool) would leave the
//! counts drifting silently. It now hangs off the bus's own `Applied` stream, which the core drains
//! before the lock is released and before the sandbox is read.
//!
//! The check drives the bus by a path that tells the sandbox nothing — core configuration
//! (`EditorCore::configure`), straight at `Bus::undo_as` / `redo_as` / `cancel_as` — and by
//! the ordinary client paths, and after each one the counts and the summary must match
//! the bus: an undone edit stops counting, a redone one counts again, a cancelled gesture
//! never counts, and the status agrees.
//!
//! Positive control (W2): `positive_control_call_site_upkeep_drifts` — with the stream hook
//! off (`CollabFaults::no_txn_hook`: only call sites that remember would move the counts)
//! the undo by configuration leaves the undone edit counted, and the check fails.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::collab::licence::MemoryEntitlement;
use forge_editor::collab::{self as c, CollabFaults};
use forge_editor::core::{CollabAttach, EditorCore, LocalBus, SharedCore};
use forge_project::collab::{MemoryCollab, MemoryIdentity};

const NOW: u64 = 20_000 * 86_400_000;

fn editor(faults: CollabFaults) -> (SharedCore, LocalBus) {
    static N: AtomicU64 = AtomicU64::new(0);
    let identity = Arc::new(MemoryIdentity::new());
    let name = format!(
        "sandbox-txn-hook-{}-{}",
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
            server,
            licence,
            owner: "ada".into(),
            email: Some("ada@studio.example".into()),
        },
    );
    EditorCore::set_collab_faults(&core, faults);
    let ui = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    (core, ui)
}

fn ok(ui: &mut LocalBus, what: &str) -> Result<(), String> {
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

/// `(counted transactions, status unpublished, summary)`.
fn state(core: &SharedCore) -> Result<(usize, usize, Vec<String>), String> {
    let counted = EditorCore::collab_counters(core)
        .ok_or("not attached")?
        .counted;
    let st = EditorCore::collab_status(core).ok_or("no status")?;
    Ok((counted, st.unpublished, st.summary.clone()))
}

fn expect(core: &SharedCore, n: usize, props: &str, after: &str) -> Result<(), String> {
    let (counted, unpublished, summary) = state(core)?;
    if counted != n || unpublished != n {
        return Err(format!(
            "after {after}: {counted} transaction(s) counted ({unpublished} in the status), the bus says {n}"
        ));
    }
    if !summary.iter().any(|l| l.contains(props)) {
        return Err(format!(
            "after {after}: the summary {summary:?} does not read {props:?}"
        ));
    }
    Ok(())
}

fn check(faults: CollabFaults) -> Result<(), String> {
    let (core, mut ui) = editor(faults);
    let lamp: EntityKey = EditorCore::read(&core, |p| p.next_key());
    ui.apply(
        EditorCommand::Spawn {
            name: "Lamp".into(),
            parent: None,
        },
        None,
    );
    ui.apply(c::create_command("Studio"), None);
    ok(&mut ui, "set-up")?;
    // Two committed edits: two transactions count.
    let a = ui.begin("Edit mass");
    ui.apply(set(lamp, "mass", 1.0), Some(a));
    ui.commit(a);
    let b = ui.begin("Edit x");
    ui.apply(set(lamp, "x", 1.0), Some(b));
    ui.commit(b);
    ok(&mut ui, "edits")?;
    expect(&core, 2, "2 properties", "two edits")?;

    // A path that tells the sandbox nothing: the bus, straight from core configuration.
    let who = Issuer::Human { user: "ada".into() };
    EditorCore::configure(&core, |bus| bus.undo_as(b, &who)).map_err(|r| r.error.to_string())?;
    expect(&core, 1, "1 property", "an undo by configuration")?;
    EditorCore::configure(&core, |bus| bus.redo_as(b, &who)).map_err(|r| r.error.to_string())?;
    expect(&core, 2, "2 properties", "a redo by configuration")?;
    let g = ui.begin("Drag y");
    ui.apply(set(lamp, "y", 3.0), Some(g));
    ok(&mut ui, "a gesture frame")?;
    expect(&core, 3, "3 properties", "a gesture's first frame")?;
    EditorCore::configure(&core, |bus| bus.cancel_as(g, &who)).map_err(|r| r.error.to_string())?;
    expect(&core, 2, "2 properties", "a cancel by configuration")?;

    // The ordinary client paths agree with the same hook.
    ui.undo(a);
    ok(&mut ui, "undo")?;
    expect(&core, 1, "1 property", "a client's undo")?;
    ui.redo(a);
    ok(&mut ui, "redo")?;
    expect(&core, 2, "2 properties", "a client's redo")?;
    let g = ui.begin("Drag z");
    ui.apply(set(lamp, "z", 3.0), Some(g));
    ui.cancel(g);
    ok(&mut ui, "cancel")?;
    expect(&core, 2, "2 properties", "a client's cancel")?;
    Ok(())
}

#[test]
fn transaction_counts_follow_the_bus_on_every_path() {
    check(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_call_site_upkeep_drifts() {
    let e = check(CollabFaults {
        no_txn_hook: true,
        ..CollabFaults::default()
    })
    .expect_err("counts kept only by call sites must drift");
    assert!(e.contains("an undo by configuration"), "{e}");
}
