//! `test_install_live_atomic` (WP-34, the WP-21 verifier's finding; Ch.32 §32.7; gate row
//! `C-install-live-atomic`): **a WASM plugin dropped into the running editor installs all of
//! its items or none.**
//!
//! WP-21's live install put the plugin's commands on the core's bus and in the live registry
//! before its presets and importers were added; a later failure left the commands installed
//! while the directory was marked refused — and the retry after a fix then failed on its own
//! leftovers (`DuplicateHandler`). Now everything is staged first (every item checked against
//! the live registries and the core's bus) and committed only when every step succeeded:
//! presets and importers into the live registries (each add undone if a later one fails), then
//! the commands, which the core takes all or none under its lock.
//!
//! The plugin here provides one item at each point the live install handles: a `Command`, a
//! `Preset`, an `Importer` and an `EditorPanel`. For every step (`HostingFaults::
//! fail_install_at`: staging each point, the registry commit, the bus), the injected failure
//! leaves nothing: no command on the core's bus or in the live registry, no preset in the
//! registry or the new-project catalog, no importer, no panel, no install counted — and the
//! plugin is reported not loaded. The fault then goes, the plugin's files change (a fix), and
//! the next wake installs every item: a failed install leaves nothing in the way of a retry.
//!
//! Panels are staged too (the WP-34 verifier's follow-up): each panel id is checked against
//! the running shell's panel host, which can hold panels no loaded manifest declares. A
//! plugin whose panel could not join it installs nothing (`a_plugin_whose_panel_id_is_taken_
//! installs_nothing`); before, its commands went live and the shell dropped its panel.
//! Positive control (W2): `positive_control_unchecked_panels_install_partly` —
//! `HostingFaults::skip_panel_check`: the plugin's command goes live without its panel.
//!
//! Positive control (W2): `positive_control_committing_as_it_goes_leaves_commands_installed` —
//! `HostingFaults::commit_as_it_goes` (the WP-21 order) with a failure at the importer step:
//! the command is left on the core's bus, and the check fails.

use std::path::{Path, PathBuf};
use std::time::Duration;

use forge_editor::core::EditorCore;
use forge_editor::hosting::{HostingFaults, HostingSetup, InstallStep, PluginDirs};
use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble_hosted;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_plugin::points::{Command, Preset};

const MANIFEST: &str = r#"Plugin(id: "com.example.multi", version: "0.1.0", engine: "^0.1",
    kind: Wasm, provides: [Command("example.multi"), Preset("com.example.multi"),
                           Importer("mmat"), EditorPanel("example.multi_panel")])"#;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("forge-wp34-atomic-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn write(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(name), text).map_err(|e| format!("{}: {e}", dir.display()))
}

fn code() -> String {
    let plan = r#"{"ops":[{"op":"set_setting","key":"example.multi","value":{"Int":1}}]}"#;
    let preset = r#"(label: "Multi", kind: ThreeD, defaults: {})"#;
    let describe = r#"{"version":1,"extensions":["mmat"]}"#;
    let view = r#"{"title":"Multi","dock":"right","body":[{"heading":"Multi"}]}"#;
    forge_wasm::wat::by_point(&[
        ("Command", plan.as_bytes()),
        ("Preset", preset.as_bytes()),
        ("Importer", describe.as_bytes()),
        ("EditorPanel", view.as_bytes()),
    ])
}

fn editor(config: &Path, faults: HostingFaults) -> Result<Rig, String> {
    let core = EditorCore::new();
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
    cfg.services.hosting.borrow_mut().faults = faults;
    Rig::with_core(cfg, core).map_err(|e| e.to_string())
}

fn wake(rig: &mut Rig) {
    let now = rig.h.now();
    rig.h.set_now(now + Duration::from_secs(2));
    rig.turn();
    rig.settle();
}

/// Which of the plugin's items are live, by name.
fn live(rig: &Rig) -> Vec<&'static str> {
    let services = rig.shell.handles().services();
    let hosting = services.hosting.borrow();
    let ext = hosting.extensions();
    let mut v = Vec::new();
    if EditorCore::configure(&rig.core, |bus| bus.targets().any(|t| t == "example.multi")) {
        v.push("the command on the core's bus");
    }
    if ext
        .registry::<Command>()
        .is_some_and(|r| r.get("example.multi").is_some())
    {
        v.push("the command in the live registry");
    }
    if ext
        .registry::<Preset>()
        .is_some_and(|r| r.get("com.example.multi").is_some())
    {
        v.push("the preset in the live registry");
    }
    if hosting
        .presets()
        .read()
        .map(|c| c.get("com.example.multi").is_some())
        .unwrap_or(true)
    {
        v.push("the preset in the new-project catalog");
    }
    if ext
        .registry::<forge_asset::ImporterPoint>()
        .is_some_and(|r| r.get("mmat").is_some())
    {
        v.push("the importer");
    }
    if rig
        .shell
        .panels()
        .iter()
        .any(|(p, _)| p.as_str() == "example.multi_panel")
    {
        v.push("the panel");
    }
    if hosting.stats().installs > 0 {
        v.push("an install counted");
    }
    v
}

fn check_atomic(step: InstallStep, commit_as_it_goes: bool, tag: &str) -> Result<(), String> {
    let root = tmp(tag);
    let config = root.join("config");
    let dir = config.join("plugins").join("multi");
    std::fs::create_dir_all(config.join("plugins")).map_err(|e| e.to_string())?;
    let mut rig = editor(
        &config,
        HostingFaults {
            fail_install_at: Some(step),
            commit_as_it_goes,
            ..HostingFaults::default()
        },
    )?;
    write(&dir, "plugin.ron", MANIFEST)?;
    write(&dir, "plugin.wat", &code())?;
    wake(&mut rig);
    let left = live(&rig);
    if !left.is_empty() {
        return Err(format!(
            "the install failed at {step:?} but left {} live",
            left.join(", ")
        ));
    }
    let reported =
        rig.shell.session().notifications.history().any(|n| {
            n.title == "A plugin was not loaded" && n.detail.contains("com.example.multi")
        });
    if !reported {
        return Err(format!("the failure at {step:?} was not reported"));
    }
    // The fault goes, the files change (a fix): the retry installs every item.
    rig.shell.handles().services().hosting.borrow_mut().faults = HostingFaults::default();
    write(&dir, "plugin.ron", &format!("{MANIFEST}\n"))?;
    wake(&mut rig);
    let now = live(&rig);
    if now.len() != 7 {
        return Err(format!(
            "the retry after a failure at {step:?} installed only: {}",
            now.join(", ")
        ));
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn a_live_install_is_all_or_nothing_at_every_step() {
    for (i, step) in [
        InstallStep::Commands,
        InstallStep::Presets,
        InstallStep::Importers,
        InstallStep::Panels,
        InstallStep::Registries,
        InstallStep::Bus,
    ]
    .into_iter()
    .enumerate()
    {
        check_atomic(step, false, &format!("step{i}")).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn positive_control_committing_as_it_goes_leaves_commands_installed() {
    let e = check_atomic(InstallStep::Importers, true, "control")
        .expect_err("committing as it goes must fail the all-or-nothing check");
    assert!(e.contains("left the command on the core's bus"), "{e}");
}

/// A plugin whose panel id the running editor's panel host already holds — a panel the
/// manifest conflict check cannot see (registered here straight on the host) — installs
/// nothing: its panel could not join the host, so its commands, preset and importer stay out
/// too, the host keeps the panel it had, and the refusal names the panel. After the panel id
/// is free (here: a plugin with another panel id, a fix), the retry installs every item.
fn check_panel_taken(skip_panel_check: bool, tag: &str) -> Result<(), String> {
    let root = tmp(tag);
    let config = root.join("config");
    let dir = config.join("plugins").join("multi");
    std::fs::create_dir_all(config.join("plugins")).map_err(|e| e.to_string())?;
    let mut rig = editor(
        &config,
        HostingFaults {
            skip_panel_check,
            ..HostingFaults::default()
        },
    )?;
    let taken = forge_plugin::points::PanelDescriptor::<forge_editor::panels::PanelCx> {
        title: "Taken".into(),
        icon: None,
        default_dock: forge_plugin::points::Dock::Right,
        build: std::sync::Arc::new(|_| {}),
    };
    if !rig
        .shell
        .panel_host()
        .add_panel(forge_ui::dock::PanelId::new("example.multi_panel"), taken)
    {
        return Err("the test could not register the taken panel".into());
    }
    write(&dir, "plugin.ron", MANIFEST)?;
    write(&dir, "plugin.wat", &code())?;
    wake(&mut rig);
    let title = |rig: &Rig| {
        rig.shell
            .panels()
            .into_iter()
            .find(|(p, _)| p.as_str() == "example.multi_panel")
            .map(|(_, t)| t)
    };
    let left: Vec<&str> = live(&rig)
        .into_iter()
        .filter(|w| *w != "the panel")
        .collect();
    if !left.is_empty() {
        return Err(format!(
            "a plugin whose panel id is taken went live without its panel: {} live, the panel still {:?}",
            left.join(", "),
            title(&rig)
        ));
    }
    if title(&rig).as_deref() != Some("Taken") {
        return Err(format!("the host's own panel changed: {:?}", title(&rig)));
    }
    let reported = rig.shell.session().notifications.history().any(|n| {
        n.title == "A plugin was not loaded"
            && n.detail.contains("com.example.multi")
            && n.detail
                .contains("example.multi_panel is already a panel in this editor")
    });
    if !reported {
        return Err("the refusal did not name the taken panel".into());
    }
    // The fix: the plugin's panel under an id of its own. Every item installs.
    write(
        &dir,
        "plugin.ron",
        &MANIFEST.replace("example.multi_panel", "example.multi_own_panel"),
    )?;
    wake(&mut rig);
    let now = live(&rig);
    let own = rig
        .shell
        .panels()
        .iter()
        .any(|(p, t)| p.as_str() == "example.multi_own_panel" && t == "Multi");
    if now.len() != 7 || !own {
        return Err(format!(
            "the retry with a free panel id installed only: {} (own panel: {own})",
            now.join(", ")
        ));
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn a_plugin_whose_panel_id_is_taken_installs_nothing() {
    check_panel_taken(false, "panel").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_unchecked_panels_install_partly() {
    let e = check_panel_taken(true, "panel-control")
        .expect_err("an install that does not stage its panels must fail the check");
    assert!(e.contains("went live without its panel"), "{e}");
}
