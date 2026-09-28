//! `test_viewport` (Ch.21 §21.21, DoD M2-32): the viewport panel in the running shell.
//!
//! * **Gizmo edits are undoable commands** (`gizmo_drag_is_one_undoable_gesture`): a
//!   60-frame drag of the move gizmo's X handle is one transaction ("Move 1 entity"), moves
//!   the entity along X only, and Ctrl+Z puts it back bit for bit; Esc during a drag cancels
//!   it and leaves no history.
//! * **Picking** selects and clears (session state, never a command).
//! * **Redraws only on change**: 10 s idle with the viewport open is 0 frames and 0
//!   wakeups; orbiting draws; idle again afterwards.
//! * **Multiple viewports**: four cells, each its own camera.
//!
//! Positive control (W2): `positive_control_a_gizmo_without_a_gesture_fails` replaces the
//! move tool, through the ordinary `ViewportTool` point, with a gizmo whose drag frames are
//! separate transactions — the gesture check must fail.

mod common;

use std::time::Duration;

use common::{history_len, part, rig_with, set_prop, spawn};
use forge_cmd::{EntityKey, Value};
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;
use forge_editor::viewport::camera::EditorCamera;
use forge_editor::viewport::gizmo::{self, GizmoSpace};
use forge_editor::viewport::scene::{Drawable, P_LOCAL};
use forge_editor::viewport::tool::{ToolCategory, TransformTool, ViewportTool, ViewportToolItem};
use forge_frames::{DQuat, FrameId, FramePos, Tick};
use forge_panels_scene::viewport::ViewportCanvas;
use forge_plugin::{InstallCx, Manifest, PluginError, SourcePlugin};
use forge_ui::{InputEvent, KeyCode, KeyEvent, Modifiers, Point, PointerButton, Rect, WidgetId};

const VP: &str = "forge.viewport";

fn canvas(rig: &Rig, i: u32) -> WidgetId {
    let id = part(rig, VP, &["cells"]).child(&forge_ui::Key::Index(i));
    assert!(rig.h.ui.contains(id), "no cell {i}");
    id
}

fn rect(rig: &Rig, id: WidgetId) -> Rect {
    rig.h.ui.rect(id).unwrap_or_else(|| panic!("no rect"))
}

fn local_of(rig: &Rig, e: EntityKey) -> [f64; 3] {
    match rig.shell.mirror().property(e, P_LOCAL) {
        Some(Value::Vec3(v)) => *v,
        other => panic!("no position: {other:?}"),
    }
}

fn press(rig: &mut Rig, p: Point, button: PointerButton, down: bool) {
    rig.h.ui.handle(InputEvent::PointerButton {
        pos: p,
        button,
        pressed: down,
    });
    rig.turn();
}

fn move_to(rig: &mut Rig, p: Point) {
    rig.h.ui.handle(InputEvent::PointerMoved(p));
    rig.turn();
}

/// A rig with the viewport open and one selected entity at the frame origin.
fn setup(extra: &[&dyn SourcePlugin]) -> (Rig, EntityKey) {
    let mut rig = rig_with(&[VP], PanelFaults::default(), |_| {}, extra);
    let e = spawn(&mut rig, "Crate", None);
    set_prop(&mut rig, e, "transform.position.frame", Value::Int(0));
    set_prop(&mut rig, e, P_LOCAL, Value::Vec3([0.0; 3]));
    rig.shell.session_mut().set_selection(vec![e]);
    rig.settle();
    (rig, e)
}

/// Where the move gizmo's X handle is, `frac` of its length out, in window pixels.
fn x_handle(rig: &Rig, cell: Rect, frac: f64) -> Point {
    let cam = EditorCamera::new(FrameId(0));
    let tree = forge_editor::services::default_frames();
    let d = Drawable {
        key: EntityKey(0),
        name: String::new(),
        pos: FramePos::origin_of(FrameId(0)),
        rotation: DQuat::IDENTITY,
        scale: 1.0,
        seed_path: None,
        locked: false,
        layer: false,
    };
    let (w, h) = (f64::from(cell.w), f64::from(cell.h));
    let g = gizmo::place(&cam, tree.as_ref(), Tick(0), &d, GizmoSpace::World, h).expect("gizmo");
    let p = cam
        .project(g.rel + g.axes[0] * (g.len * frac), w, h)
        .expect("front");
    let _ = rig;
    Point::new(cell.x + p.x as f32, cell.y + p.y as f32)
}

/// Drag the X handle over 60 frames. Returns the history entries it added.
fn drag_x(rig: &mut Rig, cancel: bool) -> usize {
    let c = canvas(rig, 0);
    let r = rect(rig, c);
    let before = history_len(rig);
    let from = x_handle(rig, r, 0.6);
    let to = x_handle(rig, r, 2.4);
    move_to(rig, from);
    press(rig, from, PointerButton::Primary, true);
    for i in 1..=60 {
        let t = i as f32 / 60.0;
        move_to(
            rig,
            Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t),
        );
        if cancel && i == 30 {
            rig.h.ui.handle(InputEvent::Key(KeyEvent::press(
                KeyCode::Escape,
                Modifiers::NONE,
            )));
            rig.turn();
        }
    }
    press(rig, to, PointerButton::Primary, false);
    rig.settle();
    history_len(rig) - before
}

fn check_gizmo(extra: &[&dyn SourcePlugin]) -> Result<(), String> {
    let (mut rig, e) = setup(extra);
    let added = drag_x(&mut rig, false);
    if added != 1 {
        return Err(format!(
            "a 60-frame gizmo drag made {added} undo entries, not 1"
        ));
    }
    let (label, _) = common::last_entry(&rig);
    if label != "Move 1 entity" {
        return Err(format!("the drag's entry is labelled {label:?}"));
    }
    let p = local_of(&rig, e);
    if !(p[0] > 0.5 && p[1] == 0.0 && p[2] == 0.0) {
        return Err(format!("the X handle moved the entity to {p:?}"));
    }
    if !rig.mirror_matches() {
        return Err("the mirror diverged from the core".into());
    }
    // Undo puts it back exactly.
    rig.chord(KeyCode::Char('z'), Modifiers::CTRL);
    rig.settle();
    let back = local_of(&rig, e);
    if back != [0.0; 3] {
        return Err(format!("undo left the entity at {back:?}"));
    }
    // Esc during a drag cancels it: no entry, no movement.
    let added = drag_x(&mut rig, true);
    let p = local_of(&rig, e);
    if added != 0 || p != [0.0; 3] {
        return Err(format!("Esc left {added} entries and the entity at {p:?}"));
    }
    Ok(())
}

#[test]
fn gizmo_drag_is_one_undoable_gesture() {
    check_gizmo(&[]).unwrap_or_else(|e| panic!("{e}"));
}

/// A plugin replacing the move tool with a gizmo that sends each frame on its own.
struct NoGesture(Manifest);
impl SourcePlugin for NoGesture {
    fn manifest(&self) -> &Manifest {
        &self.0
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.replace::<ViewportTool>(
            "forge.tool.move",
            ViewportToolItem::new("Move", ToolCategory::Transform, "W", || {
                Box::new(TransformTool::without_gesture_for_control(
                    forge_editor::viewport::gizmo::GizmoMode::Translate,
                ))
            }),
        )
    }
}

#[test]
fn positive_control_a_gizmo_without_a_gesture_fails() {
    let p = NoGesture(
        Manifest::parse(
            r#"Plugin(id: "com.test.gizmo", version: "1.0.0", engine: "^0.1", kind: Source,
                replaces: [ ViewportTool("forge.tool.move") ])"#,
        )
        .expect("manifest"),
    );
    let e = check_gizmo(&[&p]).expect_err("a gizmo without a gesture must fail");
    assert!(e.contains("undo entries"), "{e}");
}

#[test]
fn picking_selects_and_clears_without_commands() {
    let (mut rig, e) = setup(&[]);
    rig.shell.session_mut().set_selection(vec![]);
    rig.settle();
    let c = canvas(&rig, 0);
    let r = rect(&rig, c);
    let before = history_len(&rig);
    let at = x_handle(&rig, r, 0.0);
    press(&mut rig, at, PointerButton::Primary, true);
    press(&mut rig, at, PointerButton::Primary, false);
    assert_eq!(rig.shell.session().selection, vec![e]);
    let empty = Point::new(r.x + 10.0, r.y + r.h - 10.0);
    press(&mut rig, empty, PointerButton::Primary, true);
    press(&mut rig, empty, PointerButton::Primary, false);
    assert!(rig.shell.session().selection.is_empty());
    assert_eq!(history_len(&rig), before, "selection is session state");
}

fn cell(rig: &Rig, i: u32) -> (EditorCamera, u64) {
    let v = rig
        .h
        .ui
        .widget::<ViewportCanvas>(canvas(rig, i))
        .unwrap_or_else(|| panic!("cell {i} is not a viewport canvas"));
    (v.camera(), v.paints())
}

/// 10 s of idle with the viewport open: frames and wakeups.
fn idle(faults: PanelFaults) -> (u64, u64) {
    let mut rig = rig_with(&[VP], faults, |_| {}, &[]);
    let e = spawn(&mut rig, "Crate", None);
    set_prop(&mut rig, e, P_LOCAL, Value::Vec3([0.0; 3]));
    // Let the layout's debounced autosave deadline pass first (it is the shell's, not ours).
    rig.advance(Duration::from_secs(5));
    rig.advance(Duration::from_secs(10))
}

#[test]
fn positive_control_a_viewport_that_redraws_every_frame_fails() {
    let (frames, _) = idle(PanelFaults {
        viewport_redraw_always: true,
        ..PanelFaults::default()
    });
    assert!(
        frames > 100,
        "a render-loop viewport must draw while idle: {frames} frames"
    );
}

#[test]
fn an_idle_viewport_draws_nothing_and_orbiting_draws() {
    assert_eq!(idle(PanelFaults::default()), (0, 0), "idle viewport");
    let (mut rig, _) = setup(&[]);
    rig.advance(Duration::from_secs(5));
    let (cam0, paints0) = cell(&rig, 0);
    let c = canvas(&rig, 0);
    let r = rect(&rig, c);
    let from = Point::new(r.x + 40.0, r.y + r.h - 40.0);
    press(&mut rig, from, PointerButton::Primary, true);
    let f0 = rig.h.frames;
    for i in 1..=10 {
        move_to(&mut rig, Point::new(from.x + 8.0 * i as f32, from.y));
    }
    press(
        &mut rig,
        Point::new(from.x + 80.0, from.y),
        PointerButton::Primary,
        false,
    );
    let (cam1, paints1) = cell(&rig, 0);
    assert!(
        rig.h.frames - f0 >= 10,
        "orbiting drew {} frames",
        rig.h.frames - f0
    );
    assert!(
        paints1 >= paints0 + 10,
        "the cell repainted {} times",
        paints1 - paints0
    );
    assert_ne!(cam0, cam1, "the camera orbited");
    rig.settle();
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    assert_eq!((frames, wakeups), (0, 0), "idle again after orbiting");
}

#[test]
fn four_viewports_have_their_own_cameras() {
    let (mut rig, _) = setup(&[]);
    let layout = part(&rig, VP, &["bar", "layout"]);
    // The layout control: pick "4" by keyboard (focus it, End selects the last option).
    rig.h.ui.set_focus(Some(layout), true);
    rig.h.ui.handle(InputEvent::Key(KeyEvent::press(
        KeyCode::End,
        Modifiers::NONE,
    )));
    rig.settle();
    for i in 0..4 {
        let r = rect(&rig, canvas(&rig, i));
        assert!(r.w > 50.0 && r.h > 50.0, "cell {i} is shown: {r:?}");
    }
    let a = rect(&rig, canvas(&rig, 0));
    let b = rect(&rig, canvas(&rig, 1));
    assert!(a.x < b.x, "cells side by side");
    let (a_cam, a_paints) = cell(&rig, 0);
    let (b_cam, _) = cell(&rig, 1);
    assert_ne!(a_cam, b_cam, "each cell has its own camera");
    // Orbit cell 1: its camera moves, cell 0's neither moves nor repaints.
    let from = Point::new(b.x + 30.0, b.y + b.h - 30.0);
    press(&mut rig, from, PointerButton::Primary, true);
    for i in 1..=5 {
        move_to(&mut rig, Point::new(from.x + 10.0 * i as f32, from.y));
    }
    press(
        &mut rig,
        Point::new(from.x + 50.0, from.y),
        PointerButton::Primary,
        false,
    );
    let (a2, a2_paints) = cell(&rig, 0);
    let (b2, _) = cell(&rig, 1);
    assert_ne!(b2, b_cam, "cell 1 orbited");
    assert_eq!(a2, a_cam, "cell 0's camera is its own");
    assert_eq!(a2_paints, a_paints, "cell 0 did not repaint");
}
