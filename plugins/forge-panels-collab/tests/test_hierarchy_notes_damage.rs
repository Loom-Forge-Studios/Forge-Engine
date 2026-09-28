//! `test_hierarchy_notes_damage` (D-5, owner rule 2; WP-U10): **teammates' notes cost the
//! hierarchy nothing while they do not change.**
//!
//! In a team project the hierarchy draws notes after rows (who else is looking, who claimed
//! it) and brings them up to date on every sync. Rewriting them through `VirtualTree::edit`
//! repaints the tree and rebuilds its accessibility node, so doing it when no note changed
//! would make every sync (a console line, a camera move, an edit the hierarchy does not
//! even show, a session change) repaint the hierarchy — damage the idle test cannot see, as it
//! only watches a fully idle editor.
//!
//! The check: the same changes (ten console lines, which run every panel's sync, and ten edits
//! of a property the hierarchy does not show) rebuild exactly as many accessibility nodes in a
//! team editor, with a teammate's note on a row, as in an editor with no team at all — while
//! the note stays on its row.
//!
//! Positive control (W2): `positive_control_notes_rewritten_every_sync_fail` makes the
//! hierarchy rewrite its notes on every sync (`PanelFaults::notes_always_edit`, the defect as
//! found); the team editor then rebuilds more, and the check fails.

mod common;

use common::*;
use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::collab::PanelFaults;
use forge_editor::console::{LogLevel, LogSource};
use forge_editor::core::EditorCore;
use forge_editor::testing::Rig;
use forge_project::collab::{CollabView, Presence, Role};
use forge_ui::widgets::VirtualTree;

const H: &str = "forge.hierarchy";
const EDITS: u32 = 10;

fn note_of(rig: &Rig, k: EntityKey) -> Option<String> {
    let tree = part(rig, H, &["tree"]);
    rig.h
        .ui
        .widget::<VirtualTree>(tree)
        .and_then(|t| t.item(k.0))
        .and_then(|i| i.note.clone())
}

/// Accessibility nodes rebuilt by `EDITS` console lines and edits the hierarchy does not show;
/// `team`: in a team project with a teammate's note on a row.
fn rebuilt(team: bool, faults: PanelFaults) -> Result<u64, String> {
    let s = server();
    let mut rig = if team {
        rig_with(&s, &[H], |cfg| cfg.services.collab.faults = faults)
    } else {
        let mut rig = Rig::with_core(config(), EditorCore::new()).map_err(|e| e.to_string())?;
        rig.show_panels(&[H]).map_err(|e| e.to_string())?;
        rig
    };
    let mut keys = Vec::new();
    for i in 0..5 {
        keys.push(EditorCore::read(&rig.core, |p| p.next_key()));
        rig.shell.emitter().emit(EditorCommand::Spawn {
            name: format!("E{i}"),
            parent: None,
        });
        rig.settle();
    }
    if team {
        rig.shell
            .emitter()
            .emit(forge_editor::collab::create_command("Studio"));
        rig.settle();
        let _bob = join(&s, "bob", Role::Developer);
        s.server.set_presence(Presence {
            user: "bob".into(),
            selection: vec![keys[0].0],
            panel: Some(H.into()),
            at_ms: NOW,
        });
        feed(&mut rig);
        if !note_of(&rig, keys[0]).is_some_and(|n| n.contains("bob")) {
            return Err(format!(
                "bob's note is not on the row: {:?}",
                note_of(&rig, keys[0])
            ));
        }
    }
    rig.h.ui.a11y_activate();
    rig.settle();
    let before = rig.h.ui.a11y_nodes_built();
    for i in 0..EDITS {
        // A console line: every panel's sync runs, the hierarchy has nothing to show.
        rig.shell.handles().services().log.borrow_mut().push(
            LogLevel::Info,
            "test",
            &format!("line {i}"),
            LogSource::None,
        );
        rig.settle();
        // An edit of a property the hierarchy does not show.
        rig.shell.emitter().emit(EditorCommand::SetProperty {
            entity: keys[1],
            path: "mass".into(),
            value: Value::Float(f64::from(i)),
        });
        rig.settle();
    }
    if team && !note_of(&rig, keys[0]).is_some_and(|n| n.contains("bob")) {
        return Err("the note went away".into());
    }
    Ok(rig.h.ui.a11y_nodes_built() - before)
}

fn check(faults: PanelFaults) -> Result<(), String> {
    let solo = rebuilt(false, PanelFaults::default())?;
    let team = rebuilt(true, faults)?;
    if team == solo {
        Ok(())
    } else {
        Err(format!(
            "{EDITS} console lines and edits the hierarchy does not show rebuilt {team} accessibility nodes in a team editor, {solo} alone"
        ))
    }
}

#[test]
fn unchanged_notes_cost_the_hierarchy_nothing() {
    check(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_notes_rewritten_every_sync_fail() {
    let e = check(PanelFaults {
        notes_always_edit: true,
        ..PanelFaults::default()
    })
    .expect_err("rewriting the notes on every sync must fail");
    assert!(e.contains("in a team editor"), "{e}");
}
