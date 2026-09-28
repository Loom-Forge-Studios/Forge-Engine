//! `test_collab_sync_not_sendable` (WP-34, CMD-0014; Ch.37 §37.8, Ch.21 §21.18; gate row
//! `C-collab-sync-core-only`): **no client — an automation session above all — can send
//! `forge.collab.sync` itself.** A permanent regression guard.
//!
//! The team pull's patch (`forge.collab.sync`) is `PERFORMED_POLICY` since WP-21 (so an
//! automation session's pull can apply it as that session), and it is a reserved writer of
//! `security.*` and `plugins.*` (a baseline carries the team's plugin grants). So the only
//! thing between a non-human client and writing any plugin grant it likes is that the core
//! refuses the target from every client (`collab::CORE_ONLY`): the core alone issues it,
//! after checking a pull and holding what it would widen. Here an automation session that
//! holds every capability a person can grant (`Command(Destructive)` included) sends a sync
//! whose patch grants a plugin `Fs(ProjectWrite)`:
//!
//! * through a core client of its own on every path a client has: `apply`, `preview`,
//!   `apply_batch` and `preview_batch`;
//! * and a person's client is refused too (no client sends it).
//!
//! Each is refused, nothing is applied, the grant is not in effect and the setting is absent.
//!
//! Positive control (W2): `positive_control_a_sendable_sync_lets_a_session_grant` — a core
//! that lets a client send its core-only targets (`CoreFaults::core_only_sendable`): the
//! session's sync applies and the plugin holds the grant.

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::{CoreFaults, EditorCore, SharedCore};
use forge_editor::security;
use forge_plugin::{Capability, CommandClass, FsScope, PluginId, Principal};
use forge_project::merge::{PatchOp, SYNC_CMD, sync_args};

const WRITE: Capability = Capability::Fs(FsScope::ProjectWrite);
/// The automation session the checks run as.
const SESSION: &str = "auto-1";
fn rivers() -> Result<Principal, String> {
    PluginId::new("com.example.rivers")
        .map(Principal::Plugin)
        .map_err(|e| e.to_string())
}

/// A sync whose patch grants plugin `rivers` `Fs(ProjectWrite)`.
fn sync() -> Result<EditorCommand, String> {
    let key = security::grant_key(&rivers()?, WRITE).ok_or("no grant key")?;
    Ok(EditorCommand::Invoke {
        target: SYNC_CMD.into(),
        args: sync_args(
            &[PatchOp::Setting {
                key,
                before: None,
                after: Some(Value::Bool(true)),
            }],
            None,
            "a baseline of my own",
        )
        .to_string(),
    })
}

fn granted(core: &SharedCore) -> Result<bool, String> {
    let key = security::grant_key(&rivers()?, WRITE).ok_or("no grant key")?;
    Ok(EditorCore::grants(core).has(&rivers()?, WRITE)
        || EditorCore::read(core, |p| p.setting(&key).is_some()))
}

fn check(faults: CoreFaults) -> Result<(), String> {
    let core = EditorCore::new();
    EditorCore::set_faults(&core, faults);
    // The person trusts the session with everything a person can grant.
    let me = Principal::Automation(SESSION.into());
    let mut person = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    for cap in [
        Capability::Command(CommandClass::Ordinary),
        Capability::Command(CommandClass::Destructive),
        WRITE,
    ] {
        person.apply(security::grant_command(&me, cap), None);
    }
    if let Some(r) = person.pump().refused.first() {
        return Err(format!("setup grant: {}", r.rejection));
    }
    // Through a core client of its own, on every path.
    let mut client = EditorCore::connect(
        &core,
        Issuer::Automation {
            session: SESSION.into(),
            tool: "apply".into(),
        },
    );
    client.apply(sync()?, None);
    if client.pump().refused.is_empty() || granted(&core)? {
        return Err(format!(
            "an automation session's client applied {SYNC_CMD} and granted a plugin"
        ));
    }
    if client.preview(&sync()?).is_ok() {
        return Err(format!(
            "an automation session's client previewed {SYNC_CMD}"
        ));
    }
    if client.preview_batch(&[sync()?]).is_ok() {
        return Err(format!(
            "an automation session's client previewed a batch with {SYNC_CMD}"
        ));
    }
    let cover = EditorCommand::Spawn {
        name: "Cover".into(),
        parent: None,
    };
    if client.apply_batch(vec![cover, sync()?], None, None).is_ok() || granted(&core)? {
        return Err(format!(
            "an automation session's client applied a batch with {SYNC_CMD}"
        ));
    }
    // No client at all: a person's is refused too.
    person.apply(sync()?, None);
    let refused = person.pump().refused;
    let why = refused
        .first()
        .map(|r| r.rejection.to_string())
        .ok_or_else(|| format!("a person's client applied {SYNC_CMD}"))?;
    if !why.contains("performed by the core") || granted(&core)? {
        return Err(format!(
            "a person's {SYNC_CMD} was refused for another reason: {why}"
        ));
    }
    Ok(())
}

#[test]
fn no_client_can_send_the_team_sync() {
    check(CoreFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_sendable_sync_lets_a_session_grant() {
    let e = check(CoreFaults {
        core_only_sendable: true,
        ..CoreFaults::default()
    })
    .expect_err("a core that lets clients send its sync must fail the check");
    assert!(
        e.contains("client applied forge.collab.sync and granted a plugin"),
        "{e}"
    );
}
