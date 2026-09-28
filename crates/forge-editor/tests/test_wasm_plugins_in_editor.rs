//! `test_wasm_plugins_in_editor` (WP-21, Ch.32 §32.7, Ch.21 §21.19; gate row
//! `C-wasm-plugins-in-editor`): **a WASM plugin dropped into the plugin directory installs in
//! the running editor and hot-reloads**, end to end through the headless shell, the core and
//! the real WASM host — and an idle editor stays idle while it hosts plugins (D-5).
//!
//! * At start, `assemble_hosted` loads a WASM importer from the user plugin directory
//!   together with the source plugins (one load): its extension is importable in the asset
//!   database.
//! * While the editor runs, a plugin directory is dropped in: a `Command` and an
//!   `EditorPanel` (a ViewSpec). The next time the editor wakes it is installed: the core
//!   runs its command (refused until a person grants `Command(Ordinary)`, then it applies),
//!   the palette lists it, the Plugin manager lists it hosted, and its panel opens in the
//!   dock with a live setting readout and a button that runs the plugin's own command — a
//!   button naming another plugin's (or the core's) command is not a button at all.
//! * The code is rewritten: the next wake runs the new code under the same installed items
//!   (generation 2); a broken save keeps the old code running and says so.
//! * Idle: once the install's toast has timed out, ten seconds of an idle editor hosting and
//!   watching plugins render no frame and make no wakeup.
//!
//! Positive controls (W2):
//! * `positive_control_an_editor_that_never_polls_installs_nothing` — a shell that never
//!   polls its plugins (`HostingFaults::never_poll`, the editor before WP-21) must fail the
//!   install check;
//! * `positive_control_a_timer_poll_wakes_the_idle_editor` — a poll on a timer
//!   (`HostingFaults::poll_wakes_the_loop`) must fail the idle check.

use std::path::{Path, PathBuf};
use std::time::Duration;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_editor::hosting::{HostingFaults, HostingSetup, PluginDirs};
use forge_editor::presets::builtin_preset;
use forge_editor::security;
use forge_editor::shell::assemble_hosted;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_plugin::{Capability, CommandClass, PluginId};
use forge_ui::Key;

const PANEL: &str = "example.flag_panel";

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "forge-wp21-wasm-editor-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The echo importer from `forge-wasm`'s test, reduced: describe, then source = artefact.
fn importer_wat(ext: &str) -> String {
    let describe = format!(r#"{{"version":1,"extensions":["{ext}"]}}"#);
    let describe = describe.as_str();
    let oh = r#"{"artefacts":[{"label":"","kind":"material","name":""}]}"#;
    forge_wasm::wat::component(
        &[],
        &format!(
            r#"{}
    {}
    (func (export "call") (param i32 i32 i32 i32) (param $in i32) (param $inl i32) (result i32)
      (local $hl i32) (local $sl i32) (local $out i32) (local $n i32)
      (if (i32.eqz (local.get $inl)) (then (return (call $ok (i32.const 1024) (i32.const {})))))
      (local.set $hl (i32.load (local.get $in)))
      (local.set $sl (i32.load (i32.add (i32.add (local.get $in) (i32.const 4)) (local.get $hl))))
      (local.set $n (i32.add (i32.const {}) (local.get $sl)))
      (local.set $out (call $alloc (local.get $n)))
      (i32.store (local.get $out) (i32.const {}))
      (memory.copy (i32.add (local.get $out) (i32.const 4)) (i32.const 2048) (i32.const {}))
      (i32.store (i32.add (local.get $out) (i32.const {})) (local.get $sl))
      (memory.copy (i32.add (local.get $out) (i32.const {}))
                   (i32.add (i32.add (local.get $in) (i32.const 8)) (local.get $hl)) (local.get $sl))
      (call $ok (local.get $out) (local.get $n)))"#,
            forge_wasm::wat::data(1024, describe.as_bytes()),
            forge_wasm::wat::data(2048, oh.as_bytes()),
            describe.len(),
            8 + oh.len(),
            oh.len(),
            oh.len(),
            4 + oh.len(),
            8 + oh.len(),
        ),
    )
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(dir.join(name), text).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
}

/// The dropped-in plugin's code: its command sets `example.flag` to `value`; its panel.
fn flag_wat(value: i64) -> String {
    let plan = format!(
        r#"{{"ops":[{{"op":"set_setting","key":"example.flag","value":{{"Int":{value}}}}}]}}"#
    );
    let view = r#"{"title":"Flag","dock":"right","body":[
        {"heading":"Flag"},
        {"setting":{"key":"example.flag","label":"Flag"}},
        {"row":[{"button":{"label":"Raise","command":"example.flag"}},
                {"button":{"label":"Save","command":"forge.project.save"}}]}]}"#;
    forge_wasm::wat::by_point(&[
        ("Command", plan.as_bytes()),
        ("EditorPanel", view.as_bytes()),
    ])
}

const FLAG_MANIFEST: &str = r#"Plugin(id: "com.example.flag", version: "0.1.0", engine: "^0.1",
    kind: Wasm, provides: [Command("example.flag"), EditorPanel("example.flag_panel")],
    capabilities: [Command(Ordinary)])"#;

struct Setup {
    rig: Rig,
    root: PathBuf,
    plugins: PathBuf,
}

fn setup(tag: &str, faults: HostingFaults) -> Setup {
    let root = tmp(tag);
    let config = root.join("config");
    let plugins = config.join("plugins");
    let fmat = plugins.join("fmat");
    write(
        &fmat,
        "plugin.ron",
        r#"Plugin(id: "com.example.fmat", version: "0.1.0", engine: "^0.1", kind: Wasm,
            provides: [Importer("fmat")])"#,
    );
    write(&fmat, "plugin.wat", &importer_wat("fmat"));
    let core = EditorCore::new();
    let stand_in = StandInPanels::without(&[]).unwrap_or_else(|e| panic!("{e}"));
    let mut cfg = assemble_hosted(
        builtin_preset("3d").unwrap_or_else(|e| panic!("{e}")),
        &[&stand_in],
        &[],
        Some(config.clone()),
        Some(HostingSetup {
            core: core.clone(),
            dirs: PluginDirs::for_user(Some(&config)),
        }),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    cfg.services.hosting.borrow_mut().faults = faults;
    // The asset database over the editor's plugin registries (the WASM importer included).
    let catalog = forge_editor::assets::ServerCatalog::over_extensions(
        &[
            ("mats/a.fmat".to_string(), b"(colour: 1)".to_vec()),
            ("mats/b.gmat".to_string(), b"(colour: 2)".to_vec()),
        ],
        cfg.services.hosting.borrow().extensions(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    cfg.services.assets = Some(std::rc::Rc::new(std::cell::RefCell::new(catalog)));
    let rig = Rig::with_core(cfg, core).unwrap_or_else(|e| panic!("{e}"));
    Setup { rig, root, plugins }
}

/// The person comes back to the window (focus, a pointer move): the loop turns.
fn wake(rig: &mut Rig) {
    let now = rig.h.now();
    rig.h.set_now(now + Duration::from_secs(2));
    rig.turn();
    rig.settle();
}

/// Invoke `example.flag` as a person; the refusal, if any.
fn run_flag(rig: &Rig) -> Option<String> {
    let mut h = rig.connect(Issuer::Human {
        user: "tester".into(),
    });
    h.apply(
        EditorCommand::Invoke {
            target: "example.flag".into(),
            args: "{}".into(),
        },
        None,
    );
    h.pump().refused.first().map(|r| r.rejection.to_string())
}

fn flag(rig: &Rig) -> Option<Value> {
    EditorCore::read(&rig.core, |p| p.setting("example.flag").cloned())
}

fn manager_row(rig: &Rig) -> Option<String> {
    let m = rig
        .shell
        .handles()
        .services()
        .connect
        .plugins
        .borrow()
        .rows(&rig.shell.mirror());
    m.into_iter()
        .find(|r| r.id == "com.example.flag")
        .map(|r| r.label())
}

fn check_installs(faults: HostingFaults, tag: &str) -> Result<(), String> {
    let Setup {
        mut rig,
        root,
        plugins,
    } = setup(tag, faults);
    // Loaded at start with the source plugins: its extension is importable.
    let importable = |rig: &Rig, p: &str| {
        rig.shell
            .handles()
            .services()
            .assets
            .as_ref()
            .is_some_and(|a| a.borrow().importable(p))
    };
    if !importable(&rig, "mats/a.fmat") {
        return Err("the WASM importer loaded at start does not claim .fmat".into());
    }
    if importable(&rig, "mats/b.gmat") {
        return Err("setup: .gmat is importable before its plugin is dropped in".into());
    }
    // Dropped in while the editor runs.
    let dir = plugins.join("flag");
    write(&dir, "plugin.ron", FLAG_MANIFEST);
    write(&dir, "plugin.wat", &flag_wat(1));
    let gmat = plugins.join("gmat");
    write(
        &gmat,
        "plugin.ron",
        r#"Plugin(id: "com.example.gmat", version: "0.1.0", engine: "^0.1", kind: Wasm,
            provides: [Importer("gmat")])"#,
    );
    write(&gmat, "plugin.wat", &importer_wat("gmat"));
    wake(&mut rig);
    if !importable(&rig, "mats/b.gmat") {
        return Err("the dropped-in importer did not install: .gmat is not importable".into());
    }
    let on_core =
        EditorCore::configure(&rig.core, |bus| bus.targets().any(|t| t == "example.flag"));
    if !on_core {
        return Err("the dropped-in plugin did not install: the core has no example.flag".into());
    }
    if !rig
        .shell
        .plugin_commands()
        .iter()
        .any(|c| c.target == "example.flag")
    {
        return Err("the palette does not list the plugin's command".into());
    }
    let row = manager_row(&rig).ok_or("the Plugin manager does not list the plugin")?;
    if !row.contains("hosted in the WASM sandbox (generation 1)") {
        return Err(format!("the manager's row: {row}"));
    }
    // Refused until a person grants it; then it applies.
    let refused = run_flag(&rig).ok_or("the plugin's command ran with no grant")?;
    if !refused.contains("Command(Ordinary)") {
        return Err(format!("refused for another reason: {refused}"));
    }
    let who = forge_plugin::Principal::Plugin(
        PluginId::new("com.example.flag").map_err(|e| e.to_string())?,
    );
    let mut person = rig.connect(Issuer::Human {
        user: "tester".into(),
    });
    person.apply(
        security::grant_command(&who, Capability::Command(CommandClass::Ordinary)),
        None,
    );
    if let Some(r) = person.pump().refused.first() {
        return Err(format!("the grant: {}", r.rejection));
    }
    if let Some(r) = run_flag(&rig) {
        return Err(format!("the granted command was refused: {r}"));
    }
    if flag(&rig) != Some(Value::Int(1)) {
        return Err(format!(
            "the plugin's command did not apply: {:?}",
            flag(&rig)
        ));
    }
    // Its panel opens in the dock; the button runs its own command, the foreign one is not
    // a button.
    if !rig.shell.panels().iter().any(|(p, _)| p.as_str() == PANEL) {
        return Err("the plugin's panel is not registered".into());
    }
    rig.show_panels(&[PANEL]).map_err(|e| e.to_string())?;
    let frame = rig
        .panel_frame(PANEL)
        .ok_or("the plugin's panel did not open")?;
    let content = frame.child(&Key::Str("content".into()));
    let readout = content.child(&Key::Str("n1".into()));
    let text = rig
        .h
        .ui
        .widget::<forge_ui::widgets::Label>(readout)
        .map(|l| l.text(rig.h.ui.rt()))
        .unwrap_or_default();
    if text != "Flag: 1" {
        return Err(format!("the setting readout: {text:?}"));
    }
    let row = content.child(&Key::Str("n2".into()));
    let raise = row.child(&Key::Str("n2.0".into()));
    let foreign = row.child(&Key::Str("n2.1".into()));
    if rig
        .h
        .ui
        .widget::<forge_ui::widgets::Button>(raise)
        .is_none()
    {
        return Err("the plugin's own command is not a button".into());
    }
    if rig
        .h
        .ui
        .widget::<forge_ui::widgets::Button>(foreign)
        .is_some()
    {
        return Err("a button runs a command that is not the plugin's".into());
    }
    // Hot reload: new code under the same installed items.
    write(&dir, "plugin.wat", &flag_wat(22));
    wake(&mut rig);
    rig.h.ui.raise(raise, forge_ui::widgets::Pressed(raise));
    rig.turn();
    rig.settle();
    if flag(&rig) != Some(Value::Int(22)) {
        return Err(format!(
            "the panel's button did not run the reloaded code: {:?}",
            flag(&rig)
        ));
    }
    let row = manager_row(&rig).ok_or("the plugin left the manager")?;
    if !row.contains("(generation 2)") {
        return Err(format!("the manager does not show the reload: {row}"));
    }
    // A broken save keeps the old code.
    write(&dir, "plugin.wat", "(component (this is not wat");
    wake(&mut rig);
    if let Some(r) = run_flag(&rig) {
        return Err(format!("a broken save took the plugin down: {r}"));
    }
    if flag(&rig) != Some(Value::Int(22)) {
        return Err("a broken save changed what the plugin does".into());
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn a_wasm_plugin_dropped_in_installs_in_the_running_editor_and_hot_reloads() {
    check_installs(HostingFaults::default(), "installs").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_editor_that_never_polls_installs_nothing() {
    let e = check_installs(
        HostingFaults {
            never_poll: true,
            ..HostingFaults::default()
        },
        "never-polls",
    )
    .expect_err("an editor that never polls must fail the install check");
    assert!(e.contains("did not install"), "{e}");
}

fn check_idle(faults: HostingFaults, tag: &str) -> Result<(), String> {
    let Setup {
        mut rig,
        root,
        plugins,
    } = setup(tag, faults);
    let dir = plugins.join("flag");
    write(&dir, "plugin.ron", FLAG_MANIFEST);
    write(&dir, "plugin.wat", &flag_wat(1));
    wake(&mut rig);
    // The install's notice is a toast that times out: let it go, then watch the idle editor.
    let _ = rig.advance(Duration::from_secs(60));
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&root);
    if frames != 0 || wakeups != 0 {
        return Err(format!(
            "an idle editor hosting WASM plugins rendered {frames} frame(s) and woke {wakeups} time(s) in 10 s"
        ));
    }
    Ok(())
}

#[test]
fn an_idle_editor_hosting_plugins_makes_no_wakeups() {
    check_idle(HostingFaults::default(), "idle").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_timer_poll_wakes_the_idle_editor() {
    let e = check_idle(
        HostingFaults {
            poll_wakes_the_loop: true,
            ..HostingFaults::default()
        },
        "timer",
    )
    .expect_err("a poll on a timer must fail the idle check");
    assert!(e.contains("woke"), "{e}");
}
