//! `test_security_commands` (Ch.21 §21.18, WP-U9): the security class.
//!
//! * **Only a human issues a security command.** Every grant, revoke, policy change,
//!   approval and rejection an automation session, a script or a test issuer sends is refused with
//!   `CMD-0014`, and nothing changes: not the project, not the grant table.
//! * **Undo and redo never grant.** Undoing a grant revokes it in the one grant table; the
//!   undone grant cannot be redone; a revoke and a policy change cannot be undone.
//! * **The grant table follows the project**: a plugin grant reaches the table the WASM host
//!   and every session host check; a saved automation grant from another run grants nothing when
//!   the project is loaded (session ids are reused).
//! * **Approve and reject are commands** the core performs on an automation session's open preview
//!   transaction (and only on one), audited with the person who did it.
//!
//! Positive control (W2): `positive_control_security_commands_registered_as_ordinary_fail`
//! registers the same planners with the ordinary policy; the checks must fail.

use forge_cmd::{Bus, CommandPolicy, EditorCommand, Issuer, TxnState, Value};
use forge_editor::client::BusClient;
use forge_editor::connect::audit::AuditQuery;
use forge_editor::core::{EditorCore, SharedCore};
use forge_editor::security::{self, AUTOMATION_POLICY_CMD};
use forge_plugin::{Capability, CommandClass, FsScope, PluginId, Principal, PrincipalKind};

const DESTRUCTIVE: Capability = Capability::Command(CommandClass::Destructive);

fn human() -> Issuer {
    Issuer::Human { user: "ada".into() }
}

fn auto(s: &str) -> Issuer {
    Issuer::Automation {
        session: s.into(),
        tool: "apply".into(),
    }
}

fn plugin(id: &str) -> Principal {
    Principal::Plugin(PluginId::new(id).unwrap_or_else(|e| panic!("{e}")))
}

/// Send `cmd` as `issuer`; the refusal's code, if any.
fn send(core: &SharedCore, issuer: Issuer, cmd: EditorCommand) -> Option<String> {
    let mut c = EditorCore::connect(core, issuer);
    c.apply(cmd, None);
    c.pump()
        .refused
        .first()
        .map(|r| r.rejection.code().as_str().to_string())
}

/// The checks, against a core (the real one, or the control's).
fn check(core: &SharedCore) -> Result<(), String> {
    let table = EditorCore::grants(core);
    let a1 = Principal::Automation("auto-1".into());
    let rivers = plugin("com.example.rivers");
    // 1. Non-human issuers are refused, and nothing changes.
    for who in [
        auto("auto-1"),
        Issuer::Script {
            path: "x.fscript".into(),
        },
        Issuer::Test,
    ] {
        let before = EditorCore::read(core, forge_cmd::Project::state_hash);
        for cmd in [
            security::grant_command(&a1, DESTRUCTIVE),
            security::grant_command(&rivers, DESTRUCTIVE),
            security::revoke_command(&a1, Capability::Fs(FsScope::ProjectRead)),
            security::policy_command(&[Capability::Fs(FsScope::ProjectRead)]),
            // The generic path to the same state (`test_reserved_security_settings` in full).
            EditorCommand::SetSetting {
                key: security::grant_key(&a1, DESTRUCTIVE).unwrap_or_default(),
                value: Some(Value::Text(security::epoch().into())),
            },
        ] {
            let label = cmd.label();
            match send(core, who.clone(), cmd) {
                Some(code) if code == "CMD-0014" => {}
                other => return Err(format!("{who} sent {label}: {other:?}, not CMD-0014")),
            }
        }
        if EditorCore::read(core, forge_cmd::Project::state_hash) != before {
            return Err(format!("a refused command from {who} changed the project"));
        }
        if table.has(&a1, DESTRUCTIVE) {
            return Err(format!("{who} granted auto-1 a destructive capability"));
        }
    }
    // 2. A human grants; undo revokes; redo is refused.
    let mut ui = EditorCore::connect(core, human());
    ui.apply(security::grant_command(&a1, DESTRUCTIVE), None);
    if !table.has(&a1, DESTRUCTIVE) {
        return Err("a human's grant did not reach the table".into());
    }
    let t = ui.undo_target().ok_or("the grant is not undoable")?;
    ui.undo(t);
    if table.has(&a1, DESTRUCTIVE) {
        return Err("undoing the grant did not revoke it".into());
    }
    ui.redo(t);
    let refused: Vec<String> = ui
        .pump()
        .refused
        .iter()
        .map(|r| r.rejection.code().as_str().to_string())
        .collect();
    if table.has(&a1, DESTRUCTIVE) || refused != ["CMD-0014"] {
        return Err(format!(
            "redoing an undone grant granted it again (refused: {refused:?})"
        ));
    }
    // 3. A revoke cannot be undone (that would grant).
    ui.apply(security::grant_command(&rivers, DESTRUCTIVE), None);
    ui.apply(security::revoke_command(&rivers, DESTRUCTIVE), None);
    let _ = ui.pump();
    let t = ui.undo_target().ok_or("nothing to undo")?;
    ui.undo(t);
    if table.has(&rivers, DESTRUCTIVE) {
        return Err("undoing a revoke granted the capability again".into());
    }
    // 4. A policy change cannot be undone either.
    ui.apply(
        security::policy_command(&[Capability::Fs(FsScope::ProjectRead)]),
        None,
    );
    let _ = ui.pump();
    let t = ui.undo_target().ok_or("nothing to undo")?;
    ui.undo(t);
    let p = table
        .default_policy(PrincipalKind::Automation)
        .ok_or("no default automation policy")?;
    if p.contains(Capability::Command(CommandClass::Ordinary)) {
        return Err("undoing a policy change put a capability back".into());
    }
    Ok(())
}

#[test]
fn security_commands_are_human_only_and_undo_never_grants() {
    let core = EditorCore::new();
    check(&core).unwrap_or_else(|e| panic!("{e}"));
    // Every security target has the human-only policy on the editor's bus.
    let c = EditorCore::connect(&core, human());
    for t in security::SECURITY_TARGETS {
        let p = c.policy_of(&EditorCommand::Invoke {
            target: (*t).into(),
            args: "{}".into(),
        });
        assert!(p.human_only, "{t} is not human-only");
        assert!(!p.redoable, "{t} is redoable");
    }
    let p = c.policy_of(&EditorCommand::Invoke {
        target: AUTOMATION_POLICY_CMD.into(),
        args: "{}".into(),
    });
    assert!(!p.undoable);
}

#[test]
fn positive_control_security_commands_registered_as_ordinary_fail() {
    let mut bus = Bus::new();
    for (target, _, plan) in security::handlers(security::epoch()) {
        bus.register_handler(
            target,
            CommandPolicy::ORDINARY,
            move |b: &mut forge_cmd::DiffBuilder<'_>, a: &serde_json::Value| plan(b, a),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }
    let core = EditorCore::with_bus(bus);
    let e = check(&core).expect_err("ordinary security commands must fail the checks");
    assert!(e.contains("auto-1") || e.contains("CMD-0014"), "{e}");
}

#[test]
fn the_grant_table_follows_the_project_and_saved_automation_grants_do_not_outlive_their_run() {
    let core = EditorCore::new();
    let table = EditorCore::grants(&core);
    let rivers = plugin("com.example.rivers");
    let mut ui = EditorCore::connect(&core, human());
    ui.apply(
        security::grant_command(&rivers, Capability::Fs(FsScope::ProjectRead)),
        None,
    );
    assert!(table.has(&rivers, Capability::Fs(FsScope::ProjectRead)));
    // The grant is ordinary project state: a plain key, a plain value.
    let key =
        security::grant_key(&rivers, Capability::Fs(FsScope::ProjectRead)).unwrap_or_default();
    assert_eq!(
        EditorCore::read(&core, |p| p.setting(&key).cloned()),
        Some(Value::Bool(true))
    );
    // A project saved by another run holds an automation grant: loading it grants nothing.
    let stale = security::grant_key(&Principal::Automation("auto-1".into()), DESTRUCTIVE)
        .unwrap_or_default();
    let fresh = security::grant_key(&Principal::Automation("auto-2".into()), DESTRUCTIVE)
        .unwrap_or_default();
    let mut doc = forge_project::format::ProjectDoc::default();
    doc.settings
        .insert(stale.clone(), Value::Text("run-0-another-run".into()));
    doc.settings
        .insert(fresh.clone(), Value::Text(security::epoch().into()));
    doc.settings.insert(key.clone(), Value::Bool(true));
    let load = doc.load_command().unwrap_or_else(|e| panic!("{e}"));
    ui.apply(load, None);
    let refused = ui.pump().refused;
    assert!(refused.is_empty(), "{refused:?}");
    assert!(
        !table.has(&Principal::Automation("auto-1".into()), DESTRUCTIVE),
        "a saved automation grant of another run must not grant"
    );
    // Nor one carrying this run's epoch (WP-20: automation grants are per run, a load drops
    // them — an automation session that wrote the file would otherwise grant itself).
    assert!(
        !table.has(&Principal::Automation("auto-2".into()), DESTRUCTIVE),
        "a loaded automation grant must not grant, even with this run's epoch"
    );
    assert_eq!(
        EditorCore::read(&core, |p| p.setting(&fresh).cloned()),
        None
    );
    // Plugin grants still ship with the project.
    assert!(table.has(&rivers, Capability::Fs(FsScope::ProjectRead)));
}

#[test]
fn approve_and_reject_commit_or_cancel_an_sessions_preview_transaction_as_audited_commands() {
    let core = EditorCore::new();
    let book = EditorCore::audit_book(&core);
    let mut a = EditorCore::connect(&core, auto("auto-7"));
    let mut ui = EditorCore::connect(&core, human());
    let spawn = |n: &str| EditorCommand::Spawn {
        name: n.into(),
        parent: None,
    };
    // Approve: the session's pending work is committed, one undo step, by the human's command.
    let t = BusClient::begin(&mut a, "Automation session auto-7");
    a.apply_batch(vec![spawn("Tree"), spawn("Rock")], Some(t), None)
        .unwrap_or_else(|(_, r)| panic!("{}", r.error));
    assert_eq!(EditorCore::open_automation_txns(&core).len(), 1);
    let (_, changes) = EditorCore::txn_changes(&core, t).unwrap_or_else(|| panic!("pending"));
    assert_eq!(changes.len(), 2);
    ui.apply(security::approve_command(t), None);
    assert!(ui.pump().refused.is_empty());
    assert_eq!(a.txn_state(t), Some(TxnState::Committed));
    assert!(EditorCore::open_automation_txns(&core).is_empty());
    // Reject: reverted.
    let t2 = BusClient::begin(&mut a, "Automation session auto-7");
    a.apply_batch(vec![spawn("Bush")], Some(t2), None)
        .unwrap_or_else(|(_, r)| panic!("{}", r.error));
    ui.apply(security::reject_command(t2), None);
    assert!(ui.pump().refused.is_empty());
    assert_eq!(a.txn_state(t2), Some(TxnState::Cancelled));
    let names: Vec<String> = EditorCore::read(&core, |p| {
        p.entities().map(|(_, e)| e.name().to_string()).collect()
    });
    assert_eq!(names.len(), 2, "{names:?}");
    // Only an automation session's open transaction can be approved: not a closed one, not a
    // human's.
    ui.apply(security::approve_command(t2), None);
    let h = BusClient::begin(&mut ui, "Drag");
    ui.apply(security::approve_command(h), None);
    let codes: Vec<String> = ui
        .pump()
        .refused
        .iter()
        .map(|r| r.rejection.code().as_str().to_string())
        .collect();
    assert_eq!(codes, ["CMD-0006", "CMD-0014"]);
    // An automation session cannot approve itself.
    let t3 = BusClient::begin(&mut a, "Automation session auto-7");
    a.apply_batch(vec![spawn("Fern")], Some(t3), None)
        .unwrap_or_else(|(_, r)| panic!("{}", r.error));
    assert_eq!(
        send(&core, auto("auto-7"), security::approve_command(t3)).as_deref(),
        Some("CMD-0014")
    );
    assert_eq!(a.txn_state(t3), Some(TxnState::Open));
    // Audited, with the person who did it.
    let q = AuditQuery::parse("session:auto-7 user:ada");
    let events: Vec<String> = book.query(&q).into_iter().map(|r| r.event).collect();
    assert_eq!(events, ["approve", "reject"]);
}
