//! An idle split editor is free (D-5, Ch.21 §21.22): the laptop's editor, connected to a
//! core over QUIC and doing nothing, draws **no frames and wakes never** — the connection's
//! keep-alives run on the QUIC stack's own thread, the host sends a session nothing while
//! nothing changes, and the client wakes its loop only when a message arrives. A real edit on
//! the host then wakes it exactly as a local change would.
//!
//! Positive control (W2): `positive_control_a_heartbeat_wakes_the_idle_editor` makes the host
//! send an empty batch twice a second (`HostFaults::heartbeat`); the idle check must fail.

mod common;

use std::time::Duration;

use common::{Host, remote_rig, shell_config};
use forge_cmd::{EditorCommand, Issuer};
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_remote::HostFaults;

fn check(faults: HostFaults) -> Result<(), String> {
    let mut host = Host::with_faults(EditorCore::new(), faults);
    let (mut rig, _handle, _) = remote_rig(&mut host, shell_config(), "laptop");
    rig.settle();
    rig.advance(Duration::from_secs(2));
    // Let real time pass so anything periodic on the wire would have arrived.
    std::thread::sleep(Duration::from_millis(1600));
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    if (frames, wakeups) != (0, 0) {
        return Err(format!(
            "NOT IDLE: a connected, idle split editor drew {frames} frames and woke {wakeups} times"
        ));
    }
    // A change on the host wakes it, and shows.
    let mut auto = EditorCore::connect(
        &host.core,
        Issuer::Automation {
            session: "s1".into(),
            tool: "apply".into(),
        },
    );
    auto.apply(
        EditorCommand::Spawn {
            name: "FromTheHost".into(),
            parent: None,
        },
        None,
    );
    let shown = forge_remote::device::wait_for(Duration::from_secs(5), || {
        rig.advance(Duration::from_millis(20));
        rig.shell.mirror().len() == 1
    });
    if !shown {
        return Err("a change on the host never reached the idle editor".into());
    }
    Ok(())
}

#[test]
fn an_idle_split_editor_draws_nothing_and_wakes_never() {
    check(HostFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_heartbeat_wakes_the_idle_editor() {
    let e = check(HostFaults {
        heartbeat: true,
        ..HostFaults::default()
    })
    .expect_err("a host that sends a heartbeat must fail the idle check");
    assert!(e.contains("NOT IDLE"), "{e}");
}
