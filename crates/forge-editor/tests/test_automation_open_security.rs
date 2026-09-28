//! `test_automation_open_security` (WP-33, the WP-20 verifier's residual escalation; Ch.21
//! §21.18, E-27): **a project an automation session opens cannot widen plugin grants or the
//! default automation policy without a person's decision, and no save writes an automation
//! grant.**
//!
//! Plugin grants (`security.grants.plugin.*`) and the default automation policy
//! (`security.automation_policy.*`) ship with a project by design, so the core's project load
//! writes them. A non-human client a human let write files could therefore put them into the
//! project's `settings.ron` and `forge.project.open` it: every later session would start with
//! the capabilities it chose, and a plugin would hold grants no person gave. Since WP-33 a
//! load a non-human (an automation session or a script) started writes only what is in
//! effect now for those keys; the file's wider values wait as a proposal a human accepts or
//! discards (`forge.security.accept_held` / `discard_held`, human-only). A restriction takes
//! effect.
//!
//! Every check drives the core as an automation session: a client issuing through the bus
//! as `Issuer::Automation` (what a plugin that hosts sessions over the core gives each of
//! them; the premium edition's protocol server runs these same checks through its server):
//!
//! * `a_sessions_open_holds_widened_security_for_a_person` — the human restricted the default
//!   automation policy to reading and saved; the session (trusted by the human with file
//!   writes and ordinary commands) writes `command_ordinary` into the file's default policy
//!   and a plugin grant, and opens the project. A new session would still not be pre-granted
//!   ordinary commands, the plugin holds nothing, both settings are held and audited, and a
//!   plugin grant the file dropped is gone (restrictions apply). The session's own
//!   `accept_held` is refused (`CMD-0014`); the person's accept then grants both.
//! * `a_discarded_proposal_grants_nothing` — the person discards: nothing is granted.
//! * `a_save_while_settings_are_held_keeps_them_on_disk` — while settings are held, the
//!   session (trusted again with ordinary commands) spawns and saves: every held key's lines
//!   in `settings.ron` are byte-identical to the file's, the proposal is still held, and a
//!   person opening the saved project in a new editor gets the plugin grant and the default
//!   automation policy (a save of what took effect would have reverted a teammate's grants).
//! * `a_headless_open_holds_unless_the_launcher_accepts` — a headless script's open holds the
//!   plugin grant; with `--accept-project-security` (a launch flag no file sets) the runner
//!   accepts it as the launcher right after the line, and it takes effect. The first run is the
//!   second's control.
//! * `a_persons_open_is_unchanged` — the same file opened by a person takes effect at once.
//! * `saved_projects_hold_no_automation_grants` — a save after a human granted a session
//!   writes no `security.grants.automation.*` key and never the run's epoch.
//!
//! Positive controls (W2), each failing its check:
//! * `positive_control_an_unheld_session_open_widens_security` — the core loads a session's
//!   project like a person's (`CoreFaults::automation_load_unheld`): new sessions are
//!   pre-granted ordinary commands and the plugin holds the grant.
//! * `positive_control_a_save_that_keeps_per_run_settings_leaks_the_epoch` — the host writes
//!   every setting (`CoreFaults::save_keeps_per_run`): the saved file holds the automation
//!   grant and the epoch.
//! * `positive_control_a_save_of_what_took_effect_reverts_held_settings` — the host writes
//!   the held keys as the load left them in memory (`CoreFaults::save_writes_held`): the save
//!   changes them on disk.
//! * `positive_control_an_unaudited_narrowing_leaves_no_trace` — see `check_narrowed`.

use std::collections::BTreeMap;
use std::sync::Arc;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::{CoreFaults, EditorCore, LocalBus, SharedCore};
use forge_editor::project::{ProjectOp, Template};
use forge_editor::security;
use forge_plugin::{Capability, CommandClass, FsScope, PluginId, Principal, PrincipalKind};
use forge_project::format::{MANIFEST_PATH, ProjectDoc, SCENE_PATH, SETTINGS_PATH};

const READ: Capability = Capability::Fs(FsScope::ProjectRead);
const WRITE: Capability = Capability::Fs(FsScope::ProjectWrite);
const ORDINARY: Capability = Capability::Command(CommandClass::Ordinary);
const DESTRUCTIVE: Capability = Capability::Command(CommandClass::Destructive);

fn human() -> Issuer {
    Issuer::Human { user: "ada".into() }
}

fn plugin(id: &str) -> Result<Principal, String> {
    PluginId::new(id)
        .map(Principal::Plugin)
        .map_err(|e| e.to_string())
}

/// The automation session the checks run as.
const SESSION: &str = "auto-1";

/// An automation session's client of the core: a non-human client issuing through the bus
/// (what a plugin that hosts sessions over the core gives each of them).
fn automation(core: &SharedCore) -> LocalBus {
    EditorCore::connect(
        core,
        Issuer::Automation {
            session: SESSION.into(),
            tool: "apply".into(),
        },
    )
}

/// Send `cmd` through the session's client; the refusal's text, if any.
fn auto_sends(c: &mut LocalBus, cmd: EditorCommand) -> Result<(), String> {
    let what = cmd.label();
    c.apply(cmd, None);
    if let Some(r) = c.pump().refused.first() {
        return Err(format!("{what}: {}", r.rejection));
    }
    Ok(())
}

/// Send `cmd` as a human and wait for any lifecycle outcome; the refusal's text, if any.
fn human_sends(core: &SharedCore, cmd: EditorCommand) -> Result<(), String> {
    let mut c = EditorCore::connect(core, human());
    let what = cmd.label();
    c.apply(cmd, None);
    if let Some(r) = c.pump().refused.first() {
        return Err(format!("{what}: {}", r.rejection));
    }
    EditorCore::wait_transfers(core);
    Ok(())
}

/// The outcome of the newest lifecycle operation `op`.
fn outcome(core: &SharedCore, op: &str) -> Result<String, String> {
    EditorCore::project_status(core)
        .outcomes
        .iter()
        .rev()
        .find(|o| o.op == op)
        .map(|o| o.result.clone().map_err(|(c, m)| format!("{c}: {m}")))
        .unwrap_or_else(|| Err(format!("no {op} outcome")))
}

fn project_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "forge-wp33-open-security-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A file write: the project's settings file re-encoded after `edit` (what a session with a
/// file-write capability and a text editor can do).
fn edit_settings(
    dir: &std::path::Path,
    edit: impl FnOnce(&mut BTreeMap<String, Value>),
) -> Result<(), String> {
    let text = |p: &str| std::fs::read_to_string(dir.join(p)).map_err(|e| format!("{p}: {e}"));
    let (_, mut doc) = ProjectDoc::decode(
        &text(MANIFEST_PATH)?,
        Some(&text(SETTINGS_PATH)?),
        Some(&text(SCENE_PATH)?),
    )
    .map_err(|e| e.to_string())?;
    edit(&mut doc.settings);
    for (path, body) in doc.encode().map_err(|e| e.to_string())? {
        if path == SETTINGS_PATH {
            std::fs::write(dir.join(path), body).map_err(|e| format!("{path}: {e}"))?;
        }
    }
    Ok(())
}

/// Whether a new automation session may send ordinary commands: what the default automation
/// policy in effect pre-grants every new session (the standard policy when the project sets
/// none).
fn new_session_may_edit(core: &SharedCore) -> bool {
    EditorCore::grants(core)
        .default_policy(PrincipalKind::Automation)
        .unwrap_or_else(security::standard_automation_policy)
        .contains(ORDINARY)
}

struct Setup {
    dir: std::path::PathBuf,
    location: String,
    auto: LocalBus,
    rivers: Principal,
    lakes: Principal,
}

/// A saved project whose default automation policy is read-only and whose plugin `lakes` holds
/// `Fs(ProjectRead)`; the automation session, trusted by the person with file
/// writes and ordinary commands; then the session's file edit: `command_ordinary` on in the
/// default policy, `Fs(ProjectWrite)` granted to plugin `rivers`, `lakes`' grant dropped.
fn setup(core: &SharedCore, tag: &str) -> Result<Setup, String> {
    let dir = project_dir(tag);
    let location = format!("file:{}", dir.display());
    human_sends(
        core,
        ProjectOp::Create {
            location: location.clone(),
            name: "Held".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
    )?;
    outcome(core, forge_editor::project::CREATE_CMD)?;
    let rivers = plugin("com.example.rivers")?;
    let lakes = plugin("com.example.lakes")?;
    human_sends(core, security::policy_command(&[READ]))?;
    human_sends(core, security::grant_command(&lakes, READ))?;
    human_sends(
        core,
        forge_editor::project::save_command("read-only sessions"),
    )?;
    outcome(core, forge_editor::project::SAVE_CMD)?;
    if new_session_may_edit(core) {
        return Err("setup: the read-only default policy lets a new session edit".into());
    }
    let auto = automation(core);
    let me = Principal::Automation(SESSION.into());
    human_sends(core, security::grant_command(&me, WRITE))?;
    human_sends(core, security::grant_command(&me, ORDINARY))?;

    let policy_key = format!("{}.{}", security::AUTOMATION_POLICY_PREFIX, ORDINARY.key());
    let rivers_key = security::grant_key(&rivers, WRITE).ok_or("no grant key")?;
    let lakes_key = security::grant_key(&lakes, READ).ok_or("no grant key")?;
    edit_settings(&dir, |s| {
        s.insert(policy_key, Value::Bool(true));
        s.insert(rivers_key, Value::Bool(true));
        s.remove(&lakes_key);
    })?;
    Ok(Setup {
        dir,
        location,
        auto,
        rivers,
        lakes,
    })
}

fn open_command(location: &str) -> EditorCommand {
    ProjectOp::Open {
        location: location.to_string(),
        discard_unsaved: true,
    }
    .command()
}

/// The session opens the project it edited, through its client.
fn automation_opens(core: &SharedCore, st: &mut Setup) -> Result<String, String> {
    auto_sends(&mut st.auto, open_command(&st.location))
        .map_err(|e| format!("the session's open was refused: {e}"))?;
    EditorCore::wait_transfers(core);
    outcome(core, forge_editor::project::OPEN_CMD)
        .map_err(|e| format!("the session's open failed: {e}"))
}

fn audit_events(core: &SharedCore) -> Vec<String> {
    EditorCore::audit_book(core)
        .query(&forge_editor::connect::audit::AuditQuery::parse(""))
        .into_iter()
        .map(|r| r.event)
        .collect()
}

/// After the session's open: nothing widened (both are checked, so the control shows both
/// fail), the restriction applied, the proposal held and audited.
fn check_held(core: &SharedCore, st: &mut Setup) -> Result<security::HeldSecurity, String> {
    let said = automation_opens(core, st)?;
    let grants = EditorCore::grants(core);
    let mut problems = Vec::new();
    if new_session_may_edit(core) {
        problems.push(
            "the session's open widened the default automation policy: a new session may edit"
                .to_string(),
        );
    }
    if grants.has(&st.rivers, WRITE) {
        problems.push(format!(
            "the session's open granted plugin {} {WRITE}",
            st.rivers
        ));
    }
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    if grants.has(&st.lakes, READ) {
        return Err(
            "a plugin grant the file dropped still holds (a restriction is not held)".into(),
        );
    }
    let held = EditorCore::held_security(core).ok_or("nothing is held")?;
    let keys: Vec<&str> = held.items.iter().map(|i| i.key.as_str()).collect();
    let policy_key = format!("{}.{}", security::AUTOMATION_POLICY_PREFIX, ORDINARY.key());
    let rivers_key = security::grant_key(&st.rivers, WRITE).ok_or("no grant key")?;
    if keys.len() != 2
        || !keys.contains(&policy_key.as_str())
        || !keys.contains(&rivers_key.as_str())
    {
        return Err(format!("held: {keys:?}"));
    }
    if !held.by.starts_with("automation:") || !said.contains("held until a person") {
        return Err(format!("the proposal / the outcome: {} / {said}", held.by));
    }
    if !audit_events(core).iter().any(|e| e == "held") {
        return Err("the held settings are not audited".into());
    }
    Ok(held)
}

fn check_accept(core: &SharedCore, tag: &str) -> Result<(), String> {
    let mut st = setup(core, tag)?;
    let held = check_held(core, &mut st)?;
    // The session may not answer for the person: even trusted with destructive commands (the
    // load ended its per-run grants; an accept is never undone, so a host classes it
    // destructive), its own accept is refused by the bus's human-only policy.
    let me = Principal::Automation(SESSION.into());
    human_sends(core, security::grant_command(&me, DESTRUCTIVE))?;
    match auto_sends(&mut st.auto, security::accept_held_command(&held)) {
        Err(e) if e.contains("CMD-0014") => {}
        other => {
            return Err(format!(
                "the session accepted its own held settings: {other:?}"
            ));
        }
    }
    if EditorCore::held_security(core).is_none() || EditorCore::grants(core).has(&st.rivers, WRITE)
    {
        return Err("the session's refused accept changed something".into());
    }
    // An accept that does not match the proposal is refused.
    let mut forged = held.clone();
    forged.items.truncate(1);
    if human_sends(core, security::accept_held_command(&forged)).is_ok() {
        return Err("an accept of other settings than the held ones went through".into());
    }
    // The person accepts: both take effect.
    human_sends(core, security::accept_held_command(&held))?;
    if !EditorCore::grants(core).has(&st.rivers, WRITE) {
        return Err("the person's accept did not grant the plugin".into());
    }
    if !new_session_may_edit(core) {
        return Err("the person's accept did not widen the default automation policy".into());
    }
    if EditorCore::held_security(core).is_some() {
        return Err("the proposal outlived its accept".into());
    }
    if !audit_events(core).iter().any(|e| e == "accept_held") {
        return Err("the accept is not audited".into());
    }
    let _ = std::fs::remove_dir_all(&st.dir);
    Ok(())
}

/// A core whose person trusts every project whatever it carries (`--trust-project`,
/// `TrustEvery`): the file edit below is not the person's, so since WP-35 project trust would
/// hold the new plugin grant on its own (`test_project_trust` covers that). These checks are
/// about who opened the project (WP-33), so trust is set aside and only that rule holds.
fn trust_aside() -> SharedCore {
    let core = EditorCore::new();
    EditorCore::set_trust_book(&core, Arc::new(forge_editor::trust::TrustEvery));
    core
}

#[test]
fn a_sessions_open_holds_widened_security_for_a_person() {
    check_accept(&trust_aside(), "accept").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_unheld_session_open_widens_security() {
    let core = trust_aside();
    EditorCore::set_faults(
        &core,
        CoreFaults {
            automation_load_unheld: true,
            ..CoreFaults::default()
        },
    );
    let e = check_accept(&core, "control").expect_err("an unheld session open must fail the check");
    assert!(e.contains("widened the default automation policy"), "{e}");
    assert!(e.contains("granted plugin"), "{e}");
}

#[test]
fn a_discarded_proposal_grants_nothing() {
    let core = EditorCore::new();
    let run = || -> Result<(), String> {
        let mut st = setup(&core, "discard")?;
        let held = check_held(&core, &mut st)?;
        human_sends(&core, security::discard_held_command(&held))?;
        if EditorCore::held_security(&core).is_some() {
            return Err("the proposal outlived its discard".into());
        }
        if EditorCore::grants(&core).has(&st.rivers, WRITE) || new_session_may_edit(&core) {
            return Err("a discarded proposal granted something".into());
        }
        // A discarded (or replaced) proposal cannot be accepted later.
        if human_sends(&core, security::accept_held_command(&held)).is_ok() {
            return Err("a discarded proposal was accepted".into());
        }
        if !audit_events(&core).iter().any(|e| e == "discard_held") {
            return Err("the discard is not audited".into());
        }
        let _ = std::fs::remove_dir_all(&st.dir);
        Ok(())
    };
    run().unwrap_or_else(|e| panic!("{e}"));
}

/// The lines of the project's `settings.ron` that name `key`, as bytes on disk.
fn setting_lines(dir: &std::path::Path, key: &str) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(dir.join(SETTINGS_PATH)).map_err(|e| e.to_string())?;
    let quoted = format!("\"{key}\"");
    Ok(text
        .lines()
        .filter(|l| l.contains(&quoted))
        .map(str::to_string)
        .collect())
}

/// While settings are held, a save by the session (trusted again with ordinary commands, after
/// it spawned something) and a build leave every held key on disk exactly as the file held
/// it; the proposal is still held; and a person opening the saved project in a new editor
/// gets the teammate's grants, which a save of what took effect would have reverted.
fn check_held_save(faults: CoreFaults, tag: &str) -> Result<(), String> {
    let core = EditorCore::new();
    EditorCore::set_faults(&core, faults);
    let mut st = setup(&core, tag)?;
    let held = check_held(&core, &mut st)?;
    let before: Vec<(String, Vec<String>)> = held
        .items
        .iter()
        .map(|i| setting_lines(&st.dir, &i.key).map(|l| (i.key.clone(), l)))
        .collect::<Result<_, _>>()?;
    if before.iter().all(|(_, l)| l.is_empty()) {
        return Err("setup: the file names none of the held keys".into());
    }
    // The load ended the session's per-run grants; the person trusts it with ordinary
    // commands again, and it edits and saves (a save is an ordinary command).
    let me = Principal::Automation(SESSION.into());
    human_sends(&core, security::grant_command(&me, ORDINARY))?;
    let spawn = EditorCommand::Spawn {
        name: "Edited".into(),
        parent: None,
    };
    auto_sends(&mut st.auto, spawn).map_err(|e| format!("the session's spawn was refused: {e}"))?;
    auto_sends(
        &mut st.auto,
        forge_editor::project::save_command("the session's edit"),
    )
    .map_err(|e| format!("the session's save was refused: {e}"))?;
    EditorCore::wait_transfers(&core);
    let said = outcome(&core, forge_editor::project::SAVE_CMD)?;
    if !said.contains("Saved revision") {
        return Err(format!("the session's save wrote nothing: {said}"));
    }
    let mut changed = Vec::new();
    for (key, lines) in &before {
        let now = setting_lines(&st.dir, key)?;
        if &now != lines {
            changed.push(format!("{key}: {lines:?} -> {now:?}"));
        }
    }
    if !changed.is_empty() {
        return Err(format!(
            "the save changed held settings on disk: {}",
            changed.join("; ")
        ));
    }
    if EditorCore::held_security_id(&core) != Some(held.id) {
        return Err("the save answered the held proposal".into());
    }
    // Nothing reverted: a person opening the saved project in a new editor, and trusting it
    // there (a new editor has not been told it is trusted, WP-34), gets the file's plugin
    // grant and default automation policy.
    let fresh = EditorCore::new();
    human_sends(&fresh, open_command(&st.location))?;
    outcome(&fresh, forge_editor::project::OPEN_CMD)?;
    EditorCore::decide_trust(&fresh, "ada", forge_editor::trust::Trust::Trusted)?;
    if !EditorCore::grants(&fresh).has(&st.rivers, WRITE) {
        return Err("the saved project lost the plugin grant the file held".into());
    }
    if !new_session_may_edit(&fresh) {
        return Err("the saved project lost the default automation policy the file held".into());
    }
    let _ = std::fs::remove_dir_all(&st.dir);
    Ok(())
}

#[test]
fn a_save_while_settings_are_held_keeps_them_on_disk() {
    check_held_save(CoreFaults::default(), "held-save").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_save_of_what_took_effect_reverts_held_settings() {
    let e = check_held_save(
        CoreFaults {
            save_writes_held: true,
            ..CoreFaults::default()
        },
        "held-save-control",
    )
    .expect_err("a save that writes the held settings as they took effect must fail the check");
    assert!(e.contains("changed held settings on disk"), "{e}");
}

/// A headless script opens the project in a new editor: its settings are held (a script
/// file may be a session's) unless the launcher passed `--accept-project-security`.
fn headless_open(accept: bool, tag: &str) -> Result<(bool, String), String> {
    use forge_editor::headless::{HeadlessOptions, run};
    let st = setup(&EditorCore::new(), tag)?;
    let core = EditorCore::new();
    let line = serde_json::to_string(&open_command(&st.location)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    run(
        &core,
        "ci.forge",
        Some(std::io::Cursor::new(line)),
        &HeadlessOptions {
            accept_project_security: accept,
            ..HeadlessOptions::default()
        },
        &mut out,
    )
    .map_err(|e| e.to_string())?;
    let out = String::from_utf8_lossy(&out).into_owned();
    let granted = EditorCore::grants(&core).has(&st.rivers, WRITE);
    if granted == EditorCore::held_security(&core).is_some() {
        return Err(format!(
            "granted {granted} while the proposal is held: {out}"
        ));
    }
    let _ = std::fs::remove_dir_all(&st.dir);
    Ok((granted, out))
}

#[test]
fn a_headless_open_holds_unless_the_launcher_accepts() {
    let (granted, out) = headless_open(false, "headless").unwrap_or_else(|e| panic!("{e}"));
    assert!(!granted, "a script's open granted the plugin: {out}");
    assert!(out.contains("held until a person"), "{out}");
    let (granted, out) = headless_open(true, "headless-accept").unwrap_or_else(|e| panic!("{e}"));
    assert!(
        granted,
        "--accept-project-security did not grant the plugin: {out}"
    );
    assert!(out.contains("accepted 1 held security setting(s)"), "{out}");
}

#[test]
fn a_persons_open_is_unchanged() {
    let core = trust_aside();
    let run = || -> Result<(), String> {
        let st = setup(&core, "human")?;
        human_sends(&core, open_command(&st.location))?;
        let said = outcome(&core, forge_editor::project::OPEN_CMD)?;
        if said.contains("held") || EditorCore::held_security(&core).is_some() {
            return Err(format!("a person's open held settings: {said}"));
        }
        if !EditorCore::grants(&core).has(&st.rivers, WRITE) {
            return Err("a person's open did not load the plugin grant".into());
        }
        if EditorCore::grants(&core).has(&st.lakes, READ) {
            return Err("a person's open kept a grant the file dropped".into());
        }
        if !new_session_may_edit(&core) {
            return Err("a person's open did not load the default automation policy".into());
        }
        let _ = std::fs::remove_dir_all(&st.dir);
        Ok(())
    };
    run().unwrap_or_else(|e| panic!("{e}"));
}

fn check_save(faults: CoreFaults, tag: &str) -> Result<(), String> {
    let core = EditorCore::new();
    EditorCore::set_faults(&core, faults);
    let dir = project_dir(tag);
    let location = format!("file:{}", dir.display());
    human_sends(
        &core,
        ProjectOp::Create {
            location: location.clone(),
            name: "Saved".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
    )?;
    let me = Principal::Automation(SESSION.into());
    human_sends(&core, security::grant_command(&me, WRITE))?;
    let rivers = plugin("com.example.rivers")?;
    human_sends(&core, security::grant_command(&rivers, READ))?;
    human_sends(&core, forge_editor::project::save_command("grants"))?;
    outcome(&core, forge_editor::project::SAVE_CMD)?;
    let text = std::fs::read_to_string(dir.join(SETTINGS_PATH)).map_err(|e| e.to_string())?;
    if text.contains(security::AUTOMATION_GRANTS_PREFIX) || text.contains(security::epoch()) {
        return Err(format!(
            "the saved settings hold an automation grant: {text}"
        ));
    }
    // Plugin grants still ship with the project.
    let rivers_key = security::grant_key(&rivers, READ).ok_or("no grant key")?;
    if !text.contains(&rivers_key) {
        return Err(format!("the saved settings lost the plugin grant: {text}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn saved_projects_hold_no_automation_grants() {
    check_save(CoreFaults::default(), "save").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_save_that_keeps_per_run_settings_leaks_the_epoch() {
    let e = check_save(
        CoreFaults {
            save_keeps_per_run: true,
            ..CoreFaults::default()
        },
        "save-control",
    )
    .expect_err("a save that writes per-run settings must fail the check");
    assert!(e.contains("hold an automation grant"), "{e}");
}

// ---- WP-21: restrictions a non-human's load makes are audited as `narrowed` ---------------
//
// A restriction takes effect at once (it only makes things safer), but a person must be able
// to see what a session or a script took away: the file the session opens drops plugin `lakes`'
// grant, and the core audits a `narrowed` record naming it. Control: a core that does not
// (`CoreFaults::narrowed_unaudited`) leaves no trace.

fn check_narrowed(core: &SharedCore, tag: &str) -> Result<(), String> {
    let mut st = setup(core, tag)?;
    automation_opens(core, &mut st)?;
    if EditorCore::grants(core).has(&st.lakes, READ) {
        return Err("the grant the file dropped still holds".into());
    }
    let narrowed: Vec<String> = EditorCore::audit_book(core)
        .query(&forge_editor::connect::audit::AuditQuery::parse(""))
        .into_iter()
        .filter(|r| r.event == "narrowed")
        .map(|r| r.detail)
        .collect();
    if !narrowed
        .iter()
        .any(|d| d.contains("com.example.lakes") && d.contains("dropped"))
    {
        return Err(format!(
            "the grant the session's open dropped is not audited as narrowed: {narrowed:?}"
        ));
    }
    if narrowed.iter().any(|d| d.contains("com.example.rivers")) {
        return Err(format!(
            "a held widening was audited as narrowed: {narrowed:?}"
        ));
    }
    let _ = std::fs::remove_dir_all(&st.dir);
    Ok(())
}

#[test]
fn a_narrowing_session_open_is_audited() {
    check_narrowed(&EditorCore::new(), "narrowed").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_unaudited_narrowing_leaves_no_trace() {
    let core = EditorCore::new();
    EditorCore::set_faults(
        &core,
        CoreFaults {
            narrowed_unaudited: true,
            ..CoreFaults::default()
        },
    );
    let e = check_narrowed(&core, "narrowed-control")
        .expect_err("an unaudited narrowing must fail the check");
    assert!(e.contains("not audited as narrowed"), "{e}");
}

#[test]
fn a_persons_narrowing_open_is_not_flagged() {
    // A person's load is their decision: nothing to flag (the same file, opened by ada).
    let core = EditorCore::new();
    let run = || -> Result<(), String> {
        let st = setup(&core, "narrowed-person")?;
        human_sends(&core, open_command(&st.location))?;
        outcome(&core, forge_editor::project::OPEN_CMD)?;
        if audit_events(&core).iter().any(|e| e == "narrowed") {
            return Err("a person's open was audited as narrowed".into());
        }
        let _ = std::fs::remove_dir_all(&st.dir);
        Ok(())
    };
    run().unwrap_or_else(|e| panic!("{e}"));
}
