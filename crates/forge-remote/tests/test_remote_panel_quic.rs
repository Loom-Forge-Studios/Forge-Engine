//! The WP-U9 **Remote** panel (`forge.remote`, Ch.21 §21.21, DoD M2-55) against the **real
//! QUIC transport** (M2-16): the connect / pair dialog, the session list and the latency
//! indicator bind to `forge_remote::QuicTransport` exactly as they bound to the labelled
//! loopback stand-in, and "the other device" is a real device on another thread speaking
//! QUIC over loopback.
//!
//! * the panel says what it listens on — loopback by default — and names the transport;
//! * a device that typed the **wrong** code is listed, and refused when a person presses
//!   Allow: nothing is paired, the refusal is audited;
//! * a device that typed the code on show is paired when a person allows it: the record
//!   carries its certificate fingerprint, and the device pins the host's;
//! * the paired device opens a session: the panel lists it, the latency feed goes live, and
//!   an edit it makes lands in the host's project through the bus (counted on its row);
//! * Disconnect in the panel ends the session; Unpair means the device can no longer connect.
//!
//! Positive control (W2): `positive_control_a_host_that_skips_the_code_check_fails` lists a
//! failed key exchange as verified (`HostFaults::skip_code_check`); the wrong-code device is
//! then paired and the check must fail.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use forge_cmd::EditorCommand;
use forge_editor::client::BusClient;
use forge_editor::connect::remote::RemoteTransport;
use forge_editor::core::EditorCore;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_connect::PanelsConnect;
use forge_panels_core::PanelsCore;
use forge_plugin::SourcePlugin;
use forge_remote::device::wait_for;
use forge_remote::{ConnectOptions, Device, HostConfig, HostFaults, Identity, QuicTransport};
use forge_ui::widgets::{Label, Pressed, SelectionChanged, VirtualTree};
use forge_ui::{Key, WidgetId};

const R: &str = "forge.remote";

fn config() -> ShellConfig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let connect = PanelsConnect::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_connect::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&core, &connect, &stand_in];
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &all, &[], None).unwrap_or_else(|e| panic!("{e}"))
}

fn at(rig: &Rig, path: &[&str]) -> WidgetId {
    let mut id = rig
        .panel_frame(R)
        .unwrap_or_else(|| panic!("{R} is not open"));
    for k in std::iter::once(&"content").chain(path) {
        id = id.child(&Key::Str((*k).into()));
    }
    assert!(rig.h.ui.contains(id), "no widget at {path:?}");
    id
}

fn tap(rig: &mut Rig, path: &[&str]) {
    let id = at(rig, path);
    rig.h.ui.raise(id, Pressed(id));
    rig.turn();
    rig.settle();
}

fn label(rig: &Rig, path: &[&str]) -> String {
    let id = at(rig, path);
    rig.h
        .ui
        .widget::<Label>(id)
        .map(|l| l.text(rig.h.ui.rt()))
        .unwrap_or_default()
}

fn rows(rig: &mut Rig, path: &[&str]) -> Vec<String> {
    let list = at(rig, path);
    VirtualTree::edit(&mut rig.h.ui, list, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r).and_then(|k| t.label_of(k)).map(str::to_string))
            .collect()
    })
    .unwrap_or_default()
}

fn select(rig: &mut Rig, path: &[&str], needle: &str) {
    let list = at(rig, path);
    let key = VirtualTree::edit(&mut rig.h.ui, list, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r))
            .find(|k| t.label_of(*k).is_some_and(|l| l.contains(needle)))
    })
    .flatten()
    .unwrap_or_else(|| panic!("no row contains {needle:?}"));
    VirtualTree::edit(&mut rig.h.ui, list, |t| t.select(&[key]));
    rig.h.ui.raise(
        list,
        SelectionChanged {
            view: list,
            keys: vec![key],
        },
    );
    rig.turn();
    rig.settle();
}

/// Let the live feeds deliver (they refresh at most twice a second).
fn feed(rig: &mut Rig) {
    rig.advance(Duration::from_millis(1200));
    rig.settle();
}

type Transport = Rc<RefCell<QuicTransport>>;

fn rig(faults: HostFaults) -> (Rig, Transport) {
    let core = EditorCore::new();
    let transport = QuicTransport::start(
        core.clone(),
        HostConfig {
            port: 0,
            faults,
            ..HostConfig::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut cfg = config();
    cfg.services.connect.attach(&core, None);
    transport.attach(&mut cfg.services.connect.pairing.borrow_mut());
    let transport = Rc::new(RefCell::new(transport));
    cfg.services.connect.remote = transport.clone();
    let mut rig = Rig::with_core(cfg, core).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&[R]).unwrap_or_else(|e| panic!("{e}"));
    (rig, transport)
}

fn device(name: &str) -> Device {
    Device::new(
        Identity::generate("forge-device").unwrap_or_else(|e| panic!("{e}")),
        name,
    )
}

/// The code the panel shows after "Show pairing code".
fn show_code(rig: &mut Rig) -> String {
    tap(rig, &["pairing_group", "pairing_bar", "show_code"]);
    let line = label(rig, &["pairing_group", "code"]);
    let code = line
        .split_whitespace()
        .nth(2)
        .unwrap_or_default()
        .to_string();
    assert_eq!(code.len(), 6, "{line}");
    code
}

/// A wrong-code device asks and a person presses Allow: `Err` if it got paired.
fn check_wrong_code(faults: HostFaults) -> Result<(), String> {
    let (mut rig, transport) = rig(faults);
    let code = show_code(&mut rig);
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let addr = transport
        .borrow()
        .local_addr()
        .unwrap_or_else(|| panic!("listens"));
    let intruder = device("intruder");
    let d = intruder.clone();
    let t = std::thread::spawn(move || d.pair(addr, wrong, Duration::from_secs(5)));
    assert!(wait_for(Duration::from_secs(10), || !transport
        .borrow()
        .requests()
        .is_empty()));
    feed(&mut rig);
    select(&mut rig, &["pairing_group", "requests"], "intruder");
    tap(&mut rig, &["pairing_group", "requests_bar", "allow"]);
    let device_side = t.join().unwrap_or_else(|_| panic!("device thread"));
    let paired = rig
        .shell
        .handles()
        .services()
        .connect
        .pairing
        .borrow()
        .devices()
        .len();
    if paired > 0 {
        return Err(format!(
            "a device with the wrong code was paired ({paired} device(s); the device saw {device_side:?})"
        ));
    }
    if device_side.is_ok() {
        return Err("the device believes it paired with a wrong code".into());
    }
    Ok(())
}

#[test]
fn a_wrong_code_pairs_nothing_over_quic() {
    check_wrong_code(HostFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_host_that_skips_the_code_check_fails() {
    let e = check_wrong_code(HostFaults {
        skip_code_check: true,
        ..HostFaults::default()
    })
    .expect_err("a host that lists a failed exchange as verified must fail");
    assert!(e.contains("wrong code was paired"), "{e}");
}

#[test]
fn the_remote_panel_pairs_lists_and_disconnects_over_quic() {
    let (mut rig, transport) = rig(HostFaults::default());
    let port = transport
        .borrow()
        .local_addr()
        .unwrap_or_else(|| panic!("listens"))
        .port();
    let exposure = label(&rig, &["exposure_group", "exposure"]);
    assert!(
        exposure.starts_with(&format!("Listening on 127.0.0.1:{port} ")),
        "loopback by default, on the real port: {exposure}"
    );
    let footer = label(&rig, &["footer"]);
    assert!(footer.contains("QUIC over TLS 1.3"), "{footer}");
    assert!(!footer.contains("UNBUILT"), "{footer}");

    // Pair a device that typed the code on show; a person here allows it.
    let code = show_code(&mut rig);
    let addr = transport
        .borrow()
        .local_addr()
        .unwrap_or_else(|| panic!("listens"));
    let laptop = device("studio-laptop");
    let d = laptop.clone();
    let t = std::thread::spawn(move || d.pair(addr, &code, Duration::from_secs(30)));
    assert!(wait_for(Duration::from_secs(10), || !transport
        .borrow()
        .requests()
        .is_empty()));
    feed(&mut rig);
    assert_eq!(
        rows(&mut rig, &["pairing_group", "requests"]),
        ["\u{201c}studio-laptop\u{201d} asks to pair"]
    );
    select(&mut rig, &["pairing_group", "requests"], "studio-laptop");
    tap(&mut rig, &["pairing_group", "requests_bar", "allow"]);
    let host = t
        .join()
        .unwrap_or_else(|_| panic!("device thread"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(host.fingerprint, transport.borrow().fingerprint());
    let services = rig.shell.handles().services_rc();
    let record = services.connect.pairing.borrow().devices()[0].clone();
    assert_eq!(record.fingerprint, laptop.identity().fingerprint());
    assert_eq!(record.id, host.device_id);
    let devices = rows(&mut rig, &["devices_group", "devices"]);
    assert!(
        devices
            .iter()
            .any(|l| l.contains("\u{201c}studio-laptop\u{201d}") && l.contains("allowed by tester")),
        "{devices:?}"
    );

    // The device opens a session: listed, latency live, and its edit lands in the host's
    // project through the bus.
    let mut bus = laptop
        .connect(&host, ConnectOptions::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let handle = bus.handle();
    bus.apply(
        EditorCommand::Spawn {
            name: "FromLaptop".into(),
            parent: None,
        },
        None,
    );
    assert!(handle.wait_settled(Duration::from_secs(10)));
    let p = bus.pump();
    assert!(p.refused.is_empty(), "{:?}", p.refused);
    let names: Vec<String> = EditorCore::read(&rig.core, |p| {
        p.entities().map(|(_, e)| e.name().to_string()).collect()
    });
    assert_eq!(names, ["FromLaptop"]);
    assert!(
        transport.borrow().latency().ms().is_some(),
        "latency is measured"
    );
    feed(&mut rig);
    let sessions = rows(&mut rig, &["sessions_group", "sessions"]);
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert!(
        sessions[0].contains("from studio-laptop") && sessions[0].contains("1 command(s)"),
        "{sessions:?}"
    );
    // The mirror of the host's own editor shows the laptop's edit, issued as the laptop.
    let tag = rig
        .shell
        .mirror()
        .history()
        .last()
        .map(|h| h.issuer_tag())
        .unwrap_or_default();
    assert_eq!(tag, format!("human:studio-laptop@{}", host.device_id));

    // Disconnect from the panel.
    select(&mut rig, &["sessions_group", "sessions"], "studio-laptop");
    tap(&mut rig, &["sessions_group", "disconnect"]);
    assert!(wait_for(Duration::from_secs(5), || handle
        .closed()
        .is_some()));
    assert!(rows(&mut rig, &["sessions_group", "sessions"]).is_empty());
    drop(bus);

    // Unpair: the device can no longer connect.
    select(&mut rig, &["devices_group", "devices"], "studio-laptop");
    tap(&mut rig, &["devices_group", "unpair"]);
    let again = laptop.connect(&host, ConnectOptions::default());
    assert!(
        matches!(again, Err(forge_remote::RemoteError::Refused(_))),
        "{:?}",
        again.map(|_| ())
    );
    let events: Vec<String> = services
        .connect
        .book
        .query(&forge_editor::connect::audit::AuditQuery::parse(
            "origin:remote",
        ))
        .into_iter()
        .map(|r| r.event)
        .collect();
    for e in [
        "paired",
        "connected",
        "disconnected",
        "unpaired",
        "session_refused",
    ] {
        assert!(events.iter().any(|x| x == e), "{e} not audited: {events:?}");
    }
}
