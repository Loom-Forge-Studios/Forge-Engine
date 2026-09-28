//! `test_sandbox_panel` (Ch.37 §37.2–§37.3; DoD M2-58):
//!
//! * the **Live/Pull toggle is one click and reversible**: in Live a teammate's publish
//!   reaches this editor with no click at all; back in Pull the next one waits in the
//!   incoming list until *Pull everything* (or *Pull up to the selected*);
//! * the sandbox's base and unpublished changes are shown; visibility is one click
//!   (default `Team`, O-17), and a `Private` sandbox is withheld from teammates;
//! * the plain statement E-37 requires is on the panel.

mod common;

use common::*;
use forge_cmd::{EditorCommand, Value};
use forge_editor::core::EditorCore;
use forge_project::collab::{CollabView, Policy, Role, Visibility};

const S: &str = "forge.sandbox";

fn entity_named(rig: &forge_editor::testing::Rig, name: &str) -> bool {
    EditorCore::read(&rig.core, |p| p.entities().any(|(_, e)| e.name() == name))
}

#[test]
fn live_pull_is_one_click_and_publishes_arrive_accordingly() {
    let s = server();
    let mut rig = rig(&s, &[S]);
    rig.shell
        .emitter()
        .emit(forge_editor::collab::create_command("Studio"));
    rig.settle();
    let mut bob = join(&s, "bob", Role::Developer);
    feed(&mut rig);
    let e37 = label(&rig, at(&rig, S, &["e37"]));
    assert!(e37.contains("is not supported"), "{e37}");
    assert!(label(&rig, at(&rig, S, &["head"])).contains("0 unpublished"));
    assert_eq!(s.server.policy("tester"), Policy::Pull, "Pull by default");
    // One click: Live. Bob publishes; it arrives without anyone clicking.
    tap(&mut rig, S, &["policy_bar", "policy_toggle"]);
    assert_eq!(s.server.policy("tester"), Policy::Live);
    assert!(label(&rig, at(&rig, S, &["policy_bar", "policy"])).starts_with("Live"));
    let tree = spawn(&mut bob, "Tree", None);
    mate_send(&mut bob, forge_editor::collab::publish_command("a tree"))
        .unwrap_or_else(|e| panic!("{e}"));
    feed(&mut rig);
    assert!(
        entity_named(&rig, "Tree"),
        "Live brought the teammate's publish in"
    );
    // One click back: Pull. The next publish waits.
    tap(&mut rig, S, &["policy_bar", "policy_toggle"]);
    assert_eq!(s.server.policy("tester"), Policy::Pull);
    mate_send(
        &mut bob,
        EditorCommand::Rename {
            entity: tree,
            name: "Oak".into(),
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    mate_send(&mut bob, forge_editor::collab::publish_command("renamed"))
        .unwrap_or_else(|e| panic!("{e}"));
    feed(&mut rig);
    assert!(!entity_named(&rig, "Oak"), "Pull waits");
    let incoming = at(&rig, S, &["incoming_group", "incoming"]);
    let r = rows(&mut rig, incoming);
    assert!(
        r.len() == 1 && r[0].contains("human:bob") && r[0].contains("renamed"),
        "{r:?}"
    );
    tap(&mut rig, S, &["incoming_group", "pull_bar", "pull_all"]);
    feed(&mut rig);
    assert!(entity_named(&rig, "Oak"), "pulled");
    assert!(rows(&mut rig, incoming).is_empty());
    // My own edits are unpublished changes.
    rig.shell.emitter().emit(EditorCommand::SetSetting {
        key: "editor.grid_size".into(),
        value: Some(Value::Float(2.0)),
    });
    rig.settle();
    feed(&mut rig);
    let head = label(&rig, at(&rig, S, &["head"]));
    assert!(head.contains("1 unpublished"), "{head}");
}

#[test]
fn visibility_is_one_click_and_private_is_withheld_from_teammates() {
    let s = server();
    let mut rig = rig(&s, &[S]);
    rig.shell
        .emitter()
        .emit(forge_editor::collab::create_command("Studio"));
    rig.settle();
    let _bob = join(&s, "bob", Role::Developer);
    rig.shell.emitter().emit(EditorCommand::Spawn {
        name: "Draft".into(),
        parent: None,
    });
    rig.settle();
    feed(&mut rig);
    assert_eq!(
        s.server.visibility("tester"),
        Visibility::Team,
        "Team by default"
    );
    let seen_by_bob = || {
        s.server
            .sandboxes("bob")
            .into_iter()
            .find(|r| r.owner == "tester")
            .map(|r| (r.withheld, r.unpublished))
    };
    assert_eq!(seen_by_bob(), Some((false, 1)));
    tap(&mut rig, S, &["visibility_bar", "vis_private"]);
    assert_eq!(s.server.visibility("tester"), Visibility::Private);
    assert_eq!(seen_by_bob(), Some((true, 0)), "withheld from a teammate");
    tap(&mut rig, S, &["visibility_bar", "vis_team"]);
    assert_eq!(seen_by_bob(), Some((false, 1)));
    // Teammates' sandboxes, as I may see them.
    let list = at(&rig, S, &["teammates", "sandboxes"]);
    feed(&mut rig);
    let r = rows(&mut rig, list);
    assert!(
        r.iter().any(|l| l.starts_with("bob — Team — Pull")),
        "{r:?}"
    );
}
