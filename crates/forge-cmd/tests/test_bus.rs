//! Bus behaviour the UI depends on (Ch.7, Ch.21.18): a panicking command cannot take the bus
//! down; gestures (slider drags) are one undo step and Esc cancels them; provenance reaches
//! the history, the stream and the audit log; `Invoke` handlers come from `#[forge_api]`
//! command descriptors.

use forge_cmd::{
    AppliedKind, AuditAction, Bus, Change, CmdError, CommandPolicy, CommandSink, DiffBuilder,
    EditorCommand, EntityKey, FixedClock, Issuer, TxnState, Value,
};
use forge_reflect::{ForgeRegistry, forge_api};

fn human() -> Issuer {
    Issuer::Human { user: "ada".into() }
}

fn auto() -> Issuer {
    Issuer::Automation {
        session: "s1".into(),
        tool: "apply".into(),
    }
}

fn spawn(bus: &mut Bus, name: &str, parent: Option<EntityKey>) -> EntityKey {
    let e = bus.envelope(
        human(),
        EditorCommand::Spawn {
            name: name.into(),
            parent,
        },
    );
    match bus.apply(e).expect("spawn applies").diff.changes.first() {
        Some(Change::Created { entity, .. }) => *entity,
        other => panic!("spawn produced {other:?}"),
    }
}

fn set(entity: EntityKey, path: &str, v: f64) -> EditorCommand {
    EditorCommand::SetProperty {
        entity,
        path: path.into(),
        value: Value::Float(v),
    }
}

fn prop(bus: &Bus, e: EntityKey, path: &str) -> Option<Value> {
    bus.project()
        .entity(e)
        .and_then(|d| d.property(path))
        .cloned()
}

// ---- the bus survives a panicking command ------------------------------------------------

#[test]
fn a_panicking_command_is_a_rejection_and_the_bus_keeps_working() {
    let mut bus = Bus::new();
    let cube = spawn(&mut bus, "Cube", None);
    bus.register_handler(
        "plugin.explode",
        CommandPolicy::ORDINARY,
        |b: &mut DiffBuilder<'_>, _: &serde_json::Value| -> Result<(), CmdError> {
            // A plugin that describes a change, then panics halfway through.
            b.set_property(EntityKey(0), "half", Value::Int(1))?;
            let v: Vec<u8> = Vec::new();
            let _ = v[3]; // index out of bounds
            Ok(())
        },
    )
    .expect("registers");
    let feed = bus.subscribe();
    let before = bus.project().state_hash();
    let boom = EditorCommand::Invoke {
        target: "plugin.explode".into(),
        args: "{}".into(),
    };

    // dry_run is total: the panic is a Rejection, not an unwind.
    let e = bus.envelope(auto(), boom.clone());
    let r = bus.dry_run(&e).expect_err("dry_run rejects");
    assert_eq!(r.code().as_str(), "CMD-0009");
    assert!(r.to_string().contains("index out of bounds"), "{r}");

    let r = bus.apply(e).expect_err("apply rejects");
    assert_eq!(r.code().as_str(), "CMD-0009");
    assert_eq!(bus.project().state_hash(), before, "nothing was applied");
    assert!(
        !bus.is_poisoned(),
        "a panic while planning cannot corrupt state"
    );
    assert!(
        feed.drain().is_empty(),
        "no Applied event for a refused command"
    );
    let last = bus.audit().last().expect("the refusal is audited");
    assert_eq!(last.outcome.map_err(|c| c.as_str()), Err("CMD-0009"));
    assert_eq!(last.issuer, auto());

    // ...and the bus keeps working, in the same transaction id space.
    let e = bus.envelope(human(), set(cube, "x", 1.0));
    bus.apply(e).expect("the next command applies");
    assert_eq!(prop(&bus, cube, "x"), Some(Value::Float(1.0)));
    for _ in 0..3 {
        let e = bus.envelope(auto(), boom.clone());
        assert!(bus.apply(e).is_err());
    }
    let t = bus.undo_target().expect("the good command is undoable");
    bus.undo(t).expect("undo still works after repeated panics");
    assert_eq!(prop(&bus, cube, "x"), None);
}

// ---- gestures: a drag is one undo step -----------------------------------------------------

#[test]
fn a_slider_drag_is_one_transaction_and_one_undo_step() {
    let mut bus = Bus::new();
    let cube = spawn(&mut bus, "Cube", None);
    let e = bus.envelope(human(), set(cube, "scale", 1.0));
    bus.apply(e).expect("initial value");
    let depth_before = bus.history().count();
    let feed = bus.subscribe();

    let drag = bus.begin("Drag scale", human());
    for frame in 1..=60 {
        let e = bus.envelope_in(
            drag,
            human(),
            set(cube, "scale", 1.0 + f64::from(frame) * 0.1),
        );
        let a = bus.apply(e).expect("frame applies");
        assert_eq!(a.kind, AppliedKind::Command);
        assert_eq!(a.txn, drag);
    }
    // The viewport showed real state during the drag.
    assert_eq!(
        prop(&bus, cube, "scale"),
        Some(Value::Float(1.0 + 60.0 * 0.1))
    );
    assert_eq!(
        bus.history().count(),
        depth_before,
        "not in history until commit"
    );
    bus.commit(drag).expect("commits");

    let rec = bus.transaction(drag).expect("exists");
    assert_eq!(rec.state(), TxnState::Committed);
    assert_eq!(rec.commands().count, 60);
    let changes: Vec<&Change> = rec.changes().collect();
    assert_eq!(
        changes,
        vec![&Change::Property {
            entity: cube,
            path: "scale".into(),
            before: Some(Value::Float(1.0)),
            after: Some(Value::Float(1.0 + 60.0 * 0.1)),
        }],
        "sixty frames merge into one change that remembers where the drag started"
    );
    assert_eq!(bus.history().count(), depth_before + 1, "one undo entry");
    assert_eq!(bus.undo_target(), Some(drag));

    let a = bus.undo(drag).expect("undo");
    assert_eq!(a.kind, AppliedKind::Undo);
    assert_eq!(
        prop(&bus, cube, "scale"),
        Some(Value::Float(1.0)),
        "back to the pre-drag value"
    );
    let a = bus.redo(drag).expect("redo");
    assert_eq!(a.kind, AppliedKind::Redo);
    assert_eq!(
        prop(&bus, cube, "scale"),
        Some(Value::Float(1.0 + 60.0 * 0.1))
    );

    let kinds: Vec<AppliedKind> = feed.drain().iter().map(|a| a.kind).collect();
    assert_eq!(kinds.len(), 60 + 3);
    assert_eq!(
        &kinds[60..],
        &[AppliedKind::Commit, AppliedKind::Undo, AppliedKind::Redo]
    );
}

#[test]
fn esc_cancels_a_gesture_and_a_drag_that_returns_home_leaves_no_undo_entry() {
    let mut bus = Bus::new();
    let cube = spawn(&mut bus, "Cube", None);
    let h0 = bus.project().state_hash();
    let depth = bus.history().count();

    // Esc: cancel reverts every frame.
    let drag = bus.begin("Drag x", human());
    for x in [1.0, 2.0, 3.0] {
        let e = bus.envelope_in(drag, human(), set(cube, "x", x));
        bus.apply(e).expect("frame");
    }
    let a = bus.cancel(drag).expect("cancel");
    assert_eq!(a.kind, AppliedKind::Cancel);
    assert_eq!(bus.project().state_hash(), h0);
    assert_eq!(bus.txn_state(drag), Some(TxnState::Cancelled));
    assert_eq!(bus.history().count(), depth);
    let late = bus.envelope_in(drag, human(), set(cube, "x", 9.0));
    assert_eq!(
        bus.apply(late).expect_err("closed").code().as_str(),
        "CMD-0006"
    );

    // Undo of an open transaction is a cancel too (the trait's single entry point).
    let drag = bus.begin("Drag y", human());
    let e = bus.envelope_in(drag, human(), set(cube, "y", 1.0));
    bus.apply(e).expect("frame");
    bus.undo(drag).expect("undo of open = cancel");
    assert_eq!(bus.project().state_hash(), h0);

    // A drag that ends where it started commits nothing.
    let e = bus.envelope(human(), set(cube, "z", 0.0));
    bus.apply(e).expect("z");
    let depth = bus.history().count();
    let drag = bus.begin("Drag z", human());
    for z in [0.5, 1.0, 0.0] {
        let e = bus.envelope_in(drag, human(), set(cube, "z", z));
        bus.apply(e).expect("frame");
    }
    bus.commit(drag).expect("commit");
    assert_eq!(
        bus.txn_state(drag),
        Some(TxnState::Expired),
        "nothing to undo"
    );
    assert!(bus.transaction(drag).is_none(), "and no record is kept");
    assert_eq!(bus.history().count(), depth, "no empty undo step");
}

#[test]
fn a_gesture_merges_across_properties_and_structural_changes_in_order() {
    let mut bus = Bus::new();
    let root = spawn(&mut bus, "Root", None);
    let h0 = bus.project().state_hash();
    let t = bus.begin("Build", human());
    let e = bus.envelope_in(
        t,
        human(),
        EditorCommand::Spawn {
            name: "Kid".into(),
            parent: Some(root),
        },
    );
    let kid = match &bus.apply(e).expect("spawn").diff.changes[0] {
        Change::Created { entity, .. } => *entity,
        c => panic!("{c:?}"),
    };
    for i in 0..5 {
        let e = bus.envelope_in(t, human(), set(kid, "a", f64::from(i)));
        bus.apply(e).expect("a");
        let e = bus.envelope_in(t, human(), set(root, "b", f64::from(i)));
        bus.apply(e).expect("b");
        let e = bus.envelope_in(
            t,
            human(),
            EditorCommand::Rename {
                entity: kid,
                name: format!("Kid{i}"),
            },
        );
        bus.apply(e).expect("rename");
    }
    let e = bus.envelope_in(t, human(), EditorCommand::Despawn { entity: kid });
    bus.apply(e).expect("despawn inside the gesture");
    bus.commit(t).expect("commit");
    // Created, a, b, rename (merged, each once), then a->None + Removed from the despawn:
    // the a-removal merged into the a slot, which then cancels to a no-op.
    let n = bus.transaction(t).map(|r| r.changes().count());
    assert_eq!(
        n,
        Some(4),
        "{:?}",
        bus.transaction(t).map(|r| r.changes().collect::<Vec<_>>())
    );
    bus.undo(t).expect("undo");
    assert_eq!(bus.project().state_hash(), h0);
    bus.redo(t).expect("redo");
    assert!(!bus.project().contains(kid));
    assert_eq!(prop(&bus, root, "b"), Some(Value::Float(4.0)));
}

#[test]
fn another_issuer_cannot_append_to_a_gesture() {
    let mut bus = Bus::new();
    let cube = spawn(&mut bus, "Cube", None);
    let t = bus.begin("Drag", human());
    let e = bus.envelope_in(t, auto(), set(cube, "x", 1.0));
    assert_eq!(
        bus.apply(e).expect_err("mismatch").code().as_str(),
        "CMD-0012"
    );
}

// ---- undo semantics ------------------------------------------------------------------------

#[test]
fn undo_of_an_overwritten_transaction_is_a_conflict_not_a_corruption() {
    let mut bus = Bus::new();
    let cube = spawn(&mut bus, "Cube", None);
    let a = bus.envelope(auto(), set(cube, "x", 1.0));
    let automation_txn = a.txn;
    bus.apply(a).expect("automation session sets x");
    let h = bus.envelope(human(), set(cube, "x", 2.0));
    bus.apply(h).expect("human overwrites x");
    let before = bus.project().state_hash();
    let r = bus
        .undo(automation_txn)
        .expect_err("the session's value is gone");
    assert_eq!(r.code().as_str(), "CMD-0008");
    assert_eq!(bus.project().state_hash(), before);
    // Selective undo of an independent transaction works.
    let y = bus.envelope(auto(), set(cube, "y", 5.0));
    let y_txn = y.txn;
    bus.apply(y).expect("y");
    let z = bus.envelope(human(), set(cube, "z", 6.0));
    bus.apply(z).expect("z");
    bus.undo(y_txn).expect("y is independent of z");
    assert_eq!(prop(&bus, cube, "y"), None);
    assert_eq!(prop(&bus, cube, "z"), Some(Value::Float(6.0)));
}

#[test]
fn ctrl_z_and_ctrl_y_walk_the_history() {
    let mut bus = Bus::new();
    let cube = spawn(&mut bus, "Cube", None);
    for x in 1..=3 {
        let e = bus.envelope(human(), set(cube, "x", f64::from(x)));
        bus.apply(e).expect("x");
    }
    for expect in [2.0, 1.0] {
        let t = bus.undo_target().expect("something to undo");
        bus.undo(t).expect("undo");
        assert_eq!(prop(&bus, cube, "x"), Some(Value::Float(expect)));
    }
    let t = bus.redo_target().expect("something to redo");
    bus.redo(t).expect("redo");
    assert_eq!(prop(&bus, cube, "x"), Some(Value::Float(2.0)));
    // A new edit clears Ctrl+Y (the usual editor rule).
    let e = bus.envelope(human(), set(cube, "w", 1.0));
    bus.apply(e).expect("w");
    assert_eq!(bus.redo_target(), None);
    // Undo/redo state errors are stable codes.
    let t = bus.undo_target().expect("w");
    bus.undo(t).expect("undo w");
    assert_eq!(bus.undo(t).expect_err("twice").code().as_str(), "CMD-0006");
    assert_eq!(
        bus.undo(forge_cmd::TxnId(9999))
            .expect_err("unknown")
            .code()
            .as_str(),
        "CMD-0005"
    );
}

#[test]
fn history_is_bounded_and_expired_transactions_refuse_undo() {
    let mut bus = Bus::new().with_max_history(3);
    let cube = spawn(&mut bus, "Cube", None);
    let mut txns = Vec::new();
    for x in 1..=5 {
        let e = bus.envelope(human(), set(cube, "x", f64::from(x)));
        txns.push(e.txn);
        bus.apply(e).expect("x");
    }
    assert_eq!(bus.history().count(), 3);
    let r = bus.undo(txns[0]).expect_err("expired");
    assert_eq!(r.code().as_str(), "CMD-0006");
    assert_eq!(bus.txn_state(txns[0]), Some(TxnState::Expired));
}

// ---- envelope checks, provenance, audit ----------------------------------------------------

#[test]
fn a_resent_envelope_never_applies_twice() {
    let mut bus = Bus::new();
    let cube = spawn(&mut bus, "Cube", None);
    let e = bus.envelope(human(), set(cube, "x", 1.0));
    bus.apply(e.clone()).expect("first");
    let r = bus.apply(e).expect_err("resent");
    assert_eq!(r.code().as_str(), "CMD-0013");
}

#[test]
fn provenance_reaches_history_stream_and_audit() {
    let mut bus = Bus::with_clock(Box::new(FixedClock(1_700_000_000_000)));
    let feed = bus.subscribe();
    let cube = spawn(&mut bus, "Cube", None);
    let e = bus.envelope(auto(), set(cube, "x", 1.0));
    let txn = e.txn;
    bus.apply(e).expect("automation edit");
    let rec = bus.transaction(txn).expect("recorded");
    assert_eq!(rec.issuer().tag(), "automation:s1");
    assert_eq!(rec.opened_at_ms(), 1_700_000_000_000);
    assert_eq!(rec.label(), format!("Set {cube}.x"));
    let events = feed.drain();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].issuer, auto());
    assert!(events[0].seq < events[1].seq);

    // A human undoes the session's edit: the undo is attributed to the human.
    bus.undo_as(txn, &human()).expect("undo");
    let last = bus.audit().last().expect("audited");
    assert_eq!(last.action, AuditAction::Undo);
    assert_eq!(last.issuer, human());
    assert_eq!(last.outcome, Ok(1));
    let cmd_line = &bus.audit()[1];
    assert!(matches!(
        cmd_line.action,
        AuditAction::Command(EditorCommand::SetProperty { .. })
    ));
    assert_eq!(cmd_line.issuer, auto());

    // Invalid input is refused with stable codes, and every refusal is in the audit log.
    let n = bus.audit().len();
    for (cmd, code) in [
        (set(cube, "bad..path", 1.0), "CMD-0003"),
        (set(cube, "x", f64::NAN), "CMD-0004"),
        (set(EntityKey(77), "x", 1.0), "CMD-0001"),
        (
            EditorCommand::Rename {
                entity: cube,
                name: String::new(),
            },
            "CMD-0007",
        ),
        (
            EditorCommand::Invoke {
                target: "nope".into(),
                args: "{}".into(),
            },
            "CMD-0010",
        ),
    ] {
        let e = bus.envelope(auto(), cmd);
        assert_eq!(bus.apply(e).expect_err("refused").code().as_str(), code);
    }
    assert_eq!(bus.audit().len(), n + 5);
    assert!(bus.audit()[n..].iter().all(|a| a.outcome.is_err()));
    let drained = bus.drain_audit();
    assert_eq!(drained.len(), n + 5);
    assert!(bus.audit().is_empty());
}

#[test]
fn dropped_subscribers_are_pruned_and_live_ones_share_events() {
    let mut bus = Bus::new();
    let a = bus.subscribe();
    let b = bus.subscribe();
    drop(b);
    spawn(&mut bus, "Cube", None);
    let got = a.drain();
    assert_eq!(got.len(), 1);
    assert!(a.try_next().is_none());
}

#[test]
fn reparenting_into_a_descendant_is_refused() {
    let mut bus = Bus::new();
    let a = spawn(&mut bus, "A", None);
    let b = spawn(&mut bus, "B", Some(a));
    let e = bus.envelope(
        human(),
        EditorCommand::Reparent {
            entity: a,
            parent: Some(b),
        },
    );
    assert_eq!(bus.apply(e).expect_err("cycle").code().as_str(), "CMD-0002");
}

// ---- #[forge_api] command outputs become Invoke handlers -----------------------------------

/// Sets a body's health (a mutating fn: Ch.6 output 4 makes it a command).
#[forge_api(mutates)]
pub fn set_health(_world: &mut forge_core::World, #[forge(min = 0.0)] hp: f64) {
    let _ = hp;
}

#[test]
fn a_forge_api_command_desc_registers_as_a_checked_invoke_target() {
    let mut reg = ForgeRegistry::new();
    let item = reg.register::<set_health>().expect("agrees");
    let desc = item
        .command
        .clone()
        .expect("a mutating fn has a command output");
    let mut bus = Bus::new();
    let cube = spawn(&mut bus, "Cube", None);
    bus.register_api(
        &desc,
        move |b: &mut DiffBuilder<'_>, args: &serde_json::Value| {
            let hp = args["hp"].as_f64().ok_or(CmdError::BadArgs {
                target: "set_health".into(),
                why: "hp".into(),
            })?;
            b.set_property(cube, "health", Value::Float(hp))
        },
    )
    .expect("registers");
    assert_eq!(
        bus.register_api(&desc, |_: &mut DiffBuilder<'_>, _: &serde_json::Value| Ok(
            ()
        ))
        .expect_err("twice")
        .code()
        .as_str(),
        "CMD-0016"
    );
    let call = |args: &str| EditorCommand::Invoke {
        target: desc.target.clone(),
        args: args.into(),
    };
    let e = bus.envelope(auto(), call(r#"{"hp": 42.0}"#));
    bus.apply(e).expect("valid call applies");
    assert_eq!(prop(&bus, cube, "health"), Some(Value::Float(42.0)));
    for bad in [r#"{}"#, r#"{"hp": 1.0, "mana": 2}"#, "not json"] {
        let e = bus.envelope(auto(), call(bad));
        assert_eq!(
            bus.apply(e).expect_err(bad).code().as_str(),
            "CMD-0011",
            "{bad}"
        );
    }
    let t = bus.undo_target().expect("undoable like any command");
    bus.undo(t).expect("undo");
    assert_eq!(prop(&bus, cube, "health"), None);
}

// ---- only a gesture's owner can close it (CMD-0012, the rule apply() enforces) -------------

#[test]
fn an_automation_cannot_cancel_or_undo_a_human_drag_in_progress() {
    let mut bus = Bus::new();
    let cube = spawn(&mut bus, "Cube", None);
    let drag = bus.begin("Drag x", human());
    for x in 1..=3 {
        let e = bus.envelope_in(drag, human(), set(cube, "x", f64::from(x)));
        bus.apply(e).expect("frame");
    }
    let hash = bus.project().state_hash();

    // Both ways of closing another issuer's open transaction are refused, change nothing,
    // and are audited as the session's refusals.
    let r = bus.undo_as(drag, &auto()).expect_err("undo_as refused");
    assert_eq!(r.code().as_str(), "CMD-0012");
    let r = bus.cancel_as(drag, &auto()).expect_err("cancel_as refused");
    assert_eq!(r.code().as_str(), "CMD-0012");
    assert_eq!(
        bus.txn_state(drag),
        Some(TxnState::Open),
        "the drag goes on"
    );
    assert_eq!(bus.project().state_hash(), hash, "nothing was reverted");
    let refusals: Vec<_> = bus
        .audit()
        .iter()
        .filter(|a| a.txn == drag && a.action == AuditAction::Cancel)
        .collect();
    assert_eq!(refusals.len(), 2);
    assert!(
        refusals
            .iter()
            .all(|a| a.issuer == auto() && a.outcome.map_err(|c| c.as_str()) == Err("CMD-0012"))
    );

    // The owner can still send frames and finish the gesture normally.
    let e = bus.envelope_in(drag, human(), set(cube, "x", 4.0));
    bus.apply(e).expect("frame after the refusals");
    bus.undo_as(drag, &human())
        .expect("the owner's Esc cancels");
    assert_eq!(bus.txn_state(drag), Some(TxnState::Cancelled));
    assert_eq!(prop(&bus, cube, "x"), None, "the whole drag was reverted");

    // Positive control: a committed transaction is not a gesture in progress; anyone may
    // undo it (provenance-tagged), so the refusal above is the open-transaction rule, not a
    // blanket ban on automation sessions undoing human work.
    let e = bus.envelope(human(), set(cube, "y", 1.0));
    let t = e.txn;
    bus.apply(e).expect("edit");
    bus.undo_as(t, &auto())
        .expect("an automation session may undo committed work");
    assert_eq!(bus.audit().last().map(|a| a.issuer.clone()), Some(auto()));
}

/// Reserved settings (Ch.21.18: the editor core reserves its security state): a key under a
/// reserved prefix changes only through one of its writer targets, from any issuer; a plain
/// `SetSetting`, another handler, a dry run and a batch preview are all refused; a key that
/// merely shares the prefix's spelling is not reserved.
#[test]
fn a_reserved_setting_changes_only_through_its_writers() {
    let write = |b: &mut DiffBuilder<'_>, a: &serde_json::Value| {
        let key = a["key"].as_str().unwrap_or_default().to_string();
        b.set_setting(&key, Some(Value::Bool(true)))
    };
    let mut bus = Bus::new();
    bus.register_handler("sec.grant", CommandPolicy::ORDINARY, write)
        .expect("registers");
    bus.register_handler("other.write", CommandPolicy::ORDINARY, write)
        .expect("registers");
    let set = |key: &str| EditorCommand::SetSetting {
        key: key.into(),
        value: Some(Value::Bool(true)),
    };
    let invoke = |target: &str, key: &str| EditorCommand::Invoke {
        target: target.into(),
        args: serde_json::json!({ "key": key }).to_string(),
    };
    // Unreserved (the control): a plain SetSetting writes the key.
    let e = bus.envelope(auto(), set("security.grants.x"));
    assert!(bus.apply(e).is_ok(), "nothing is reserved yet");

    bus.reserve_settings("security", &["sec.grant"]);
    assert_eq!(
        bus.setting_writers("security.grants.y"),
        Some(vec!["sec.grant"])
    );
    assert_eq!(bus.setting_writers("securityx"), None);
    for (who, cmd) in [
        (auto(), set("security.grants.y")),
        (human(), set("security.grants.y")),
        (human(), set("security")),
        (auto(), invoke("other.write", "security.grants.y")),
    ] {
        let label = cmd.label();
        let e = bus.envelope(who.clone(), cmd);
        let dry = bus.dry_run(&e).map(drop).map_err(|r| r.error.code());
        let r = bus.apply(e).map(drop).map_err(|r| r.error.code());
        assert_eq!(dry, r, "{label}: a dry run must answer as the apply does");
        assert_eq!(
            r.map_err(|c| c.as_str().to_string()),
            Err("CMD-0014".to_string()),
            "{who} {label}"
        );
    }
    assert_eq!(bus.project().setting("security.grants.y"), None);
    // A batch preview refuses the reserved write at its index.
    let t = bus.next_txn_id();
    let batch = [
        bus.envelope_in(
            t,
            auto(),
            EditorCommand::Spawn {
                name: "A".into(),
                parent: None,
            },
        ),
        bus.envelope_in(t, auto(), set("security.grants.y")),
    ];
    assert_eq!(
        bus.dry_run_batch(&batch).map(drop).map_err(|(i, _)| i),
        Err(1)
    );
    // The writer does write it; a key that only shares the spelling is free.
    let e = bus.envelope(auto(), invoke("sec.grant", "security.grants.y"));
    assert!(bus.apply(e).is_ok());
    assert_eq!(
        bus.project().setting("security.grants.y"),
        Some(&Value::Bool(true))
    );
    let e = bus.envelope(auto(), set("securityx.theme"));
    assert!(bus.apply(e).is_ok());
}

/// WP-U10: a host guard is asked per command with the issuer and the planned diff — in
/// `apply`, a dry run, a batch preview, undo and redo — and its refusal applies nothing.
#[test]
fn a_host_guard_is_asked_per_command_and_its_refusal_applies_nothing() {
    use std::sync::{Arc, Mutex};

    use forge_cmd::{CommandGuard, Guarded};

    /// Refuses a human "bob" any change to entity e0, and records what it saw.
    struct NoBob(Mutex<Vec<(String, bool)>>);
    impl CommandGuard for NoBob {
        fn check(&self, g: &Guarded<'_>) -> Result<(), CmdError> {
            if let Ok(mut seen) = self.0.lock() {
                seen.push((g.issuer.tag(), g.cmd.is_some()));
            }
            let touches = g
                .diff
                .changes
                .iter()
                .any(|c| c.entity() == Some(EntityKey(0)));
            if touches && g.issuer == &(Issuer::Human { user: "bob".into() }) {
                return Err(CmdError::PolicyRefused {
                    what: "e0 is claimed".into(),
                });
            }
            Ok(())
        }
    }
    let bob = Issuer::Human { user: "bob".into() };
    let mut bus = Bus::new();
    let e0 = spawn(&mut bus, "Level", None);
    let guard = Arc::new(NoBob(Mutex::new(Vec::new())));
    bus.set_guard(Some(guard.clone()));
    let rename = |who: &Issuer, bus: &mut Bus| {
        bus.envelope(
            who.clone(),
            EditorCommand::Rename {
                entity: e0,
                name: "Mine".into(),
            },
        )
    };
    let e = rename(&bob, &mut bus);
    let dry = bus.dry_run(&e).map(drop).map_err(|r| r.error.code());
    let r = bus.apply(e).map(drop).map_err(|r| r.error.code());
    assert_eq!(dry, r, "a dry run answers as the apply does");
    assert_eq!(
        r.map_err(|c| c.as_str().to_string()),
        Err("CMD-0014".into())
    );
    assert_eq!(bus.project().entity(e0).map(|x| x.name()), Some("Level"));
    // A batch preview refuses at the guarded step.
    let t = bus.next_txn_id();
    let batch = [
        bus.envelope_in(
            t,
            bob.clone(),
            EditorCommand::Spawn {
                name: "Free".into(),
                parent: None,
            },
        ),
        bus.envelope_in(
            t,
            bob.clone(),
            EditorCommand::Rename {
                entity: e0,
                name: "x".into(),
            },
        ),
    ];
    assert_eq!(
        bus.dry_run_batch(&batch).map(drop).map_err(|(i, _)| i),
        Err(1)
    );
    // Someone else may; bob may not undo or redo it into e0 either.
    let e = rename(&human(), &mut bus);
    let txn = bus.apply(e).expect("ada renames").txn;
    let refused = bus.undo_as(txn, &bob).map(drop).map_err(|r| r.error.code());
    assert_eq!(
        refused.map_err(|c| c.as_str().to_string()),
        Err("CMD-0014".into())
    );
    assert_eq!(bus.project().entity(e0).map(|x| x.name()), Some("Mine"));
    bus.undo_as(txn, &human()).expect("ada undoes");
    assert!(bus.redo_as(txn, &bob).is_err(), "a redo is guarded too");
    let seen = guard.0.lock().map(|v| v.clone()).unwrap_or_default();
    assert!(
        seen.iter().any(|(_, cmd)| !cmd),
        "undo / redo ask with no command"
    );
    // Removing the guard (the control) lets bob through.
    bus.set_guard(None);
    let e = rename(&bob, &mut bus);
    assert!(bus.apply(e).is_ok());
}

/// WP-U10: `spawn_at` creates at the given key (a sandbox keeps the baseline's keys) and is
/// refused on a key that exists; later spawns allocate above it.
#[test]
fn spawn_at_keeps_the_key_and_refuses_one_that_exists() {
    let mut bus = Bus::new();
    bus.register_handler(
        "t.at",
        CommandPolicy::ORDINARY,
        |b: &mut DiffBuilder<'_>, a: &serde_json::Value| {
            let k = a["key"].as_u64().unwrap_or(0);
            b.spawn_at(EntityKey(k), "At", None).map(drop)
        },
    )
    .expect("registers");
    let at = |k: u64| EditorCommand::Invoke {
        target: "t.at".into(),
        args: serde_json::json!({ "key": k }).to_string(),
    };
    let e = bus.envelope(human(), at(40));
    bus.apply(e).expect("creates e40");
    assert!(bus.project().contains(EntityKey(40)));
    let e = bus.envelope(human(), at(40));
    let r = bus.apply(e).map(drop).map_err(|r| r.error.code());
    assert_eq!(
        r.map_err(|c| c.as_str().to_string()),
        Err("CMD-0008".into())
    );
    assert_eq!(spawn(&mut bus, "Next", None), EntityKey(41));
}
