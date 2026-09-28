//! The node canvas (§21.16 "Graph", WP-U8) as a user drives it: wires checked while they are
//! dragged and refused **with the reason**, node drags as one move, reroutes, pan and zoom
//! (the minimap's every-node slice untouched by them), the node search, selection, copy /
//! delete requests, comments dragging their nodes, keyboard operation, accessibility, and
//! idling at zero frames once a pan has settled.

use std::rc::Rc;
use std::time::Duration;

use forge_ui::testing::Harness;
use forge_ui::widgets::{
    CanvasComment, CanvasNode, CanvasPin, CanvasView, CanvasWire, ClipOp, ClipboardRequested,
    CommentEdited, ConnectRefused, ConnectRequested, DeleteRequested, NodeCanvas, NodeChosen,
    NodesMoved, PickItem, PinRef, RerouteRequested,
};
use forge_ui::{
    ColorRole, InputEvent, KeyCode, Modifiers, NodeStyle, Point, PointerButton, Rect, Role,
    WidgetId,
};

const REASON: &str = "f64 · m/s cannot feed f64 · J: the units differ";

fn pin(l: &str, unit: &str) -> CanvasPin {
    CanvasPin::new(l, &format!("f64 · {unit}"), ColorRole::Accent)
}

/// Node 1 (output `speed` in m/s) at (100, 100); node 2 (inputs `v` m/s, `e` J) at
/// (420, 100); node 3 far away at (5000, 5000). The check refuses m/s into J.
fn canvas() -> (Harness, WidgetId) {
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let id = NodeCanvas::build(&mut h.ui, host, "g", NodeStyle::default().fill(), "Graph")
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    NodeCanvas::edit(&mut h.ui, id, |e| {
        e.set_node(
            1,
            CanvasNode::new("Speed", Point::new(100.0, 100.0))
                .with_outputs(vec![pin("speed", "m/s")]),
        );
        e.set_node(
            2,
            CanvasNode::new("Sink", Point::new(420.0, 100.0))
                .with_inputs(vec![pin("v", "m/s"), pin("e", "J")])
                .with_outputs(vec![pin("return", "J")]),
        );
        e.set_node(
            3,
            CanvasNode::new("Far away", Point::new(5000.0, 5000.0))
                .with_inputs(vec![pin("x", "m")]),
        );
        e.set_view(CanvasView {
            pan: Point::new(0.0, 0.0),
            zoom: 1.0,
        });
    });
    if let Some(c) = h.ui.widget_mut::<NodeCanvas>(id) {
        c.set_connect_check(Rc::new(|_from: PinRef, to: PinRef| {
            if to.index == 1 {
                Err(REASON.to_string())
            } else {
                Ok(())
            }
        }));
        c.set_search(Rc::new(|from| {
            let mut items = vec![
                PickItem::new("math.add", "Add"),
                PickItem::new("physics.kinetic_energy", "Kinetic energy"),
            ];
            if from.is_some() {
                items.retain(|i| i.id != "math.add");
            }
            Rc::new(items)
        }));
    }
    h.settle();
    h.ui.take_actions();
    (h, id)
}

/// Where a pin is on screen (zoom 1, pan 0 unless the test moved the view).
fn pin_at(h: &Harness, id: WidgetId, p: PinRef) -> Point {
    let r = h.ui.rect(id).unwrap_or_default();
    let c =
        h.ui.widget::<NodeCanvas>(id)
            .unwrap_or_else(|| panic!("canvas"));
    let m = c.model();
    let v = m.view();
    let w = m
        .node(p.node)
        .unwrap_or_else(|| panic!("node {}", p.node))
        .pin_pos(p.output, p.index);
    Point::new(
        r.x + (w.x - v.pan.x) * v.zoom,
        r.y + (w.y - v.pan.y) * v.zoom,
    )
}

fn drag(h: &mut Harness, a: Point, b: Point, button: PointerButton) {
    h.input(InputEvent::PointerMoved(a));
    h.input(InputEvent::PointerButton {
        pos: a,
        button,
        pressed: true,
    });
    for i in 1..=6 {
        let t = i as f32 / 6.0;
        h.input(InputEvent::PointerMoved(Point::new(
            a.x + (b.x - a.x) * t,
            a.y + (b.y - a.y) * t,
        )));
        h.step();
    }
    h.input(InputEvent::PointerButton {
        pos: b,
        button,
        pressed: false,
    });
    h.settle();
}

#[test]
fn a_compatible_wire_is_requested_output_to_input() {
    let (mut h, id) = canvas();
    let a = pin_at(&h, id, PinRef::output(1, 0));
    let b = pin_at(&h, id, PinRef::input(2, 0));
    drag(&mut h, a, b, PointerButton::Primary);
    let got = h.take::<ConnectRequested>();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].from, PinRef::output(1, 0));
    assert_eq!(got[0].to, PinRef::input(2, 0));
    // Dragged the other way (input to output), it is still output → input.
    drag(&mut h, b, a, PointerButton::Primary);
    let got = h.take::<ConnectRequested>();
    assert_eq!(
        got.first().map(|c| (c.from, c.to)),
        Some((PinRef::output(1, 0), PinRef::input(2, 0)))
    );
}

#[test]
fn an_incompatible_wire_is_refused_with_the_reason_shown() {
    let (mut h, id) = canvas();
    let a = pin_at(&h, id, PinRef::output(1, 0));
    let b = pin_at(&h, id, PinRef::input(2, 1));
    h.input(InputEvent::PointerMoved(a));
    h.input(InputEvent::PointerButton {
        pos: a,
        button: PointerButton::Primary,
        pressed: true,
    });
    h.input(InputEvent::PointerMoved(b));
    h.step();
    h.input(InputEvent::PointerButton {
        pos: b,
        button: PointerButton::Primary,
        pressed: false,
    });
    h.settle();
    let actions = h.ui.take_actions();
    assert!(
        !actions
            .iter()
            .any(|a| a.get::<ConnectRequested>().is_some()),
        "a refused wire was requested"
    );
    let refused: Vec<ConnectRefused> = actions
        .iter()
        .filter_map(|a| a.get::<ConnectRefused>().cloned())
        .collect();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].reason, REASON);
    assert_eq!(
        (refused[0].from, refused[0].to),
        (PinRef::output(1, 0), PinRef::input(2, 1))
    );
    let c =
        h.ui.widget::<NodeCanvas>(id)
            .unwrap_or_else(|| panic!("canvas"));
    assert_eq!(c.refusal(), Some(REASON), "the reason stays on screen");
    // And it is the pin's tooltip.
    assert_eq!(
        h.ui.widget::<NodeCanvas>(id)
            .and_then(|c| forge_ui::Widget::tooltip(c, h.ui.rt()))
            .as_deref(),
        Some(REASON)
    );
    // Output to output, and a node into itself, are refused by the canvas itself.
    let out2 = pin_at(&h, id, PinRef::output(2, 0));
    drag(&mut h, a, out2, PointerButton::Primary);
    let r = h.take::<ConnectRefused>();
    assert!(
        r.first().is_some_and(|r| r.reason.contains("outputs")),
        "{r:?}"
    );
    let in2 = pin_at(&h, id, PinRef::input(2, 0));
    drag(&mut h, out2, in2, PointerButton::Primary);
    let r = h.take::<ConnectRefused>();
    assert!(
        r.first().is_some_and(|r| r.reason.contains("itself")),
        "{r:?}"
    );
}

#[test]
fn dragging_nodes_raises_one_move_with_the_new_positions() {
    let (mut h, id) = canvas();
    let r = h.ui.rect(id).unwrap_or_default();
    let title = Point::new(r.x + 130.0, r.y + 110.0);
    drag(
        &mut h,
        title,
        Point::new(title.x + 60.0, title.y + 30.0),
        PointerButton::Primary,
    );
    let moves = h.take::<NodesMoved>();
    assert_eq!(moves.len(), 1, "one action per drag, not per frame");
    assert_eq!(moves[0].moves, vec![(1, Point::new(160.0, 130.0))]);
    // Ctrl+arrows nudge the selection.
    h.press(KeyCode::Right, Modifiers::CTRL);
    let moves = h.take::<NodesMoved>();
    assert_eq!(moves[0].moves, vec![(1, Point::new(176.0, 130.0))]);
}

#[test]
fn a_double_click_on_a_wire_asks_for_a_reroute() {
    let (mut h, id) = canvas();
    NodeCanvas::edit(&mut h.ui, id, |e| {
        e.set_wire(CanvasWire {
            from: PinRef::output(1, 0),
            to: PinRef::input(2, 0),
            role: ColorRole::Accent,
        })
    });
    h.settle();
    let a = pin_at(&h, id, PinRef::output(1, 0));
    let b = pin_at(&h, id, PinRef::input(2, 0));
    let mid = Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
    h.click_at(mid);
    h.click_at(mid);
    let rr = h.take::<RerouteRequested>();
    assert_eq!(rr.len(), 1, "{rr:?}");
    assert_eq!(rr[0].to, PinRef::input(2, 0));
    // A single click selects the wire; Delete asks to remove it.
    h.advance(Duration::from_secs(1));
    h.click_at(mid);
    h.press(KeyCode::Delete, Modifiers::NONE);
    let d = h.take::<DeleteRequested>();
    assert_eq!(
        d.first().map(|d| d.wires.clone()),
        Some(vec![PinRef::input(2, 0)])
    );
}

#[test]
fn pan_and_zoom_move_the_view_without_re_recording_the_minimap() {
    let (mut h, id) = canvas();
    let stats = |h: &Harness| {
        h.ui.widget::<NodeCanvas>(id)
            .map(|c| (c.stats(), c.model().view()))
            .unwrap_or_default()
    };
    let (s0, v0) = stats(&h);
    let r = h.ui.rect(id).unwrap_or_default();
    let c = r.center();
    drag(
        &mut h,
        c,
        Point::new(c.x - 100.0, c.y - 50.0),
        PointerButton::Middle,
    );
    let (s1, v1) = stats(&h);
    assert_eq!(v1.pan, Point::new(v0.pan.x + 100.0, v0.pan.y + 50.0));
    assert_eq!(
        s1.minimap_paints, s0.minimap_paints,
        "a pan re-recorded every node's dot"
    );
    assert!(
        s1.frame_paints > s0.frame_paints,
        "the minimap's view frame did not follow"
    );
    // The wheel zooms about the pointer: the world point under it stays put.
    let at = Point::new(r.x + 300.0, r.y + 200.0);
    let world = |v: CanvasView| {
        Point::new(
            v.pan.x + (at.x - r.x) / v.zoom,
            v.pan.y + (at.y - r.y) / v.zoom,
        )
    };
    h.input(InputEvent::PointerMoved(at));
    let before = world(stats(&h).1);
    h.input(InputEvent::Wheel {
        pos: at,
        dx: 0.0,
        dy: 2.0,
    });
    h.settle();
    let (s2, v2) = stats(&h);
    assert!(v2.zoom > 1.2, "{v2:?}");
    let after = world(v2);
    assert!((after.x - before.x).abs() < 1e-2 && (after.y - before.y).abs() < 1e-2);
    assert_eq!(s2.minimap_paints, s0.minimap_paints);
    // Far-away node 3 is not recorded; zooming out until it is in view records it.
    assert!(s2.nodes_recorded <= 2, "{s2:?}");
    // A graph change does re-record the minimap.
    NodeCanvas::edit(&mut h.ui, id, |e| e.set_pos(3, Point::new(700.0, 400.0)));
    h.settle();
    assert!(stats(&h).0.minimap_paints > s0.minimap_paints);
}

#[test]
fn a_panned_canvas_idles_after_one_accessibility_refresh() {
    let (mut h, id) = canvas();
    h.ui.a11y_activate();
    let r = h.ui.rect(id).unwrap_or_default();
    drag(
        &mut h,
        r.center(),
        Point::new(r.center().x + 40.0, r.center().y),
        PointerButton::Middle,
    );
    let f0 = h.frames;
    h.advance(Duration::from_secs(1));
    assert!(
        h.frames - f0 <= 1,
        "{} frames after a pan settled",
        h.frames - f0
    );
    let f1 = h.frames;
    let w1 = h.wakeups;
    h.advance(Duration::from_secs(10));
    assert_eq!(
        (h.frames - f1, h.wakeups - w1),
        (0, 0),
        "the canvas does not idle"
    );
}

#[test]
fn the_node_search_adds_the_chosen_node_where_it_was_opened() {
    let (mut h, id) = canvas();
    h.focus(id);
    h.press(KeyCode::Space, Modifiers::NONE);
    let pending =
        h.ui.widget::<NodeCanvas>(id)
            .and_then(|c| c.pending_search());
    let (at, from) = pending.unwrap_or_else(|| panic!("no search opened"));
    assert_eq!(from, None);
    h.type_text("kin");
    h.press(KeyCode::Enter, Modifiers::NONE);
    let chosen = h.take::<NodeChosen>();
    assert_eq!(chosen.len(), 1, "{chosen:?}");
    assert_eq!(chosen[0].op, "physics.kinetic_energy");
    assert_eq!(chosen[0].at, at);
    // A wire dropped on empty space opens the search for that pin (and the list is the
    // application's answer for it).
    let a = pin_at(&h, id, PinRef::output(1, 0));
    drag(
        &mut h,
        a,
        Point::new(a.x + 60.0, a.y + 200.0),
        PointerButton::Primary,
    );
    let pending =
        h.ui.widget::<NodeCanvas>(id)
            .and_then(|c| c.pending_search());
    assert_eq!(pending.map(|p| p.1), Some(Some(PinRef::output(1, 0))));
    h.press(KeyCode::Enter, Modifiers::NONE);
    let chosen = h.take::<NodeChosen>();
    assert_eq!(
        chosen.first().map(|c| (c.op.as_str(), c.from)),
        Some(("physics.kinetic_energy", Some(PinRef::output(1, 0))))
    );
}

#[test]
fn selection_copy_delete_and_marquee() {
    let (mut h, id) = canvas();
    let r = h.ui.rect(id).unwrap_or_default();
    // Marquee over nodes 1 and 2.
    drag(
        &mut h,
        Point::new(r.x + 60.0, r.y + 60.0),
        Point::new(r.x + 700.0, r.y + 260.0),
        PointerButton::Primary,
    );
    let sel = h.ui.widget::<NodeCanvas>(id).map(|c| c.model().selected());
    assert_eq!(sel, Some(vec![1, 2]));
    h.press(KeyCode::Char('c'), Modifiers::CTRL);
    let c = h.take::<ClipboardRequested>();
    assert_eq!(
        c.first().map(|c| (c.op, c.nodes.clone())),
        Some((ClipOp::Copy, vec![1, 2]))
    );
    h.press(KeyCode::Char('d'), Modifiers::CTRL);
    assert_eq!(
        h.take::<ClipboardRequested>().first().map(|c| c.op),
        Some(ClipOp::Duplicate)
    );
    h.press(KeyCode::Delete, Modifiers::NONE);
    let d = h.take::<DeleteRequested>();
    assert_eq!(d.first().map(|d| d.nodes.clone()), Some(vec![1, 2]));
    // Ctrl+Z is not the canvas's: it bubbles to the editor's global undo.
    h.key(KeyCode::Char('z'), Modifiers::CTRL);
    h.settle();
    assert!(h.take::<DeleteRequested>().is_empty());
    // Ctrl+A selects all; Escape clears.
    h.press(KeyCode::Char('a'), Modifiers::CTRL);
    assert_eq!(
        h.ui.widget::<NodeCanvas>(id)
            .map(|c| c.model().selected().len()),
        Some(3)
    );
    h.press(KeyCode::Escape, Modifiers::NONE);
    assert_eq!(
        h.ui.widget::<NodeCanvas>(id)
            .map(|c| c.model().selected().len()),
        Some(0)
    );
}

#[test]
fn dragging_a_comment_moves_the_nodes_inside_it() {
    let (mut h, id) = canvas();
    NodeCanvas::edit(&mut h.ui, id, |e| {
        e.set_comment(
            7,
            CanvasComment {
                rect: Rect::new(60.0, 50.0, 300.0, 150.0),
                text: "Inputs".into(),
            },
        )
    });
    h.settle();
    let r = h.ui.rect(id).unwrap_or_default();
    let header = Point::new(r.x + 200.0, r.y + 60.0);
    drag(
        &mut h,
        header,
        Point::new(header.x + 50.0, header.y + 20.0),
        PointerButton::Primary,
    );
    let e = h.take::<CommentEdited>();
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].comment, 7);
    assert_eq!(e[0].rect, Rect::new(110.0, 70.0, 300.0, 150.0));
    assert_eq!(
        e[0].moved,
        vec![(1, Point::new(150.0, 120.0))],
        "node 1 is inside, node 2 is not"
    );
}

#[test]
fn nodes_in_view_are_accessible_options_and_errors_are_announced() {
    let (mut h, id) = canvas();
    NodeCanvas::edit(&mut h.ui, id, |e| {
        e.set_error(2, Some("input `e` is not connected".into()))
    });
    let node = h.node(id).unwrap_or_else(|| panic!("a11y"));
    assert_eq!(node.role(), Role::ListBox);
    assert_eq!(node.label(), Some("Graph"));
    let n2 = h.child_node(id, 2).unwrap_or_else(|| panic!("node 2"));
    let label = n2.label().unwrap_or_default().to_string();
    assert!(
        label.contains("Sink") && label.contains("error: input `e` is not connected"),
        "{label}"
    );
    assert!(
        h.child_node(id, 3).is_none(),
        "an off-screen node is not in the tree"
    );
    // `]` steps the selection node by node and brings it into view.
    h.focus(id);
    h.press(KeyCode::Char(']'), Modifiers::NONE);
    h.press(KeyCode::Char(']'), Modifiers::NONE);
    h.press(KeyCode::Char(']'), Modifiers::NONE);
    let (sel, vis) =
        h.ui.widget::<NodeCanvas>(id)
            .map(|c| (c.model().selected(), c.model().visible_world()))
            .unwrap_or_default();
    assert_eq!(sel, vec![3]);
    assert!(vis.contains(Point::new(5050.0, 5020.0)), "{vis:?}");
    // The error shows as the node's tooltip too (hover it).
    let r = h.ui.rect(id).unwrap_or_default();
    h.press(KeyCode::Char('0'), Modifiers::NONE);
    NodeCanvas::edit(&mut h.ui, id, |e| {
        e.set_view(CanvasView {
            pan: Point::new(0.0, 0.0),
            zoom: 1.0,
        })
    });
    h.settle();
    h.input(InputEvent::PointerMoved(Point::new(
        r.x + 450.0,
        r.y + 110.0,
    )));
    let tip =
        h.ui.widget::<NodeCanvas>(id)
            .and_then(|c| forge_ui::Widget::tooltip(c, h.ui.rt()));
    assert_eq!(tip.as_deref(), Some("Sink: input `e` is not connected"));
}
