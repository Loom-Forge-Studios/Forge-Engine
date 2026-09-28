//! The scene hierarchy through the running shell (DoD M2-35): every edit is a command
//! (rename, drag-reparent, delete, add, visibility, lock), several rows are one
//! transaction, locked entities refuse edits, search keeps ancestors, and another issuer's
//! changes arrive through the mirror.

mod common;

use forge_cmd::{EntityKey, Value};
use forge_panels_scene::hierarchy::{HIDDEN, LOCKED};
use forge_ui::widgets::{
    DropTarget, RowBadgeClicked, RowRenamed, RowsDeleteRequested, RowsDropped, SearchChanged,
    SelectionChanged, VirtualTree,
};
use forge_ui::{Point, WidgetId};

const P: &str = "forge.hierarchy";

fn tree(rig: &forge_editor::testing::Rig) -> WidgetId {
    common::part(rig, P, &["tree"])
}

fn rows(rig: &mut forge_editor::testing::Rig) -> Vec<(u64, String)> {
    let t = tree(rig);
    VirtualTree::edit(&mut rig.h.ui, t, |v| {
        (0..v.row_count())
            .filter_map(|r| {
                v.key_at(r)
                    .map(|k| (k, v.label_of(k).unwrap_or("").to_string()))
            })
            .collect()
    })
    .unwrap_or_default()
}

fn badge_point(rig: &forge_editor::testing::Rig, row: usize, badge: usize) -> Point {
    let r = rig.h.ui.rect(tree(rig)).unwrap_or_default();
    // Row rect: x+1 .. right-9 (the scroll bar), badges from the right, 22 px each.
    let right = r.x + r.w - 9.0;
    let x = right - (2 - badge) as f32 * 22.0 - 2.0 + 11.0;
    Point::new(x, r.y + row as f32 * 24.0 + 12.0)
}

#[test]
fn every_hierarchy_edit_is_a_command_and_undoes() {
    let mut rig = common::rig(&[P, "forge.undo_history"]);
    let start = rig.state_hash();
    let a = common::spawn(&mut rig, "Alpha", None);
    let b = common::spawn(&mut rig, "Beta", None);
    let after_setup = rig.state_hash();
    let h0 = common::history_len(&rig);
    assert_eq!(
        rows(&mut rig)
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>(),
        vec!["Alpha", "Beta"],
        "a script's entities appear (through the mirror)"
    );
    let t = tree(&rig);

    // Rename in place: one command, as the human.
    rig.h.ui.raise(
        t,
        RowRenamed {
            view: t,
            key: a.0,
            name: "Ship".into(),
        },
    );
    rig.turn();
    assert_eq!(
        rig.shell.mirror().entity(a).map(|e| e.name.clone()),
        Some("Ship".into())
    );
    let (label, issuer) = common::last_entry(&rig);
    assert!(label.starts_with("Rename"), "{label}");
    assert_eq!(issuer, "human:tester");
    assert_eq!(rows(&mut rig)[0].1, "Ship", "the row follows the mirror");

    // Drag Beta onto Ship: reparent.
    rig.h.ui.raise(
        t,
        RowsDropped {
            view: t,
            keys: vec![b.0],
            target: DropTarget {
                parent: Some(a.0),
                index: 0,
            },
        },
    );
    rig.turn();
    assert_eq!(rig.shell.mirror().entity(b).and_then(|e| e.parent), Some(a));
    assert!(common::last_entry(&rig).0.starts_with("Move"));

    // Visibility toggle with a real click on the row's badge.
    rig.h.click_at(badge_point(&rig, 0, 0));
    rig.turn();
    assert_eq!(
        rig.shell.mirror().property(a, HIDDEN),
        Some(&Value::Bool(true))
    );
    assert!(common::last_entry(&rig).0.starts_with("Hide"));
    let label = rows(&mut rig)[0].1.clone();
    assert_eq!(label, "Ship");
    let item_hidden = VirtualTree::edit(&mut rig.h.ui, t, |v| {
        v.item(a.0).map(|i| (i.muted, i.badges[0].on))
    })
    .flatten();
    assert_eq!(item_hidden, Some((true, false)), "dimmed, eye off");

    // Lock toggle; a locked entity refuses rename and delete from here.
    rig.h.click_at(badge_point(&rig, 0, 1));
    rig.turn();
    assert_eq!(
        rig.shell.mirror().property(a, LOCKED),
        Some(&Value::Bool(true))
    );
    let before = rig.state_hash();
    let hn = common::history_len(&rig);
    rig.h.ui.raise(
        t,
        RowRenamed {
            view: t,
            key: a.0,
            name: "Nope".into(),
        },
    );
    rig.turn();
    rig.h.ui.raise(
        t,
        RowsDeleteRequested {
            view: t,
            keys: vec![a.0],
        },
    );
    rig.turn();
    assert_eq!(rig.state_hash(), before, "locked: nothing changed");
    assert_eq!(common::history_len(&rig), hn, "no command was sent");
    let warned = rig
        .shell
        .session()
        .notifications
        .history()
        .any(|n| n.title.contains("locked"));
    assert!(warned, "the refusal names the lock");

    // Unlock, delete Beta, add a new entity with the button.
    rig.h.click_at(badge_point(&rig, 0, 1));
    rig.turn();
    assert_eq!(
        rig.shell.mirror().property(a, LOCKED),
        None,
        "unlocking removes the flag"
    );
    rig.h.ui.raise(
        t,
        RowsDeleteRequested {
            view: t,
            keys: vec![b.0],
        },
    );
    rig.turn();
    assert!(rig.shell.mirror().entity(b).is_none());
    rig.h.click(common::part(&rig, P, &["bar", "add"]));
    rig.turn();
    assert_eq!(rig.shell.mirror().len(), 2);
    assert!(rig.mirror_matches());

    // Every one of those was an undo entry of its own; undo them all.
    let edits = common::history_len(&rig) - h0;
    assert_eq!(edits, 7, "rename, move, hide, lock, unlock, delete, add");
    for _ in 0..edits {
        rig.shell.emitter().undo();
        rig.turn();
    }
    assert_eq!(rig.state_hash(), after_setup);
    assert_ne!(after_setup, start);
    assert_eq!(
        rows(&mut rig)
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>(),
        vec!["Alpha", "Beta"],
        "undo flows back through the mirror into the rows"
    );
}

#[test]
fn a_multi_selection_edits_as_one_transaction() {
    let mut rig = common::rig(&[P]);
    let keys: Vec<EntityKey> = (0..5)
        .map(|i| common::spawn(&mut rig, &format!("E{i}"), None))
        .collect();
    let t = tree(&rig);
    let sel: Vec<u64> = keys[1..4].iter().map(|k| k.0).collect();
    VirtualTree::edit(&mut rig.h.ui, t, |v| v.select(&sel));
    rig.h.ui.raise(
        t,
        SelectionChanged {
            view: t,
            keys: sel.clone(),
        },
    );
    rig.turn();
    assert_eq!(
        rig.shell.session().selection,
        keys[1..4].to_vec(),
        "selection is session state"
    );
    let h = common::history_len(&rig);
    // Clicking one selected row's eye hides the whole selection.
    rig.h.ui.raise(
        t,
        RowBadgeClicked {
            view: t,
            key: keys[2].0,
            badge: 0,
        },
    );
    rig.turn();
    assert_eq!(common::history_len(&rig), h + 1, "one transaction");
    for (i, k) in keys.iter().enumerate() {
        let hidden = rig.shell.mirror().property(*k, HIDDEN) == Some(&Value::Bool(true));
        assert_eq!(hidden, (1..4).contains(&i), "E{i}");
    }
    assert!(common::last_entry(&rig).0.contains("3 entities"));
    // Delete all three at once: one undo entry brings all three back.
    rig.h
        .ui
        .raise(t, RowsDeleteRequested { view: t, keys: sel });
    rig.turn();
    assert_eq!(rig.shell.mirror().len(), 2);
    rig.shell.emitter().undo();
    rig.turn();
    assert_eq!(rig.shell.mirror().len(), 5);
    // A selection made elsewhere (the session) is shown in the tree.
    rig.shell.session_mut().set_selection(vec![keys[0]]);
    rig.turn();
    let shown = VirtualTree::edit(&mut rig.h.ui, t, |v| v.selected()).unwrap_or_default();
    assert_eq!(shown, vec![keys[0].0]);
}

#[test]
fn search_shows_matches_with_their_ancestors() {
    let mut rig = common::rig(&[P]);
    let world = common::spawn(&mut rig, "World", None);
    let ship = common::spawn(&mut rig, "Ship", Some(world));
    let _engine = common::spawn(&mut rig, "Engine", Some(ship));
    let _rock = common::spawn(&mut rig, "Rock", Some(world));
    let _sky = common::spawn(&mut rig, "Sky", None);
    let search = common::part(&rig, P, &["bar", "search"]);
    rig.h.ui.raise(
        search,
        SearchChanged {
            field: search,
            query: "engine".into(),
        },
    );
    rig.turn();
    let shown: Vec<String> = rows(&mut rig).into_iter().map(|(_, l)| l).collect();
    assert_eq!(
        shown,
        vec!["World", "Ship", "Engine"],
        "the match and its ancestors, opened"
    );
    let t = tree(&rig);
    let muted = VirtualTree::edit(&mut rig.h.ui, t, |v| {
        (
            v.item(world.0).map(|i| i.muted),
            v.item(ship.0).map(|i| i.muted),
        )
    });
    assert_eq!(
        muted,
        Some((Some(true), Some(true))),
        "context rows are dimmed"
    );
    // A change while filtering keeps the filter.
    let _e2 = common::spawn(&mut rig, "Engine 2", None);
    let shown: Vec<String> = rows(&mut rig).into_iter().map(|(_, l)| l).collect();
    assert_eq!(shown, vec!["World", "Ship", "Engine", "Engine 2"]);
    rig.h.ui.raise(
        search,
        SearchChanged {
            field: search,
            query: String::new(),
        },
    );
    rig.turn();
    let shown: Vec<String> = rows(&mut rig).into_iter().map(|(_, l)| l).collect();
    assert_eq!(
        shown,
        vec!["World", "Ship", "Engine", "Rock", "Sky", "Engine 2"],
        "every entity again; the rows the filter opened stay open"
    );
}

#[test]
fn another_issuers_changes_update_only_the_rows_they_touch() {
    let mut rig = common::rig(&[P]);
    let keys: Vec<EntityKey> = (0..50)
        .map(|i| common::spawn(&mut rig, &format!("N{i}"), None))
        .collect();
    // Rename, reparent and delete through a script: the tree follows.
    let mut script = rig.connect(forge_cmd::Issuer::Script { path: "s".into() });
    use forge_editor::client::BusClient;
    script.apply(
        forge_cmd::EditorCommand::Rename {
            entity: keys[10],
            name: "Renamed".into(),
        },
        None,
    );
    script.apply(
        forge_cmd::EditorCommand::Reparent {
            entity: keys[11],
            parent: Some(keys[0]),
        },
        None,
    );
    script.apply(forge_cmd::EditorCommand::Despawn { entity: keys[12] }, None);
    let _ = script.pump();
    rig.turn();
    let t = tree(&rig);
    let (label, parent, gone, n) = VirtualTree::edit(&mut rig.h.ui, t, |v| {
        (
            v.label_of(keys[10].0).map(str::to_string),
            v.parent_of(keys[11].0),
            v.contains(keys[12].0),
            v.row_count(),
        )
    })
    .unwrap_or_default();
    assert_eq!(label.as_deref(), Some("Renamed"));
    assert_eq!(parent, Some(keys[0].0));
    assert!(!gone);
    assert_eq!(n, 48, "49 roots left, one of them now a (collapsed) child");
}

/// Every visible row: (label, dimmed as context).
fn shown(rig: &mut forge_editor::testing::Rig) -> Vec<(String, bool)> {
    let t = tree(rig);
    VirtualTree::edit(&mut rig.h.ui, t, |v| {
        (0..v.row_count())
            .filter_map(|r| {
                let k = v.key_at(r)?;
                let i = v.item(k)?;
                Some((i.label.clone(), i.muted))
            })
            .collect()
    })
    .unwrap_or_default()
}

fn search(rig: &mut forge_editor::testing::Rig, q: &str) {
    let s = common::part(rig, P, &["bar", "search"]);
    rig.h.ui.raise(
        s,
        SearchChanged {
            field: s,
            query: q.into(),
        },
    );
    rig.turn();
    let spinner = common::part(rig, P, &["bar", "filtering"]);
    while !rig.h.ui.is_hidden(spinner) {
        rig.turn();
    }
}

#[test]
fn edits_under_a_filter_update_matches_and_their_context_rows_incrementally() {
    use forge_cmd::{EditorCommand, Issuer};
    use forge_editor::client::BusClient;
    let mut rig = common::rig(&[P]);
    let world = common::spawn(&mut rig, "World", None);
    let ship = common::spawn(&mut rig, "Ship", Some(world));
    let engine = common::spawn(&mut rig, "Engine", Some(ship));
    let rock = common::spawn(&mut rig, "Rock", Some(world));
    let sky = common::spawn(&mut rig, "Sky", None);
    search(&mut rig, "engine");
    let s = |l: &str, m: bool| (l.to_string(), m);
    assert_eq!(
        shown(&mut rig),
        vec![s("World", true), s("Ship", true), s("Engine", false)]
    );
    let mut script = rig.connect(Issuer::Script { path: "s".into() });
    let mut step = |rig: &mut forge_editor::testing::Rig, c: EditorCommand| {
        script.apply(c, None);
        let _ = script.pump();
        rig.turn();
    };
    // A non-match renamed into the filter joins it, under its (already shown) parent.
    step(
        &mut rig,
        EditorCommand::Rename {
            entity: rock,
            name: "Engine B".into(),
        },
    );
    assert_eq!(
        shown(&mut rig),
        vec![
            s("World", true),
            s("Ship", true),
            s("Engine", false),
            s("Engine B", false)
        ]
    );
    // The only match under Ship renamed away: it goes, and so does its context row.
    step(
        &mut rig,
        EditorCommand::Rename {
            entity: engine,
            name: "Motor".into(),
        },
    );
    assert_eq!(
        shown(&mut rig),
        vec![s("World", true), s("Engine B", false)]
    );
    // A match moved under a parent the filter hid: the new parent appears as context, the
    // old one (now without matches) goes.
    step(
        &mut rig,
        EditorCommand::Reparent {
            entity: rock,
            parent: Some(sky),
        },
    );
    assert_eq!(shown(&mut rig), vec![s("Sky", true), s("Engine B", false)]);
    // A context row renamed so it matches itself stops being dimmed.
    step(
        &mut rig,
        EditorCommand::Rename {
            entity: sky,
            name: "Sky engine".into(),
        },
    );
    assert_eq!(
        shown(&mut rig),
        vec![s("Sky engine", false), s("Engine B", false)]
    );
    // A new match under a hidden subtree brings its ancestors with it.
    step(
        &mut rig,
        EditorCommand::Spawn {
            name: "Spare engine".into(),
            parent: Some(ship),
        },
    );
    let incremental = shown(&mut rig);
    assert_eq!(
        incremental,
        vec![
            s("World", true),
            s("Ship", true),
            s("Spare engine", false),
            s("Sky engine", false),
            s("Engine B", false)
        ]
    );
    // Deleting a match prunes the context rows it alone kept.
    let spare = rows(&mut rig)
        .into_iter()
        .find(|(_, l)| l == "Spare engine")
        .map(|(k, _)| EntityKey(k))
        .unwrap_or_else(|| panic!("spare"));
    step(&mut rig, EditorCommand::Despawn { entity: spare });
    let incremental = shown(&mut rig);
    assert_eq!(
        incremental,
        vec![s("Sky engine", false), s("Engine B", false)]
    );
    // The same as filtering from scratch.
    search(&mut rig, "");
    search(&mut rig, "engine");
    assert_eq!(shown(&mut rig), incremental);
    let _ = world;
}
