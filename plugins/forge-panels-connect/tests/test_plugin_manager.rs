//! `test_plugin_manager` (Ch.21 §21.21 "Plugin manager", Ch.32, Ch.38; DoD M2-52), headless,
//! against the real loader, manifests, `forge-wasm` host and the core's `SharedGrants`:
//!
//! * a WASM plugin hosted on the editor's one grant table is listed with its capabilities
//!   **requested vs granted**; its command is refused until a person grants
//!   `Command(Ordinary)` **in the panel** (a `forge.plugin.grant` command), then it applies;
//!   a revoke in the panel refuses it again at its next call;
//! * `replaces` is shown with **both names**; an index entry that would conflict is shown
//!   with the conflict naming both plugins, and adding it is refused before anything
//!   changes;
//! * the `forge add` equivalent: an index entry is fetched, sandbox-checked, cached (user
//!   config) and added to the project's plugin set by a command — undoable; enable and
//!   disable are commands too.
//!
//! Positive controls (W2):
//! * `positive_control_a_manager_showing_requested_as_granted_fails` — a manager that reads
//!   the manifest instead of the grants must fail the requested-vs-granted check;
//! * `positive_control_a_conflict_naming_one_plugin_fails` — a conflict line that names only
//!   one plugin must fail the both-names check.

mod common;

use std::rc::Rc;
use std::sync::Arc;

use common::{part, press, rows, select_containing};
use forge_cmd::{EditorCommand, Value};
use forge_editor::client::BusClient;
use forge_editor::connect::plugins::{ManagerFaults, MemoryIndex, PluginManager};
use forge_editor::core::EditorCore;
use forge_editor::security;
use forge_editor::testing::Rig;
use forge_plugin::points::Command;
use forge_plugin::{Extensions, Manifest};

const P: &str = "forge.plugins";

fn at(rig: &Rig, path: &[&str]) -> forge_ui::WidgetId {
    let mut p = vec!["content"];
    p.extend_from_slice(path);
    part(rig, P, &p)
}

fn flag_plugin(host: &forge_wasm::WasmHost) -> forge_wasm::WasmPlugin {
    let m = Manifest::parse(
        r#"Plugin(id: "com.example.flag", version: "0.1.0", engine: "^0.1", kind: Wasm,
            provides: [Command("example.flag")], capabilities: [Command(Ordinary)])"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let code = forge_wasm::wat::constant(
        br#"{"ops":[{"op":"set_setting","key":"flag","value":{"Bool":true}}]}"#,
    );
    host.load(m, code.as_bytes())
        .unwrap_or_else(|e| panic!("{e}"))
}

/// A rig whose editor hosts the flag plugin on the core's grant table, its command installed
/// on the core.
fn hosted_rig(faults: ManagerFaults) -> Rig {
    common::rig_with(&[P], None, |cfg, core| {
        let host = Arc::new(
            forge_wasm::WasmHost::new(EditorCore::grants(core)).unwrap_or_else(|e| panic!("{e}")),
        );
        let plugin = flag_plugin(&host);
        let mut ext = Extensions::new();
        ext.define::<Command>().unwrap_or_else(|e| panic!("{e}"));
        let mut m = cfg.services.connect.plugins.borrow_mut();
        m.faults = faults;
        m.host_wasm(host, vec![plugin], &mut ext)
            .unwrap_or_else(|e| panic!("{e}"));
        let reg = ext
            .registry::<Command>()
            .unwrap_or_else(|| panic!("Command defined"));
        EditorCore::install_commands(core, reg).unwrap_or_else(|e| panic!("{e}"));
    })
}

/// Invoke the plugin's command as a person; the refusal's code.
fn run_flag(rig: &Rig) -> Option<String> {
    let mut h = common::human(rig);
    h.apply(
        EditorCommand::Invoke {
            target: "example.flag".into(),
            args: "{}".into(),
        },
        None,
    );
    h.pump()
        .refused
        .first()
        .map(|r| format!("{} {}", r.rejection.code().as_str(), r.rejection))
}

fn check_requested_vs_granted(faults: ManagerFaults) -> Result<(), String> {
    let mut rig = hosted_rig(faults);
    let installed = at(&rig, &["installed_group", "installed"]);
    let r = rows(&mut rig, installed);
    let row = r
        .iter()
        .find(|l| l.starts_with("com.example.flag"))
        .ok_or_else(|| format!("the hosted plugin is not listed: {r:?}"))?;
    if !row.contains("hosted in the WASM sandbox") || !row.contains("0/1 granted") {
        return Err(format!(
            "before any grant the row must say 0/1 granted: {row}"
        ));
    }
    // First-party plugins are listed like any other (I16).
    if !r.iter().any(|l| l.starts_with("forge.panels.connect")) {
        return Err(format!("first-party plugins are missing: {r:?}"));
    }
    // Refused: the plugin holds no grant.
    let refused = run_flag(&rig).ok_or("the plugin's command ran without a grant")?;
    if !refused.contains("PLUGIN-0015") {
        return Err(format!("refused for another reason: {refused}"));
    }
    // Grant in the panel.
    select_containing(&mut rig, installed, "com.example.flag");
    let caps = at(&rig, &["caps_group", "caps"]);
    select_containing(&mut rig, caps, ordinary());
    let grant = at(&rig, &["caps_group", "caps_bar", "grant"]);
    press(&mut rig, grant);
    if let Some(r) = run_flag(&rig) {
        return Err(format!("the panel's grant did not reach the plugin: {r}"));
    }
    if EditorCore::read(&rig.core, |p| p.setting("flag").cloned()) != Some(Value::Bool(true)) {
        return Err("the plugin's command did not apply".into());
    }
    let r = rows(&mut rig, installed);
    if !r
        .iter()
        .any(|l| l.starts_with("com.example.flag") && l.contains("1/1 granted"))
    {
        return Err(format!("after the grant: {r:?}"));
    }
    // Revoke in the panel: the next call is refused.
    select_containing(&mut rig, caps, ordinary());
    let revoke = at(&rig, &["caps_group", "caps_bar", "revoke"]);
    press(&mut rig, revoke);
    if run_flag(&rig).is_none() {
        return Err("the panel's revoke did not reach the plugin".into());
    }
    Ok(())
}

#[test]
fn a_hosted_plugins_capabilities_are_granted_and_revoked_in_the_panel() {
    check_requested_vs_granted(ManagerFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_manager_showing_requested_as_granted_fails() {
    let e = check_requested_vs_granted(ManagerFaults {
        requested_as_granted: true,
        ..ManagerFaults::default()
    })
    .expect_err("a manager that shows the manifest as granted must fail");
    assert!(e.contains("0/1 granted"), "{e}");
}

fn source(id: &str) -> Manifest {
    Manifest::source(id, "0.1.0", "^0.1").unwrap_or_else(|e| panic!("{e}"))
}

fn check_both_names(faults: ManagerFaults) -> Result<(), String> {
    let alpha = source("com.alpha.panels").provides("EditorPanel", "demo.panel");
    let beta = source("com.beta.skin").replaces("EditorPanel", "demo.panel");
    let mut ix = MemoryIndex::new();
    ix.publish(
        r#"Plugin(id: "com.gamma.skin", version: "1.2.0", engine: "^0.1", kind: Wasm,
            replaces: [EditorPanel("demo.panel")])"#,
        forge_wasm::wat::constant(b"{}").into_bytes(),
        "another skin for the demo panel",
        "Gamma",
    )
    .map_err(|e| e.to_string())?;
    let mut rig = common::rig_with(&[P], None, |cfg, _| {
        let mut m = PluginManager::from_load(&[&alpha, &beta], &[]);
        m.faults = faults;
        *cfg.services.connect.plugins.borrow_mut() = m;
        cfg.services.connect.index = Rc::new(ix);
    });
    let installed = at(&rig, &["installed_group", "installed"]);
    let r = rows(&mut rig, installed);
    if !r
        .iter()
        .any(|l| l == "replaces EditorPanel(\"demo.panel\") from com.alpha.panels")
    {
        return Err(format!("the replace is not shown with both names: {r:?}"));
    }
    let index = at(&rig, &["index_group", "index"]);
    select_containing(&mut rig, index, "com.gamma.skin");
    let preview = common::label(&rig, at(&rig, &["index_group", "preview"]));
    if !(preview.contains("com.beta.skin") && preview.contains("com.gamma.skin")) {
        return Err(format!(
            "the conflict preview does not name both plugins: {preview}"
        ));
    }
    if !preview.contains("from com.alpha.panels") {
        return Err(format!(
            "the preview does not say whose item it replaces: {preview}"
        ));
    }
    let before = rig.state_hash();
    let add = at(&rig, &["index_group", "index_bar", "add"]);
    press(&mut rig, add);
    let status = common::label(&rig, at(&rig, &["status"]));
    if rig.state_hash() != before {
        return Err("a conflicting plugin changed the project".into());
    }
    if !(status.contains("com.beta.skin") && status.contains("com.gamma.skin")) {
        return Err(format!("the refusal does not name both plugins: {status}"));
    }
    Ok(())
}

#[test]
fn replaces_and_conflicts_name_both_plugins_before_anything_is_added() {
    check_both_names(ManagerFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_conflict_naming_one_plugin_fails() {
    let e = check_both_names(ManagerFaults {
        conflicts_name_one: true,
        ..ManagerFaults::default()
    })
    .expect_err("a conflict naming one plugin must fail");
    assert!(e.contains("both"), "{e}");
}

#[test]
fn forge_add_from_the_index_is_a_command_and_the_plugin_set_is_undoable() {
    let mut rig = common::rig(&[P]);
    let index = at(&rig, &["index_group", "index"]);
    // Search narrows the index (the debounced search reaches the panel as SearchChanged).
    let search = at(&rig, &["index_group", "index_query"]);
    rig.h.ui.raise(
        search,
        forge_ui::widgets::SearchChanged {
            field: search,
            query: "reads".into(),
        },
    );
    rig.turn();
    rig.settle();
    let r = rows(&mut rig, index);
    assert_eq!(r.len(), 1, "{r:?}");
    select_containing(&mut rig, index, "com.example.notes");
    let add = at(&rig, &["index_group", "index_bar", "add"]);
    press(&mut rig, add);
    let key = security::plugin_key("com.example.notes", "enabled");
    assert_eq!(
        rig.shell.mirror().setting(&key),
        Some(&Value::Bool(true)),
        "{}",
        common::label(&rig, at(&rig, &["status"]))
    );
    assert!(
        rig.shell
            .handles()
            .services()
            .connect
            .cache
            .borrow()
            .contains("com.example.notes", "0.2.0"),
        "the fetched bytes are in the cache"
    );
    let installed = at(&rig, &["installed_group", "installed"]);
    let r = rows(&mut rig, installed);
    assert!(
        r.iter().any(|l| l.starts_with("com.example.notes 0.2.0")
            && l.contains("in the project, not installed on this machine")),
        "{r:?}"
    );
    // Disable, then enable: commands.
    select_containing(&mut rig, installed, "com.example.notes");
    let disable = at(&rig, &["installed_group", "plugin_bar", "disable"]);
    press(&mut rig, disable);
    assert_eq!(rig.shell.mirror().setting(&key), Some(&Value::Bool(false)));
    let entry = rig.shell.mirror().history().last().map(|h| h.issuer_tag());
    assert_eq!(entry.as_deref(), Some("human:tester"));
    // Undo twice: enabled again, then not in the project at all.
    rig.shell.emitter().undo();
    rig.settle();
    assert_eq!(rig.shell.mirror().setting(&key), Some(&Value::Bool(true)));
    rig.shell.emitter().undo();
    rig.settle();
    assert_eq!(rig.shell.mirror().setting(&key), None);
    // The plugin set is ordinary project state: an automation session could add one too, but only a
    // person grants it anything (the grant is refused for the session).
    let mut auto = rig.connect(forge_cmd::Issuer::Automation {
        session: "auto-9".into(),
        tool: "apply".into(),
    });
    auto.apply(
        security::grant_command(
            &forge_plugin::Principal::Plugin(
                forge_plugin::PluginId::new("com.example.notes").unwrap_or_else(|e| panic!("{e}")),
            ),
            forge_plugin::Capability::Fs(forge_plugin::FsScope::ProjectRead),
        ),
        None,
    );
    let refused = auto.pump().refused;
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].rejection.code().as_str(), "CMD-0014");
}

// ---- WP-21: held security settings are listed and answered here too -----------------------
//
// An automation session opens (through its client) a project file carrying a plugin grant; the
// grant is held for a person. The Plugin manager lists it, and its Accept is the person's
// `forge.security.accept_held` command that grants it; its Discard answers the proposal and
// grants nothing. Each run is the other's control: the same list and button, opposite
// outcomes, both checked.

/// A saved project whose settings file gained a plugin grant (a file write an automation session
/// could make), opened by an automation session through its client. The rig, the plugin and the
/// capability.
fn automation_opens_a_granting_file(
    tag: &str,
) -> (
    Rig,
    forge_plugin::Principal,
    forge_plugin::Capability,
    std::path::PathBuf,
) {
    use forge_editor::project::{ProjectOp, Template};
    use forge_plugin::{Capability, FsScope, PluginId, Principal};
    use forge_project::format::{MANIFEST_PATH, ProjectDoc, SCENE_PATH, SETTINGS_PATH};

    let mut rig = common::rig(&[P]);
    let dir = std::env::temp_dir().join(format!(
        "forge-wp21-manager-held-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let location = format!("file:{}", dir.display());
    let mut h = common::human(&rig);
    h.apply(
        ProjectOp::Create {
            location: location.clone(),
            name: "Held".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
        None,
    );
    h.apply(forge_editor::project::save_command("start"), None);
    assert!(h.pump().refused.is_empty());
    EditorCore::wait_transfers(&rig.core);
    let rivers =
        Principal::Plugin(PluginId::new("com.example.rivers").unwrap_or_else(|e| panic!("{e}")));
    let write = Capability::Fs(FsScope::ProjectWrite);
    let text =
        |p: &str| std::fs::read_to_string(dir.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let (_, mut doc) = ProjectDoc::decode(
        &text(MANIFEST_PATH),
        Some(&text(SETTINGS_PATH)),
        Some(&text(SCENE_PATH)),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    doc.settings.insert(
        security::grant_key(&rivers, write).unwrap_or_default(),
        Value::Bool(true),
    );
    for (path, body) in doc.encode().unwrap_or_else(|e| panic!("{e}")) {
        if path == SETTINGS_PATH {
            std::fs::write(dir.join(path), body).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    let mut auto = common::automation(&rig, "auto-1");
    auto.apply(
        ProjectOp::Open {
            location,
            discard_unsaved: true,
        }
        .command(),
        None,
    );
    assert!(
        auto.pump().refused.is_empty(),
        "the automation client's open was refused"
    );
    EditorCore::wait_transfers(&rig.core);
    assert!(
        !EditorCore::grants(&rig.core).has(&rivers, write),
        "the session's open granted the plugin"
    );
    rig.settle();
    (rig, rivers, write, dir)
}

fn answer_held(accept: bool) -> Result<(), String> {
    let (mut rig, rivers, write, dir) =
        automation_opens_a_granting_file(if accept { "accept" } else { "discard" });
    let held = at(&rig, &["held_group", "held"]);
    let r = rows(&mut rig, held);
    if !(r.len() == 1 && r[0].contains("com.example.rivers")) {
        return Err(format!(
            "the Plugin manager does not list the held grant: {r:?}"
        ));
    }
    let head = common::label(&rig, at(&rig, &["held_group", "held_head"]));
    if !head.contains("held until a person") {
        return Err(format!("the held heading: {head}"));
    }
    let btn = at(
        &rig,
        &[
            "held_group",
            "held_bar",
            if accept {
                "accept_held"
            } else {
                "discard_held"
            },
        ],
    );
    press(&mut rig, btn);
    let granted = EditorCore::grants(&rig.core).has(&rivers, write);
    if granted != accept {
        return Err(format!(
            "after {}: the plugin is {}granted",
            if accept { "Accept" } else { "Discard" },
            if granted { "" } else { "not " }
        ));
    }
    if EditorCore::held_security(&rig.core).is_some() {
        return Err("the proposal outlived the panel's answer".into());
    }
    let r = rows(&mut rig, held);
    if !r.is_empty() {
        return Err(format!("the answered proposal is still listed: {r:?}"));
    }
    let want = if accept {
        security::ACCEPT_HELD_CMD
    } else {
        security::DISCARD_HELD_CMD
    };
    let tags: Vec<String> = rig
        .shell
        .mirror()
        .history()
        .iter()
        .map(|h| format!("{} {}", h.issuer_tag(), h.label))
        .collect();
    let audited = EditorCore::audit_book(&rig.core)
        .query(&forge_editor::connect::audit::AuditQuery::parse(""))
        .into_iter()
        .any(|r| {
            r.event
                == if accept {
                    "accept_held"
                } else {
                    "discard_held"
                }
        });
    if !audited {
        return Err("the panel's answer is not audited".into());
    }
    if accept
        && !tags
            .iter()
            .any(|t| t.starts_with("human:tester") && t.contains(want))
    {
        return Err(format!("the accept is not the person's command: {tags:?}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn held_security_settings_are_listed_and_accepted_in_the_plugin_manager() {
    answer_held(true).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn held_security_settings_are_discarded_in_the_plugin_manager() {
    answer_held(false).unwrap_or_else(|e| panic!("{e}"));
}

// ---- WP-34: the open project's trust is decided in the Plugin manager ------------------------
//
// A person opens a project a teammate's editor saved with a plugin grant (not a project this
// editor created, so nobody here has trusted it): the grant is held, and the *Project trust*
// group lists it with *Trust this project* / *Don't trust*. Trusting grants it (through the
// person's accept); not trusting leaves it held. Each run is the other's control.

fn decide_in_panel(trust: bool) -> Result<(), String> {
    decide_shown(trust, false)
}

/// `stale`: the question changes (a pull adds a plugin folder) after the group showed it and
/// before the press, which the panel has not seen yet.
fn decide_shown(trust: bool, stale: bool) -> Result<(), String> {
    use forge_editor::project::{ProjectOp, Template};
    use forge_plugin::{Capability, FsScope, PluginId, Principal};

    let tag = match (trust, stale) {
        (true, false) => "trust",
        (false, false) => "distrust",
        (_, true) => "stale",
    };
    let dir = std::env::temp_dir().join(format!(
        "forge-wp34-manager-trust-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let location = format!("file:{}", dir.display());
    let rivers = Principal::Plugin(PluginId::new("com.example.rivers").map_err(|e| e.to_string())?);
    let write = Capability::Fs(FsScope::ProjectWrite);
    // The teammate's editor.
    let teammate = EditorCore::new();
    let mut t = EditorCore::connect(&teammate, forge_cmd::Issuer::Human { user: "bo".into() });
    t.apply(
        ProjectOp::Create {
            location: location.clone(),
            name: "Shared".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
        None,
    );
    t.apply(security::grant_command(&rivers, write), None);
    t.apply(forge_editor::project::save_command("grant rivers"), None);
    if let Some(r) = t.pump().refused.first() {
        return Err(format!("the teammate: {}", r.rejection));
    }
    EditorCore::wait_transfers(&teammate);
    // This person opens it.
    let mut rig = common::rig(&[P]);
    let mut h = common::human(&rig);
    h.apply(
        ProjectOp::Open {
            location,
            discard_unsaved: true,
        }
        .command(),
        None,
    );
    if let Some(r) = h.pump().refused.first() {
        return Err(format!("the open: {}", r.rejection));
    }
    EditorCore::wait_transfers(&rig.core);
    rig.settle();
    if EditorCore::grants(&rig.core).has(&rivers, write) {
        return Err("an untrusted project's grant took effect on open".into());
    }
    let list = at(&rig, &["trust_group", "trust_list"]);
    let r = rows(&mut rig, list);
    if !(r.len() == 1 && r[0].contains("com.example.rivers")) {
        return Err(format!("the trust group does not list the grant: {r:?}"));
    }
    let head = common::label(&rig, at(&rig, &["trust_group", "trust_head"]));
    if !head.contains("Do you trust this project?") {
        return Err(format!("the trust heading: {head}"));
    }
    if rig.h.ui.is_hidden(at(&rig, &["trust_group", "trust_bar"])) {
        return Err("a project carrying a grant shows no Trust / Don't trust buttons".into());
    }
    let btn = at(
        &rig,
        &[
            "trust_group",
            "trust_bar",
            if trust { "trust" } else { "distrust" },
        ],
    );
    if stale {
        // A pull lands a plugin folder between the look and the press: the question changes
        // under the group, which has not synced since.
        let before = EditorCore::project_trust_id(&rig.core);
        EditorCore::vet_carried(
            &rig.core,
            &[forge_editor::trust::CarriedItem {
                item: "plugins/late".into(),
                digest: "late".into(),
                line: "plugins/late is new".into(),
                code: true,
            }],
        );
        if EditorCore::project_trust_id(&rig.core) == before {
            return Err("setup: the question did not change".into());
        }
        press(&mut rig, btn);
        if EditorCore::grants(&rig.core).has(&rivers, write) {
            return Err("an answer to a question that changed since it was shown was taken".into());
        }
        let status = common::label(&rig, at(&rig, &["status"]));
        if !status.contains("changed since it was shown") {
            return Err(format!("the refusal does not say why: {status}"));
        }
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(());
    }
    press(&mut rig, btn);
    let granted = EditorCore::grants(&rig.core).has(&rivers, write);
    if granted != trust {
        return Err(format!(
            "after {}: the plugin is {}granted",
            if trust { "Trust" } else { "Don't trust" },
            if granted { "" } else { "not " }
        ));
    }
    let head = common::label(&rig, at(&rig, &["trust_group", "trust_head"]));
    let want = if trust {
        "is trusted"
    } else {
        "is not trusted"
    };
    if !head.contains(want) {
        return Err(format!("the heading after the answer: {head}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn an_untrusted_project_is_trusted_in_the_plugin_manager() {
    decide_in_panel(true).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn an_untrusted_project_is_not_trusted_in_the_plugin_manager() {
    decide_in_panel(false).unwrap_or_else(|e| panic!("{e}"));
}

// WP-36: the buttons answer the question the group showed (`decide_trust_seen`). A question
// that changed between the look and the press (a pull) is refused, and the status says so;
// the person looks again. The two runs above, where the question did not change, are its
// control: the same press is taken.
#[test]
fn a_press_on_a_question_that_changed_since_it_was_shown_is_refused() {
    decide_shown(true, true).unwrap_or_else(|e| panic!("{e}"));
}

// A project that carries no plugins, plugin grants or wider automation policy asks nothing: the
// *Project trust* group says so and shows no buttons, and an answer given anyway (the call the
// buttons make) is refused, so nothing about the folder is remembered. The run above, where the
// buttons show, is its control.
#[test]
fn a_project_carrying_nothing_shows_no_trust_buttons_and_records_no_answer() {
    use forge_editor::project::{ProjectOp, Template};
    let dir =
        std::env::temp_dir().join(format!("forge-wp34-manager-nothing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let location = format!("file:{}", dir.display());
    let mut rig = common::rig(&[P]);
    let mut h = common::human(&rig);
    h.apply(
        ProjectOp::Create {
            location,
            name: "Plain".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
        None,
    );
    assert!(h.pump().refused.is_empty(), "the create was refused");
    EditorCore::wait_transfers(&rig.core);
    rig.settle();
    assert!(EditorCore::project_trust(&rig.core).is_none());
    let head = common::label(&rig, at(&rig, &["trust_group", "trust_head"]));
    assert!(head.contains("nothing to trust"), "{head}");
    assert!(
        rig.h.ui.is_hidden(at(&rig, &["trust_group", "trust_bar"])),
        "Trust / Don't trust shown for a project that carries nothing"
    );
    let e = EditorCore::decide_trust(&rig.core, "tester", forge_editor::trust::Trust::Untrusted)
        .expect_err("an answer about nothing must be refused");
    assert!(e.contains("nothing to trust"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The capability rows' text for `Command(Ordinary)` (`capability_label`).
fn ordinary() -> &'static str {
    forge_editor::connect::capability_label(forge_plugin::Capability::Command(
        forge_plugin::CommandClass::Ordinary,
    ))
}
