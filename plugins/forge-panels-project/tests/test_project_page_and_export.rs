//! The settings window's **Project** page (Ch.21 §21.21, DoD M2-47) and **Build and
//! export** (DoD M2-51), end to end headless:
//!
//! * the page is generated from the reflected `ProjectLifecycleSettings`; the name is a
//!   project-setting row (its edit a command, undoable); the remote, store and preset rows
//!   are read-only, show where the value comes from, and open the dialog that changes it
//!   (so a lossy change keeps its one confirmation path);
//! * the export panel shows the E-64 attribution the packager will insert — credits,
//!   executable metadata, `forge.json`, the `NOTICES` line, the store page and the
//!   splash — and a `NOTICES` preview; building is `forge.export.build`, performed by the
//!   core's packager (in memory: the executable is UNBUILT, the rest is real).

mod common;

use forge_cmd::Value;
use forge_editor::project::{ProjectOp, Template};
use forge_editor::testing::Rig;

const S: &str = "forge.settings";
const E: &str = "forge.export";

fn create(rig: &mut Rig, tag: &str) {
    rig.shell.emitter().emit(
        ProjectOp::Create {
            location: format!("memory:page-{tag}-{}", std::process::id()),
            name: "Skyfall".into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
    );
    rig.settle();
}

fn row(rig: &Rig, key: &str, part: &str) -> forge_ui::WidgetId {
    common::part(rig, S, &["content", &format!("lifecycle.{key}"), part])
}

#[test]
fn the_project_page_edits_through_commands_and_opens_the_dialogs() {
    let dir = common::tmp("page");
    let mut rig = common::rig(&[S], &dir);
    create(&mut rig, "a");
    // Read-only rows show the project as it is.
    let store = row(&rig, "project.store_backend", "value");
    assert!(
        common::label(&rig, store).contains("memory:page-a"),
        "{}",
        common::label(&rig, store)
    );
    let preset = row(&rig, "project.preset", "value");
    assert_eq!(common::label(&rig, preset), "3D");
    let remote = row(&rig, "project.remote", "value");
    assert!(common::label(&rig, remote).starts_with("Not linked"));
    // The name row is an ordinary project-setting row: its edit is a command, undoable.
    let name = row(&rig, "project.name", "edit");
    rig.h.ui.raise(
        name,
        forge_ui::widgets::Submitted {
            field: name,
            text: "Skyfall II".into(),
        },
    );
    rig.turn();
    rig.settle();
    assert_eq!(
        common::setting(&rig, "project.name"),
        Some(Value::Text("Skyfall II".into()))
    );
    let entry = rig.shell.mirror().history().last().map(|h| h.issuer_tag());
    assert_eq!(entry.as_deref(), Some("human:tester"));
    rig.shell.emitter().undo();
    rig.settle();
    assert_eq!(
        common::setting(&rig, "project.name"),
        Some(Value::Text("Skyfall".into()))
    );
    // The read-only rows do not edit: they open the dialog that does.
    for (key, panel) in [
        ("project.preset", "forge.presets"),
        ("project.remote", "forge.history"),
    ] {
        let open = row(&rig, key, "open");
        let h = rig.state_hash();
        // The page scrolls; press the button as a click on it would.
        rig.h.ui.raise(open, forge_ui::widgets::Pressed(open));
        rig.turn();
        rig.settle();
        assert_eq!(
            rig.state_hash(),
            h,
            "{key}: opening a dialog changes nothing"
        );
        assert!(
            rig.shell
                .layout()
                .panels()
                .iter()
                .any(|p| p.as_str() == panel),
            "{key} opened {panel}: {:?}",
            rig.shell.layout().panels()
        );
    }
    // A change made elsewhere shows on the page (a script switches the preset to 2D).
    let mut s = common::script(&rig);
    use forge_editor::client::BusClient;
    s.apply(
        forge_editor::project::promote::promote_command(forge_editor::presets::Family::TwoD, true),
        None,
    );
    let _ = s.pump();
    rig.settle();
    let preset = row(&rig, "project.preset", "value");
    assert_eq!(common::label(&rig, preset), "2D");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_shows_the_attribution_and_notices_and_builds_through_the_core() {
    let dir = common::tmp("export");
    let mut rig = common::rig(&[E], &dir);
    // No project: the build is refused with its code (the attribution still previews).
    let build = common::part(&rig, E, &["content", "build_bar", "build"]);
    common::click(&mut rig, build);
    let result = common::part(&rig, E, &["content", "result"]);
    assert!(
        common::label(&rig, result).contains("PROJECT-0001"),
        "{}",
        common::label(&rig, result)
    );
    create(&mut rig, "export");
    let rows: Vec<String> = (0..6)
        .map(|i| {
            let id = common::part(&rig, E, &["content", "attribution", &format!("a{i}")]);
            common::label(&rig, id)
        })
        .collect();
    let all = rows.join("\n");
    for want in [
        "In-product credits: Made with Forge Engine",
        "Executable metadata: VERSIONINFO",
        "ProductName = Skyfall",
        "\"engine\": \"Forge Engine\"",
        "NOTICES:",
        "Store page:",
        "splash: on by default",
    ] {
        assert!(all.contains(want), "missing {want:?} in:\n{all}");
    }
    let notices = common::part(&rig, E, &["content", "notices_group", "notices"]);
    assert!(common::label(&rig, notices).starts_with("FORGE ENGINE"));
    // Linux: the metadata goes in an ELF note; build it.
    let targets = common::part(&rig, E, &["content", "targets"]);
    common::select_row(&mut rig, targets, 1);
    let rows1 = common::part(&rig, E, &["content", "attribution", "a1"]);
    assert!(common::label(&rig, rows1).contains(".note.forge"));
    let build = common::part(&rig, E, &["content", "build_bar", "build"]);
    common::click(&mut rig, build);
    let r = common::label(&rig, result);
    for want in [
        "Built \u{201c}Skyfall\u{201d} for Linux",
        "forge.json",
        "NOTICES",
        "credits.txt",
        "Skyfall (UNBUILT)",
    ] {
        assert!(r.contains(want), "missing {want:?} in {r:?}");
    }
    let st = common::status(&rig);
    let b = st.last_build.as_ref().unwrap_or_else(|| panic!("a build"));
    assert!(b.in_memory, "labelled: the in-memory packager");
    assert!(b.attribution.forge_json.contains("\"target\": \"linux\""));
    let _ = std::fs::remove_dir_all(&dir);
}
