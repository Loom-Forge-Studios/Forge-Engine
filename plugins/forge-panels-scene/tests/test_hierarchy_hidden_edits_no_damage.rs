//! `test_hierarchy_hidden_edits_no_damage` (D-5, owner rule 2; WP-U14): **an edit the
//! hierarchy does not show costs the hierarchy nothing.**
//!
//! The hierarchy follows the mirror's change log. A row shows an entity's name, its place in
//! the tree and the hidden / locked flags; a gizmo drag writes the dragged entity's transform
//! every frame, and a physics tweak writes a mass — none of it shown. Handing such a change
//! to `VirtualTree::edit` repaints the tree and rebuilds its accessibility node every frame of
//! the drag, even with the editor alone (no team, no notes).
//!
//! The check, alone in the editor: 30 frames of a drag writing a transform property and 10
//! edits of a mass rebuild **zero** accessibility nodes in the hierarchy — measured as the
//! editor's rebuilds with the hierarchy open minus the same edits' rebuilds with an inert
//! stand-in panel open instead (the status bar's undo label is the shell's own, and legit).
//! Its positive half: a rename and a hide (both shown) do rebuild nodes and change their
//! rows, so the measure sees the hierarchy when it should.
//!
//! Positive control (W2): `positive_control_editing_on_every_change_fails` — the hierarchy
//! editing its tree on every mirror change (`PanelFaults::hierarchy_edit_every_change`, the
//! code as found) rebuilds nodes for the hidden edits, and the check fails.

mod common;

use common::*;
use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;
use forge_ui::widgets::VirtualTree;

const H: &str = "forge.hierarchy";
/// A panel this configuration fills with a stand-in: it follows nothing.
const INERT: &str = "forge.team";

fn row(rig: &Rig, k: EntityKey) -> Option<(String, bool)> {
    let tree = part(rig, H, &["tree"]);
    rig.h
        .ui
        .widget::<VirtualTree>(tree)
        .and_then(|t| t.item(k.0))
        .map(|i| (i.label.clone(), i.muted))
}

/// A rig showing `panel` with five entities and assistive technology on.
fn setup(panel: &str, faults: PanelFaults) -> (Rig, Vec<EntityKey>) {
    let mut rig = rig_with(&[panel], faults, |_| {}, &[]);
    let keys = (0..5)
        .map(|i| spawn(&mut rig, &format!("E{i}"), None))
        .collect();
    rig.settle();
    rig.h.ui.a11y_activate();
    rig.settle();
    (rig, keys)
}

/// Accessibility nodes rebuilt by a 30-frame transform drag and 10 mass edits.
fn hidden_edits(rig: &mut Rig, keys: &[EntityKey]) -> u64 {
    let before = rig.h.ui.a11y_nodes_built();
    let mut g = rig.shell.emitter().gesture("Move E1");
    for f in 0..30u32 {
        g.update_all(vec![EditorCommand::SetProperty {
            entity: keys[1],
            path: "transform.position.local".into(),
            value: Value::Vec3([f64::from(f), 0.0, 0.0]),
        }]);
        rig.settle();
    }
    g.commit();
    rig.settle();
    for i in 0..10u32 {
        rig.shell.emitter().emit(EditorCommand::SetProperty {
            entity: keys[2],
            path: "mass".into(),
            value: Value::Float(f64::from(i)),
        });
        rig.settle();
    }
    rig.h.ui.a11y_nodes_built() - before
}

fn check(faults: PanelFaults) -> Result<(), String> {
    let (mut base, base_keys) = setup(INERT, PanelFaults::default());
    let shell = hidden_edits(&mut base, &base_keys);
    let (mut rig, keys) = setup(H, faults);
    let with_hierarchy = hidden_edits(&mut rig, &keys);
    let hierarchy = with_hierarchy.saturating_sub(shell);
    if with_hierarchy != shell {
        return Err(format!(
            "40 edits the hierarchy does not show rebuilt {hierarchy} accessibility node(s) in it with the editor alone ({with_hierarchy} with it open, {shell} without)"
        ));
    }
    // Shown changes still reach the rows (the measure is not blind to the hierarchy).
    let before = rig.h.ui.a11y_nodes_built();
    rig.shell.emitter().emit(EditorCommand::Rename {
        entity: keys[3],
        name: "Renamed".into(),
    });
    rig.settle();
    rig.shell.emitter().emit(EditorCommand::SetProperty {
        entity: keys[4],
        path: forge_panels_scene::hierarchy::HIDDEN.into(),
        value: Value::Bool(true),
    });
    rig.settle();
    if rig.h.ui.a11y_nodes_built() == before {
        return Err(
            "a rename and a hide rebuilt no accessibility node: the measure is blind".into(),
        );
    }
    if row(&rig, keys[3]).map(|r| r.0) != Some("Renamed".into()) {
        return Err(format!(
            "the rename did not reach its row: {:?}",
            row(&rig, keys[3])
        ));
    }
    if row(&rig, keys[4]).map(|r| r.1) != Some(true) {
        return Err("the hide did not dim its row".into());
    }
    Ok(())
}

#[test]
fn hidden_edits_cost_the_hierarchy_nothing() {
    check(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_editing_on_every_change_fails() {
    let e = check(PanelFaults {
        hierarchy_edit_every_change: true,
        ..PanelFaults::default()
    })
    .expect_err("editing the tree on every change must fail");
    assert!(e.contains("does not show rebuilt"), "{e}");
    eprintln!("control (the code as found): {e}");
}
