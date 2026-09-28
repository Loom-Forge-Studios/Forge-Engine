// timed-gates: exempt(open and edit times are printed; the asserts are on counts and recorded
// nodes)
//! The graph editor (`forge.graph`, DoD M2-46) through the running shell: unit-typed pins
//! refuse an incompatible wire with the reason and write nothing (gate row
//! `C-graph-unit-check`), every edit is a command and a multi-setting edit is one undo entry
//! (`C-graph-edits-undoable`), node search from the `#[forge_api]` registry, reroutes,
//! comments dragging their nodes, copy / paste, compile errors on their nodes and in the
//! console (with the console's click-through back to the node), the graph ↔ text view, an
//! automation session's edits reaching the canvas, and zero idle.
//!
//! Positive controls (W2): `positive_control_a_graph_editor_without_the_unit_check_fails`
//! (the check skipped: the N·s → m/s wire is written) and
//! `positive_control_ungrouped_graph_edits_fail_single_undo` (a paste sent as separate
//! commands: one undo leaves half of it).

use std::time::Duration;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::console::LogSource;
use forge_editor::graph::library::REROUTE;
use forge_editor::graph::{self as gm, GraphKind, Graphs};
use forge_editor::presets::builtin_preset;
use forge_editor::services::PanelFaults;
use forge_editor::session::Reveal;
use forge_editor::shell::assemble;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_assets::PanelsAssets;
use forge_panels_core::PanelsCore;
use forge_plugin::SourcePlugin;
use forge_ui::widgets::{CanvasView, MultilineEditor, NodeCanvas, PinRef, RowActivated};
use forge_ui::{InputEvent, Key, KeyCode, Modifiers, Point, PointerButton, WidgetId};

const P: &str = "forge.graph";

fn rig(faults: PanelFaults) -> Rig {
    rig_showing(faults, &[P])
}

fn rig_showing(faults: PanelFaults, panels: &[&str]) -> Rig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let assets = PanelsAssets::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_assets::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&core, &assets, &stand_in];
    let mut cfg = assemble(
        builtin_preset("3d").unwrap_or_else(|e| panic!("{e}")),
        &all,
        &[],
        None,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    cfg.services.faults = faults;
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(panels).unwrap_or_else(|e| panic!("{e}"));
    rig.settle();
    rig
}

fn part(rig: &Rig, path: &[&str]) -> WidgetId {
    let mut id = rig.panel_frame(P).unwrap_or_else(|| panic!("open"));
    for k in path {
        id = id.child(&Key::Str((*k).into()));
    }
    assert!(rig.h.ui.contains(id), "no widget at {path:?}");
    id
}

fn canvas(rig: &Rig) -> WidgetId {
    part(rig, &["body", "canvas"]).child(&Key::Static("canvas"))
}

/// The op path of a built-in node.
fn op(rig: &Rig, ident: &str) -> String {
    rig.shell
        .handles()
        .services()
        .graph_library
        .op_of_ident(ident)
        .unwrap_or_else(|| panic!("{ident}"))
        .to_string()
}

/// Apply commands as another issuer (an automation session or a script), one transaction.
fn auto(rig: &mut Rig, cmds: Vec<EditorCommand>) {
    let mut s = rig.connect(Issuer::Script { path: "t".into() });
    let t = s.begin("automation edit");
    for c in cmds {
        s.apply(c, Some(t));
    }
    s.commit(t);
    let _ = s.pump();
    rig.settle();
}

fn setting(rig: &Rig, key: &str) -> Option<Value> {
    rig.shell.mirror().setting(key).cloned()
}

/// Graph `g1` (`Motion`, a blueprint) with: 1 momentum (→ N·s), 2 kinetic energy (mass kg,
/// speed m/s), 3 speed from impulse (impulse N·s, mass kg). The view at zoom 1, pan 0.
fn motion(rig: &mut Rig) {
    let (m, k, s) = (
        op(rig, "momentum"),
        op(rig, "kinetic_energy"),
        op(rig, "speed_from_impulse"),
    );
    let mut cmds = gm::new_graph(&Graphs::default(), "Motion", GraphKind::Blueprint).1;
    cmds.extend(gm::add_node("g1", 1, &m, 40.0, 40.0));
    cmds.extend(gm::add_node("g1", 2, &k, 360.0, 40.0));
    cmds.extend(gm::add_node("g1", 3, &s, 360.0, 240.0));
    auto(rig, cmds);
    let c = canvas(rig);
    NodeCanvas::edit(&mut rig.h.ui, c, |e| {
        e.set_view(CanvasView {
            pan: Point::new(0.0, 0.0),
            zoom: 1.0,
        })
    });
    rig.settle();
}

fn pin_at(rig: &Rig, p: PinRef) -> Point {
    let c = canvas(rig);
    let r = rig.h.ui.rect(c).unwrap_or_default();
    let w = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .unwrap_or_else(|| panic!("canvas"));
    let m = w.model();
    let v = m.view();
    let q = m
        .node(p.node)
        .unwrap_or_else(|| panic!("node {} not on the canvas", p.node))
        .pin_pos(p.output, p.index);
    Point::new(
        r.x + (q.x - v.pan.x) * v.zoom,
        r.y + (q.y - v.pan.y) * v.zoom,
    )
}

fn drag(rig: &mut Rig, a: Point, b: Point, button: PointerButton) {
    rig.h.input(InputEvent::PointerMoved(a));
    rig.h.input(InputEvent::PointerButton {
        pos: a,
        button,
        pressed: true,
    });
    for i in 1..=5 {
        let t = i as f32 / 5.0;
        rig.h.input(InputEvent::PointerMoved(Point::new(
            a.x + (b.x - a.x) * t,
            a.y + (b.y - a.y) * t,
        )));
        rig.turn();
    }
    rig.h.input(InputEvent::PointerButton {
        pos: b,
        button,
        pressed: false,
    });
    rig.settle();
}

fn history_len(rig: &Rig) -> usize {
    rig.shell.mirror().history().len()
}

/// The acceptance's "connection unit-check test": N·s into m/s is refused with the reason
/// and writes nothing; N·s into N·s is written.
fn check_units(faults: PanelFaults) -> Result<(), String> {
    let mut rig = rig(faults);
    motion(&mut rig);
    let h0 = history_len(&rig);
    // momentum.return (N·s) → kinetic_energy.speed (m/s).
    let a = pin_at(&rig, PinRef::output(1, 0));
    let b = pin_at(&rig, PinRef::input(2, 1));
    drag(&mut rig, a, b, PointerButton::Primary);
    if let Some(v) = setting(&rig, "graph.g1.node.2.in.speed") {
        return Err(format!("an N·s output was wired into an m/s input ({v:?})"));
    }
    if history_len(&rig) != h0 {
        return Err("a refused wire entered the undo history".into());
    }
    let c = canvas(&rig);
    let reason = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .and_then(|w| w.refusal().map(str::to_string))
        .unwrap_or_default();
    if !(reason.contains("N·s") && reason.contains("m/s") && reason.contains("units differ")) {
        return Err(format!("the refusal's reason does not say why: {reason:?}"));
    }
    let noticed = rig
        .shell
        .session()
        .notifications
        .history()
        .any(|n| n.title == "Connection refused" && n.detail == reason);
    if !noticed {
        return Err("no notification names the refused connection".into());
    }
    // momentum.return (N·s) → speed_from_impulse.impulse (N·s): written, one undo entry.
    let a = pin_at(&rig, PinRef::output(1, 0));
    let b = pin_at(&rig, PinRef::input(3, 0));
    drag(&mut rig, a, b, PointerButton::Primary);
    if setting(&rig, "graph.g1.node.3.in.impulse") != Some(Value::Text("1.return".into())) {
        return Err("a compatible wire was not written".into());
    }
    if history_len(&rig) != h0 + 1 {
        return Err("a wire is not one undo entry".into());
    }
    let wired = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .is_some_and(|w| w.model().wire_into(PinRef::input(3, 0)).is_some());
    if !wired {
        return Err("the wire did not reach the canvas".into());
    }
    Ok(())
}

#[test]
fn an_incompatible_connection_is_refused_with_the_reason_and_writes_nothing() {
    check_units(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_graph_editor_without_the_unit_check_fails() {
    let e = check_units(PanelFaults {
        graph_skips_unit_check: true,
        ..PanelFaults::default()
    })
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("was wired"),
        "the unchecked editor passed: {e:?}"
    );
}

/// Duplicate two wired nodes (one command set), then undo once: both are gone again.
fn check_single_undo(faults: PanelFaults) -> Result<(), String> {
    let mut rig = rig(faults);
    motion(&mut rig);
    let wire = gm::connect(
        &Graphs::read(&rig.shell.mirror())
            .get("g1")
            .cloned()
            .unwrap_or_default(),
        &rig.shell.handles().services().graph_library,
        &gm::Source::new(1, "return"),
        3,
        "impulse",
    )?;
    auto(&mut rig, wire);
    let c = canvas(&rig);
    NodeCanvas::edit(&mut rig.h.ui, c, |e| e.select(&[1, 3]));
    rig.h.focus(c);
    let h0 = history_len(&rig);
    rig.h.key(KeyCode::Char('d'), Modifiers::CTRL);
    rig.settle();
    let g = Graphs::read(&rig.shell.mirror());
    let doc = g.get("g1").cloned().unwrap_or_default();
    if doc.nodes.len() != 5 {
        return Err(format!(
            "duplicate made {} nodes, expected 5",
            doc.nodes.len()
        ));
    }
    if doc.nodes.get(&5).and_then(|n| n.inputs.get("impulse"))
        != Some(&gm::Source::new(4, "return"))
    {
        return Err("the duplicate's inner wire was not remapped".into());
    }
    let selected = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .map(|w| w.model().selected());
    if selected != Some(vec![4, 5]) {
        return Err(format!("the duplicate is not selected: {selected:?}"));
    }
    let _ = rig.shell.emitter().undo();
    rig.settle();
    let n = Graphs::read(&rig.shell.mirror())
        .get("g1")
        .map_or(0, |d| d.nodes.len());
    if n != 3 {
        return Err(format!(
            "one undo left {n} nodes (history grew by {})",
            history_len(&rig) - h0
        ));
    }
    Ok(())
}

#[test]
fn a_multi_setting_edit_is_one_undo_entry() {
    check_single_undo(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_ungrouped_graph_edits_fail_single_undo() {
    let e = check_single_undo(PanelFaults {
        graph_edits_not_grouped: true,
        ..PanelFaults::default()
    })
    .err()
    .unwrap_or_default();
    assert!(e.contains("one undo left"), "ungrouped edits passed: {e:?}");
}

#[test]
fn a_new_graph_is_a_command_and_opens() {
    let mut rig = rig(PanelFaults::default());
    let name = part(&rig, &["bar", "name"]);
    rig.h.focus(name);
    rig.h.type_text("Shading");
    // Kind: the second choice (Material graph).
    let kind = part(&rig, &["bar", "kind"]);
    rig.h.focus(kind);
    rig.h.press(KeyCode::Right, Modifiers::NONE);
    let new = part(&rig, &["bar", "new"]);
    rig.h.click(new);
    rig.settle();
    assert_eq!(
        setting(&rig, "graph.g1.name"),
        Some(Value::Text("Shading".into()))
    );
    assert_eq!(
        setting(&rig, "graph.g1.kind"),
        Some(Value::Text("Material".into()))
    );
    // A material graph's search offers no physics node: Space, "kinetic", Enter adds nothing.
    let c = canvas(&rig);
    rig.h.focus(c);
    rig.h.press(KeyCode::Space, Modifiers::NONE);
    rig.h.type_text("kinetic");
    rig.h.press(KeyCode::Enter, Modifiers::NONE);
    rig.settle();
    assert!(
        Graphs::read(&rig.shell.mirror())
            .get("g1")
            .is_some_and(|d| d.nodes.is_empty())
    );
    // "fresnel" finds the material node; it is added where the search opened.
    rig.h.press(KeyCode::Escape, Modifiers::NONE);
    rig.h.focus(c);
    rig.h.press(KeyCode::Space, Modifiers::NONE);
    rig.h.type_text("fresnel");
    rig.h.press(KeyCode::Enter, Modifiers::NONE);
    rig.settle();
    let doc = Graphs::read(&rig.shell.mirror())
        .get("g1")
        .cloned()
        .unwrap_or_default();
    assert_eq!(doc.nodes.len(), 1);
    assert_eq!(
        doc.nodes.get(&1).map(|n| n.op.clone()),
        Some(op(&rig, "schlick_fresnel"))
    );
    let on_canvas = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .map(|w| w.model().nodes().len());
    assert_eq!(on_canvas, Some(1));
}

#[test]
fn a_wire_dropped_on_empty_space_adds_a_compatible_node_wired_to_it() {
    let mut rig = rig(PanelFaults::default());
    motion(&mut rig);
    // Drop momentum's N·s output far from any node; search "impulse"; Enter.
    let a = pin_at(&rig, PinRef::output(1, 0));
    drag(
        &mut rig,
        a,
        Point::new(a.x + 80.0, a.y + 330.0),
        PointerButton::Primary,
    );
    rig.h.type_text("impulse");
    rig.h.press(KeyCode::Enter, Modifiers::NONE);
    rig.settle();
    assert_eq!(
        setting(&rig, "graph.g1.node.4.op"),
        Some(Value::Text(op(&rig, "speed_from_impulse")))
    );
    assert_eq!(
        setting(&rig, "graph.g1.node.4.in.impulse"),
        Some(Value::Text("1.return".into()))
    );
}

#[test]
fn reroutes_and_comments_are_commands() {
    let mut rig = rig(PanelFaults::default());
    motion(&mut rig);
    auto(
        &mut rig,
        vec![forge_editor::domain::set(
            "graph.g1.node.3.in.impulse",
            Value::Text("1.return".into()),
        )],
    );
    // Double-click the wire: a reroute on it, the wire through it, one undo entry.
    let a = pin_at(&rig, PinRef::output(1, 0));
    let b = pin_at(&rig, PinRef::input(3, 0));
    let c = canvas(&rig);
    // The midpoint of the wire's curve (a symmetric cubic: halfway between its ends).
    let mid = Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
    let h0 = history_len(&rig);
    rig.h.click_at(mid);
    rig.h.click_at(mid);
    rig.settle();
    assert_eq!(
        setting(&rig, "graph.g1.node.4.op"),
        Some(Value::Text(REROUTE.into()))
    );
    assert_eq!(
        setting(&rig, "graph.g1.node.4.in.in"),
        Some(Value::Text("1.return".into()))
    );
    assert_eq!(
        setting(&rig, "graph.g1.node.3.in.impulse"),
        Some(Value::Text("4.out".into()))
    );
    assert_eq!(history_len(&rig), h0 + 1);
    // C around the selection: a comment; dragging its header moves it and node 1.
    NodeCanvas::edit(&mut rig.h.ui, c, |e| e.select(&[1]));
    rig.h.focus(c);
    rig.h.press(KeyCode::Char('c'), Modifiers::NONE);
    rig.settle();
    assert_eq!(
        setting(&rig, "graph.g1.comment.1.text"),
        Some(Value::Text("Comment".into()))
    );
    let r = rig.h.ui.rect(c).unwrap_or_default();
    // The comment frames node 1 (at 40, 40) with a 44 px header above it.
    let header = Point::new(r.x + 60.0, r.y + 8.0);
    let h1 = history_len(&rig);
    drag(
        &mut rig,
        header,
        Point::new(header.x + 100.0, header.y + 50.0),
        PointerButton::Primary,
    );
    assert_eq!(
        setting(&rig, "graph.g1.node.1.x"),
        Some(Value::Float(140.0))
    );
    assert_eq!(setting(&rig, "graph.g1.node.1.y"), Some(Value::Float(90.0)));
    assert_eq!(
        history_len(&rig),
        h1 + 1,
        "the comment and its node move as one edit"
    );
}

#[test]
fn compile_errors_land_on_their_nodes_and_the_console_links_back() {
    let mut rig = rig_showing(PanelFaults::default(), &[P, "forge.console"]);
    motion(&mut rig);
    // An automation session writes the N·s → m/s wire past the editor's check, and an unknown
    // node.
    auto(
        &mut rig,
        vec![
            forge_editor::domain::set("graph.g1.node.2.in.speed", Value::Text("1.return".into())),
            forge_editor::domain::set("graph.g1.node.7.op", Value::Text("no::such::node".into())),
        ],
    );
    let compile = part(&rig, &["bar", "compile"]);
    rig.h.click(compile);
    rig.settle();
    let c = canvas(&rig);
    let err = |rig: &Rig, n: u64| {
        rig.h
            .ui
            .widget::<NodeCanvas>(c)
            .and_then(|w| w.model().node(n).and_then(|x| x.error.clone()))
    };
    assert!(
        err(&rig, 2).is_some_and(|e| e.contains("GRAPH-0004") && e.contains("N·s")),
        "{:?}",
        err(&rig, 2)
    );
    assert!(
        err(&rig, 7).is_some_and(|e| e.contains("GRAPH-0001")),
        "{:?}",
        err(&rig, 7)
    );
    assert!(err(&rig, 1).is_none() && err(&rig, 3).is_none());
    // The console carries each, with a click-through to the node.
    let entry = rig
        .shell
        .handles()
        .services()
        .log
        .borrow()
        .entries()
        .find(|e| {
            e.target == "forge.graph"
                && e.source
                    == LogSource::GraphNode {
                        graph: "g1".into(),
                        node: 2,
                    }
        })
        .map(|e| e.id)
        .unwrap_or_else(|| panic!("no console entry for node 2"));
    // The console's click-through selects the node and frames it (M2-39).
    NodeCanvas::edit(&mut rig.h.ui, c, |e| {
        e.set_view(CanvasView {
            pan: Point::new(5000.0, 5000.0),
            zoom: 1.0,
        })
    });
    let console = rig
        .panel_frame("forge.console")
        .unwrap_or_else(|| panic!("console"))
        .child(&Key::Str("list".into()));
    rig.h.ui.raise(
        console,
        RowActivated {
            view: console,
            key: entry,
        },
    );
    rig.settle();
    assert_eq!(
        rig.shell.session().pending_reveal().map(|(_, r)| r.clone()),
        Some(Reveal::GraphNode {
            graph: "g1".into(),
            node: 2
        })
    );
    let (sel, vis) = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .map(|w| (w.model().selected(), w.model().visible_world()))
        .unwrap_or_default();
    assert_eq!(sel, vec![2]);
    assert!(vis.contains(Point::new(400.0, 60.0)), "{vis:?}");
    // Fixing the node clears its error at once (a stale error is not left on it).
    auto(
        &mut rig,
        vec![forge_editor::domain::clear("graph.g1.node.2.in.speed")],
    );
    assert!(err(&rig, 2).is_none());
}

#[test]
fn graph_and_text_side_by_side_round_trip_through_commands() {
    let mut rig = rig(PanelFaults::default());
    motion(&mut rig);
    auto(
        &mut rig,
        vec![
            forge_editor::domain::set("graph.g1.node.3.in.impulse", Value::Text("1.return".into())),
            forge_editor::domain::set("graph.g1.node.1.lit.mass", Value::Float(2.0)),
        ],
    );
    let toggle = part(&rig, &["bar", "text"]);
    rig.h.click(toggle);
    rig.settle();
    let editor = part(&rig, &["body", "text_col", "text"]);
    let sig = rig
        .h
        .ui
        .widget::<MultilineEditor>(editor)
        .map(MultilineEditor::model)
        .unwrap_or_else(|| panic!("editor"));
    let text = |rig: &Rig| sig.get(rig.h.ui.rt());
    let t = text(&rig);
    assert!(t.contains("let n1 = momentum(2.0, 0.0);"), "{t}");
    assert!(t.contains("let n3 = speed_from_impulse(n1, 0.0);"), "{t}");
    // The text follows an edit of the graph.
    auto(
        &mut rig,
        vec![forge_editor::domain::set(
            "graph.g1.node.1.lit.velocity",
            Value::Float(3.5),
        )],
    );
    assert!(
        text(&rig).contains("let n1 = momentum(2.0, 3.5);"),
        "{}",
        text(&rig)
    );
    // Edit the text; Apply: the difference becomes one transaction.
    let edited = text(&rig).replace("momentum(2.0, 3.5)", "momentum(4.0, 3.5)");
    sig.set(rig.h.ui.rt_mut(), edited);
    rig.settle();
    let h0 = history_len(&rig);
    let apply = part(&rig, &["bar", "apply"]);
    rig.h.click(apply);
    rig.settle();
    assert_eq!(
        setting(&rig, "graph.g1.node.1.lit.mass"),
        Some(Value::Float(4.0))
    );
    assert_eq!(history_len(&rig), h0 + 1);
    let _ = rig.shell.emitter().undo();
    rig.settle();
    assert_eq!(
        setting(&rig, "graph.g1.node.1.lit.mass"),
        Some(Value::Float(2.0))
    );
    // Text that does not parse changes nothing.
    sig.set(
        rig.h.ui.rt_mut(),
        "pub fn motion() {\n    let n1 = nope();\n}\n".into(),
    );
    rig.settle();
    let h1 = history_len(&rig);
    rig.h.click(apply);
    rig.settle();
    assert_eq!(history_len(&rig), h1);
    assert!(setting(&rig, "graph.g1.node.1.op").is_some());
}

#[test]
fn an_sessions_edit_reaches_the_canvas_and_the_editor_idles() {
    let mut rig = rig(PanelFaults::default());
    motion(&mut rig);
    let add = op(&rig, "add");
    auto(&mut rig, gm::add_node("g1", 9, &add, 700.0, 40.0));
    let c = canvas(&rig);
    let title = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .and_then(|w| w.model().node(9).map(|n| n.title.clone()));
    assert_eq!(title.as_deref(), Some("Add"));
    // Past the shell's own debounced layout save (2 s after the panel was shown).
    rig.advance(Duration::from_secs(5));
    let (f, w) = rig.advance(Duration::from_secs(10));
    assert_eq!((f, w), (0, 0), "the graph editor does not idle");
}

/// A 2,000-node graph in the running editor: it opens, an automation session's move of one node is
/// applied to that node alone (the canvas's minimap is re-recorded once, the canvas records
/// only what is in view), and pan frames record only on-screen nodes. Times are printed
/// for the record (the wall-clock gate is `ui_graph_2k_nodes`).
#[test]
fn a_2000_node_graph_opens_and_follows_an_edit_node_by_node() {
    let mut rig = rig(PanelFaults::default());
    let add = op(&rig, "add");
    let mut cmds = gm::new_graph(&Graphs::default(), "Big", GraphKind::Blueprint).1;
    for n in 1..=2000u64 {
        cmds.extend(gm::add_node(
            "g1",
            n,
            &add,
            ((n - 1) % 50) as f64 * 240.0,
            ((n - 1) / 50) as f64 * 150.0,
        ));
        if n % 50 != 1 {
            cmds.push(forge_editor::domain::set(
                gm::node_key("g1", n, "in.a"),
                Value::Text(format!("{}.return", n - 1)),
            ));
        }
    }
    let t0 = std::time::Instant::now();
    auto(&mut rig, cmds);
    let open_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let c = canvas(&rig);
    let (nodes, wires) = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .map(|w| (w.model().nodes().len(), w.model().wires().len()))
        .unwrap_or_default();
    assert_eq!((nodes, wires), (2000, 1960));
    let s0 = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .map(|w| w.stats())
        .unwrap_or_default();
    let t0 = std::time::Instant::now();
    auto(&mut rig, gm::move_nodes("g1", &[(77, 10.0, 20.0)]));
    let edit_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let (s1, pos, visible) = rig
        .h
        .ui
        .widget::<NodeCanvas>(c)
        .map(|w| {
            (
                w.stats(),
                w.model().node(77).map(|n| n.pos),
                w.model().visible_nodes(),
            )
        })
        .unwrap_or_default();
    assert_eq!(pos, Some(Point::new(10.0, 20.0)));
    assert!(s1.minimap_paints - s0.minimap_paints <= 1);
    assert!(
        s1.nodes_recorded <= visible,
        "{} recorded, {visible} in view",
        s1.nodes_recorded
    );
    println!(
        "graph editor, 2,000 nodes: an automation session's 2,000-node graph opened in {open_ms:.1} ms (bus, mirror, panel, canvas); an automation session's move of one node {edit_ms:.2} ms to the next frame; {} of 2,000 nodes recorded",
        s1.nodes_recorded
    );
}
