//! `test_split_editor_parity` (Ch.34 §34.5, DoD M2-16): **the same command sequence produces
//! byte-identical project state whether issued locally or over the wire.**
//!
//! One script — spawns, renames, reparenting, property and setting writes with values chosen
//! to expose any lossy encoding (0.1, 1/3, -0.0, subnormals, 1e-300, a Vec3), a 40-frame
//! gesture committed and one cancelled, removals, a despawn, undo, redo, and a command both
//! sides must refuse — runs twice: through `LocalBus` on one core, and through
//! `forge_remote::RemoteBus` over real QUIC on loopback to a second core, from a device that
//! paired the way a person does. The two projects must serialize to the same bytes (the
//! canonical wire form), with the same key allocator, the same undo history and the same
//! refusals; and the remote client's own replica must be those bytes too.
//!
//! Positive control (W2): `positive_control_a_lossy_wire_fails_parity` sends floats as `f32`
//! (`WireFaults::f32_floats`); parity must fail.

mod common;

use std::time::Duration;

use common::Host;
use common::parity_script::{codes, drive, rows};
use forge_cmd::{EntityKey, Issuer, Project};
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_remote::{ConnectOptions, WireFaults};

/// What a run produced.
struct Outcome {
    bytes: Vec<u8>,
    next_key: EntityKey,
    history: Vec<(String, String)>,
    refused: Vec<String>,
    client_bytes: Option<Vec<u8>>,
}

fn bytes(p: &Project) -> Vec<u8> {
    forge_remote::wire::encode(p).unwrap_or_else(|e| panic!("{e}"))
}

fn local() -> Outcome {
    let core = EditorCore::new();
    let mut c = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    let refused = drive(&mut c, &mut |c| codes(c.pump()));
    Outcome {
        bytes: EditorCore::read(&core, bytes),
        next_key: EditorCore::read(&core, Project::next_key),
        history: rows(c.history()),
        refused,
        client_bytes: None,
    }
}

fn remote(faults: WireFaults) -> Outcome {
    let core = EditorCore::new();
    let mut host = Host::new(core.clone());
    let (device, paired) = host.pair("studio-laptop");
    // Despawn and removals are destructive: a person here lets this device make them.
    host.grant_all(&paired.device_id);
    let mut c = device
        .connect(
            &paired,
            ConnectOptions {
                faults,
                ..ConnectOptions::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let handle = c.handle();
    let refused = drive(&mut c, &mut |c| {
        assert!(
            handle.wait_settled(Duration::from_secs(10)),
            "the host did not answer: {:?}",
            handle.closed()
        );
        codes(c.pump())
    });
    let (client, _) = c.snapshot();
    Outcome {
        bytes: EditorCore::read(&core, bytes),
        next_key: EditorCore::read(&core, Project::next_key),
        history: rows(c.history()),
        refused,
        client_bytes: Some(bytes(&client)),
    }
}

fn check(faults: WireFaults) -> Result<(), String> {
    let a = local();
    let b = remote(faults);
    if a.bytes != b.bytes {
        return Err(format!(
            "PARITY BROKEN: the project issued over the wire differs from the local one ({} vs {} bytes)",
            b.bytes.len(),
            a.bytes.len()
        ));
    }
    if a.next_key != b.next_key {
        return Err(format!(
            "the key allocators differ: {} vs {}",
            a.next_key, b.next_key
        ));
    }
    if a.history != b.history {
        return Err(format!(
            "the undo histories differ:\n{:?}\n{:?}",
            a.history, b.history
        ));
    }
    if a.refused != b.refused {
        return Err(format!(
            "the refusals differ: {:?} vs {:?}",
            a.refused, b.refused
        ));
    }
    if b.client_bytes.as_ref() != Some(&b.bytes) {
        return Err("the remote client's replica is not the host's project".into());
    }
    Ok(())
}

#[test]
fn the_same_commands_give_byte_identical_projects_locally_and_over_the_wire() {
    check(WireFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    let a = local();
    assert_eq!(
        a.refused.len(),
        1,
        "exactly the ghost rename is refused: {:?}",
        a.refused
    );
    assert!(a.refused[0].contains("CMD-0001"), "{:?}", a.refused);
    assert!(a.history.len() >= 10, "{:?}", a.history);
}

#[test]
fn positive_control_a_lossy_wire_fails_parity() {
    let e = check(WireFaults { f32_floats: true })
        .expect_err("floats sent as f32 must break byte-identical parity");
    assert!(e.contains("PARITY BROKEN"), "{e}");
}
