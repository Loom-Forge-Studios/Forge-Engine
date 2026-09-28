//! Revision history and the store (Ch.21 §21.21 "Revision history", Ch.33 §33.4, §33.5,
//! O-11, E-36; DoD M2-50 and M2-47's "link a remote on first save"), end to end headless:
//!
//! * the first save — a `forge.project.save` command the core performs — offers to link a
//!   remote (a toast and the panel's banner); later saves do not;
//! * history is semantic: a revision lists what each issuer did (a human and an automation
//!   session);
//! * linking a remote is a command the core checks (a Git URL is refused with what works
//!   today); push, pull and clone are commands; a second editor on its own core clones the
//!   project, works, pushes, and the first pulls it back; divergence is refused, not merged.

mod common;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::project::{ProjectOp, Template};
use forge_editor::testing::Rig;

const H: &str = "forge.history";
const L: &str = "forge.launcher";

fn create(rig: &mut Rig, location: &str, name: &str) {
    let e = rig.shell.emitter().clone();
    e.emit(
        ProjectOp::Create {
            location: location.into(),
            name: name.into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
    );
    rig.settle();
    assert!(
        common::status(rig).open.is_some(),
        "{:?}",
        common::status(rig).outcomes
    );
}

fn rows(rig: &mut Rig) -> Vec<String> {
    let list = common::part(rig, H, &["content", "revisions"]);
    forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r).and_then(|k| t.label_of(k)).map(str::to_string))
            .collect()
    })
    .unwrap_or_default()
}

fn status_line(rig: &Rig) -> String {
    let status = common::part(rig, H, &["content", "status"]);
    common::label(rig, status)
}

fn press(rig: &mut Rig, path: &[&str]) {
    let id = common::part(rig, H, path);
    common::click(rig, id);
}

#[test]
fn first_save_offers_a_remote_history_is_semantic_and_two_editors_share_through_a_remote() {
    let dir = common::tmp("history");
    let mut rig = common::rig(&[H], &dir);
    create(
        &mut rig,
        &format!("file:{}", dir.join("orbits").display()),
        "Orbits",
    );
    let empty = common::part(&rig, H, &["content", "empty"]);
    assert!(common::visible(&rig, empty), "no revisions yet");
    // Work by the human and by an automation session.
    rig.shell.emitter().emit(EditorCommand::Spawn {
        name: "Crate".into(),
        parent: None,
    });
    let mut auto = rig.connect(Issuer::Automation {
        session: "sess-4".into(),
        tool: "apply".into(),
    });
    for _ in 0..3 {
        auto.apply(
            EditorCommand::Spawn {
                name: "Tree".into(),
                parent: None,
            },
            None,
        );
    }
    let _ = auto.pump();
    rig.settle();
    let head = common::part(&rig, H, &["content", "head"]);
    assert!(
        common::label(&rig, head).contains("unsaved"),
        "{}",
        common::label(&rig, head)
    );

    // First save: a command; the core commits; the remote is offered (O-11).
    let message = common::part(&rig, H, &["content", "save_bar", "message"]);
    common::type_into(&mut rig, message, "first light");
    press(&mut rig, &["content", "save_bar", "save"]);
    let st = common::status(&rig);
    let open = st.open.as_ref().unwrap_or_else(|| panic!("open"));
    assert_eq!(open.revisions.len(), 1);
    assert_eq!(open.revisions[0].message, "first light");
    assert!(open.offer_remote);
    let offer = common::part(&rig, H, &["content", "offer"]);
    assert!(
        common::visible(&rig, offer),
        "the first save offers a remote"
    );
    let (title, _) = common::last_notice(&rig).unwrap_or_default();
    assert!(title.contains("link a remote"), "{title}");
    // Semantic history: who did what.
    let r = rows(&mut rig);
    assert!(
        r[0].contains("first light") && r[0].contains("automation:sess-4"),
        "{r:?}"
    );
    assert!(
        r.iter()
            .any(|l| l == "automation:sess-4: placed 3 \u{201c}Tree\u{201d}"),
        "{r:?}"
    );
    assert!(
        r.iter()
            .any(|l| l == "human:tester: placed 1 \u{201c}Crate\u{201d}"),
        "{r:?}"
    );
    // "Not now" dismisses the offer.
    press(
        &mut rig,
        &["content", "offer", "offer_bar", "offer_dismiss"],
    );
    assert!(!common::visible(&rig, offer));

    // An SSH Git URL is refused with its HTTPS form (WP-16: Git remotes are HTTPS); a folder
    // remote links (Enter works).
    let url = common::part(
        &rig,
        H,
        &["content", "remote_group", "remote_bar", "remote_url"],
    );
    common::type_into(&mut rig, url, "git@github.com:ada/orbits.git");
    press(&mut rig, &["content", "remote_group", "remote_bar", "link"]);
    let (title, detail) = common::last_notice(&rig).unwrap_or_default();
    assert!(
        title.contains("refused") && detail.contains("https://github.com/ada/orbits.git"),
        "{title}: {detail}"
    );
    assert_eq!(common::setting(&rig, "project.remote"), None);
    let remote_dir = dir.join("nas").join("orbits");
    let remote = format!("file:{}", remote_dir.display());
    rig.h.ui.raise(
        url,
        forge_ui::widgets::Submitted {
            field: url,
            text: remote.clone(),
        },
    );
    rig.turn();
    rig.settle();
    assert_eq!(
        common::setting(&rig, "project.remote"),
        Some(Value::Text(remote.clone()))
    );
    // Linking is an edit: save it (no second offer), then push.
    press(&mut rig, &["content", "save_bar", "save"]);
    assert!(
        !common::status(&rig)
            .open
            .as_ref()
            .is_some_and(|o| o.offer_remote)
    );
    press(&mut rig, &["content", "remote_group", "remote_bar", "push"]);
    assert!(
        status_line(&rig).contains("Pushed 2 revision(s)"),
        "{}",
        status_line(&rig)
    );
    assert!(
        remote_dir.join("forge-project.ron").is_file(),
        "the remote holds the project"
    );

    // A second editor on its own core clones it from the launcher.
    let bob_dir = dir.join("bob");
    let mut bob = common::rig(&[L, H], &bob_dir);
    let field = common::part(&bob, L, &["content", "clone_bar", "clone_url"]);
    common::type_into(&mut bob, field, &remote_dir.display().to_string());
    let clone = common::part(&bob, L, &["content", "clone_bar", "clone"]);
    common::click(&mut bob, clone);
    let bst = common::status(&bob);
    let bopen = bst
        .open
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", bst.outcomes));
    assert_eq!(bopen.name, "Orbits");
    assert_eq!(bopen.revisions.len(), 2);
    assert_eq!(
        bopen.head,
        common::status(&rig)
            .open
            .as_ref()
            .and_then(|o| o.head.clone()),
        "the same revision ids on both sides"
    );
    assert!(bob_dir.join("orbits").join("forge-project.ron").is_file());
    assert!(common::names(&bob).contains(&"Tree".to_string()));
    assert_eq!(
        common::setting(&bob, "project.remote"),
        Some(Value::Text(remote.clone())),
        "the clone stays linked"
    );
    // Bob works, saves and pushes; Ada pulls it back.
    bob.shell.emitter().emit(EditorCommand::Spawn {
        name: "Bob's beacon".into(),
        parent: None,
    });
    bob.settle();
    press(&mut bob, &["content", "save_bar", "save"]);
    press(&mut bob, &["content", "remote_group", "remote_bar", "push"]);
    assert!(
        status_line(&bob).contains("Pushed 1 revision(s)"),
        "{}",
        status_line(&bob)
    );
    press(&mut rig, &["content", "remote_group", "remote_bar", "pull"]);
    assert!(
        status_line(&rig).contains("Pulled 1 revision(s)"),
        "{}",
        status_line(&rig)
    );
    assert!(common::names(&rig).contains(&"Bob's beacon".to_string()));
    assert!(rig.mirror_matches());
    // Pull refuses to overwrite unsaved work.
    rig.shell.emitter().emit(EditorCommand::Spawn {
        name: "Unsaved".into(),
        parent: None,
    });
    rig.settle();
    press(&mut rig, &["content", "remote_group", "remote_bar", "pull"]);
    assert!(
        status_line(&rig).contains("PROJECT-0008"),
        "{}",
        status_line(&rig)
    );
    // Both save on top of the same head: the second push is refused, nothing is merged.
    press(&mut rig, &["content", "save_bar", "save"]);
    bob.shell.emitter().emit(EditorCommand::Spawn {
        name: "Bob's second".into(),
        parent: None,
    });
    bob.settle();
    press(&mut bob, &["content", "save_bar", "save"]);
    press(&mut bob, &["content", "remote_group", "remote_bar", "push"]);
    press(&mut rig, &["content", "remote_group", "remote_bar", "push"]);
    assert!(
        status_line(&rig).contains("PROJECT-0007"),
        "{}",
        status_line(&rig)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_file_menu_save_is_the_save_command() {
    let dir = common::tmp("menu-save");
    let mut rig = common::rig(&[H], &dir);
    // No project: the save says why, and nothing changes.
    let h = rig.state_hash();
    rig.run("forge.file.save");
    rig.settle();
    assert_eq!(rig.state_hash(), h);
    let (title, detail) = common::last_notice(&rig).unwrap_or_default();
    assert!(
        title == "Save failed" && detail.contains("PROJECT-0001"),
        "{title}: {detail}"
    );
    create(&mut rig, "memory:menu-save", "Menu");
    rig.shell.emitter().emit(EditorCommand::Spawn {
        name: "Rock".into(),
        parent: None,
    });
    rig.settle();
    rig.run("forge.file.save");
    rig.settle();
    let st = common::status(&rig);
    let open = st.open.as_ref().unwrap_or_else(|| panic!("open"));
    assert_eq!(open.revisions.len(), 1);
    assert!(!open.dirty);
    assert!(
        open.revisions[0].summary[0].contains("placed 1 \u{201c}Rock\u{201d}"),
        "{:?}",
        open.revisions[0].summary
    );
    let _ = std::fs::remove_dir_all(&dir);
}
