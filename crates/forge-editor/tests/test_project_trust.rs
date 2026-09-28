//! `test_project_trust` (WP-34, the WP-21 verifier's finding; Ch.32 §32.7, Ch.21 §21.18,
//! E-27; gate row `C-project-trust`): **opening a project that carries plugin code and the
//! plugin grants to use it cannot run that code with those grants without a person's
//! decision.**
//!
//! The project here is what a clone of a hostile repository looks like: its `settings.ron`
//! grants plugin `com.example.flag` `Command(Ordinary)` (written by a teammate's editor, so
//! it is an ordinary saved grant), and its `plugins/flag/` folder holds that plugin's WASM
//! code, whose command writes `example.flag`. A person opens it in an editor that hosts WASM
//! plugins (the real shell, the real core, the real WASM host), and the editor wakes:
//!
//! * the plugin's code is not loaded (the core has no `example.flag`), the Plugin manager
//!   lists it *not loaded: the project is not trusted*;
//! * the grant is not in effect (the one grant table does not hold it) — it is held, marked
//!   as held for trust, and a save keeps it on disk as the file had it;
//! * running `example.flag` as the person is refused, so nothing it would write is written;
//! * the person is asked: a notice, and the project's trust question lists the plugin folder
//!   and the grant.
//!
//! Then the person decides (the Plugin manager's call, `EditorCore::decide_trust`):
//! * `trusting_runs_the_projects_plugin_with_its_grants` — trusted: the held grant is
//!   accepted through the human-only command, the next wake installs the plugin, and its
//!   command applies. The answer is in the user config file, not in the project, and a new
//!   editor with the same user config opens the project trusted at once, asking nothing.
//! * `not_trusting_keeps_the_code_out_and_the_grants_held` — not trusted: the next wake still
//!   loads nothing, the grant stays held, and a new editor with the same user config remembers
//!   the answer (it does not ask again, and loads nothing).
//! * `a_headless_run_trusts_only_with_the_launch_flag` — a headless core (the remote host's)
//!   trusts nothing: a person's open through it holds the grant; after a run with
//!   `--trust-project` (`HeadlessOptions::trust_project`) the same open takes effect.
//!
//! Positive controls (W2), each failing the untrusted-open check at its own layer:
//! * `positive_control_a_hosting_that_ignores_trust_runs_the_projects_code` —
//!   `HostingFaults::ignore_trust` (the WP-21 hosting): the plugin's code is installed;
//! * `positive_control_an_untrusted_load_takes_the_projects_grants` —
//!   `CoreFaults::untrusted_load_unheld` (the WP-21 load): the grant is in effect;
//! * `positive_control_both_run_the_projects_code_with_its_grants` — both: the person's
//!   click on the plugin's command writes `example.flag` with no decision made.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::{CoreFaults, EditorCore, SharedCore};
use forge_editor::hosting::{HostingFaults, HostingSetup, PluginDirs};
use forge_editor::presets::builtin_preset;
use forge_editor::project::{ProjectOp, Template};
use forge_editor::security;
use forge_editor::shell::assemble_hosted;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_editor::trust::{FileTrust, Trust, TrustState};
use forge_plugin::{Capability, CommandClass, PluginId, Principal};

const ORDINARY: Capability = Capability::Command(CommandClass::Ordinary);
const FLAG_MANIFEST: &str = r#"Plugin(id: "com.example.flag", version: "0.1.0", engine: "^0.1",
    kind: Wasm, provides: [Command("example.flag")], capabilities: [Command(Ordinary)])"#;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("forge-wp34-trust-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn write(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(name), text).map_err(|e| format!("{}: {e}", dir.display()))
}

fn flag() -> Result<Principal, String> {
    PluginId::new("com.example.flag")
        .map(Principal::Plugin)
        .map_err(|e| e.to_string())
}

fn person() -> Issuer {
    Issuer::Human {
        user: "tester".into(),
    }
}

/// Send `cmd` as the person and let any lifecycle transfer finish; the refusal, if any.
fn send(core: &SharedCore, cmd: EditorCommand) -> Result<(), String> {
    let mut c = EditorCore::connect(core, person());
    let what = cmd.label();
    c.apply(cmd, None);
    if let Some(r) = c.pump().refused.first() {
        return Err(format!("{what}: {}", r.rejection));
    }
    EditorCore::wait_transfers(core);
    Ok(())
}

fn outcome(core: &SharedCore, op: &str) -> Result<String, String> {
    EditorCore::project_status(core)
        .outcomes
        .iter()
        .rev()
        .find(|o| o.op == op)
        .map(|o| o.result.clone().map_err(|(c, m)| format!("{c}: {m}")))
        .unwrap_or_else(|| Err(format!("no {op} outcome")))
}

/// The cloned project: a teammate's editor created and saved it with the grant; then the
/// repository's `plugins/flag/` folder (the plugin's code) is in it too.
fn cloned_project(dir: &Path) -> Result<String, String> {
    let location = format!("file:{}", dir.display());
    let teammate = EditorCore::new();
    send(
        &teammate,
        ProjectOp::Create {
            location: location.clone(),
            name: "Cloned".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
    )?;
    outcome(&teammate, forge_editor::project::CREATE_CMD)?;
    send(&teammate, security::grant_command(&flag()?, ORDINARY))?;
    send(&teammate, forge_editor::project::save_command("grant flag"))?;
    outcome(&teammate, forge_editor::project::SAVE_CMD)?;
    let code = dir.join("plugins").join("flag");
    write(&code, "plugin.ron", FLAG_MANIFEST)?;
    let plan = r#"{"ops":[{"op":"set_setting","key":"example.flag","value":{"Int":1}}]}"#;
    write(
        &code,
        "plugin.wat",
        &forge_wasm::wat::by_point(&[("Command", plan.as_bytes())]),
    )?;
    Ok(location)
}

struct Setup {
    rig: Rig,
    root: PathBuf,
    config: PathBuf,
    project: PathBuf,
    location: String,
}

/// An editor hosting WASM plugins over a fresh core whose trust answers live in
/// `<root>/config/trusted-projects.ron`, with the cloned project on disk (not opened).
fn setup(tag: &str, core_faults: CoreFaults, hosting: HostingFaults) -> Result<Setup, String> {
    let root = tmp(tag);
    let config = root.join("config");
    std::fs::create_dir_all(config.join("plugins")).map_err(|e| e.to_string())?;
    let project = root.join("cloned");
    let location = cloned_project(&project)?;
    let rig = editor(&config, core_faults, hosting)?;
    Ok(Setup {
        rig,
        root,
        config,
        project,
        location,
    })
}

fn editor(config: &Path, core_faults: CoreFaults, hosting: HostingFaults) -> Result<Rig, String> {
    editor_on(trusting_core(config, core_faults), config, hosting)
}

/// A core whose trust answers live in `<config>/trusted-projects.ron`.
fn trusting_core(config: &Path, core_faults: CoreFaults) -> SharedCore {
    let core = EditorCore::new();
    EditorCore::set_faults(&core, core_faults);
    EditorCore::set_trust_book(&core, Arc::new(FileTrust::open(config)));
    core
}

/// An editor hosting WASM plugins over `core` (which may already hold an open project: the
/// start-up trust gate), plugins from `<config>/plugins`.
fn editor_on(core: SharedCore, config: &Path, hosting: HostingFaults) -> Result<Rig, String> {
    let stand_in = StandInPanels::without(&[]).map_err(|e| e.to_string())?;
    let cfg = assemble_hosted(
        builtin_preset("3d").map_err(|e| e.to_string())?,
        &[&stand_in],
        &[],
        Some(config.to_path_buf()),
        Some(HostingSetup {
            core: core.clone(),
            dirs: PluginDirs::for_user(Some(config)),
        }),
    )
    .map_err(|e| e.to_string())?;
    cfg.services.hosting.borrow_mut().faults = hosting;
    Rig::with_core(cfg, core).map_err(|e| e.to_string())
}

/// The person comes back to the window: the loop turns (and polls its plugins).
fn wake(rig: &mut Rig) {
    let now = rig.h.now();
    rig.h.set_now(now + Duration::from_secs(2));
    rig.turn();
    rig.settle();
}

/// The person opens the project in the editor, which then wakes.
fn open(rig: &mut Rig, location: &str) -> Result<(), String> {
    send(
        &rig.core,
        ProjectOp::Open {
            location: location.to_string(),
            discard_unsaved: true,
        }
        .command(),
    )?;
    outcome(&rig.core, forge_editor::project::OPEN_CMD)?;
    rig.settle();
    wake(rig);
    Ok(())
}

/// The person runs the plugin's command; its refusal, if any.
fn run_flag(core: &SharedCore) -> Option<String> {
    send(
        core,
        EditorCommand::Invoke {
            target: "example.flag".into(),
            args: "{}".into(),
        },
    )
    .err()
}

fn installed(core: &SharedCore) -> bool {
    EditorCore::configure(core, |bus| bus.targets().any(|t| t == "example.flag"))
}

fn flag_row(rig: &Rig) -> Option<String> {
    let rows = rig
        .shell
        .handles()
        .services()
        .connect
        .plugins
        .borrow()
        .rows(&rig.shell.mirror());
    rows.into_iter()
        .find(|r| r.id == "com.example.flag")
        .map(|r| r.label())
}

fn settings_text(project: &Path) -> Result<String, String> {
    std::fs::read_to_string(project.join(forge_project::format::SETTINGS_PATH))
        .map_err(|e| e.to_string())
}

/// The untrusted open (see the module docs): every way the project's code could run with its
/// grants before a person decided. `Err` lists each one that happened.
fn check_untrusted_open(st: &mut Setup) -> Result<(), String> {
    open(&mut st.rig, &st.location)?;
    let core = st.rig.core.clone();
    let grant_key = security::grant_key(&flag()?, ORDINARY).ok_or("no grant key")?;
    let mut broken = Vec::new();
    if installed(&core) {
        broken.push("the project's plugin code was installed before a decision".to_string());
    }
    if EditorCore::grants(&core).has(&flag()?, ORDINARY) {
        broken.push("the project's grant is in effect before a decision".to_string());
    }
    if run_flag(&core).is_none() {
        broken.push("the project's plugin ran with the project's grant".to_string());
    }
    if EditorCore::read(&core, |p| p.setting("example.flag").cloned()).is_some() {
        broken.push("the plugin's command wrote example.flag".to_string());
    }
    if !broken.is_empty() {
        return Err(broken.join("; "));
    }
    // Held for trust, and asked about.
    let held = EditorCore::held_security(&core).ok_or("the grant is not held")?;
    if !held.untrusted || !held.items.iter().any(|i| i.key == grant_key) {
        return Err(format!("the held proposal: {held:?}"));
    }
    let t = EditorCore::project_trust(&core).ok_or("the project's trust is not asked")?;
    if t.state != TrustState::Undecided
        || !t.plugins.iter().any(|p| p == "plugins/flag")
        || t.grants.len() != 1
    {
        return Err(format!("the trust question: {t:?}"));
    }
    let asked = st
        .rig
        .shell
        .session()
        .notifications
        .history()
        .any(|n| n.title == "Do you trust this project?");
    if !asked {
        return Err("the person was not asked".into());
    }
    let row = flag_row(&st.rig).ok_or("the Plugin manager does not list the plugin")?;
    if !row.contains("not loaded") || !row.contains("not trusted") {
        return Err(format!("the Plugin manager's row: {row}"));
    }
    // A save while undecided keeps the grant on disk (a teammate's grant is not stripped).
    send(&core, forge_editor::project::save_command("undecided"))?;
    if !settings_text(&st.project)?.contains(&grant_key) {
        return Err("a save while undecided stripped the project's grant from its files".into());
    }
    Ok(())
}

fn untrusted_open(tag: &str, core: CoreFaults, hosting: HostingFaults) -> Result<Setup, String> {
    let mut st = setup(tag, core, hosting)?;
    check_untrusted_open(&mut st)?;
    Ok(st)
}

fn trusting(tag: &str) -> Result<(), String> {
    let mut st = untrusted_open(tag, CoreFaults::default(), HostingFaults::default())?;
    let core = st.rig.core.clone();
    let said = EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
    if !said.contains("accepted") {
        return Err(format!("trusting did not accept the held grant: {said}"));
    }
    if EditorCore::held_security(&core).is_some()
        || !EditorCore::grants(&core).has(&flag()?, ORDINARY)
    {
        return Err("trusting did not put the project's grant in effect".into());
    }
    wake(&mut st.rig);
    if !installed(&core) {
        return Err("the trusted project's plugin did not install".into());
    }
    if let Some(r) = run_flag(&core) {
        return Err(format!("the trusted plugin's command was refused: {r}"));
    }
    if EditorCore::read(&core, |p| p.setting("example.flag").cloned()) != Some(Value::Int(1)) {
        return Err("the trusted plugin's command did not apply".into());
    }
    // Remembered in user config, not in the project.
    let book = std::fs::read_to_string(st.config.join(forge_editor::trust::TRUST_FILE))
        .map_err(|e| format!("the trust file: {e}"))?;
    if !book.contains("Trusted") {
        return Err(format!("the trust file: {book}"));
    }
    if settings_text(&st.project)?.contains("trust") {
        return Err("the answer was written into the project".into());
    }
    // A new editor with the same user config: trusted at once, asks nothing.
    let mut again = editor(&st.config, CoreFaults::default(), HostingFaults::default())?;
    open(&mut again, &st.location)?;
    if EditorCore::held_security(&again.core).is_some()
        || !EditorCore::grants(&again.core).has(&flag()?, ORDINARY)
        || !installed(&again.core)
    {
        return Err("a new editor did not remember the project is trusted".into());
    }
    if again
        .shell
        .session()
        .notifications
        .history()
        .any(|n| n.title == "Do you trust this project?")
    {
        return Err("a new editor asked again about a trusted project".into());
    }
    let _ = std::fs::remove_dir_all(&st.root);
    Ok(())
}

#[test]
fn trusting_runs_the_projects_plugin_with_its_grants() {
    trusting("trusted").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn not_trusting_keeps_the_code_out_and_the_grants_held() {
    let r = (|| -> Result<(), String> {
        let mut st = untrusted_open("untrusted", CoreFaults::default(), HostingFaults::default())?;
        let core = st.rig.core.clone();
        EditorCore::decide_trust(&core, "tester", Trust::Untrusted)?;
        wake(&mut st.rig);
        if installed(&core) || EditorCore::grants(&core).has(&flag()?, ORDINARY) {
            return Err("an untrusted project's plugin or grant took effect".into());
        }
        if EditorCore::held_security(&core).is_none() {
            return Err("not trusting dropped the held grant (a save would strip it)".into());
        }
        let t = EditorCore::project_trust(&core).ok_or("no trust state")?;
        if t.state != TrustState::Untrusted {
            return Err(format!("the state: {t:?}"));
        }
        let mut again = editor(&st.config, CoreFaults::default(), HostingFaults::default())?;
        open(&mut again, &st.location)?;
        if installed(&again.core) || EditorCore::grants(&again.core).has(&flag()?, ORDINARY) {
            return Err("a new editor ran the untrusted project's plugin".into());
        }
        if again
            .shell
            .session()
            .notifications
            .history()
            .any(|n| n.title == "Do you trust this project?")
        {
            return Err("a new editor asked again though the person answered".into());
        }
        let _ = std::fs::remove_dir_all(&st.root);
        Ok(())
    })();
    r.unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn a_headless_run_trusts_only_with_the_launch_flag() {
    let r = (|| -> Result<(), String> {
        let root = tmp("headless");
        let location = cloned_project(&root.join("cloned"))?;
        for flag_on in [false, true] {
            let core = EditorCore::new();
            let mut out = Vec::new();
            forge_editor::headless::run(
                &core,
                "prepare.txt",
                Some(std::io::Cursor::new("# nothing\n")),
                &forge_editor::headless::HeadlessOptions {
                    trust_project: flag_on,
                    ..Default::default()
                },
                &mut out,
            )
            .map_err(|e| e.to_string())?;
            // A paired device's person opens it through the headless host.
            send(
                &core,
                ProjectOp::Open {
                    location: location.clone(),
                    discard_unsaved: true,
                }
                .command(),
            )?;
            outcome(&core, forge_editor::project::OPEN_CMD)?;
            let granted = EditorCore::grants(&core).has(&flag()?, ORDINARY);
            if granted != flag_on {
                return Err(format!(
                    "with --trust-project {}: the project's grant is {}in effect",
                    if flag_on { "on" } else { "off" },
                    if granted { "" } else { "not " }
                ));
            }
        }
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    })();
    r.unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_hosting_that_ignores_trust_runs_the_projects_code() {
    let e = untrusted_open(
        "control-hosting",
        CoreFaults::default(),
        HostingFaults {
            ignore_trust: true,
            ..HostingFaults::default()
        },
    )
    .err()
    .unwrap_or_default();
    assert!(e.contains("plugin code was installed"), "{e}");
}

#[test]
fn positive_control_an_untrusted_load_takes_the_projects_grants() {
    let e = untrusted_open(
        "control-core",
        CoreFaults {
            untrusted_load_unheld: true,
            ..CoreFaults::default()
        },
        HostingFaults::default(),
    )
    .err()
    .unwrap_or_default();
    assert!(e.contains("grant is in effect"), "{e}");
}

#[test]
fn positive_control_both_run_the_projects_code_with_its_grants() {
    let e = untrusted_open(
        "control-both",
        CoreFaults {
            untrusted_load_unheld: true,
            ..CoreFaults::default()
        },
        HostingFaults {
            ignore_trust: true,
            ..HostingFaults::default()
        },
    )
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("ran with the project's grant") && e.contains("wrote example.flag"),
        "{e}"
    );
}

// ---- A wider default automation policy waits for trust too (the WP-34 verifier's follow-up)
// ------
//
// A cloned project whose default automation policy adds a capability to the standard one would
// widen what every automation session the person connects may do. Opened by a person, untrusted:
// the capability is not in effect (the load writes the standard policy's answer), the trust
// question lists it, and trusting puts it in effect. Positive control:
// `positive_control_an_untrusted_load_takes_the_projects_agent_policy` —
// `CoreFaults::untrusted_load_unheld`: the capability is in effect on open.

fn check_untrusted_policy(tag: &str, faults: CoreFaults) -> Result<(), String> {
    let root = tmp(tag);
    let config = root.join("config");
    std::fs::create_dir_all(&config).map_err(|e| e.to_string())?;
    let standard: Vec<Capability> = security::standard_automation_policy()
        .capabilities()
        .collect();
    let wider = Capability::ALL
        .into_iter()
        .find(|c| c.may_be_default() && !standard.contains(c))
        .ok_or("no default-able capability beyond the standard policy")?;
    let mut caps = standard.clone();
    caps.push(wider);
    let key = format!("{}.{}", security::AUTOMATION_POLICY_PREFIX, wider.key());
    // A teammate's editor saves the project with the wider policy.
    let location = format!("file:{}", root.join("cloned").display());
    let teammate = EditorCore::new();
    send(
        &teammate,
        ProjectOp::Create {
            location: location.clone(),
            name: "Cloned".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
    )?;
    send(&teammate, security::policy_command(&caps))?;
    send(
        &teammate,
        forge_editor::project::save_command("wider policy"),
    )?;
    outcome(&teammate, forge_editor::project::SAVE_CMD)?;
    // This person opens it; nobody here has trusted it.
    let core = EditorCore::new();
    EditorCore::set_faults(&core, faults);
    EditorCore::set_trust_book(&core, Arc::new(FileTrust::open(&config)));
    send(
        &core,
        ProjectOp::Open {
            location,
            discard_unsaved: true,
        }
        .command(),
    )?;
    outcome(&core, forge_editor::project::OPEN_CMD)?;
    let in_effect = |core: &SharedCore| {
        EditorCore::configure(core, |bus| {
            bus.project().setting(&key) == Some(&Value::Bool(true))
        })
    };
    if in_effect(&core) {
        return Err(format!(
            "an untrusted project's default automation capability {wider} is in effect on open"
        ));
    }
    let asked = EditorCore::project_trust(&core)
        .map(|t| t.lines().join("; "))
        .unwrap_or_default();
    if !asked.contains(&format!("default automation capability {wider}")) {
        return Err(format!(
            "the trust question does not list the capability: {asked:?}"
        ));
    }
    EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
    if !in_effect(&core) {
        return Err(format!("trusting did not put {wider} in effect"));
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn an_untrusted_projects_wider_automation_policy_waits_for_trust() {
    check_untrusted_policy("policy", CoreFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_untrusted_load_takes_the_projects_automation_policy() {
    let e = check_untrusted_policy(
        "policy-control",
        CoreFaults {
            untrusted_load_unheld: true,
            ..CoreFaults::default()
        },
    )
    .err()
    .unwrap_or_default();
    assert!(e.contains("is in effect on open"), "{e}");
}

// ---- Trust is keyed by content, not only the folder (WP-35) -------------------------------
//
// A trusted project's `git pull` (or another repository cloned into the trusted folder)
// brings a new plugin folder, changed code for a plugin the person trusted, and a plugin
// grant for the new plugin. Nothing of it runs or takes effect before the person answers;
// what they trusted keeps running; they are asked again, shown what changed; trusting the
// changes runs them; and content that did not change never asks again — nor does the person's
// own change in the editor.
//
// Positive control: `positive_control_trust_by_folder_runs_what_a_pull_brought` —
// `CoreFaults::trust_by_folder` (the WP-34 key): the pulled plugin installs, the changed code
// reloads and runs, and the pulled grant is in effect, with no question answered.

const SECOND_MANIFEST: &str = r#"Plugin(id: "com.example.second", version: "0.1.0", engine: "^0.1",
    kind: Wasm, provides: [Command("example.second")], capabilities: [Command(Ordinary)])"#;
const CHANGED_TITLE: &str = "This project changed since you trusted it";
const ASK_TITLE: &str = "Do you trust this project?";

fn second() -> Result<Principal, String> {
    PluginId::new("com.example.second")
        .map(Principal::Plugin)
        .map_err(|e| e.to_string())
}

fn plugin_code(dir: &Path, manifest: &str, key: &str, value: i64) -> Result<(), String> {
    write(dir, "plugin.ron", manifest)?;
    let plan =
        format!(r#"{{"ops":[{{"op":"set_setting","key":"{key}","value":{{"Int":{value}}}}}]}}"#);
    write(
        dir,
        "plugin.wat",
        &forge_wasm::wat::by_point(&[("Command", plan.as_bytes())]),
    )
}

fn has_target(core: &SharedCore, target: &str) -> bool {
    EditorCore::configure(core, |bus| bus.targets().any(|t| t == target))
}

fn run_as_person(core: &SharedCore, target: &str) -> Option<String> {
    send(
        core,
        EditorCommand::Invoke {
            target: target.into(),
            args: "{}".into(),
        },
    )
    .err()
}

fn setting(core: &SharedCore, key: &str) -> Option<Value> {
    EditorCore::read(core, |p| p.setting(key).cloned())
}

fn asked(rig: &Rig, title: &str) -> usize {
    rig.shell
        .session()
        .notifications
        .history()
        .filter(|n| n.title == title)
        .count()
}

/// The pull, done on disk as `git pull` would: `plugins/flag`'s code now writes 22, a new
/// `plugins/second` folder, and (a teammate's editor saved them) a grant for the new plugin
/// and a cached plugin added to the set (which the person's cache under `config` holds).
fn pull(project: &Path, location: &str, config: &Path) -> Result<(), String> {
    cache_it(config)?;
    plugin_code(
        &project.join("plugins").join("flag"),
        FLAG_MANIFEST,
        "example.flag",
        22,
    )?;
    plugin_code(
        &project.join("plugins").join("second"),
        SECOND_MANIFEST,
        "example.second",
        7,
    )?;
    let teammate = EditorCore::new();
    EditorCore::set_trust_book(&teammate, Arc::new(forge_editor::trust::TrustEvery));
    send(
        &teammate,
        ProjectOp::Open {
            location: location.to_string(),
            discard_unsaved: true,
        }
        .command(),
    )?;
    outcome(&teammate, forge_editor::project::OPEN_CMD)?;
    send(&teammate, security::grant_command(&second()?, ORDINARY))?;
    send(
        &teammate,
        security::plugin_add_command("com.example.cached", "0.1.0", "local"),
    )?;
    send(&teammate, forge_editor::project::save_command("second"))?;
    outcome(&teammate, forge_editor::project::SAVE_CMD)?;
    Ok(())
}

/// Trust the cloned project in `st`'s editor, and check the flag plugin runs its first code.
fn trust_first(st: &mut Setup) -> Result<(), String> {
    open(&mut st.rig, &st.location)?;
    let core = st.rig.core.clone();
    EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
    wake(&mut st.rig);
    if let Some(r) = run_as_person(&core, "example.flag") {
        return Err(format!("setup: the trusted plugin was refused: {r}"));
    }
    if setting(&core, "example.flag") != Some(Value::Int(1)) {
        return Err("setup: the trusted plugin's first code did not run".into());
    }
    Ok(())
}

/// After the pull, before any answer: every way what the pull brought could run or take
/// effect. `Err` lists each one that happened.
fn check_pulled_waits(st: &mut Setup) -> Result<(), String> {
    let core = st.rig.core.clone();
    let mut broken = Vec::new();
    // The editor was open during the pull: the next wake.
    wake(&mut st.rig);
    if has_target(&core, "example.second") {
        broken.push("the pulled plugin was installed before a decision".to_string());
    }
    run_as_person(&core, "example.flag");
    if setting(&core, "example.flag") == Some(Value::Int(22)) {
        broken.push("the flag plugin's changed code ran before a decision".to_string());
    }
    // The person reopens the pulled project.
    open(&mut st.rig, &st.location)?;
    if EditorCore::grants(&core).has(&second()?, ORDINARY) {
        broken.push("the pulled grant is in effect before a decision".to_string());
    }
    if has_target(&core, "example.second") {
        broken.push("the pulled plugin was installed after the reopen".to_string());
    }
    if has_target(&core, "example.cached") {
        broken.push("the cached plugin the pull named was installed".to_string());
    }
    if !broken.is_empty() {
        return Err(broken.join("; "));
    }
    // What the person trusted keeps running and in effect.
    if !EditorCore::grants(&core).has(&flag()?, ORDINARY) {
        return Err("the grant the person trusted was held after the pull".into());
    }
    if let Some(r) = run_as_person(&core, "example.flag") {
        return Err(format!("the trusted plugin stopped running: {r}"));
    }
    // Asked again, shown what changed.
    let t = EditorCore::project_trust(&core).ok_or("no trust question after the pull")?;
    if t.state != TrustState::Changed {
        return Err(format!("the state after the pull: {t:?}"));
    }
    let changed = t.changed.join("; ");
    for want in [
        "plugins/second is new",
        "plugins/flag changed",
        "to plugin:com.example.second is new",
        "com.example.cached 0.1.0 (from the plugin cache) is new",
    ] {
        if !changed.contains(want) {
            return Err(format!("the question does not say {want:?}: {changed}"));
        }
    }
    if changed.contains("to plugin:com.example.flag") {
        return Err(format!(
            "an unchanged grant is listed as changed: {changed}"
        ));
    }
    if asked(&st.rig, CHANGED_TITLE) == 0 {
        return Err("the person was not asked again".into());
    }
    let held = EditorCore::held_security(&core).ok_or("the pulled grant is not held")?;
    if !held.untrusted || held.items.len() != 1 {
        return Err(format!("the held proposal: {held:?}"));
    }
    Ok(())
}

fn pulled(tag: &str, faults: CoreFaults) -> Result<Setup, String> {
    let mut st = setup(tag, faults, HostingFaults::default())?;
    trust_first(&mut st)?;
    pull(&st.project, &st.location, &st.config)?;
    check_pulled_waits(&mut st)?;
    Ok(st)
}

#[test]
fn a_pull_into_a_trusted_project_asks_again_before_it_runs() {
    let r = (|| -> Result<(), String> {
        let mut st = pulled("pull", CoreFaults::default())?;
        let core = st.rig.core.clone();
        // A new editor with the same user config: what changed does not load either.
        let mut fresh = editor(&st.config, CoreFaults::default(), HostingFaults::default())?;
        open(&mut fresh, &st.location)?;
        if has_target(&fresh.core, "example.flag") || has_target(&fresh.core, "example.second") {
            return Err(
                "a new editor loaded a plugin that changed since the person trusted it".into(),
            );
        }
        if EditorCore::grants(&fresh.core).has(&second()?, ORDINARY)
            || !EditorCore::grants(&fresh.core).has(&flag()?, ORDINARY)
        {
            return Err("a new editor's grants are not what the person trusted".into());
        }
        if asked(&fresh, CHANGED_TITLE) != 1 {
            return Err("a new editor did not ask about the changes".into());
        }
        // The person trusts the changes: the held grant is accepted, the new plugin installs,
        // the changed code reloads.
        let said = EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
        if !said.contains("accepted") {
            return Err(format!(
                "trusting the changes did not accept the grant: {said}"
            ));
        }
        wake(&mut st.rig);
        if !has_target(&core, "example.second") || !has_target(&core, "example.cached") {
            return Err("the trusted new plugins did not install".into());
        }
        if run_as_person(&core, "example.second").is_some()
            || setting(&core, "example.second") != Some(Value::Int(7))
        {
            return Err("the trusted new plugin's command did not apply".into());
        }
        run_as_person(&core, "example.flag");
        if setting(&core, "example.flag") != Some(Value::Int(22)) {
            return Err("the trusted changed code did not reload".into());
        }
        // Unchanged content never asks again: reopening, and in a new editor.
        let before = (asked(&st.rig, CHANGED_TITLE), asked(&st.rig, ASK_TITLE));
        open(&mut st.rig, &st.location)?;
        wake(&mut st.rig);
        if (asked(&st.rig, CHANGED_TITLE), asked(&st.rig, ASK_TITLE)) != before {
            return Err("reopening unchanged content asked again".into());
        }
        let t = EditorCore::project_trust(&core).ok_or("no trust state")?;
        if t.state != TrustState::Trusted || EditorCore::held_security(&core).is_some() {
            return Err(format!("unchanged content is not trusted: {t:?}"));
        }
        let mut again = editor(&st.config, CoreFaults::default(), HostingFaults::default())?;
        open(&mut again, &st.location)?;
        if asked(&again, CHANGED_TITLE) + asked(&again, ASK_TITLE) != 0 {
            return Err("a new editor asked about unchanged content".into());
        }
        if !has_target(&again.core, "example.second")
            || !EditorCore::grants(&again.core).has(&second()?, ORDINARY)
        {
            return Err("a new editor did not run what the person trusted".into());
        }
        let _ = std::fs::remove_dir_all(&st.root);
        Ok(())
    })();
    r.unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_trust_by_folder_runs_what_a_pull_brought() {
    let e = pulled(
        "pull-control",
        CoreFaults {
            trust_by_folder: true,
            ..CoreFaults::default()
        },
    )
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("pulled plugin was installed before a decision")
            && e.contains("changed code ran")
            && e.contains("pulled grant is in effect"),
        "{e}"
    );
}

/// The person's own change in the editor is theirs: a grant they make, a plugin they add to
/// the set (at the version they chose), their automation policy — none asks again when they
/// reopen the project, or open it in a new editor. The same change made by an automation session
/// does.
#[test]
fn a_persons_own_change_never_asks_again() {
    let r = (|| -> Result<(), String> {
        let mut st = setup("own", CoreFaults::default(), HostingFaults::default())?;
        trust_first(&mut st)?;
        let core = st.rig.core.clone();
        send(&core, security::grant_command(&second()?, ORDINARY))?;
        send(
            &core,
            security::plugin_add_command("com.example.mine", "1.0.0", "local"),
        )?;
        let standard: Vec<Capability> = security::standard_automation_policy()
            .capabilities()
            .collect();
        let wider = Capability::ALL
            .into_iter()
            .find(|c| c.may_be_default() && !standard.contains(c))
            .ok_or("no wider default capability")?;
        let mut caps = standard.clone();
        caps.push(wider);
        send(&core, security::policy_command(&caps))?;
        send(&core, forge_editor::project::save_command("mine"))?;
        let before = asked(&st.rig, CHANGED_TITLE);
        open(&mut st.rig, &st.location)?;
        let t = EditorCore::project_trust(&core).ok_or("no trust state")?;
        if t.state != TrustState::Trusted
            || EditorCore::held_security(&core).is_some()
            || asked(&st.rig, CHANGED_TITLE) != before
        {
            return Err(format!("the person's own change asked again: {t:?}"));
        }
        let mut again = editor(&st.config, CoreFaults::default(), HostingFaults::default())?;
        open(&mut again, &st.location)?;
        if asked(&again, CHANGED_TITLE) + asked(&again, ASK_TITLE) != 0
            || !EditorCore::grants(&again.core).has(&second()?, ORDINARY)
        {
            return Err("a new editor asked about the person's own change".into());
        }
        // An automation session's addition to the plugin set is not the person's: asked on the next
        // open.
        let mut auto = EditorCore::connect(
            &core,
            Issuer::Automation {
                session: "auto-1".into(),
                tool: "apply".into(),
            },
        );
        auto.apply(
            security::plugin_add_command("com.example.theirs", "1.0.0", "local"),
            None,
        );
        if let Some(r) = auto.pump().refused.first() {
            return Err(format!("setup: the session's plugin add: {}", r.rejection));
        }
        send(&core, forge_editor::project::save_command("theirs"))?;
        open(&mut st.rig, &st.location)?;
        let t = EditorCore::project_trust(&core).ok_or("no trust state")?;
        if t.state != TrustState::Changed || !t.changed.join("; ").contains("com.example.theirs") {
            return Err(format!(
                "an automation session's plugin-set change was taken as the person's: {t:?}"
            ));
        }
        let _ = std::fs::remove_dir_all(&st.root);
        Ok(())
    })();
    r.unwrap_or_else(|e| panic!("{e}"));
}

// ---- A cached plugin version the project's set names (the WP-34 verifier's gap) -----------
//
// An untrusted project whose plugin set names `com.example.cached` 0.1.0, which is in the
// person's plugin cache (`<config>/plugins/com.example.cached/0.1.0/`), and grants it: opened
// by the person, the cached plugin is not loaded (the hosting's scan checks the cache branch
// against trust — `scan`'s `if trusted` there: breaking it to `if true || trusted` fails this
// test, as `positive_control_a_hosting_that_ignores_trust_runs_the_cached_plugin` does
// through `HostingFaults::ignore_trust`), the question lists it, and trusting runs it.

const CACHED_MANIFEST: &str = r#"Plugin(id: "com.example.cached", version: "0.1.0", engine: "^0.1",
    kind: Wasm, provides: [Command("example.cached")], capabilities: [Command(Ordinary)])"#;

fn cached() -> Result<Principal, String> {
    PluginId::new("com.example.cached")
        .map(Principal::Plugin)
        .map_err(|e| e.to_string())
}

fn cache_it(config: &Path) -> Result<(), String> {
    plugin_code(
        &config
            .join("plugins")
            .join("com.example.cached")
            .join("0.1.0"),
        CACHED_MANIFEST,
        "example.cached",
        5,
    )
}

/// A teammate's project naming the cached plugin (and granting it), and the plugin in the
/// person's cache under `config`.
fn cached_project(dir: &Path, config: &Path) -> Result<String, String> {
    cache_it(config)?;
    let location = format!("file:{}", dir.display());
    let teammate = EditorCore::new();
    send(
        &teammate,
        ProjectOp::Create {
            location: location.clone(),
            name: "Cached".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
    )?;
    outcome(&teammate, forge_editor::project::CREATE_CMD)?;
    send(
        &teammate,
        security::plugin_add_command("com.example.cached", "0.1.0", "local"),
    )?;
    send(&teammate, security::grant_command(&cached()?, ORDINARY))?;
    send(&teammate, forge_editor::project::save_command("cached"))?;
    outcome(&teammate, forge_editor::project::SAVE_CMD)?;
    Ok(location)
}

fn check_cached(tag: &str, hosting: HostingFaults) -> Result<(), String> {
    let root = tmp(tag);
    let config = root.join("config");
    let location = cached_project(&root.join("cached"), &config)?;
    let mut rig = editor(&config, CoreFaults::default(), hosting)?;
    open(&mut rig, &location)?;
    let core = rig.core.clone();
    if has_target(&core, "example.cached") {
        return Err("the cached plugin the untrusted project names was installed".into());
    }
    if run_as_person(&core, "example.cached").is_none()
        || setting(&core, "example.cached").is_some()
    {
        return Err("the cached plugin ran for an untrusted project".into());
    }
    let t = EditorCore::project_trust(&core).ok_or("no trust question")?;
    if !t
        .plugins
        .iter()
        .any(|p| p == "com.example.cached 0.1.0 (from the plugin cache)")
    {
        return Err(format!(
            "the question does not list the cached plugin: {t:?}"
        ));
    }
    EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
    wake(&mut rig);
    if run_as_person(&core, "example.cached").is_some()
        || setting(&core, "example.cached") != Some(Value::Int(5))
    {
        return Err("the trusted project's cached plugin did not run".into());
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn an_untrusted_projects_cached_plugin_version_is_not_loaded() {
    check_cached("cached", HostingFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_hosting_that_ignores_trust_runs_the_cached_plugin() {
    let e = check_cached(
        "cached-control",
        HostingFaults {
            ignore_trust: true,
            ..HostingFaults::default()
        },
    )
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("cached plugin the untrusted project names was installed"),
        "{e}"
    );
}

// ---- The start-up trust gate (`assemble_hosted`) --------------------------------------------
//
// When a project is already open on the core the editor assembles over (a start-up open of a
// project, or a core shared with another shell), `assemble_hosted` loads the project's own
// plugins — its `plugins/` folders and the cached versions its set names — only as the person
// trusted them, before the editor's first turn: an untrusted project's are not installed and
// the question lists them; trusted, they are installed at start; after a pull that changed a
// plugin's code, that plugin is not, the rest is. Positive control:
// `positive_control_trust_by_folder_at_startup_loads_changed_code` (`CoreFaults::trust_by_folder`).

/// A core with the project at `location` open (as a start-up open would leave it), then an
/// editor assembled over it — nothing has turned yet.
fn start_with_open(
    config: &Path,
    location: &str,
    faults: CoreFaults,
) -> Result<(SharedCore, Rig), String> {
    let core = trusting_core(config, faults);
    send(
        &core,
        ProjectOp::Open {
            location: location.to_string(),
            discard_unsaved: true,
        }
        .command(),
    )?;
    outcome(&core, forge_editor::project::OPEN_CMD)?;
    let rig = editor_on(core.clone(), config, HostingFaults::default())?;
    Ok((core, rig))
}

fn check_startup(tag: &str, faults: CoreFaults) -> Result<(), String> {
    let root = tmp(tag);
    let config = root.join("config");
    let project = root.join("cloned");
    // The cloned project carries `plugins/flag` and names the cached plugin too.
    let location = cloned_project(&project)?;
    cache_it(&config)?;
    let teammate = EditorCore::new();
    EditorCore::set_trust_book(&teammate, Arc::new(forge_editor::trust::TrustEvery));
    send(
        &teammate,
        ProjectOp::Open {
            location: location.clone(),
            discard_unsaved: true,
        }
        .command(),
    )?;
    outcome(&teammate, forge_editor::project::OPEN_CMD)?;
    send(
        &teammate,
        security::plugin_add_command("com.example.cached", "0.1.0", "local"),
    )?;
    send(&teammate, forge_editor::project::save_command("cached"))?;
    outcome(&teammate, forge_editor::project::SAVE_CMD)?;

    // Untrusted at start: neither is installed, the question lists both.
    let (core, _rig) = start_with_open(&config, &location, faults)?;
    if has_target(&core, "example.flag") || has_target(&core, "example.cached") {
        return Err("an untrusted project's plugin was installed at start".into());
    }
    let t = EditorCore::project_trust(&core).ok_or("no trust question at start")?;
    let lines = t.lines().join("; ");
    if !lines.contains("plugins/flag") || !lines.contains("com.example.cached 0.1.0") {
        return Err(format!("the start-up question: {lines}"));
    }
    EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;

    // Trusted at start: both installed before the first turn.
    let (core, _rig) = start_with_open(&config, &location, faults)?;
    if !has_target(&core, "example.flag") || !has_target(&core, "example.cached") {
        return Err("a trusted project's plugins were not installed at start".into());
    }

    // A pull changed the flag plugin's code: at start it is not installed, the cached one is.
    plugin_code(
        &project.join("plugins").join("flag"),
        FLAG_MANIFEST,
        "example.flag",
        22,
    )?;
    let (core, _rig) = start_with_open(&config, &location, faults)?;
    if has_target(&core, "example.flag") {
        return Err(
            "a plugin whose code changed since the person trusted it was installed at start".into(),
        );
    }
    if !has_target(&core, "example.cached") {
        return Err("an unchanged trusted plugin was not installed at start".into());
    }
    let t = EditorCore::project_trust(&core).ok_or("no trust question at start")?;
    if t.state != TrustState::Changed || !t.changed.join("; ").contains("plugins/flag changed") {
        return Err(format!("the start-up question after the pull: {t:?}"));
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn the_startup_gate_loads_an_open_projects_plugins_only_as_trusted() {
    check_startup("startup", CoreFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_trust_by_folder_at_startup_loads_changed_code() {
    let e = check_startup(
        "startup-control",
        CoreFaults {
            trust_by_folder: true,
            ..CoreFaults::default()
        },
    )
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("changed since the person trusted it was installed at start"),
        "{e}"
    );
}

// ---- A team pull into a trusted project folder (WP-36, the WP-35 verifier's gap) ----------
//
// Team sandboxes joined from a team have no project folder, so no earlier test reached the
// team pull's trust gate (`collab_host.rs`'s `let trusted = pulled.iter().all(..)`): breaking
// it to `true || ..` left every test green. Here the person's project **has** a folder and is
// trusted (they created it in this editor); they make a team of it, a teammate joins, grants a
// plugin and adds a cached plugin to the set, and publishes. The person's pull:
// * does not put the pulled grant in effect — it is held, marked as held for trust;
// * asks again, listing the grant and the cached plugin as new since they trusted it;
// * and trusting the change accepts the grant.
// A later pull that brings nothing new to trust asks nothing (the gate is not a blanket
// refusal). Positive control: `positive_control_trust_by_folder_takes_what_a_team_pull_brought`
// (`CoreFaults::trust_by_folder`: the folder's answer covers whatever the pull brings). The
// `true ||` break of the gate itself fails
// `a_team_pull_into_a_trusted_project_asks_before_it_takes_effect` the same way (observed when
// this test was written, WP-36).

mod team {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use forge_cmd::{EditorCommand, Issuer};
    use forge_editor::client::BusClient;
    use forge_editor::collab::licence::MemoryEntitlement;
    use forge_editor::collab::{self as c};
    use forge_editor::core::{CollabAttach, EditorCore, SharedCore};
    use forge_project::collab::{
        CollabView, IdentityBackend, InviteTo, MemoryCollab, MemoryIdentity, Role,
    };

    const NOW: u64 = 20_000 * 86_400_000;

    pub struct Server {
        identity: Arc<MemoryIdentity>,
        server: Arc<MemoryCollab>,
        licence: Arc<MemoryEntitlement>,
    }

    pub fn server() -> Server {
        static N: AtomicU64 = AtomicU64::new(0);
        let identity = Arc::new(MemoryIdentity::new());
        let name = format!(
            "wp36-team-trust-{}-{}",
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

    pub fn attach(s: &Server, core: &SharedCore, user: &str) {
        EditorCore::attach_collab(
            core,
            CollabAttach {
                identity: s.identity.clone(),
                server: s.server.clone(),
                licence: s.licence.clone(),
                owner: user.into(),
                email: Some(format!("{user}@studio.example")),
            },
        );
    }

    /// Send a command as `user`; its refusal or its failed team outcome, if any.
    pub fn send(core: &SharedCore, user: &str, cmd: EditorCommand) -> Result<(), String> {
        let before = EditorCore::collab_status(core).map_or(0, |s| s.outcomes.len());
        let mut ui = EditorCore::connect(core, Issuer::Human { user: user.into() });
        ui.apply(cmd, None);
        if let Some(r) = ui.pump().refused.first() {
            return Err(r.rejection.to_string());
        }
        EditorCore::wait_transfers(core);
        let st = EditorCore::collab_status(core);
        match st.as_ref().and_then(|s| s.outcomes.last()) {
            Some(o) if st.as_ref().is_some_and(|s| s.outcomes.len() != before) && !o.ok => {
                Err(o.message.clone())
            }
            _ => Ok(()),
        }
    }

    /// `who` at `joiner` joins the team `by` made at `owner`, by a join code.
    pub fn join(
        s: &Server,
        owner: &SharedCore,
        by: &str,
        joiner: &SharedCore,
        who: &str,
    ) -> Result<(), String> {
        send(owner, by, c::invite_command(None, Role::Developer, &[]))?;
        let team = s.server.team().ok_or("no team")?;
        let code = s
            .identity
            .team_record(&team)
            .and_then(|t| {
                t.invites.iter().rev().find_map(|i| match &i.to {
                    InviteTo::Code(c) => Some(c.clone()),
                    InviteTo::Email(_) => None,
                })
            })
            .ok_or("no join code")?;
        send(joiner, who, c::accept_command(&code))
    }
}

fn check_team_pull(tag: &str, faults: CoreFaults) -> Result<(), String> {
    use forge_editor::collab as c;
    let root = tmp(tag);
    let config = root.join("config");
    std::fs::create_dir_all(&config).map_err(|e| e.to_string())?;
    let location = format!("file:{}", root.join("studio").display());
    // The person's own project, in a folder: trusted from the start.
    let core = trusting_core(&config, faults);
    let s = team::server();
    team::attach(&s, &core, "tester");
    send(
        &core,
        ProjectOp::Create {
            location: location.clone(),
            name: "Studio".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
    )?;
    outcome(&core, forge_editor::project::CREATE_CMD)?;
    team::send(&core, "tester", c::create_command("Studio"))?;
    if EditorCore::read_location(&core).as_deref() != Some(location.as_str()) {
        return Err("setup: the team's project is not the person's folder".into());
    }
    // A teammate joins, grants a plugin, adds a cached plugin to the set, and publishes.
    let bob = EditorCore::new();
    team::attach(&s, &bob, "bob");
    team::join(&s, &core, "tester", &bob, "bob")?;
    team::send(&bob, "bob", security::grant_command(&second()?, ORDINARY))?;
    team::send(
        &bob,
        "bob",
        security::plugin_add_command("com.example.cached", "0.1.0", "local"),
    )?;
    team::send(&bob, "bob", c::publish_command("grant second"))?;
    // The person pulls.
    team::send(&core, "tester", c::pull_command(None))?;
    let mut broken = Vec::new();
    if EditorCore::grants(&core).has(&second()?, ORDINARY) {
        broken.push("the pulled grant is in effect before a decision".to_string());
    }
    let t = EditorCore::project_trust(&core);
    let changed = t.as_ref().map(|t| t.changed.join("; ")).unwrap_or_default();
    if t.as_ref().map(|t| t.state) != Some(TrustState::Changed) {
        broken.push(format!("the person was not asked about the pull: {t:?}"));
    }
    for want in [
        "to plugin:com.example.second is new",
        "com.example.cached 0.1.0 (from the plugin cache) is new",
    ] {
        if !changed.contains(want) {
            broken.push(format!("the question does not say {want:?}: {changed}"));
        }
    }
    if !broken.is_empty() {
        return Err(broken.join("; "));
    }
    let grant_key = security::grant_key(&second()?, ORDINARY).ok_or("no grant key")?;
    let held = EditorCore::held_security(&core).ok_or("the pulled grant is not held")?;
    if !held.untrusted || !held.items.iter().any(|i| i.key == grant_key) {
        return Err(format!("the held proposal: {held:?}"));
    }
    // Trusting the change accepts the grant.
    let said = EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
    if !said.contains("accepted") || !EditorCore::grants(&core).has(&second()?, ORDINARY) {
        return Err(format!(
            "trusting the pulled change did not accept it: {said}"
        ));
    }
    // A pull that brings nothing new to trust asks nothing.
    let entities = EditorCore::read(&core, forge_cmd::Project::len);
    team::send(
        &bob,
        "bob",
        EditorCommand::Spawn {
            name: "Lamp".into(),
            parent: None,
        },
    )?;
    team::send(&bob, "bob", c::publish_command("lamp"))?;
    team::send(&core, "tester", c::pull_command(None))?;
    if EditorCore::read(&core, forge_cmd::Project::len) <= entities {
        return Err("setup: the second pull brought nothing".into());
    }
    let t = EditorCore::project_trust(&core).ok_or("no trust state")?;
    if t.state != TrustState::Trusted || EditorCore::held_security(&core).is_some() {
        return Err(format!(
            "a pull with nothing new to trust asked again: {t:?}"
        ));
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn a_team_pull_into_a_trusted_project_asks_before_it_takes_effect() {
    check_team_pull("team-pull", CoreFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_trust_by_folder_takes_what_a_team_pull_brought() {
    let e = check_team_pull(
        "team-pull-control",
        CoreFaults {
            trust_by_folder: true,
            ..CoreFaults::default()
        },
    )
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("pulled grant is in effect before a decision")
            && e.contains("was not asked about the pull"),
        "{e}"
    );
}

// ---- What the trust gate vetted is what runs (WP-36, the WP-35 verifier's TOCTOU) --------
//
// The hosting's scan hashes a project plugin folder for the trust gate; before WP-36 the load
// and the hot-reload watcher then read the files **again** and compiled what they found, so a
// `git pull` landing between the two reads ran code nobody vetted, once. Now the load
// compiles the one read whose digest the gate approved and the watcher shows the gate the
// bytes it would swap in. `HostingFaults::swap_after_vet` is the seam: a writer replaces each
// approved folder's code right after the gate's look — before the load, and before the
// watcher reads.
// * a trusted, loaded plugin: the swapped code is not reloaded (the code the person trusted
//   keeps serving) and the person is asked about the change;
// * a plugin trusted but not installed yet: the swapped code is not installed.
// Positive control: `positive_control_unchecked_reads_run_the_swapped_code`
// (`HostingFaults::unchecked_reads`, the pre-WP-36 reads: no digest comparison at the load, no
// vet at the hot reload): both run the swapped code.

/// The code a writer swaps in after the gate's look: its command writes 33.
fn swapped() -> &'static str {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        let plan = r#"{"ops":[{"op":"set_setting","key":"example.flag","value":{"Int":33}}]}"#;
        forge_wasm::wat::by_point(&[("Command", plan.as_bytes())])
    })
}

fn set_hosting_faults(rig: &Rig, f: HostingFaults) {
    rig.shell.handles().services().hosting.borrow_mut().faults = f;
}

fn hosting_stats(rig: &Rig) -> forge_editor::hosting::HostingStats {
    rig.shell.handles().services().hosting.borrow().stats()
}

/// `Err` lists each way the swapped code ran.
fn check_swap(tag: &str, unchecked: bool) -> Result<(), String> {
    let base = HostingFaults {
        unchecked_reads: unchecked,
        ..HostingFaults::default()
    };
    let seam = HostingFaults {
        swap_after_vet: Some(swapped()),
        ..base
    };
    let mut broken = Vec::new();

    // A trusted plugin, loaded and running: the swap lands before the watcher reads.
    let mut st = setup(&format!("{tag}-reload"), CoreFaults::default(), base)?;
    trust_first(&mut st)?;
    let core = st.rig.core.clone();
    set_hosting_faults(&st.rig, seam);
    wake(&mut st.rig);
    set_hosting_faults(&st.rig, base);
    run_as_person(&core, "example.flag");
    if setting(&core, "example.flag") == Some(Value::Int(33)) {
        broken.push("the watcher reloaded code swapped in after the vet".to_string());
    } else {
        if setting(&core, "example.flag") != Some(Value::Int(1)) {
            return Err("the trusted code stopped serving after the swap".into());
        }
        let t = EditorCore::project_trust(&core).ok_or("no trust state after the swap")?;
        if t.state != TrustState::Changed || !t.changed.join("; ").contains("plugins/flag changed")
        {
            return Err(format!("the person was not asked about the swap: {t:?}"));
        }
    }
    let _ = std::fs::remove_dir_all(&st.root);

    // A plugin the person just trusted, not installed yet: the swap lands before the load.
    let mut st = untrusted_open(&format!("{tag}-install"), CoreFaults::default(), base)?;
    let core = st.rig.core.clone();
    EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
    set_hosting_faults(&st.rig, seam);
    wake(&mut st.rig);
    set_hosting_faults(&st.rig, base);
    if installed(&core) {
        run_as_person(&core, "example.flag");
        if setting(&core, "example.flag") == Some(Value::Int(33)) {
            broken.push("the load installed code swapped in after the vet".to_string());
        }
    } else {
        // The next wake vets the folder as it is now: changed since trusted, asked.
        wake(&mut st.rig);
        if installed(&core) {
            return Err("the swapped folder installed without a decision".into());
        }
        let t = EditorCore::project_trust(&core).ok_or("no trust state after the swap")?;
        if t.state != TrustState::Changed || !t.changed.join("; ").contains("plugins/flag changed")
        {
            return Err(format!("the person was not asked about the swap: {t:?}"));
        }
    }
    let _ = std::fs::remove_dir_all(&st.root);
    if broken.is_empty() {
        Ok(())
    } else {
        Err(broken.join("; "))
    }
}

#[test]
fn code_swapped_in_after_the_trust_gate_looked_never_runs() {
    check_swap("swap", false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_unchecked_reads_run_the_swapped_code() {
    let e = check_swap("swap-control", true).err().unwrap_or_default();
    assert!(
        e.contains("watcher reloaded code swapped in")
            && e.contains("load installed code swapped in"),
        "{e}"
    );
}

// ---- The bytes checked are the bytes compiled (WP-36, the WP-36 verifier's gap) -----------
//
// `swap_after_vet` lands before the read, so it shows the check at the read (the digest
// comparison, the watcher's vet) — not that the checked bytes are the ones compiled. Here the
// writer lands **after** the check: `HostingFaults::swap_after_check` replaces the folder's
// code once the load's digest comparison passed, and once the watcher's vet allowed a change
// the person trusted — before either compiles. The bytes checked must run, and the write is a
// change of its own, asked about at the next poll.
// * a plugin the person just trusted, not installed yet: the trusted code installs;
// * a trusted plugin whose change the person just trusted: the trusted change reloads.
// Positive control: `positive_control_reading_again_after_the_check_runs_the_later_write`
// (`HostingFaults::reread_after_vet`: the load and the watcher check what they read, then read
// the files again and compile that): both run the later write.

/// The write after the check was asked about at the next wake and never ran.
fn later_write_asked(st: &mut Setup, want: i64) -> Result<(), String> {
    let core = st.rig.core.clone();
    wake(&mut st.rig);
    run_as_person(&core, "example.flag");
    if setting(&core, "example.flag") != Some(Value::Int(want)) {
        return Err("the write after the check ran at the next poll".into());
    }
    let t = EditorCore::project_trust(&core).ok_or("no trust state after the write")?;
    if t.state != TrustState::Changed || !t.changed.join("; ").contains("plugins/flag changed") {
        return Err(format!("the person was not asked about the write: {t:?}"));
    }
    Ok(())
}

/// `Err` lists each way the later write ran.
fn check_write_after_check(tag: &str, reread: bool) -> Result<(), String> {
    let base = HostingFaults {
        reread_after_vet: reread,
        ..HostingFaults::default()
    };
    let seam = HostingFaults {
        swap_after_check: Some(swapped()),
        ..base
    };
    let mut broken = Vec::new();

    // A plugin the person just trusted, not installed yet: the write lands after the load's
    // digest comparison passed.
    let mut st = untrusted_open(&format!("{tag}-install"), CoreFaults::default(), base)?;
    let core = st.rig.core.clone();
    EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
    set_hosting_faults(&st.rig, seam);
    wake(&mut st.rig);
    set_hosting_faults(&st.rig, base);
    if !installed(&core) {
        return Err("the trusted plugin did not install".into());
    }
    run_as_person(&core, "example.flag");
    match setting(&core, "example.flag") {
        Some(Value::Int(33)) => {
            broken.push("the load compiled code written after its check".to_string());
        }
        Some(Value::Int(1)) => later_write_asked(&mut st, 1)?,
        other => return Err(format!("the trusted code did not run: {other:?}")),
    }
    let _ = std::fs::remove_dir_all(&st.root);

    // A trusted plugin, loaded and running, whose change the person trusts: the write lands
    // after the watcher's vet allowed the change.
    let mut st = setup(&format!("{tag}-reload"), CoreFaults::default(), base)?;
    trust_first(&mut st)?;
    let core = st.rig.core.clone();
    let flag_dir = st.project.join("plugins").join("flag");
    plugin_code(&flag_dir, FLAG_MANIFEST, "example.flag", 22)?;
    wake(&mut st.rig);
    run_as_person(&core, "example.flag");
    if setting(&core, "example.flag") != Some(Value::Int(1)) {
        return Err("setup: the change ran before it was trusted".into());
    }
    EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
    set_hosting_faults(&st.rig, seam);
    wake(&mut st.rig);
    set_hosting_faults(&st.rig, base);
    run_as_person(&core, "example.flag");
    match setting(&core, "example.flag") {
        Some(Value::Int(33)) => {
            broken.push("the watcher reloaded code written after its vet".to_string());
        }
        Some(Value::Int(22)) => later_write_asked(&mut st, 22)?,
        other => return Err(format!("the trusted change did not reload: {other:?}")),
    }
    let _ = std::fs::remove_dir_all(&st.root);
    if broken.is_empty() {
        Ok(())
    } else {
        Err(broken.join("; "))
    }
}

#[test]
fn code_written_after_the_check_never_runs_the_checked_bytes_do() {
    check_write_after_check("after-check", false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_reading_again_after_the_check_runs_the_later_write() {
    let e = check_write_after_check("after-check-control", true)
        .err()
        .unwrap_or_default();
    assert!(
        e.contains("load compiled code written after its check")
            && e.contains("watcher reloaded code written after its vet"),
        "{e}"
    );
}

// ---- An idle poll asks the trust gate nothing (WP-36) -------------------------------------
//
// The hosting keeps the gate's answers against the core's trust version
// (`EditorCore::trust_version`) and the open project: a trusted project whose plugins did not
// change costs no core lock per plugin per poll (`HostingStats::vets` counts the questions).
// The cache hides no change: a pull that changes the plugin's code is asked about at the next
// poll (a new digest), and its code waits. Positive control:
// `positive_control_without_the_cache_every_poll_asks` (`HostingFaults::no_vet_cache`).

fn check_idle_vets(tag: &str, hosting: HostingFaults) -> Result<(), String> {
    let root = tmp(tag);
    let config = root.join("config");
    let project = root.join("cloned");
    let location = cloned_project(&project)?;
    // It also names a cached plugin: two plugins for the gate.
    cache_it(&config)?;
    let teammate = EditorCore::new();
    EditorCore::set_trust_book(&teammate, Arc::new(forge_editor::trust::TrustEvery));
    send(
        &teammate,
        ProjectOp::Open {
            location: location.clone(),
            discard_unsaved: true,
        }
        .command(),
    )?;
    send(
        &teammate,
        security::plugin_add_command("com.example.cached", "0.1.0", "local"),
    )?;
    send(&teammate, forge_editor::project::save_command("cached"))?;
    let mut rig = editor(&config, CoreFaults::default(), hosting)?;
    open(&mut rig, &location)?;
    let core = rig.core.clone();
    EditorCore::decide_trust(&core, "tester", Trust::Trusted)?;
    wake(&mut rig);
    if !installed(&core) || !has_target(&core, "example.cached") {
        return Err("setup: the trusted project's plugins did not install".into());
    }
    wake(&mut rig);
    let before = hosting_stats(&rig);
    for _ in 0..5 {
        wake(&mut rig);
    }
    let after = hosting_stats(&rig);
    if after.polls < before.polls + 5 {
        return Err(format!(
            "setup: the idle wakes did not poll: {before:?} {after:?}"
        ));
    }
    if after.vets != before.vets {
        return Err(format!(
            "idle polls asked the core's trust gate {} time(s) over 5 polls",
            after.vets - before.vets
        ));
    }
    // A pull changes the plugin's code: asked at the next poll, and its code waits.
    plugin_code(
        &project.join("plugins").join("flag"),
        FLAG_MANIFEST,
        "example.flag",
        22,
    )?;
    wake(&mut rig);
    if hosting_stats(&rig).vets == after.vets {
        return Err("a changed plugin was not put to the trust gate".into());
    }
    run_as_person(&core, "example.flag");
    if setting(&core, "example.flag") != Some(Value::Int(1)) {
        return Err("the cache let changed code run".into());
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn idle_polls_of_a_trusted_project_ask_the_trust_gate_nothing() {
    check_idle_vets("idle-vets", HostingFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_without_the_cache_every_poll_asks() {
    let e = check_idle_vets(
        "idle-vets-control",
        HostingFaults {
            no_vet_cache: true,
            ..HostingFaults::default()
        },
    )
    .err()
    .unwrap_or_default();
    // Two plugins, five polls: ten questions.
    assert!(
        e.contains("idle polls asked the core's trust gate 10 time(s)"),
        "{e}"
    );
}
