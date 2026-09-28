//! A remote editor session is a **peer, never a privileged path** (Ch.34 §34.5, ADR 0036,
//! E-27): every request is checked in the core's one grant table as
//! `Principal::Remote(<device>)` with the classes every non-human principal is checked with, and
//! then the bus's own policies apply.
//!
//! * A project load (it replaces the security settings too) is never taken from a remote
//!   session, whatever it is granted.
//! * A newly paired device reads the project and sends ordinary commands; a destructive one
//!   (a despawn) and a security-class one (a plugin grant) are refused before the bus, and
//!   audited, until a person here grants `Command(Destructive)`.
//! * With that grant the bus's policies still apply: a plain `SetSetting` of a reserved
//!   `security.*` key is refused (`CMD-0014`), as it is for a local person.
//! * A session may commit or cancel only transactions it opened (not an automation session's
//!   preview).
//! * Revoking the grant reaches the open session at its next request.
//!
//! Positive control (W2): `positive_control_a_host_without_the_grant_check_fails` lets remote
//! requests skip the grant check (`HostFaults::skip_grant_check`); the remote project load
//! (and then the despawn) goes through and the check must fail.

mod common;

use std::time::Duration;

use common::Host;
use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::{BusClient, Pumped};
use forge_editor::connect::audit::AuditQuery;
use forge_editor::core::EditorCore;
use forge_remote::{ConnectOptions, HostFaults, RemoteBus, RemoteHandle};

fn answered(c: &mut RemoteBus, h: &RemoteHandle) -> Pumped {
    assert!(
        h.wait_settled(Duration::from_secs(10)),
        "no answer: {:?}",
        h.closed()
    );
    c.pump()
}

fn codes(p: &Pumped) -> Vec<String> {
    p.refused
        .iter()
        .map(|r| r.rejection.code().as_str().to_string())
        .collect()
}

fn names(core: &forge_editor::core::SharedCore) -> Vec<String> {
    EditorCore::read(core, |p| {
        p.entities().map(|(_, e)| e.name().to_string()).collect()
    })
}

fn check(faults: HostFaults) -> Result<(), String> {
    let core = EditorCore::new();
    let mut host = Host::with_faults(core.clone(), faults);
    let (device, paired) = host.pair("laptop");
    let mut c = device
        .connect(&paired, ConnectOptions::default())
        .map_err(|e| e.to_string())?;
    let h = c.handle();

    // The project load replaces everything, security settings included: never from a remote
    // session, whatever it holds (its issuer is a person, so the core's own non-human check
    // would not stop it).
    let key = format!(
        "security.grants.plugin.{}.{}",
        forge_editor::security::encode_id("com.example.x"),
        forge_plugin::Capability::Net(forge_plugin::NetUse::Outbound).key()
    );
    let doc = forge_project::format::ProjectDoc {
        settings: [(key.clone(), Value::Bool(true))].into_iter().collect(),
        entities: Vec::new(),
    };
    c.apply(doc.load_command().map_err(|e| e.to_string())?, None);
    let p = answered(&mut c, &h);
    if EditorCore::read(&core, |p| p.setting(&key).is_some()) {
        return Err(
            "GRANT BYPASSED: a remote device loaded a project document carrying a plugin grant"
                .into(),
        );
    }
    if codes(&p) != ["CMD-0014"] {
        return Err(format!("the remote load's refusal: {:?}", codes(&p)));
    }

    // Ordinary: allowed.
    c.apply(
        EditorCommand::Spawn {
            name: "Cube".into(),
            parent: None,
        },
        None,
    );
    let p = answered(&mut c, &h);
    if !p.refused.is_empty() || names(&core) != ["Cube"] {
        return Err(format!("an ordinary spawn was refused: {:?}", codes(&p)));
    }
    // Destructive and security-class: refused before the bus, and audited.
    c.apply(
        EditorCommand::Despawn {
            entity: EntityKey(0),
        },
        None,
    );
    let p = answered(&mut c, &h);
    if names(&core) != ["Cube"] {
        return Err(
            "GRANT BYPASSED: a device without Command(Destructive) despawned an entity".into(),
        );
    }
    if codes(&p) != ["CMD-0014"] {
        return Err(format!("the despawn's refusal: {:?}", codes(&p)));
    }
    let grant = EditorCommand::Invoke {
        target: forge_editor::security::PLUGIN_GRANT_CMD.into(),
        args:
            serde_json::json!({"principal": "plugin:com.example.x", "capability": "Net(Outbound)"})
                .to_string(),
    };
    c.apply(grant.clone(), None);
    let p = answered(&mut c, &h);
    if codes(&p) != ["CMD-0014"] {
        return Err(format!("the security command's refusal: {:?}", codes(&p)));
    }
    let denied = EditorCore::audit_book(&core)
        .query(&AuditQuery::parse("origin:remote"))
        .into_iter()
        .filter(|r| r.event == "denied")
        .count();
    if denied != 3 {
        return Err(format!("{denied} denials audited, not 3"));
    }

    // A person here grants everything: the bus's own policies still hold.
    host.grant_all(&paired.device_id);
    c.apply(
        EditorCommand::SetSetting {
            key: "security.grants.plugin.id_x.net_outbound".into(),
            value: Some(Value::Bool(true)),
        },
        None,
    );
    let p = answered(&mut c, &h);
    if codes(&p) != ["CMD-0014"] {
        return Err(format!(
            "a plain write of a reserved security key was not refused by the bus: {:?}",
            codes(&p)
        ));
    }
    c.apply(
        EditorCommand::Despawn {
            entity: EntityKey(0),
        },
        None,
    );
    let p = answered(&mut c, &h);
    if !p.refused.is_empty() || !names(&core).is_empty() {
        return Err(format!("the granted despawn: {:?}", codes(&p)));
    }

    // Only its own transactions: an automation session's open preview cannot be committed from
    // here.
    let mut auto = EditorCore::connect(
        &core,
        Issuer::Automation {
            session: "s9".into(),
            tool: "apply".into(),
        },
    );
    let t = auto.begin("automation preview");
    auto.apply(
        EditorCommand::Spawn {
            name: "Preview".into(),
            parent: None,
        },
        Some(t),
    );
    c.commit(t);
    let p = answered(&mut c, &h);
    if codes(&p) != ["CMD-0014"] || auto.txn_state(t) != Some(forge_cmd::TxnState::Open) {
        return Err(format!("another issuer's transaction: {:?}", codes(&p)));
    }

    // A revoke reaches the open session at its next request.
    let id = paired.device_id.clone();
    host.store
        .set_capabilities(
            &id,
            &forge_editor::connect::remote::default_remote_capabilities(),
            "tester",
        )
        .map_err(|e| e.to_string())?;
    c.apply(
        EditorCommand::SetSetting {
            key: "editor.grid".into(),
            value: None,
        },
        None,
    );
    let p = answered(&mut c, &h);
    if codes(&p) != ["CMD-0014"] {
        return Err(format!("a revoked grant still worked: {:?}", codes(&p)));
    }
    Ok(())
}

#[test]
fn a_remote_session_goes_through_the_grant_table_and_the_bus_policy() {
    check(HostFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_host_without_the_grant_check_fails() {
    let e = check(HostFaults {
        skip_grant_check: true,
        ..HostFaults::default()
    })
    .expect_err("a host that skips the grant check must fail");
    assert!(e.contains("GRANT BYPASSED"), "{e}");
}
