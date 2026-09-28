//! `test_audit_log_panel` (Ch.21 §21.21 "Audit log", Ch.34 §34.5; DoD M2-54): every
//! envelope a plugin-hosted session sent (an audit feed attached to the connect services),
//! every security command (as the core applied it) and every device pairing are on one
//! timeline, **greppable by session, user and sandbox**.
//!
//! Positive control (W2): `positive_control_an_audit_missing_security_commands_fails` — an
//! audit source that leaves the security records out must fail the "every security command
//! is there" check.

mod common;

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use common::{feed, part, rows};
use forge_editor::client::BusClient;
use forge_editor::connect::audit::{AuditFeed, AuditOrigin, AuditQuery, AuditRecord, EditorAudit};
use forge_editor::security;
use forge_editor::testing::Rig;
use forge_plugin::{Capability, CommandClass, Principal};
use forge_ui::widgets::SearchChanged;

const L: &str = "forge.audit_log";

/// A plugin's own log of the sessions it hosts (what a protocol server attaches): two
/// sessions' envelopes, numbered from 0.
#[derive(Default)]
struct SessionLog {
    lines: Mutex<Vec<AuditRecord>>,
    cell: Arc<forge_ui::LiveCell>,
}

impl SessionLog {
    fn record(&self, session: &str, event: &str, detail: &str) {
        let mut l = self
            .lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = l.len() as u64;
        l.push(AuditRecord {
            n,
            at_ms: n,
            origin: AuditOrigin::Automation,
            session: session.into(),
            user: format!("automation:{session}"),
            sandbox: String::new(),
            event: event.into(),
            detail: detail.into(),
        });
        drop(l);
        self.cell.bump();
    }
}

impl AuditFeed for SessionLog {
    fn backend(&self) -> String {
        "a plugin's session log".into()
    }
    fn generation(&self) -> u64 {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len() as u64
    }
    fn records_from(&self, q: &AuditQuery, from: u64, out: &mut Vec<AuditRecord>) -> u64 {
        let l = self
            .lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        out.extend(l.iter().filter(|r| r.n >= from && q.matches(r)).cloned());
        l.len() as u64
    }
    fn live(&self) -> Option<Arc<dyn forge_ui::LiveSource>> {
        Some(self.cell.clone() as Arc<dyn forge_ui::LiveSource>)
    }
}

fn grep(rig: &mut Rig, q: &str) -> Vec<String> {
    let field = part(rig, L, &["content", "query"]);
    rig.h.ui.raise(
        field,
        SearchChanged {
            field,
            query: q.into(),
        },
    );
    rig.turn();
    rig.settle();
    let list = part(rig, L, &["content", "records"]);
    rows(rig, list)
}

fn check(skip_security: bool) -> Result<(), String> {
    let log = Arc::new(SessionLog::default());
    log.cell.set_live(true);
    let attached = Arc::clone(&log);
    let mut rig = common::rig_with(&[L], None, move |cfg, _| {
        cfg.services.connect.attach_audit_feed(attached.clone());
        if skip_security {
            cfg.services.connect.audit = Rc::new(EditorAudit {
                book: cfg.services.connect.book.clone(),
                feed: Some(attached),
                skip_security: true,
            });
        }
    });
    // Two hosted sessions working.
    log.record("auto-1", "command", "apply Spawn One");
    log.record("auto-2", "command", "apply Spawn Two");
    // A person grants auto-2 a destructive capability and undoes it (a revoke by undo).
    let mut h = common::human(&rig);
    h.apply(
        security::grant_command(
            &Principal::Automation("auto-2".into()),
            Capability::Command(CommandClass::Destructive),
        ),
        None,
    );
    let t = h.undo_target().ok_or("the grant is undoable")?;
    h.undo(t);
    let _ = h.pump();
    // A device pairs (user config, audited).
    rig.shell
        .handles()
        .services()
        .connect
        .pairing
        .borrow_mut()
        .pair("studio-laptop", "tester", 1)
        .map_err(|e| e.to_string())?;
    feed(&mut rig);

    let all = grep(&mut rig, "");
    if all.is_empty() {
        return Err("the audit log is empty".into());
    }
    // By session: exactly auto-1's lines, all of them.
    let s1 = grep(&mut rig, "session:auto-1");
    if s1.is_empty() || !s1.iter().all(|l| l.contains("session:auto-1 ")) {
        return Err(format!("session:auto-1 matched other lines: {s1:?}"));
    }
    if !s1.iter().any(|l| l.contains("[automation] command")) {
        return Err(format!("auto-1's envelope is not there: {s1:?}"));
    }
    if s1.iter().any(|l| l.contains("auto-2")) {
        return Err(format!("auto-2 leaked into auto-1's grep: {s1:?}"));
    }
    // By user: the person's security commands, the grant and its undo.
    let u = grep(&mut rig, "user:tester origin:security");
    let grant = u
        .iter()
        .any(|l| l.contains("[security] grant") && l.contains("automation:auto-2"));
    let undone = u
        .iter()
        .any(|l| l.contains("[security] revoke") && l.contains("(by undo)"));
    if !(grant && undone) {
        return Err(format!("the security commands are not all there: {u:?}"));
    }
    // The pairing.
    let r = grep(&mut rig, "origin:remote");
    if !r
        .iter()
        .any(|l| l.contains("paired") && l.contains("studio-laptop"))
    {
        return Err(format!("the pairing is not there: {r:?}"));
    }
    // A sandbox grep matches nothing (no sandboxes until forge-collab), and says so.
    let sb = grep(&mut rig, "sandbox:alice");
    if !sb.is_empty() {
        return Err(format!("sandbox:alice matched {sb:?}"));
    }
    let footer = common::label(&rig, part(&rig, L, &["content", "footer"]));
    if !footer.contains("teammate envelopes: UNBUILT") {
        return Err(format!(
            "the footer does not say teammates are unbuilt: {footer}"
        ));
    }
    Ok(())
}

#[test]
fn the_audit_log_is_one_timeline_greppable_by_session_user_and_sandbox() {
    check(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_audit_missing_security_commands_fails() {
    let e = check(true).expect_err("an audit without the security records must fail");
    assert!(e.contains("security commands are not all there"), "{e}");
}

/// Following the log costs what is new: after a full list, one new record writes one row.
fn check_follow_cost(full_rebuild: bool) -> Result<(), String> {
    let mut rig = common::rig_with(&[L], None, |cfg, _| {
        cfg.services.connect.faults.audit_full_rebuild = full_rebuild;
    });
    let book = rig.shell.handles().services().connect.book.clone();
    for i in 0..5000 {
        book.record(
            forge_editor::connect::audit::AuditOrigin::Remote,
            "remote-1",
            "tester",
            "command",
            format!("edit {i}"),
        );
    }
    feed(&mut rig);
    let list = part(&rig, L, &["content", "records"]);
    let written = |rig: &mut Rig| {
        forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| t.rows_written())
            .unwrap_or_default()
    };
    let before = written(&mut rig);
    book.record(
        forge_editor::connect::audit::AuditOrigin::Remote,
        "remote-1",
        "tester",
        "command",
        "one more",
    );
    feed(&mut rig);
    let cost = written(&mut rig) - before;
    let top = rows(&mut rig, list).first().cloned().unwrap_or_default();
    if !top.contains("one more") {
        return Err(format!("the newest record is not on top: {top}"));
    }
    if cost > 2 {
        return Err(format!(
            "one new record cost {cost} row writes (the list holds 5001): the panel re-reads the log"
        ));
    }
    Ok(())
}

#[test]
fn following_the_log_costs_only_what_is_new() {
    check_follow_cost(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_panel_rebuilding_on_every_record_fails() {
    let e = check_follow_cost(true).expect_err("a full rebuild per record must fail");
    assert!(e.contains("row writes"), "{e}");
}
