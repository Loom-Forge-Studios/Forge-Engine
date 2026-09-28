//! `test_reserved_team_settings` (Ch.37 §37.7, Ch.21 §21.18; WP-U10, ADR 0039): **team state
//! changes only through its audited commands**, never a plain `SetSetting`.
//!
//! The team binding (`team.*`) and the review rules (`collab.*`) are project settings. The
//! core reserves both prefixes on its bus (`collab::RESERVED_SETTINGS`, as ADR 0036 reserves
//! `security.*`) to the team commands, its project load and its baseline sync. This guard
//! runs a real three-person team (an Owner, a Developer, a Viewer) on the labelled in-memory
//! identity database and team server (D-4) and tries the generic path from every seat:
//!
//! * each human's plain `SetSetting` of `team.id`, `team.name`, `collab.review.required` and a
//!   `collab.review.rule.*` key — refused with `CMD-0014` (the reservation), the Owner's too:
//!   an Owner changes review rules with `forge.collab.review_policy`, which is audited;
//! * an automation session in the Owner's sandbox and one in the Developer's sandbox doing the same
//!   — refused with `CMD-0014`;
//! * afterwards every sandbox still holds its team binding, review is still required, and no
//!   refused write changed a project.
//!
//! First the audited command does write the review rule (the check is not satisfied by a
//! core that refuses everything).
//!
//! Positive control (W2): `positive_control_unreserved_team_settings_let_a_developer_rebind`
//! runs the same check on a core whose collaboration settings are not reserved (the security
//! settings still are); the Developer's plain `SetSetting` then rebinds its sandbox to another
//! team, and the check must fail.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::collab::licence::MemoryEntitlement;
use forge_editor::collab::{self as c};
use forge_editor::core::{CollabAttach, EditorCore, LocalBus, SharedCore};
use forge_project::collab::{
    CollabView, IdentityBackend, InviteTo, MemoryCollab, MemoryIdentity, Role,
};

const NOW: u64 = 20_000 * 86_400_000;

struct Server {
    identity: Arc<MemoryIdentity>,
    server: Arc<MemoryCollab>,
    licence: Arc<MemoryEntitlement>,
}

fn server() -> Server {
    static N: AtomicU64 = AtomicU64::new(0);
    let identity = Arc::new(MemoryIdentity::new());
    let name = format!(
        "reserved-team-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let store = forge_project::memory::open_named(&name, "forge-server");
    let server = Arc::new(MemoryCollab::new(store, identity.clone()));
    let licence = Arc::new(MemoryEntitlement::team_for_a_year(NOW));
    licence.set_now(Some(NOW));
    Server {
        identity,
        server,
        licence,
    }
}

struct Editor {
    core: SharedCore,
    ui: LocalBus,
}

fn editor(s: &Server, make: fn() -> SharedCore, user: &str) -> Editor {
    let core = make();
    EditorCore::attach_collab(
        &core,
        CollabAttach {
            identity: s.identity.clone(),
            server: s.server.clone(),
            licence: s.licence.clone(),
            owner: user.into(),
            email: Some(format!("{user}@studio.example")),
        },
    );
    let ui = EditorCore::connect(&core, Issuer::Human { user: user.into() });
    Editor { core, ui }
}

/// Send `cmd` through `client`; the refusal's code, if any.
fn refusal(client: &mut LocalBus, cmd: EditorCommand) -> Option<String> {
    client.apply(cmd, None);
    client
        .pump()
        .refused
        .first()
        .map(|r| r.rejection.code().as_str().to_string())
}

fn team_id(s: &Server) -> Result<String, String> {
    s.server.team().ok_or_else(|| "no team".to_string())
}

fn join(
    s: &Server,
    make: fn() -> SharedCore,
    by: &mut Editor,
    user: &str,
    role: Role,
) -> Result<Editor, String> {
    if let Some(r) = refusal(&mut by.ui, c::invite_command(None, role, &[])) {
        return Err(format!("invite: {r}"));
    }
    let code = s
        .identity
        .team_record(&team_id(s)?)
        .and_then(|t| {
            t.invites.iter().rev().find_map(|i| match &i.to {
                InviteTo::Code(c) => Some(c.clone()),
                InviteTo::Email(_) => None,
            })
        })
        .ok_or("no join code")?;
    let mut e = editor(s, make, user);
    if let Some(r) = refusal(&mut e.ui, c::accept_command(&code)) {
        return Err(format!("{user} joins: {r}"));
    }
    Ok(e)
}

fn setting(core: &SharedCore, key: &str) -> Option<Value> {
    EditorCore::read(core, |p| p.setting(key).cloned())
}

/// The writes an attacker would try: rebind the team, rename it, switch review off.
fn attempts() -> Vec<(String, Value)> {
    vec![
        (
            c::TEAM_ID_SETTING.to_string(),
            Value::Text("rogue-team".into()),
        ),
        (
            c::TEAM_NAME_SETTING.to_string(),
            Value::Text("Rogue".into()),
        ),
        (c::REVIEW_REQUIRED_SETTING.to_string(), Value::Bool(false)),
        (c::review_rule_key("scene/**"), Value::Bool(false)),
    ]
}

fn check(make: fn() -> SharedCore) -> Result<(), String> {
    let s = server();
    let mut ada = editor(&s, make, "ada");
    if let Some(r) = refusal(
        &mut ada.ui,
        EditorCommand::Spawn {
            name: "Lamp".into(),
            parent: None,
        },
    ) {
        return Err(format!("spawn: {r}"));
    }
    if let Some(r) = refusal(&mut ada.ui, c::create_command("Studio")) {
        return Err(format!("create: {r}"));
    }
    let mut bob = join(&s, make, &mut ada, "bob", Role::Developer)?;
    let mut carol = join(&s, make, &mut ada, "carol", Role::Viewer)?;
    let team = team_id(&s)?;

    // The audited command does write the review rule.
    if let Some(r) = refusal(&mut ada.ui, c::review_policy_command(true, &[])) {
        return Err(format!("the Owner's review policy was refused: {r}"));
    }
    if setting(&ada.core, c::REVIEW_REQUIRED_SETTING) != Some(Value::Bool(true)) {
        return Err("the review policy command did not write its setting".into());
    }

    let before: Vec<u64> = [&ada, &bob, &carol]
        .iter()
        .map(|e| EditorCore::read(&e.core, forge_cmd::Project::state_hash))
        .collect();
    let auto = |core: &SharedCore| {
        EditorCore::connect(
            core,
            Issuer::Automation {
                session: "auto-1".into(),
                tool: "apply".into(),
            },
        )
    };
    let mut ada_automation = auto(&ada.core);
    let mut bob_automation = auto(&bob.core);
    let seats: Vec<(&str, &SharedCore, &mut LocalBus)> = vec![
        ("bob (Developer)", &bob.core, &mut bob.ui),
        ("ada (Owner)", &ada.core, &mut ada.ui),
        ("carol (Viewer)", &carol.core, &mut carol.ui),
        (
            "an automation session in ada's sandbox",
            &ada.core,
            &mut ada_automation,
        ),
        (
            "an automation session in bob's sandbox",
            &bob.core,
            &mut bob_automation,
        ),
    ];
    for (who, core, client) in seats {
        for (key, value) in attempts() {
            let was = setting(core, &key);
            let cmd = EditorCommand::SetSetting {
                key: key.clone(),
                value: Some(value.clone()),
            };
            match refusal(client, cmd) {
                Some(code) if code == "CMD-0014" => {}
                Some(code) => {
                    return Err(format!(
                        "{who}'s SetSetting of {key} was refused as {code}, not as reserved"
                    ));
                }
                None => {
                    let now = setting(core, &key);
                    return Err(format!(
                        "{who}'s SetSetting of {key} was accepted ({was:?} -> {now:?})"
                    ));
                }
            }
        }
    }

    for (who, e) in [("ada", &ada), ("bob", &bob), ("carol", &carol)] {
        if setting(&e.core, c::TEAM_ID_SETTING) != Some(Value::Text(team.clone())) {
            return Err(format!("{who}'s sandbox lost its team binding"));
        }
    }
    if setting(&ada.core, c::REVIEW_REQUIRED_SETTING) != Some(Value::Bool(true)) {
        return Err("review was switched off by a refused write".into());
    }
    let after: Vec<u64> = [&ada, &bob, &carol]
        .iter()
        .map(|e| EditorCore::read(&e.core, forge_cmd::Project::state_hash))
        .collect();
    if after != before {
        return Err("a refused write changed a project".into());
    }
    Ok(())
}

#[test]
fn only_the_team_commands_write_the_team_settings() {
    check(EditorCore::new).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_unreserved_team_settings_let_a_developer_rebind() {
    let e = check(EditorCore::collab_unreserved_for_control)
        .expect_err("a core without the collaboration reservation must fail the check");
    // The first seat tried is the Developer, the first write the team binding: it goes
    // through and really rebinds the sandbox.
    assert!(
        e.contains("bob (Developer)'s SetSetting of team.id was accepted")
            && e.contains("rogue-team"),
        "{e}"
    );
}
