//! The launcher (Ch.21 §21.21 "Launcher and new project", Ch.33 §33.5, O-11, O-14; DoD
//! M2-47), end to end through the headless shell and core:
//!
//! * **three clicks** — *New project…*, a template, *Create* — make a real project folder
//!   (`LocalFs`) from the template, through `forge.project.create` issued by the human;
//! * no remote is asked for at creation (O-11); the recent list remembers the project;
//! * opening by location is `forge.project.open`; unsaved changes are never dropped
//!   silently (save first, discard, or cancel).

mod common;

use forge_cmd::{EditorCommand, Value};
use forge_editor::client::BusClient;
use forge_editor::project::{ProjectOp, Template};

const L: &str = "forge.launcher";

#[test]
fn three_clicks_create_a_project_from_a_template() {
    let dir = common::tmp("three-clicks");
    let mut rig = common::rig(&[L, "forge.history"], &dir);
    // Click 1: New project…
    let new = common::part(&rig, L, &["content", "new"]);
    common::click(&mut rig, new);
    let form = common::part(&rig, L, &["content", "form"]);
    assert!(common::visible(&rig, form), "the form opens");
    // Click 2: the 3D template.
    let templates = common::part(&rig, L, &["content", "form", "templates"]);
    common::select_row(&mut rig, templates, 1);
    // Click 3: Create.
    let create = common::part(&rig, L, &["content", "form", "form_bar", "create"]);
    common::click(&mut rig, create);

    let st = common::status(&rig);
    let o = st.outcomes.last().unwrap_or_else(|| panic!("an outcome"));
    assert_eq!(o.op, "forge.project.create");
    assert_eq!(
        o.issuer, "human:tester",
        "issued through the shell's bus client"
    );
    assert!(o.result.is_ok(), "{:?}", o.result);
    let open = st
        .open
        .as_ref()
        .unwrap_or_else(|| panic!("a project is open"));
    assert_eq!(open.name, "New Project");
    assert_eq!(
        open.backend, "local-fs",
        "the default store is a local folder"
    );
    let folder = dir.join("new_project");
    assert!(
        folder.join("forge-project.ron").is_file(),
        "a real project folder: {}",
        folder.display()
    );
    assert!(folder.join("project").join("scene.ron").is_file());
    // The template: the 3D preset and its new scene.
    assert_eq!(
        common::setting(&rig, "project.preset"),
        Some(Value::Text("3d".into()))
    );
    assert!(common::names(&rig).contains(&"Ground".to_string()));
    assert!(rig.mirror_matches());
    // O-11: nothing about remotes at creation — no revision yet, no offer.
    assert!(open.revisions.is_empty() && !open.offer_remote);
    let offer = common::part(&rig, "forge.history", &["content", "offer"]);
    assert!(!common::visible(&rig, offer));
    // The creation cannot be undone into the previous project: history starts fresh.
    assert!(rig.shell.mirror().history().is_empty());
    // The recent list remembers it, and the form closed.
    let recent = rig
        .shell
        .handles()
        .services()
        .launcher
        .borrow()
        .recent
        .clone();
    assert_eq!(recent.entries.len(), 1);
    assert_eq!(recent.entries[0].name, "New Project");
    assert_eq!(recent.entries[0].template, "3d");
    assert!(!common::visible(&rig, form));
    let status = common::part(&rig, L, &["content", "status"]);
    assert!(
        common::label(&rig, status).starts_with("Created"),
        "{}",
        common::label(&rig, status)
    );
    // A second create gets its own folder (never over an existing project).
    common::click(&mut rig, new);
    let create = common::part(&rig, L, &["content", "form", "form_bar", "create"]);
    common::click(&mut rig, create);
    assert!(
        dir.join("new_project_2")
            .join("forge-project.ron")
            .is_file()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_by_location_and_unsaved_changes_are_never_dropped_silently() {
    let dir = common::tmp("open");
    let mut rig = common::rig(&[L], &dir);
    // A setup free of automation sessions: a script makes a project elsewhere and saves it.
    let other = dir.join("other");
    let mut s = common::script(&rig);
    s.apply(
        ProjectOp::Create {
            location: format!("file:{}", other.display()),
            name: "Other".into(),
            template: Template::TwoD,
            discard_unsaved: false,
        }
        .command(),
        None,
    );
    s.apply(
        EditorCommand::Spawn {
            name: "Hero".into(),
            parent: None,
        },
        None,
    );
    s.apply(forge_editor::project::save_command("hero"), None);
    let _ = s.pump();
    // Now the tester's own project, with an unsaved edit.
    s.apply(
        ProjectOp::Create {
            location: format!("file:{}", dir.join("mine").display()),
            name: "Mine".into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
        None,
    );
    let _ = s.pump();
    rig.settle();
    let emitter = rig.shell.emitter();
    emitter.emit(EditorCommand::Spawn {
        name: "Unsaved crate".into(),
        parent: None,
    });
    rig.settle();
    assert!(common::status(&rig).open.as_ref().is_some_and(|o| o.dirty));

    let field = common::part(&rig, L, &["content", "open_bar", "open_location"]);
    common::type_into(&mut rig, field, &other.display().to_string());
    let open = common::part(&rig, L, &["content", "open_bar", "open"]);
    common::click(&mut rig, open);
    // Asked first: nothing was sent.
    let unsaved = common::part(&rig, L, &["content", "unsaved"]);
    assert!(
        common::visible(&rig, unsaved),
        "the unsaved-changes question"
    );
    let q = common::part(&rig, L, &["content", "unsaved", "unsaved_text"]);
    assert!(
        common::label(&rig, q).contains("Mine"),
        "{}",
        common::label(&rig, q)
    );
    assert!(common::names(&rig).contains(&"Unsaved crate".to_string()));
    let keep = common::part(&rig, L, &["content", "unsaved", "unsaved_bar", "keep"]);
    common::click(&mut rig, keep);
    assert!(!common::visible(&rig, unsaved));
    assert!(common::names(&rig).contains(&"Unsaved crate".to_string()));
    // Save first, then open: both go, in order.
    common::click(&mut rig, open);
    let save_first = common::part(
        &rig,
        L,
        &["content", "unsaved", "unsaved_bar", "save_first"],
    );
    common::click(&mut rig, save_first);
    let st = common::status(&rig);
    assert_eq!(st.open.as_ref().map(|o| o.name.as_str()), Some("Other"));
    assert!(common::names(&rig).contains(&"Hero".to_string()));
    let saved = st
        .outcomes
        .iter()
        .any(|o| o.op == "forge.project.save" && o.issuer == "human:tester" && o.result.is_ok());
    assert!(saved, "the unsaved work was saved before the open");
    // The 2D project's own preset came with it.
    assert_eq!(
        common::setting(&rig, "project.preset"),
        Some(Value::Text("2d".into()))
    );
    // Re-open "Mine" from the recent list: it has the crate that was saved.
    let recent = common::part(&rig, L, &["content", "recent_group", "recent"]);
    let labels: Vec<String> = forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, recent, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r).and_then(|k| t.label_of(k)).map(str::to_string))
            .collect()
    })
    .unwrap_or_default();
    assert!(labels.iter().any(|l| l.contains("Other")), "{labels:?}");
    // A location with no project is refused with its code, and nothing changes.
    common::type_into(&mut rig, field, &dir.join("nothing").display().to_string());
    common::click(&mut rig, open);
    let status = common::part(&rig, L, &["content", "status"]);
    assert!(
        common::label(&rig, status).contains("PROJECT-0003"),
        "{}",
        common::label(&rig, status)
    );
    assert_eq!(
        common::status(&rig).open.as_ref().map(|o| o.name.as_str()),
        Some("Other")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
