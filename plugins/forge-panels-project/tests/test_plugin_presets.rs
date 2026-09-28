//! `test_plugin_presets` (WP-21, Ch.31, Ch.32 §32.7): **a third-party preset plugin shows in
//! the new-project flow**, end to end through the headless editor.
//!
//! The editor lists presets from its **real** `Preset` registry — the one `assemble_hosted`
//! loads every plugin into — not a private copy of the built-in three:
//!
//! * a WASM preset plugin in the user plugin directory at start (`com.example.rivers`, a 3D
//!   preset carrying its own defaults) is a row of the launcher's template list; choosing it
//!   and pressing *Create* makes a project from it (its label in the outcome, its defaults in
//!   the project, `project.template` naming it);
//! * a second preset plugin dropped in while the editor runs (`com.example.lakes`, 2D) joins
//!   the list the next time the editor wakes, with no restart.
//!
//! Positive control (W2): a launcher that lists the built-in templates only
//! (`PanelFaults::launcher_builtin_templates_only`, the pre-WP-21 flow) must fail the check.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use forge_cmd::Value;
use forge_editor::core::EditorCore;
use forge_editor::hosting::{HostingSetup, PluginDirs};
use forge_editor::presets::builtin_preset;
use forge_editor::services::PanelFaults;
use forge_editor::shell::assemble_hosted;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_core::PanelsCore;
use forge_panels_project::PanelsProject;
use forge_plugin::SourcePlugin;

const L: &str = "forge.launcher";

fn write_plugin(root: &Path, dir: &str, id: &str, label: &str, kind: &str, defaults: &str) {
    let d = root.join(dir);
    std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        d.join("plugin.ron"),
        format!(
            r#"Plugin(id: "{id}", version: "0.1.0", engine: "^0.1", kind: Wasm, provides: [Preset("{id}")])"#
        ),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let ron = format!(r#"(label: "{label}", kind: {kind}, defaults: {{{defaults}}})"#);
    std::fs::write(
        d.join("plugin.wat"),
        forge_wasm::wat::constant(ron.as_bytes()),
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

struct Setup {
    rig: Rig,
    root: PathBuf,
    plugins: PathBuf,
}

fn setup(tag: &str, faults: PanelFaults) -> Setup {
    let root = common::tmp(&format!("plugin-presets-{tag}"));
    let config = root.join("config");
    let plugins = config.join("plugins");
    write_plugin(
        &plugins,
        "rivers",
        "com.example.rivers",
        "Rivers",
        "ThreeD",
        r#""rivers.enabled": "true", "frames.depth": "2""#,
    );
    let core = EditorCore::new();
    let panels_core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let project = PanelsProject::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_project::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&panels_core, &project, &stand_in];
    let mut cfg = assemble_hosted(
        builtin_preset("3d").unwrap_or_else(|e| panic!("{e}")),
        &all,
        &[],
        Some(config.clone()),
        Some(HostingSetup {
            core: core.clone(),
            dirs: PluginDirs::for_user(Some(&config)),
        }),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    cfg.services.faults = faults;
    cfg.services.launcher.borrow_mut().projects_dir = root.join("projects");
    let mut rig = Rig::with_core(cfg, core).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&[L]).unwrap_or_else(|e| panic!("{e}"));
    Setup { rig, root, plugins }
}

fn template_rows(rig: &mut Rig) -> Vec<String> {
    let t = common::part(rig, L, &["content", "form", "templates"]);
    forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, t, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r).and_then(|k| t.label_of(k)).map(str::to_string))
            .collect()
    })
    .unwrap_or_default()
}

fn check(faults: PanelFaults, tag: &str) -> Result<(), String> {
    let Setup {
        mut rig,
        root,
        plugins,
    } = setup(tag, faults);
    let new = common::part(&rig, L, &["content", "new"]);
    common::click(&mut rig, new);
    let rows = template_rows(&mut rig);
    let at = rows
        .iter()
        .position(|r| r.starts_with("Rivers") && r.contains("com.example.rivers"))
        .ok_or_else(|| format!("the plugin's preset is not in the new-project flow: {rows:?}"))?;
    if at < forge_editor::project::Template::ALL.len() {
        return Err(format!(
            "the built-in templates do not come first: {rows:?}"
        ));
    }
    let templates = common::part(&rig, L, &["content", "form", "templates"]);
    common::select_row(&mut rig, templates, at as u64);
    let create = common::part(&rig, L, &["content", "form", "form_bar", "create"]);
    common::click(&mut rig, create);
    let st = common::status(&rig);
    let o = st.outcomes.last().ok_or("no outcome")?;
    let said = o
        .result
        .clone()
        .map_err(|e| format!("the create failed: {e:?}"))?;
    if !said.contains("(Rivers)") {
        return Err(format!("the outcome does not name the preset: {said}"));
    }
    let want = [
        ("rivers.enabled", Value::Bool(true)),
        ("frames.depth", Value::Int(2)),
        ("project.preset", Value::Text("3d".into())),
        (
            "project.template",
            Value::Text("preset:com.example.rivers".into()),
        ),
    ];
    for (k, v) in want {
        if common::setting(&rig, k).as_ref() != Some(&v) {
            return Err(format!(
                "{k}: {:?}, the preset says {v:?}",
                common::setting(&rig, k)
            ));
        }
    }
    // A second preset plugin dropped in while the editor runs joins the list when the
    // editor next wakes (the person comes back to the window).
    write_plugin(
        &plugins,
        "lakes",
        "com.example.lakes",
        "Lakes",
        "TwoD",
        r#""lakes.depth": "3""#,
    );
    let now = rig.h.now();
    rig.h.set_now(now + Duration::from_secs(2));
    rig.turn();
    rig.settle();
    let new = common::part(&rig, L, &["content", "new"]);
    common::click(&mut rig, new);
    let rows = template_rows(&mut rig);
    if !rows
        .iter()
        .any(|r| r.starts_with("Lakes") && r.contains("a 2D preset from com.example.lakes"))
    {
        return Err(format!(
            "a preset plugin dropped in did not join the list: {rows:?}"
        ));
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn a_third_party_preset_plugin_shows_in_the_new_project_flow() {
    check(PanelFaults::default(), "listed").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_launcher_of_builtin_templates_misses_the_plugin_preset() {
    let e = check(
        PanelFaults {
            launcher_builtin_templates_only: true,
            ..PanelFaults::default()
        },
        "control",
    )
    .expect_err("a launcher that lists only the built-in templates must fail the check");
    assert!(e.contains("not in the new-project flow"), "{e}");
}
