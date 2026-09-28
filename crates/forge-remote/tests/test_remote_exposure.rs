//! Exposure follows the pairing store (Ch.34 §34.5): the host listens on loopback by default,
//! the store's confirmed LAN exposure moves the endpoint to the LAN address **on the same
//! port** (so a paired device's address stays valid), and back to loopback moves it back.
//! The test's "LAN" is a second loopback address (`127.0.0.2`): the rebind is exercised
//! without opening a port to the network.
//!
//! Positive control (W2): `positive_control_a_host_that_ignores_exposure_fails` never attaches
//! the store; the endpoint then stays where it started and the check must fail.

mod common;

use std::net::{Ipv4Addr, SocketAddr};

use forge_editor::connect::remote::{Exposure, RemotePairingStore};
use forge_editor::core::EditorCore;
use forge_remote::{HostConfig, QuicTransport};

fn check(attach: bool) -> Result<(), String> {
    let core = EditorCore::new();
    let lan = Ipv4Addr::new(127, 0, 0, 2);
    let t = QuicTransport::start(
        core.clone(),
        HostConfig {
            port: 0,
            lan_ip: lan,
            ..HostConfig::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let (mut store, _) = RemotePairingStore::open(None, EditorCore::audit_book(&core));
    if attach {
        t.attach(&mut store);
    }
    let at = t.local_addr().ok_or("no address")?;
    if at.ip() != std::net::IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(format!("not loopback by default: {at}"));
    }
    store
        .set_exposure(Exposure::Lan, true, "tester")
        .map_err(|e| e.to_string())?;
    let lan_at = t.local_addr().ok_or("no address")?;
    if lan_at != SocketAddr::from((lan, at.port())) {
        return Err(format!(
            "EXPOSURE NOT FOLLOWED: LAN exposure should listen on {lan}:{}, listens on {lan_at}",
            at.port()
        ));
    }
    store
        .set_exposure(Exposure::Loopback, false, "tester")
        .map_err(|e| e.to_string())?;
    let back = t.local_addr().ok_or("no address")?;
    if back != at {
        return Err(format!(
            "back to loopback should listen on {at}, listens on {back}"
        ));
    }
    Ok(())
}

#[test]
fn exposure_follows_the_pairing_store_on_the_same_port() {
    check(true).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_host_that_ignores_exposure_fails() {
    let e = check(false).expect_err("a host that ignores the exposure must fail");
    assert!(e.contains("EXPOSURE NOT FOLLOWED"), "{e}");
}
