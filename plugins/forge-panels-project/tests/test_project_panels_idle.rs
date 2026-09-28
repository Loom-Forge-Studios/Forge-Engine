//! D-5 for the lifecycle panels: with a project open and all four panels showing, an idle
//! editor draws nothing and never wakes (their sync steps compare revisions; nothing
//! polls). A lifecycle change elsewhere (an automation session saves) wakes it once, and it
//! settles.
//!
//! Rule 2 for the preset switcher: open on a big project (2,000 meshes, 1,024 tile-map
//! chunks), a target picked so the promotion plan shows, **an edit costs what it changed** —
//! a painted chunk (a setting the rules do not read) costs the panel nothing, and moving a
//! mesh costs one property read and no new plan. Positive control (W2):
//! `positive_control_rebuilding_the_view_per_change_fails` — the panel rebuilds its view of
//! the project on every change (`PanelFaults::project_view_full_rebuild`): both bounds
//! fail.
//!
//! Positive control (W2) for the idle bound itself:
//! `positive_control_a_panel_that_always_wants_a_turn_fails` — the revision history panel's
//! sync step asking for another turn every turn, changed or not
//! (`PanelFaults::history_always_want_turn`), keeps the editor from ever settling, and the
//! idle check above must catch it.

mod common;

use std::time::Duration;

use forge_editor::client::BusClient;
use forge_editor::project::{ProjectOp, Template, save_command};
use forge_editor::services::PanelFaults;
use forge_ui::widgets::VirtualTree;

#[test]
fn the_lifecycle_panels_are_idle_when_nothing_changes() {
    let dir = common::tmp("idle");
    let mut rig = common::rig(
        &[
            "forge.launcher",
            "forge.presets",
            "forge.history",
            "forge.export",
        ],
        &dir,
    );
    rig.shell.emitter().emit(
        ProjectOp::Create {
            location: format!("memory:idle-{}", std::process::id()),
            name: "Idle".into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
    );
    rig.settle();
    // Let caret blinks and first-frame work finish.
    rig.advance(Duration::from_secs(6));
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    println!("idle with the lifecycle panels: {frames} frames, {wakeups} wakeups over 10 s");
    assert_eq!((frames, wakeups), (0, 0));
    // An automation session saves: the panels follow, then the editor is idle again.
    let mut auto = rig.connect(forge_cmd::Issuer::Automation {
        session: "s".into(),
        tool: "apply".into(),
    });
    auto.apply(save_command("by the session"), None);
    let _ = auto.pump();
    let (frames, _) = rig.advance(Duration::from_secs(1));
    assert!(frames >= 1, "the save shows");
    let head = common::part(&rig, "forge.history", &["content", "head"]);
    let label = common::label(&rig, head);
    // A plain substring check on "1 revision" also matches the old, buggy "1 revision(s)"
    // wording, so pin the exact singular form instead.
    assert!(
        label.contains("1 revision \u{b7}") || label.ends_with("1 revision"),
        "wrong wording for a single revision: {label}"
    );
    rig.advance(Duration::from_secs(6));
    assert_eq!(rig.advance(Duration::from_secs(10)), (0, 0));
    // A real edit (a save with nothing dirty is a no-op), then a second save: the head
    // touches the new row alone, not the whole history.
    auto.apply(
        forge_cmd::EditorCommand::Spawn {
            name: "Marker".into(),
            parent: None,
        },
        None,
    );
    let _ = auto.pump();
    rig.settle();
    let list = common::part(&rig, "forge.history", &["content", "revisions"]);
    let rows_before = rows_written(&rig, list);
    auto.apply(save_command("a second change"), None);
    let _ = auto.pump();
    let (frames, _) = rig.advance(Duration::from_secs(1));
    assert!(frames >= 1, "the second save shows");
    let label = common::label(&rig, head);
    assert!(
        label.contains("2 revisions \u{b7}") || label.ends_with("2 revisions"),
        "wrong wording for several revisions: {label}"
    );
    let cost = rows_written(&rig, list) - rows_before;
    assert!(
        cost <= 2,
        "a new revision must cost the one row it added (plus its summary line), not a \
         rebuild of the whole history: {cost} rows written"
    );
    rig.advance(Duration::from_secs(6));
    assert_eq!(rig.advance(Duration::from_secs(10)), (0, 0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Rows the history list has written (inserted, replaced or removed) so far.
fn rows_written(rig: &forge_editor::testing::Rig, list: forge_ui::WidgetId) -> u64 {
    rig.h
        .ui
        .widget::<VirtualTree>(list)
        .map(VirtualTree::rows_written)
        .unwrap_or(0)
}

#[test]
fn positive_control_a_panel_that_always_wants_a_turn_fails() {
    let dir = common::tmp("idle-fault");
    let mut rig = common::rig_in(
        &["forge.history"],
        &dir,
        PanelFaults {
            history_always_want_turn: true,
            ..PanelFaults::default()
        },
        |_| {},
    );
    rig.shell.emitter().emit(
        ProjectOp::Create {
            location: format!("memory:idle-fault-{}", std::process::id()),
            name: "IdleFault".into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
    );
    rig.settle();
    rig.advance(Duration::from_secs(6));
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    println!("with the fault: {frames} frames, {wakeups} wakeups over 10 s");
    assert!(
        frames > 0 || wakeups > 0,
        "the fault must keep the editor from settling: {frames} frames, {wakeups} wakeups"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

const MESHES: usize = 2000;
const CHUNKS: usize = 1024;
const EDITS: usize = 32;

/// The work (settings and properties read, entities planned over) the preset switcher did
/// for `EDITS` painted chunks, then for `EDITS` sideways moves of one mesh.
fn edit_cost(fault: bool) -> (usize, usize) {
    use forge_cmd::{EditorCommand, Value};
    use forge_editor::services::PanelFaults;
    use std::cell::Cell;
    use std::rc::Rc;

    let dir = common::tmp(&format!("edit-cost-{fault}"));
    let probe = Rc::new(Cell::new(0));
    let mut rig = common::rig_in(
        &["forge.presets"],
        &dir,
        PanelFaults {
            project_view_full_rebuild: fault,
            project_view_probe: Some(probe.clone()),
            ..PanelFaults::default()
        },
        |_| {},
    );
    let e = rig.shell.emitter().clone();
    e.emit(
        ProjectOp::Create {
            location: format!("memory:edit-cost-{fault}-{}", std::process::id()),
            name: "Big".into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
    );
    rig.settle();
    for i in 0..MESHES {
        e.emit(EditorCommand::Spawn {
            name: format!("Mesh {i}"),
            parent: None,
        });
    }
    rig.settle();
    let meshes: Vec<_> = rig
        .shell
        .mirror()
        .entities()
        .filter(|(_, x)| x.name.starts_with("Mesh "))
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(meshes.len(), MESHES);
    let mut big = Vec::new();
    for k in &meshes {
        big.push(EditorCommand::SetProperty {
            entity: *k,
            path: "mesh.source".into(),
            value: Value::Text("crate.glb".into()),
        });
    }
    let chunk = |i: usize, v: &str| EditorCommand::SetSetting {
        key: format!("d2.tilemap.m.layer.0.chunk.c{}_{}", i % 32, i / 32),
        value: Some(Value::Text(v.repeat(256))),
    };
    for i in 0..CHUNKS {
        big.push(chunk(i, "a"));
    }
    e.emit_all("A big project", big);
    rig.settle();
    let list = common::part(&rig, "forge.presets", &["content", "presets"]);
    common::select_row(&mut rig, list, 0);
    let losses = common::part(&rig, "forge.presets", &["content", "detail", "losses"]);
    let text = common::label(&rig, losses);
    assert!(
        text.contains("entities lose their 3D-only data") && text.contains("more"),
        "the plan shows: {text}"
    );

    // Painting: EDITS chunk edits, one step each.
    probe.set(0);
    for i in 0..EDITS {
        e.emit(chunk(i, "b"));
        rig.settle();
    }
    let paint = probe.get();
    // Dragging one mesh sideways: EDITS moves.
    probe.set(0);
    for i in 0..EDITS {
        e.emit(EditorCommand::SetProperty {
            entity: meshes[0],
            path: "transform.position.local".into(),
            value: Value::Vec3([i as f64, 0.0, 0.0]),
        });
        rig.settle();
    }
    let drag = probe.get();
    // The panel still follows what it shows: depth on a mesh is a new loss. "Mesh 0" is
    // already named among the 3D-only losses, so look at the flattened loss alone: before,
    // it is the camera and the tilted sun; after, it names the mesh too.
    let flattened = |text: &str| -> String {
        text.split("; ")
            .find(|l| l.contains("flattened onto the 2D plane"))
            .unwrap_or_default()
            .to_string()
    };
    let before = flattened(&common::label(&rig, losses));
    assert!(
        before.contains("2 entities are flattened") && !before.contains("Mesh 0"),
        "before the depth edit: {before}"
    );
    e.emit(EditorCommand::SetProperty {
        entity: meshes[0],
        path: "transform.position.local".into(),
        value: Value::Vec3([0.0, 0.0, 5.0]),
    });
    rig.settle();
    let after = flattened(&common::label(&rig, losses));
    assert!(
        after.contains("3 entities are flattened") && after.contains("\u{201c}Mesh 0\u{201d}"),
        "after the depth edit the dialog must name the mesh: {after}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    (paint, drag)
}

#[test]
fn an_edit_costs_what_it_changed_with_the_panels_open() {
    let (paint, drag) = edit_cost(false);
    println!(
        "the preset switcher open, {MESHES} meshes, {CHUNKS} chunks: {EDITS} painted chunks cost {paint}, {EDITS} mesh moves cost {drag}"
    );
    assert_eq!(paint, 0, "a painted chunk costs the panel nothing");
    assert!(
        drag <= EDITS,
        "a moved mesh costs one property read: {drag} for {EDITS} moves"
    );
}

#[test]
fn positive_control_rebuilding_the_view_per_change_fails() {
    let (paint, drag) = edit_cost(true);
    println!("with the fault: paint {paint}, drag {drag}");
    assert!(
        paint > 0 && drag > EDITS,
        "the bounds must catch a rebuild per change: paint {paint}, drag {drag}"
    );
    assert!(
        paint >= EDITS * MESHES,
        "every step walked every mesh: {paint}"
    );
}
