//! `test_queue_and_conflicts` (Ch.37 §37.3–§37.4, O-16; DoD M2-60, M2-61, M6-13):
//!
//! * **Publish** from the panel is a command the core performs (`ProjectStore::commit` on
//!   the baseline); the panel never touches the store;
//! * with approval required, a Developer's publish **waits in the queue**; a reviewer
//!   approves it from the panel and it lands; its author cannot approve it;
//! * a pull that meets an edit of mine **stops**; the Conflicts panel shows base, mine and
//!   theirs; choosing a side and *Apply* finishes the pull with that choice.

mod common;

use common::*;
use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::core::EditorCore;
use forge_project::collab::{CollabView, RequestState, Role};

const Q: &str = "forge.publish_queue";
const C: &str = "forge.conflicts";

fn prop(rig: &forge_editor::testing::Rig, k: EntityKey, p: &str) -> Option<Value> {
    EditorCore::read(&rig.core, |x| {
        x.entity(k).and_then(|e| e.property(p)).cloned()
    })
}

#[test]
fn publish_review_and_resolve_from_the_panels() {
    let s = server();
    let mut rig = rig(&s, &[Q, C]);
    let lamp = EditorCore::read(&rig.core, |p| p.next_key());
    rig.shell.emitter().emit(EditorCommand::Spawn {
        name: "Lamp".into(),
        parent: None,
    });
    rig.shell.emitter().emit(EditorCommand::SetProperty {
        entity: lamp,
        path: "colour".into(),
        value: Value::Float(1.0),
    });
    rig.shell
        .emitter()
        .emit(forge_editor::collab::create_command("Studio"));
    rig.settle();
    let mut bob = join(&s, "bob", Role::Developer);
    feed(&mut rig);
    // Publish from the panel (an Owner publishes directly).
    rig.shell.emitter().emit(EditorCommand::SetProperty {
        entity: lamp,
        path: "size".into(),
        value: Value::Float(2.0),
    });
    rig.settle();
    let head = s.server.head();
    write(
        &mut rig,
        Q,
        &["publish_group", "publish_bar", "message"],
        "bigger lamp",
    );
    tap(&mut rig, Q, &["publish_group", "publish_bar", "publish"]);
    feed(&mut rig);
    assert_ne!(s.server.head(), head, "the publish landed");
    assert_eq!(
        s.server
            .revisions(head, 10)
            .first()
            .map(|r| r.message.clone()),
        Some("bigger lamp".into())
    );
    // Require approval (the rules are the baseline's: publish them).
    tap(
        &mut rig,
        Q,
        &["rules_group", "rules_bar", "toggle_required"],
    );
    tap(&mut rig, Q, &["publish_group", "publish_bar", "publish"]);
    feed(&mut rig);
    let label_now = label(&rig, at(&rig, Q, &["rules_group", "required"]));
    assert!(label_now.contains("required"), "{label_now}");
    // Bob's publish now waits for review.
    mate_send(&mut bob, forge_editor::collab::pull_command(None)).unwrap_or_else(|e| panic!("{e}"));
    let before = s.server.head();
    mate_send(
        &mut bob,
        EditorCommand::SetProperty {
            entity: lamp,
            path: "height".into(),
            value: Value::Float(3.0),
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    mate_send(&mut bob, forge_editor::collab::publish_command("taller"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.server.head(), before, "gated");
    feed(&mut rig);
    let list = at(&rig, Q, &["requests_group", "requests"]);
    select_containing(&mut rig, list, "waiting for review");
    tap(&mut rig, Q, &["requests_group", "review_bar", "approve"]);
    feed(&mut rig);
    assert_ne!(s.server.head(), before, "approved and published");
    assert!(matches!(
        s.server.requests().last().map(|r| r.state.clone()),
        Some(RequestState::Approved { .. })
    ));
    let r = rows(&mut rig, list);
    assert!(r.iter().any(|l| l.contains("approved by tester")), "{r:?}");
    // A conflict: both change the colour; bob publishes first (after pulling).
    mate_send(&mut bob, forge_editor::collab::pull_command(None)).unwrap_or_else(|e| panic!("{e}"));
    rig.shell
        .emitter()
        .emit(forge_editor::collab::pull_command(None));
    rig.settle();
    rig.shell.emitter().emit(EditorCommand::SetProperty {
        entity: lamp,
        path: "colour".into(),
        value: Value::Float(3.0),
    });
    rig.settle();
    // Bob is a Maintainer now, so his publish goes straight in.
    rig.shell
        .emitter()
        .emit(forge_editor::collab::set_role_command(
            "bob",
            Role::Maintainer,
        ));
    rig.settle();
    mate_send(
        &mut bob,
        EditorCommand::SetProperty {
            entity: lamp,
            path: "colour".into(),
            value: Value::Float(2.0),
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    mate_send(&mut bob, forge_editor::collab::publish_command("red"))
        .unwrap_or_else(|e| panic!("{e}"));
    rig.shell
        .emitter()
        .emit(forge_editor::collab::pull_command(None));
    rig.settle();
    feed(&mut rig);
    assert_eq!(
        prop(&rig, lamp, "colour"),
        Some(Value::Float(3.0)),
        "mine kept"
    );
    let clist = at(&rig, C, &["conflicts"]);
    let r = rows(&mut rig, clist);
    assert!(
        r.iter().any(|l| l.contains("Lamp")
            && l.contains("colour")
            && l.contains("base 1")
            && l.contains("mine 3")
            && l.contains("theirs 2")
            && l.contains("choose a side")),
        "{r:?}"
    );
    select_containing(&mut rig, clist, "colour");
    tap(&mut rig, C, &["choose_bar", "take_theirs"]);
    let r = rows(&mut rig, clist);
    assert!(r.iter().any(|l| l.contains("taking theirs")), "{r:?}");
    tap(&mut rig, C, &["apply_bar", "apply"]);
    feed(&mut rig);
    assert_eq!(
        prop(&rig, lamp, "colour"),
        Some(Value::Float(2.0)),
        "theirs taken"
    );
    assert!(rows(&mut rig, clist).is_empty(), "resolved");
    assert!(label(&rig, at(&rig, C, &["head"])).contains("No conflicts"));
    let e37 = label(&rig, at(&rig, C, &["e37"]));
    assert!(e37.contains("is not supported"), "{e37}");
}
