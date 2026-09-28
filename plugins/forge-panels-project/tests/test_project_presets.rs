//! The WP-U7 preset switcher over the **real preset data** (WP-16, M2-12; gate row
//! `C-project-presets-from-folder`): a project that carries its own copy of a preset in its
//! `presets/` folder ("users author their own presets by copying one", Ch.31 §31.3) uses that
//! copy — the switcher marks it, and switching to it applies the copy's defaults, through the
//! core's promote command exactly as the panel planned it; a user's own value stays; a copy
//! that does not load is named and the built-in preset stays in its place.
//!
//! Positive control (W2): `positive_control_a_core_that_ignores_the_projects_presets_fails`
//! — a core whose promote planner uses only the built-in presets (the panel still shows the
//! project's copy) applies the wrong defaults, and the check names each one.

mod common;

use forge_cmd::{CommandPolicy, EditorCommand, Value};
use forge_editor::core::EditorCore;
use forge_editor::presets::Family;
use forge_editor::project::{PROMOTE_CMD, ProjectOp, Template};
use forge_editor::testing::Rig;

const P: &str = "forge.presets";

/// The built-in 3D preset, copied into a project and edited: an orthographic camera and two
/// defaults no built-in preset has.
const OWN_3D: &str = r#"(
    version: 1,
    id: "studio.preset.3d",
    label: "Studio 3D",
    family: ThreeD,
    plugins: ["forge.editor", "forge.panels.core", "forge.panels.scene"],
    layout: "layout.ron",
    open_panels: [
        "forge.hierarchy", "forge.viewport", "forge.play_controls", "forge.console",
        "forge.assets", "forge.inspector",
    ],
    settings: {
        "frames.depth": "1",
        "render.path": "\"3d\"",
        "physics.backend": "\"avian3d\"",
        "camera.projection": "\"orthographic\"",
        "render.exposure": "1.5",
        "render.bloom": "true",
    },
    scene_template: Some("templates/3d_empty.scene.ron"),
)"#;

/// A copy placed in the wrong directory: it says 3D but sits in `presets/2d`.
const MISPLACED: &str = r#"(
    version: 1, id: "x", label: "X", family: ThreeD, plugins: [], layout: "layout.ron",
    open_panels: [
        "forge.hierarchy", "forge.viewport", "forge.play_controls", "forge.console",
        "forge.assets", "forge.inspector",
    ],
)"#;

fn write(dir: &std::path::Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{e}"));
    }
    std::fs::write(&p, text).unwrap_or_else(|e| panic!("{e}"));
}

/// Create a 2D project, give it its own 3D preset (and a misplaced copy), reopen it, check
/// the switcher shows the copy, switch to 3D with one click. The failures, one line
/// each (empty: the project's own preset is what switching applied).
fn own_preset_applies(rig: &mut Rig, dir: &std::path::Path) -> Vec<String> {
    let mut bad = Vec::new();
    let project = dir.join("orbits");
    let location = format!("file:{}", project.display());
    rig.shell.emitter().emit(
        ProjectOp::Create {
            location: location.clone(),
            name: "Orbits".into(),
            template: Template::TwoD,
            discard_unsaved: false,
        }
        .command(),
    );
    rig.settle();
    assert!(common::status(rig).open.is_some());
    // The user copies the 3D preset into the project and edits it (and misplaces a copy).
    let layout = include_str!("../../../presets/3d/layout.ron");
    let scene = include_str!("../../../presets/3d/templates/3d_empty.scene.ron");
    write(&project, "presets/3d/workspace.ron", OWN_3D);
    write(&project, "presets/3d/layout.ron", layout);
    write(&project, "presets/3d/templates/3d_empty.scene.ron", scene);
    write(&project, "presets/2d/workspace.ron", MISPLACED);
    write(&project, "presets/2d/layout.ron", layout);
    // A value of the user's for a key only the copy gives a default.
    rig.shell.emitter().emit(EditorCommand::SetSetting {
        key: "render.exposure".into(),
        value: Some(Value::Float(2.0)),
    });
    rig.settle();
    rig.shell
        .emitter()
        .emit(forge_editor::project::save_command("before presets"));
    rig.settle();
    // Opening reads the project's presets from its store.
    rig.shell.emitter().emit(
        ProjectOp::Open {
            location,
            discard_unsaved: false,
        }
        .command(),
    );
    rig.settle();
    let st = common::status(rig);
    let open = st
        .open
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", st.outcomes));
    assert_eq!(
        open.presets.keys().cloned().collect::<Vec<_>>(),
        ["2d", "3d"],
        "the project's own presets travel with it"
    );

    // The switcher marks the project's copy and names the one that does not load.
    let list = common::part(rig, P, &["content", "presets"]);
    let label = |rig: &mut Rig, k: u64| {
        forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| {
            t.label_of(k).map(str::to_string)
        })
        .flatten()
        .unwrap_or_default()
    };
    if !label(rig, 1).contains("the project's own preset") {
        bad.push(format!(
            "the 3D row does not say it is the project's: {}",
            label(rig, 1)
        ));
    }
    if label(rig, 0).contains("own preset") {
        bad.push("a broken copy is used".into());
    }
    let problems = common::part(rig, P, &["content", "preset_problems"]);
    if !common::visible(rig, problems) || !common::label(rig, problems).contains("presets/2d") {
        bad.push(format!(
            "the misplaced copy is not named: {:?}",
            common::label(rig, problems)
        ));
    }

    // 2D → 3D (additive: lossless): one click, and the copy's defaults apply.
    let i = Family::ALL
        .iter()
        .position(|f| *f == Family::ThreeD)
        .unwrap_or(1) as u64;
    common::select_row(rig, list, i);
    let promote = common::part(rig, P, &["content", "detail", "detail_bar", "promote"]);
    common::click(rig, promote);
    let mut want = |key: &str, v: Option<Value>, why: &str| {
        let got = common::setting(rig, key);
        if got != v {
            bad.push(format!("{key} is {got:?}, not {v:?}: {why}"));
        }
    };
    want("project.preset", Some(Value::Text("3d".into())), "switched");
    want(
        "camera.projection",
        Some(Value::Text("orthographic".into())),
        "the project's 3D default (the 2D default was orthographic too), not the built-in's perspective",
    );
    want(
        "render.bloom",
        Some(Value::Bool(true)),
        "a default only the copy has",
    );
    want(
        "render.exposure",
        Some(Value::Float(2.0)),
        "a user's own value stays (I15: presets set defaults)",
    );
    want(
        "camera.pixel_snap",
        None,
        "the 2D-only default went (the project still had it as a default)",
    );
    bad
}

#[test]
fn a_projects_own_preset_is_what_switching_applies() {
    let dir = common::tmp("project-presets");
    let mut rig = common::rig(&[P], &dir);
    let bad = own_preset_applies(&mut rig, &dir);
    assert!(bad.is_empty(), "{bad:#?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn positive_control_a_core_that_ignores_the_projects_presets_fails() {
    let dir = common::tmp("project-presets-control");
    let cfg = common::config("3d");
    cfg.services.launcher.borrow_mut().projects_dir = dir.clone();
    // The core's promote planner is the built-in-only one: it never sees the project's copy.
    let mut bus = EditorCore::editor_bus();
    bus.register_handler(
        PROMOTE_CMD,
        CommandPolicy::ORDINARY,
        forge_editor::project::promote::plan_promote,
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
    let host = forge_project::host::ProjectHost::first_party().unwrap_or_else(|e| panic!("{e}"));
    let core = EditorCore::with_host(bus, host);
    let mut rig = Rig::with_core(cfg, core).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&[P]).unwrap_or_else(|e| panic!("{e}"));
    let bad = own_preset_applies(&mut rig, &dir);
    assert!(
        bad.iter().any(|l| l.starts_with("camera.projection")),
        "a core ignoring the project's presets passed: {bad:#?}"
    );
    assert!(
        bad.iter().any(|l| l.starts_with("render.bloom")),
        "{bad:#?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
