//! `test_reserved_security_settings` (Ch.21 §21.18, WP-U9): **only the security commands
//! write the security settings**, so nothing but a human's security command grants.
//!
//! The grant table follows the project's `security.*` settings (`GrantSync`). The core
//! reserves `security.*` and `plugins.*` on its bus to the security and plugin-set commands
//! (and its own project load). This guard drives the core as an automation session (a
//! non-human client issuing through the bus, what a plugin that hosts sessions over the core
//! gives each of them) and tries every generic path:
//!
//! * a plain `SetSetting` of its own automation grant (with this run's epoch, the value a real
//!   grant holds) — refused, and the grant table does not hold it;
//! * a plain `SetSetting` of a plugin's `Net(Outbound)` grant (no epoch needed) — refused, and
//!   the table does not hold it;
//! * a plain `SetSetting` of a plugin-set key and of the default automation policy — refused;
//! * a direct `forge.project.load` of a document holding a grant, and an ordinary plugin
//!   command writing a grant key — refused with `CMD-0014`;
//! * even a human's plain `SetSetting` of a grant key is refused: a grant is a security
//!   command (human-only, audited, never redone), not a setting edit.
//!
//! Then a human's `forge.automation.grant` does reach the table (the check is not satisfied by
//! a core that refuses everything).
//!
//! Positive control (W2): `positive_control_an_unreserved_core_lets_a_session_grant_itself`
//! runs the same check on a core whose security settings are not reserved; the session's
//! plain `SetSetting` then grants it `Command(Destructive)`, and the check must fail.

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::{EditorCore, SharedCore};
use forge_editor::security;
use forge_plugin::{Capability, CommandClass, NetUse, PluginId, Principal};
use serde_json::{Value as Json, json};

const DESTRUCTIVE: Capability = Capability::Command(CommandClass::Destructive);
const NET: Capability = Capability::Net(NetUse::Outbound);
/// An ordinary plugin command that writes whatever setting it is given.
const WRITE_SETTING: &str = "test.write_setting";
/// The session the checks run as.
const SESSION: &str = "auto-1";

fn session() -> Issuer {
    Issuer::Automation {
        session: SESSION.into(),
        tool: "apply".into(),
    }
}

/// Send `cmd` as `issuer` through the core's client; the refusal's `(code, text)`, if any.
fn send(core: &SharedCore, issuer: Issuer, cmd: EditorCommand) -> Option<(String, String)> {
    let mut c = EditorCore::connect(core, issuer);
    c.apply(cmd, None);
    c.pump().refused.first().map(|r| {
        (
            r.rejection.code().as_str().to_string(),
            r.rejection.to_string(),
        )
    })
}

fn set_setting(key: &str, value: Value) -> EditorCommand {
    EditorCommand::SetSetting {
        key: key.into(),
        value: Some(value),
    }
}

/// The refusal names the reservation (not some other failure of the same command).
fn refused_as_reserved(r: &Option<(String, String)>) -> bool {
    r.as_ref()
        .is_some_and(|(_, t)| t.contains("is reserved to"))
}

fn check(core: &SharedCore) -> Result<(), String> {
    EditorCore::configure(core, |bus| {
        bus.register_handler(
            WRITE_SETTING,
            forge_cmd::CommandPolicy::ORDINARY,
            |b: &mut forge_cmd::DiffBuilder<'_>, a: &Json| {
                let key = a["key"].as_str().unwrap_or_default().to_string();
                b.set_setting(&key, Some(Value::Bool(true)))
            },
        )
    })
    .map_err(|e| e.to_string())?;
    let table = EditorCore::grants(core);
    let me = Principal::Automation(SESSION.into());
    let rivers = Principal::Plugin(PluginId::new("com.example.rivers").map_err(|e| e.to_string())?);
    for name in ["Rock", "Tree"] {
        let spawn = EditorCommand::Spawn {
            name: name.into(),
            parent: None,
        };
        if let Some((_, t)) = send(core, session(), spawn) {
            return Err(format!("the session's ordinary spawn failed: {t}"));
        }
    }
    let before = EditorCore::read(core, forge_cmd::Project::state_hash);

    // 1. The session writes its own destructive grant, with the value a real grant holds.
    let own = security::grant_key(&me, DESTRUCTIVE).ok_or("no grant key")?;
    let r = send(
        core,
        session(),
        set_setting(&own, Value::Text(security::epoch().into())),
    );
    if !refused_as_reserved(&r) {
        return Err(format!(
            "an automation session's SetSetting of {own} was accepted: {r:?}"
        ));
    }
    if table.has(&me, DESTRUCTIVE) {
        return Err(
            "an automation session granted itself Command(Destructive) with a SetSetting".into(),
        );
    }

    // 2. A plugin grant needs no epoch: still refused.
    let net = security::grant_key(&rivers, NET).ok_or("no grant key")?;
    let r = send(core, session(), set_setting(&net, Value::Bool(true)));
    if !refused_as_reserved(&r) || table.has(&rivers, NET) {
        return Err(format!(
            "an automation session granted a plugin Net(Outbound) with a SetSetting: {r:?}"
        ));
    }

    // 3. The plugin set and the default automation policy are reserved too.
    for key in [
        security::plugin_key("com.example.rivers", "enabled"),
        format!(
            "{}.{}",
            security::AUTOMATION_POLICY_PREFIX,
            Capability::Fs(forge_plugin::FsScope::ProjectWrite).key()
        ),
    ] {
        let r = send(core, session(), set_setting(&key, Value::Bool(true)));
        if !refused_as_reserved(&r) {
            return Err(format!(
                "an automation session's SetSetting of {key} was accepted: {r:?}"
            ));
        }
    }

    // 4. Any other command writing a grant key, and a direct load of a document holding one.
    let write = EditorCommand::Invoke {
        target: WRITE_SETTING.into(),
        args: json!({ "key": net }).to_string(),
    };
    match send(core, session(), write) {
        Some((c, _)) if c == "CMD-0014" => {}
        other => return Err(format!("an ordinary command wrote {net}: {other:?}")),
    }
    let mut doc = forge_project::format::ProjectDoc::default();
    doc.settings
        .insert(own.clone(), Value::Text(security::epoch().into()));
    let load = doc.load_command().map_err(|e| e.to_string())?;
    match send(core, session(), load) {
        Some((c, _)) if c == "CMD-0014" => {}
        other => {
            return Err(format!(
                "an automation session's direct project load was not refused: {other:?}"
            ));
        }
    }
    if table.has(&me, DESTRUCTIVE) || table.has(&rivers, NET) {
        return Err("a generic command granted a capability".into());
    }

    // 5. Even a human's plain setting edit is not a grant.
    let human = Issuer::Human { user: "ada".into() };
    let edit = set_setting(&own, Value::Text(security::epoch().into()));
    match send(core, human.clone(), edit) {
        Some((c, _)) if c == "CMD-0014" => {}
        other => {
            return Err(format!(
                "a human's SetSetting of {own} was accepted: {other:?}"
            ));
        }
    }
    if EditorCore::read(core, forge_cmd::Project::state_hash) != before {
        return Err("a refused write changed the project".into());
    }

    // 6. The security command does grant, and the table the core checks holds it.
    if let Some((code, _)) = send(core, human, security::grant_command(&me, DESTRUCTIVE)) {
        return Err(format!("the human's grant was refused: {code}"));
    }
    if !table.has(&me, DESTRUCTIVE) {
        return Err("the human's grant did not reach the grant table".into());
    }
    Ok(())
}

#[test]
fn only_the_security_commands_write_the_security_settings() {
    check(&EditorCore::new()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_unreserved_core_lets_a_session_grant_itself() {
    let core = EditorCore::unreserved_for_control();
    let e = check(&core).expect_err("an unreserved core must fail the check");
    assert!(e.contains("SetSetting"), "{e}");
    // And the escalation is real: the session's own write put the grant in the table.
    assert!(
        EditorCore::grants(&core).has(&Principal::Automation(SESSION.into()), DESTRUCTIVE),
        "the control's SetSetting did not grant"
    );
}
