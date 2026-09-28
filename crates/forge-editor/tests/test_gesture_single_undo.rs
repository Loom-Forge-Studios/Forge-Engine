//! `test_gesture_single_undo` (Ch.21 §21.18, §21.23; DoD M2-28): a 60-frame slider drag
//! through the real shell loop is **one** transaction and **one** undo entry, sends at most
//! one command per frame, shows real state during the drag, and Esc cancels it (the value
//! goes back and nothing enters the history).
//!
//! Positive control (W2): the same drag through a "gesture" that sends each frame as its
//! own transaction (`gesture_without_txn_for_control` — what a panel without a txn does)
//! must fail the check.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use forge_cmd::{EditorCommand, Value};
use forge_editor::emitter::Gesture;
use forge_editor::panels::PanelCx;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use forge_ui::widgets::{Slider, SliderEdit, SliderPhase};
use forge_ui::{InputEvent, KeyCode, Modifiers, NodeStyle, Point, PointerButton, WidgetId};

const KEY: &str = "test.drag_value";

/// A panel with one slider driving the setting `KEY` as a gesture.
struct DragPanel {
    manifest: Manifest,
    fault_no_txn: bool,
}

impl DragPanel {
    fn new(fault_no_txn: bool) -> Self {
        let text = "Plugin(id: \"test.drag\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [EditorPanel(\"test.drag\")])";
        Self {
            manifest: Manifest::parse(text).unwrap_or_else(|e| panic!("{e}")),
            fault_no_txn,
        }
    }
}

fn build(cx: &mut PanelCx, fault: bool) {
    cx.add_live(move |pb| {
        let sig = pb.b.signal(0.0_f32);
        let s = pb.b.add(
            pb.parent,
            "slider",
            NodeStyle::leaf().width(600.0),
            Slider::new(sig, "Drag value", 0.0, 600.0, 1.0),
        )?;
        let g: Rc<RefCell<Option<Gesture>>> = Rc::new(RefCell::new(None));
        pb.on(s, move |act, e: &SliderEdit| {
            let cmd = EditorCommand::SetSetting {
                key: KEY.into(),
                value: Some(Value::Float(f64::from(e.value))),
            };
            match e.phase {
                SliderPhase::Begin => {
                    let mut gg = if fault {
                        act.cmd.gesture_without_txn_for_control("Drag value")
                    } else {
                        act.cmd.gesture("Drag value")
                    };
                    gg.update(cmd);
                    *g.borrow_mut() = Some(gg);
                }
                SliderPhase::Update => {
                    if let Some(gg) = g.borrow_mut().as_mut() {
                        gg.update(cmd);
                    }
                }
                SliderPhase::End => {
                    if let Some(gg) = g.borrow_mut().take() {
                        gg.commit();
                    }
                }
                SliderPhase::Cancel => {
                    if let Some(gg) = g.borrow_mut().take() {
                        gg.cancel();
                    }
                }
                SliderPhase::Step => {
                    act.cmd.emit(cmd);
                }
            }
        });
        Ok(())
    });
}

impl SourcePlugin for DragPanel {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        let fault = self.fault_no_txn;
        cx.add::<EditorPanel<PanelCx>>(
            "test.drag",
            PanelDescriptor {
                title: "Drag".into(),
                icon: None,
                default_dock: Dock::Center,
                build: Arc::new(move |cx: &mut PanelCx| build(cx, fault)),
            },
            Order::Last,
        )
    }
}

fn rig(fault: bool) -> (Rig, WidgetId) {
    let stand_in = StandInPanels::new().unwrap_or_else(|e| panic!("{e}"));
    let drag = DragPanel::new(fault);
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    let cfg = assemble(preset, &[&stand_in, &drag], &[], None).unwrap_or_else(|e| panic!("{e}"));
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&["test.drag"])
        .unwrap_or_else(|e| panic!("{e}"));
    let frame = rig
        .panel_frame("test.drag")
        .unwrap_or_else(|| panic!("panel open"));
    let slider = frame.child(&forge_ui::Key::Static("slider"));
    assert!(rig.h.ui.contains(slider), "the slider is built");
    (rig, slider)
}

fn value(rig: &Rig) -> Option<f64> {
    match rig.shell.mirror().setting(KEY) {
        Some(Value::Float(x)) => Some(*x),
        _ => None,
    }
}

/// Drag the slider over `frames` frames. `esc_at`: press Esc at that frame instead of
/// releasing. Returns the values the mirror showed during the drag.
fn drag(rig: &mut Rig, slider: WidgetId, frames: u32, esc_at: Option<u32>) -> Vec<Option<f64>> {
    let r = rig
        .h
        .ui
        .rect(slider)
        .unwrap_or_else(|| panic!("slider laid out"));
    let y = r.y + r.h * 0.5;
    let x0 = r.x + 10.0;
    rig.h.ui.handle(InputEvent::PointerMoved(Point::new(x0, y)));
    rig.h.ui.handle(InputEvent::PointerButton {
        pos: Point::new(x0, y),
        button: PointerButton::Primary,
        pressed: true,
    });
    rig.turn();
    let mut seen = Vec::new();
    for i in 1..=frames {
        if esc_at == Some(i) {
            rig.h.ui.handle(InputEvent::Key(forge_ui::KeyEvent::press(
                KeyCode::Escape,
                Modifiers::NONE,
            )));
            rig.turn();
            return seen;
        }
        let x = x0 + (r.w - 20.0) * (i as f32 / frames as f32);
        // Two pointer moves in one frame: the gesture must still send at most one command.
        rig.h
            .ui
            .handle(InputEvent::PointerMoved(Point::new(x - 1.0, y)));
        rig.h.ui.handle(InputEvent::PointerMoved(Point::new(x, y)));
        rig.turn();
        seen.push(value(rig));
    }
    let end = Point::new(x0 + r.w - 20.0, y);
    rig.h.ui.handle(InputEvent::PointerButton {
        pos: end,
        button: PointerButton::Primary,
        pressed: false,
    });
    rig.turn();
    seen
}

/// The property (see the module docs).
fn check(fault: bool) -> Result<(), String> {
    let (mut rig, slider) = rig(fault);
    let start = rig.state_hash();
    let history0 = rig.shell.mirror().history().len();
    let sent0 = rig.shell.emitter().emitted();
    let seen = drag(&mut rig, slider, 60, None);
    let added = rig.shell.mirror().history().len() - history0;
    let sent = rig.shell.emitter().emitted() - sent0;
    if added != 1 {
        return Err(format!("a 60-frame drag made {added} undo entries, not 1"));
    }
    // Loop turns: pointer down, 60 moves, release.
    if sent > 62 {
        return Err(format!(
            "{sent} commands in 62 frames: more than one per frame"
        ));
    }
    // The viewport (every panel) showed real state during the drag.
    let distinct: std::collections::BTreeSet<u64> =
        seen.iter().flatten().map(|x| x.to_bits()).collect();
    if distinct.len() < 30 {
        return Err(format!(
            "the mirror showed only {} values during the drag",
            distinct.len()
        ));
    }
    if value(&rig).is_none_or(|v| v < 500.0) {
        return Err(format!("the drag ended at {:?}", value(&rig)));
    }
    if !rig.mirror_matches() {
        return Err("the mirror disagrees with the core".into());
    }
    // One Ctrl+Z reverts the whole drag.
    rig.chord(KeyCode::Char('z'), Modifiers::CTRL);
    if rig.state_hash() != start || value(&rig).is_some() {
        return Err(format!("one undo left {:?}", value(&rig)));
    }
    rig.chord(KeyCode::Char('y'), Modifiers::CTRL);
    if value(&rig).is_none_or(|v| v < 500.0) {
        return Err("redo did not bring the drag back".into());
    }
    rig.chord(KeyCode::Char('z'), Modifiers::CTRL);

    // Esc during a drag cancels it: back to where it began, nothing in the history.
    let history1 = rig.shell.mirror().history().len();
    let mid = drag(&mut rig, slider, 60, Some(30));
    if mid.iter().flatten().count() < 20 {
        return Err("the cancelled drag never showed its values".into());
    }
    if value(&rig).is_some() || rig.state_hash() != start {
        return Err(format!("Esc left {:?}", value(&rig)));
    }
    let history2 = rig.shell.mirror().history().len();
    if history2 != history1 {
        return Err(format!(
            "a cancelled drag left {} entries",
            history2 - history1
        ));
    }
    Ok(())
}

#[test]
fn a_60_frame_drag_is_one_undo_entry_and_esc_cancels() {
    check(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_one_command_per_frame_without_a_txn_fails() {
    let r = check(true);
    assert!(
        r.as_ref().is_err_and(|e| e.contains("undo entries")),
        "the no-txn drag must fail the single-undo check: {r:?}"
    );
}
