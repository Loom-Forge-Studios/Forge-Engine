//! The split editor's costs on this machine's loopback:
//! `cargo run --release -p forge-remote --example split_measure`.
//!
//! * pairing (TLS 1.3 handshake + SPAKE2 + a person's "allow") and a session's welcome;
//! * a request's round trip (a property set, answered with its event) — what an edit costs
//!   before the host's answer can reconcile it (the prediction shows at once regardless);
//! * bytes on the wire per drag frame (one `SetProperty` up, its `Applied` down), QUIC framing
//!   and TLS included;
//! * the welcome snapshot of a 10,000-entity project (bytes and time).
//!
//! Numbers are recorded in the plan (Ch.34 §34.3, implementation) and ADR 0037.

use std::time::{Duration, Instant};

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::connect::remote::{RemotePairingStore, RemoteTransport};
use forge_editor::core::EditorCore;
use forge_remote::device::wait_for;
use forge_remote::{ConnectOptions, Device, HostConfig, Identity, QuicTransport};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let core = EditorCore::new();
    // A 10,000-entity project, built by an ordinary client.
    let mut setup = EditorCore::connect(&core, Issuer::Test);
    for i in 0..10_000u64 {
        setup.apply(
            EditorCommand::Spawn {
                name: format!("Entity {i}"),
                parent: None,
            },
            None,
        );
        setup.apply(
            EditorCommand::SetProperty {
                entity: EntityKey(i),
                path: "transform.position".into(),
                value: Value::Vec3([i as f64, 0.5, -1.25]),
            },
            None,
        );
    }
    let _ = setup.pump();
    let mut transport = QuicTransport::start(
        core.clone(),
        HostConfig {
            port: 0,
            ..HostConfig::default()
        },
    )?;
    let (mut store, _) = RemotePairingStore::open(None, EditorCore::audit_book(&core));
    transport.attach(&mut store);
    let addr = transport.local_addr().ok_or("no address")?;
    let device = Device::new(Identity::generate("forge-device")?, "bench");

    // Pairing, with the "person" answering as soon as the request is listed.
    let t0 = Instant::now();
    let now = EditorCore::audit_book(&core).now_ms();
    let code = transport.begin_pairing(now).code;
    let d = device.clone();
    let waiting = std::thread::spawn(move || d.pair(addr, &code, Duration::from_secs(10)));
    wait_for(Duration::from_secs(10), || !transport.requests().is_empty());
    let name = transport.answer("bench", now)?;
    let fp = transport.take_verified(&name);
    store.pair_verified(&name, fp.as_deref(), "bench", now)?;
    let host = waiting.join().map_err(|_| "pairing thread")??;
    let pair_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();
    let mut bus = device.connect(&host, ConnectOptions::default())?;
    let welcome_ms = t1.elapsed().as_secs_f64() * 1000.0;
    let h = bus.handle();
    let (tx0, rx0) = h.bytes();
    let snapshot_bytes = forge_remote::wire::encode(&bus.snapshot().0)?.len();

    // Round trips: one property set at a time, each waited for.
    let n = 1000;
    let mut rtts = Vec::with_capacity(n);
    for i in 0..n {
        let t = Instant::now();
        bus.apply(
            EditorCommand::SetProperty {
                entity: EntityKey(7),
                path: "scale".into(),
                value: Value::Float(1.0 + i as f64 * 1e-3),
            },
            None,
        );
        h.wait_settled(Duration::from_secs(5));
        rtts.push(t.elapsed().as_secs_f64() * 1e6);
        let _ = bus.pump();
    }
    rtts.sort_by(f64::total_cmp);
    // A drag: 600 frames in one transaction, bytes per frame.
    let (tx1, rx1) = h.bytes();
    let txn = bus.begin("Drag");
    for i in 0..600 {
        bus.apply(
            EditorCommand::SetProperty {
                entity: EntityKey(9),
                path: "transform.position".into(),
                value: Value::Vec3([i as f64 * 0.01, 1.0, 2.0]),
            },
            Some(txn),
        );
        h.wait_settled(Duration::from_secs(5));
        let _ = bus.pump();
    }
    bus.commit(txn);
    h.wait_settled(Duration::from_secs(5));
    let (tx2, rx2) = h.bytes();

    println!("split editor on loopback (release)");
    println!("  pairing (TLS 1.3 + SPAKE2 + allow):   {pair_ms:.1} ms");
    println!(
        "  session welcome, 10,000 entities:    {welcome_ms:.1} ms, snapshot {:.1} KiB ({} B/entity), {:.1} KiB received",
        snapshot_bytes as f64 / 1024.0,
        snapshot_bytes / 10_000,
        (rx0 as f64) / 1024.0
    );
    println!(
        "  request round trip:                  median {:.0} us, p99 {:.0} us",
        rtts[n / 2],
        rtts[n * 99 / 100]
    );
    println!(
        "  drag frame on the wire:              {:.0} B up, {:.0} B down (QUIC + TLS included)",
        (tx2 - tx1) as f64 / 600.0,
        (rx2 - rx1) as f64 / 600.0
    );
    let _ = (tx0, rx0);
    Ok(())
}
