//! `test_presence` (Ch.37 §37.4 / §37.7; DoD M2-59, M2-69, M6-13):
//!
//! * **who is looking at what** — a teammate's selection shows in the Presence panel by
//!   name and panel, as a note on the row in the **hierarchy**, as "Also here" in the
//!   **inspector**, and in the **viewport**'s overlay; this editor publishes its own
//!   selection (session state) for them;
//! * a teammate's **claim** shows on the hierarchy row, makes the **inspector read-only** (no
//!   editable name, the rows are labels), is listed in the Ownership panel by path, and an
//!   edit sent anyway is refused by the core;
//! * presence is a **live feed at most 10 Hz, only while visible** (§21.11): a teammate
//!   moving their selection 100 times a second refreshes the panel at most ~10 times in that
//!   second. Control: an uncapped feed refreshes far more.

mod common;

use std::time::Duration;

use common::*;
use forge_cmd::{EditorCommand, EntityKey};
use forge_editor::core::EditorCore;
use forge_editor::feed::FeedRelay;
use forge_project::collab::{CollabView, Presence, Role};
use forge_ui::widgets::{Label, TextField, VirtualTree};

const P: &str = "forge.presence";
const H: &str = "forge.hierarchy";
const I: &str = "forge.inspector";
const V: &str = "forge.viewport";
const O: &str = "forge.ownership";

fn note_of(rig: &mut forge_editor::testing::Rig, k: EntityKey) -> Option<String> {
    let tree = part(rig, H, &["tree"]);
    VirtualTree::edit(&mut rig.h.ui, tree, |t| {
        t.item(k.0).and_then(|i| i.note.clone())
    })
    .flatten()
}

fn inspector_text(rig: &forge_editor::testing::Rig, path: &[&str]) -> Option<String> {
    let mut p = vec!["content"];
    p.extend_from_slice(path);
    let mut id = rig.panel_frame(I)?;
    for k in &p {
        id = id.child(&forge_ui::Key::Str((*k).into()));
    }
    rig.h.ui.widget::<Label>(id).map(|l| l.text(rig.h.ui.rt()))
}

#[test]
fn teammates_show_where_you_work_and_their_claims_are_read_only() {
    let s = server();
    let mut rig = rig(&s, &[P, H, I, V, O]);
    let cube = EditorCore::read(&rig.core, |p| p.next_key());
    rig.shell.emitter().emit(EditorCommand::Spawn {
        name: "Cube".into(),
        parent: None,
    });
    rig.shell
        .emitter()
        .emit(forge_editor::collab::create_command("Studio"));
    rig.settle();
    let mut bob = join(&s, "bob", Role::Developer);
    // Bob looks at the cube in his inspector (his editor publishes this; session state).
    s.server.set_presence(Presence {
        user: "bob".into(),
        selection: vec![cube.0],
        panel: Some("forge.inspector".into()),
        at_ms: NOW,
    });
    rig.shell.session_mut().set_selection(vec![cube]);
    rig.settle();
    feed(&mut rig);
    // The Presence panel.
    let people = at(&rig, P, &["people"]);
    let r = rows(&mut rig, people);
    assert!(
        r.iter()
            .any(|l| l == "bob \u{2014} Inspector \u{2014} looking at \u{201c}Cube\u{201d}"),
        "{r:?}"
    );
    // The hierarchy row, the inspector, the viewport.
    assert_eq!(note_of(&mut rig, cube).as_deref(), Some("\u{25cf} bob"));
    let here = inspector_text(&rig, &["collab", "here"]).unwrap_or_default();
    assert!(here.contains("Also here: bob"), "{here}");
    let vp = part(&rig, V, &["cells"]).child(&forge_ui::Key::Index(0));
    let vlabel = rig
        .h
        .node(vp)
        .and_then(|n| n.label().map(str::to_string))
        .unwrap_or_default();
    assert!(
        vlabel.contains("Teammates here: bob \u{2192} \u{201c}Cube\u{201d}"),
        "{vlabel}"
    );
    // This editor published its own selection for them.
    let mine = s.server.presence().into_iter().find(|p| p.user == "tester");
    assert_eq!(mine.map(|p| p.selection), Some(vec![cube.0]));
    // Bob claims the cube: read-only here, shown everywhere, enforced by the core.
    mate_send(&mut bob, forge_editor::collab::claim_command(cube))
        .unwrap_or_else(|e| panic!("{e}"));
    feed(&mut rig);
    let note = note_of(&mut rig, cube).unwrap_or_default();
    assert!(note.contains("claimed by bob (read-only)"), "{note}");
    let claimed = inspector_text(&rig, &["collab", "claimed"]).unwrap_or_default();
    assert!(claimed.contains("Claimed by bob"), "{claimed}");
    let name = rig
        .panel_frame(I)
        .map(|f| {
            f.child(&forge_ui::Key::Str("content".into()))
                .child(&forge_ui::Key::Str("head".into()))
                .child(&forge_ui::Key::Str("name".into()))
        })
        .unwrap_or_else(|| panic!("inspector"));
    assert!(
        rig.h.ui.widget::<TextField>(name).is_none(),
        "the name is not editable"
    );
    assert!(rig.h.ui.widget::<Label>(name).is_some());
    let team_claims = at(&rig, O, &["team_group", "team"]);
    let r = rows(&mut rig, team_claims);
    assert!(r.iter().any(|l| l == "scene/Cube \u{2014} bob"), "{r:?}");
    rig.shell.emitter().emit(EditorCommand::Rename {
        entity: cube,
        name: "Mine".into(),
    });
    rig.settle();
    let n = notices(&rig);
    assert!(
        n.iter()
            .any(|l| l.contains("CMD-0014") && l.contains("claimed by bob")),
        "{n:?}"
    );
    // Bob leaves: the notes go.
    s.server.clear_presence("bob");
    mate_send(&mut bob, forge_editor::collab::release_command(cube))
        .unwrap_or_else(|e| panic!("{e}"));
    feed(&mut rig);
    assert_eq!(note_of(&mut rig, cube), None);
}

/// Refreshes of the Presence panel's feed while a teammate's selection moves at 100 Hz for
/// one second.
fn refreshes_in_a_busy_second(uncapped: bool) -> u64 {
    let s = server();
    let mut rig = rig_with(&s, &[P], |cfg| {
        cfg.services.collab.faults.feed_uncapped = uncapped;
    });
    rig.shell
        .emitter()
        .emit(forge_editor::collab::create_command("Studio"));
    rig.settle();
    let _bob = join(&s, "bob", Role::Developer);
    feed(&mut rig);
    let relay = at(&rig, P, &["feed"]);
    let before = rig
        .h
        .ui
        .widget::<FeedRelay>(relay)
        .map_or(0, |r| r.refreshes);
    for i in 0..100u64 {
        s.server.set_presence(Presence {
            user: "bob".into(),
            selection: vec![i],
            panel: None,
            at_ms: NOW + i,
        });
        rig.advance(Duration::from_millis(10));
    }
    rig.h
        .ui
        .widget::<FeedRelay>(relay)
        .map_or(0, |r| r.refreshes)
        - before
}

#[test]
fn presence_is_coalesced_to_ten_hertz() {
    let n = refreshes_in_a_busy_second(false);
    assert!((1..=11).contains(&n), "{n} refreshes in a second");
}

#[test]
fn positive_control_an_uncapped_presence_feed_fails() {
    let n = refreshes_in_a_busy_second(true);
    assert!(n > 11, "the control must exceed 10 Hz: {n}");
}
