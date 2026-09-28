//! `test_remote_connect` (Ch.21 §21.21 "Remote connect", Ch.34; DoD M2-55), headless over the
//! labelled in-memory loopback transport (D-4 until `forge-remote`, M2-16) and the real
//! `RemotePairingStore`:
//!
//! * **pairing needs a person on both devices**: this editor shows a code, the other device
//!   asks with the code its person typed, a person here allows it; the device is then in the
//!   user-config pairing file (never the project) and in the audit log; a wrong code is
//!   refused, nothing is written;
//! * **loopback by default**; LAN exposure asks first, and cancelling changes nothing;
//! * remote sessions are listed; the **latency** readout is a live feed: with a producer at
//!   100 Hz it refreshes at most twice a second while visible, and a hidden panel costs 0
//!   frames and 0 wakeups.
//!
//! Positive control (W2): `positive_control_an_uncapped_latency_readout_fails` registers the
//! latency feed at 60 Hz; the ≤ 2 Hz check must fail.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use common::{feed, part, press, rows, select_containing};
use forge_editor::connect::audit::AuditQuery;
use forge_editor::connect::remote::{Exposure, LoopbackTransport, PAIRING_FILE, RemoteTransport};
use forge_editor::testing::Rig;
use forge_ui::dock::{Axis, DockNode, Layout};

const R: &str = "forge.remote";

fn at(rig: &Rig, path: &[&str]) -> forge_ui::WidgetId {
    let mut p = vec!["content"];
    p.extend_from_slice(path);
    part(rig, R, &p)
}

fn tap(rig: &mut Rig, path: &[&str]) {
    let id = at(rig, path);
    press(rig, id);
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("forge-panels-connect-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

type Loop = Rc<RefCell<LoopbackTransport>>;

/// A rig whose remote transport is a loopback the test drives as "the other device".
fn rig_with_loopback(
    panels: &[&str],
    dir: Option<&std::path::Path>,
    uncapped: bool,
) -> (Rig, Loop) {
    let lb: Loop = Rc::new(RefCell::new(LoopbackTransport::new(7)));
    let t = lb.clone();
    let rig = common::rig_with(panels, dir, move |cfg, _| {
        cfg.services.connect.remote = t;
        cfg.services.connect.faults.latency_uncapped = uncapped;
    });
    (rig, lb)
}

#[test]
fn pairing_needs_a_person_on_both_devices_and_is_audited_user_config() {
    let dir = tmp("pair");
    let (mut rig, device) = rig_with_loopback(&[R], Some(&dir), false);
    let exposure = common::label(&rig, at(&rig, &["exposure_group", "exposure"]));
    assert!(
        exposure.starts_with("Listening on 127.0.0.1:"),
        "{exposure}"
    );
    // Show a code.
    tap(&mut rig, &["pairing_group", "pairing_bar", "show_code"]);
    let code_line = common::label(&rig, at(&rig, &["pairing_group", "code"]));
    let code: String = code_line
        .split_whitespace()
        .nth(2)
        .unwrap_or_default()
        .to_string();
    assert_eq!(code.len(), 6, "{code_line}");
    let before = rig.state_hash();
    // A device asks with the wrong code: refused, nothing written.
    device.borrow_mut().device_requests("tablet", "000000");
    feed(&mut rig);
    let requests = at(&rig, &["pairing_group", "requests"]);
    select_containing(&mut rig, requests, "tablet");
    tap(&mut rig, &["pairing_group", "requests_bar", "allow"]);
    assert!(
        !dir.join(PAIRING_FILE).exists(),
        "a wrong code must not pair anything"
    );
    // The right code, allowed here.
    device.borrow_mut().device_requests("studio-laptop", &code);
    feed(&mut rig);
    select_containing(&mut rig, requests, "studio-laptop");
    tap(&mut rig, &["pairing_group", "requests_bar", "allow"]);
    let devices = at(&rig, &["devices_group", "devices"]);
    let r = rows(&mut rig, devices);
    assert!(
        r.iter().any(|l| l.contains("\u{201c}studio-laptop\u{201d}")
            && l.contains("allowed by tester")),
        "{r:?}"
    );
    let file = std::fs::read_to_string(dir.join(PAIRING_FILE)).unwrap_or_default();
    assert!(file.contains("studio-laptop"), "{file}");
    assert_eq!(rig.state_hash(), before, "pairing is never project state");
    let services = rig.shell.handles().services_rc();
    let ev: Vec<String> = services
        .connect
        .book
        .query(&AuditQuery::parse("origin:remote user:tester"))
        .into_iter()
        .map(|r| r.event)
        .collect();
    assert_eq!(ev, ["pairing_denied", "paired"]);
    // LAN: asks first; cancel changes nothing; confirm exposes.
    tap(&mut rig, &["exposure_group", "exposure_bar", "expose_lan"]);
    let confirm = at(&rig, &["exposure_group", "lan_confirm"]);
    assert!(!rig.h.ui.is_hidden(confirm));
    tap(
        &mut rig,
        &["exposure_group", "lan_confirm", "lan_bar", "lan_no"],
    );
    assert_eq!(
        services.connect.pairing.borrow().exposure(),
        Exposure::Loopback
    );
    tap(&mut rig, &["exposure_group", "exposure_bar", "expose_lan"]);
    tap(
        &mut rig,
        &["exposure_group", "lan_confirm", "lan_bar", "lan_yes"],
    );
    assert_eq!(services.connect.pairing.borrow().exposure(), Exposure::Lan);
    let exposure = common::label(&rig, at(&rig, &["exposure_group", "exposure"]));
    assert!(exposure.contains("LAN"), "{exposure}");
    // The pairing survives a restart: a new store over the same directory reads it back.
    let (again, err) = forge_editor::connect::remote::RemotePairingStore::open(
        Some(&dir),
        forge_editor::connect::audit::AuditBook::default(),
    );
    assert!(err.is_none(), "{err:?}");
    assert_eq!(again.devices().len(), 1);
    assert_eq!(again.exposure(), Exposure::Lan);
    // A session from the paired device is listed, and can be disconnected.
    device.borrow_mut().device_connects("studio-laptop", 5);
    feed(&mut rig);
    let sessions = at(&rig, &["sessions_group", "sessions"]);
    select_containing(&mut rig, sessions, "remote-1");
    tap(&mut rig, &["sessions_group", "disconnect"]);
    assert!(rows(&mut rig, sessions).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Measure the latency at 100 Hz for `secs`: (frames drawn, wakeups).
fn produce(rig: &mut Rig, device: &Loop, secs: u64) -> (u64, u64) {
    let (f0, w0) = (rig.h.frames, rig.h.wakeups);
    let feed = device.borrow().latency();
    feed.set_live(true);
    for i in 0..secs * 100 {
        feed.record(8.0 + (i % 13) as f64);
        rig.advance(Duration::from_millis(10));
    }
    feed.set_live(false);
    (rig.h.frames - f0, rig.h.wakeups - w0)
}

fn check_rate(uncapped: bool) -> Result<(), String> {
    let (mut rig, device) = rig_with_loopback(&[R], None, uncapped);
    rig.advance(Duration::from_secs(6));
    let (frames, _) = produce(&mut rig, &device, 10);
    if frames > 2 * 10 + 1 {
        return Err(format!(
            "the latency readout drew {frames} frames in 10 s (cap 2 Hz)"
        ));
    }
    if frames < 5 {
        return Err(format!(
            "the latency readout drew only {frames} frames in 10 s"
        ));
    }
    Ok(())
}

#[test]
fn the_latency_readout_refreshes_at_most_twice_a_second() {
    check_rate(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_uncapped_latency_readout_fails() {
    let e = check_rate(true).expect_err("an uncapped latency readout must fail the 2 Hz cap");
    assert!(e.contains("cap 2 Hz"), "{e}");
}

/// Hidden: a busy link and a pairing request cost nothing; shown again, the panel catches
/// up in the same settle (no waiting for an unrelated wake).
fn check_hidden(visibility_late: bool) -> Result<(), String> {
    let (mut rig, device) = rig_with_loopback(&[R], None, false);
    rig.h.ui.faults.feeds.visibility_on_next_frame = visibility_late;
    rig.shell.dock_mut().set_layout(Layout::new(DockNode::split(
        Axis::Horizontal,
        vec![(1.0, DockNode::tabs(&[R, "forge.console"]))],
    )));
    rig.settle();
    let mut l = rig.shell.layout().clone();
    let _ = l.activate(&forge_ui::dock::PanelId::new("forge.console"));
    rig.shell.dock_mut().set_layout(l);
    rig.advance(Duration::from_secs(6));
    let (frames, wakeups) = produce(&mut rig, &device, 5);
    if (frames, wakeups) != (0, 0) {
        return Err(format!(
            "a hidden panel with a busy link: {frames} frames, {wakeups} wakeups"
        ));
    }
    device.borrow_mut().device_requests("phone", "123456");
    let (frames, wakeups) = rig.advance(Duration::from_secs(2));
    if (frames, wakeups) != (0, 0) {
        return Err(format!(
            "a request while hidden: {frames} frames, {wakeups} wakeups"
        ));
    }
    let mut l = rig.shell.layout().clone();
    let _ = l.activate(&forge_ui::dock::PanelId::new(R));
    rig.shell.dock_mut().set_layout(l);
    rig.settle();
    let requests = at(&rig, &["pairing_group", "requests"]);
    let r = rows(&mut rig, requests);
    if r != ["\u{201c}phone\u{201d} asks to pair"] {
        return Err(format!("shown again, the panel did not catch up: {r:?}"));
    }
    Ok(())
}

#[test]
fn a_hidden_remote_panel_costs_nothing_and_catches_up_when_shown() {
    check_hidden(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_feeds_noticing_visibility_late_fail() {
    let e = check_hidden(true).expect_err("a panel that catches up late must fail");
    assert!(e.contains("did not catch up"), "{e}");
}

/// A device typing a code that is not the one on show is refused even when a person here
/// presses Allow: nothing is paired or written, and the refusal is audited.
fn check_wrong_code(accept_any_code: bool) -> Result<(), String> {
    let dir = tmp(if accept_any_code {
        "wrong-ctl"
    } else {
        "wrong"
    });
    let (mut rig, device) = rig_with_loopback(&[R], Some(&dir), false);
    device.borrow_mut().accept_any_code = accept_any_code;
    tap(&mut rig, &["pairing_group", "pairing_bar", "show_code"]);
    device.borrow_mut().device_requests("intruder", "999999");
    feed(&mut rig);
    let requests = at(&rig, &["pairing_group", "requests"]);
    select_containing(&mut rig, requests, "intruder");
    tap(&mut rig, &["pairing_group", "requests_bar", "allow"]);
    let paired = rig
        .shell
        .handles()
        .services()
        .connect
        .pairing
        .borrow()
        .devices()
        .len();
    let written = dir.join(PAIRING_FILE).exists();
    let _ = std::fs::remove_dir_all(&dir);
    if paired > 0 || written {
        return Err(format!(
            "a device with the wrong code was paired ({paired} device(s), file written: {written})"
        ));
    }
    Ok(())
}

#[test]
fn a_wrong_pairing_code_pairs_nothing() {
    check_wrong_code(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_transport_accepting_any_code_fails() {
    let e = check_wrong_code(true).expect_err("a transport that ignores the code must fail");
    assert!(e.contains("wrong code was paired"), "{e}");
}
