//! The **graph editor** (`forge.graph`, Ch.21 §21.21, DoD M2-46; Ch.24): blueprints,
//! material, generator and PCG graphs on one node canvas, over the typed IR contract
//! (`forge_editor::graph::ir::GraphIr`; the labelled in-memory stub IR until `forge-graph`).
//!
//! * **Pins are typed with units.** The canvas checks every wire while it is dragged with
//!   `forge_editor::graph::check_connect` (types, unit dimensions through
//!   `forge_reflect::connect`, cycles) and shows the reason for a refusal next to the pin;
//!   the panel checks again before it sends the command, and says why in the status line and
//!   a notification.
//! * **Node search from the `#[forge_api]` registry**: Space, a double click on empty space,
//!   or a wire dropped on empty space (then only nodes with a compatible pin, which the new
//!   node is wired to). Only the nodes this kind of graph may use are offered.
//! * **Comments / groups, reroutes, minimap, copy / cut / paste / duplicate** (the clipboard
//!   carries a graph fragment; its wires stay inside the copied nodes).
//! * **Compile** runs the IR: errors are drawn on their nodes, listed (a click frames the
//!   node) and logged to the console with a click-through back to the node.
//! * **Graph ↔ text**: the generated Rust beside the canvas, following the graph; edit it and
//!   **Apply text** turns the difference into commands.
//! * **Every edit is a command** (I7), and an edit touching several settings (a paste, a
//!   delete, a reroute, a comment dragging its nodes) is one transaction and one undo entry.
//!   Pan, zoom and selection are view state and send nothing.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use forge_cmd::EditorCommand;
use forge_editor::console::{LogLevel, LogSource};
use forge_editor::graph::ir::{Compiled, GraphIr, apply_text, lower_generator};
use forge_editor::graph::library::{NodeLibrary, REROUTE, pin_detail, pin_role};
use forge_editor::graph::{
    self as gm, GraphDoc, GraphKind, GraphNode, Graphs, Source, canvas_pins, check_connect,
};
use forge_editor::mirror::{ProjectMirror, Watch};
use forge_editor::panel_rt::PanelAct;
use forge_editor::panels::PanelCx;
use forge_editor::services::EditorServices;
use forge_editor::session::{Reveal, SessionState};
use forge_ui::widgets::{
    Button, CanvasComment, CanvasNode, CanvasSelection, CanvasWire, ClipOp, ClipboardRequested,
    CommentEdited, CommentRequested, ConnectRefused, ConnectRequested, Container, DeleteRequested,
    DisconnectRequested, Label, LabelKind, MultilineEditor, NodeCanvas, NodeChosen, NodesMoved,
    PickItem, PinRef, Pressed, RadioGroup, RerouteRequested, RowActivated, RowItem, RowRenamed,
    SelectionChanged, TextField, VirtualTree,
};
use forge_ui::{ColorRole, NodeStyle, Point, Rect, Role, Signal, Ui, WidgetId};

const PREFIX: &str = "graph.";

/// The panel's state (shared by its handlers and its sync step).
struct Ge {
    graphs: Graphs,
    /// The graph shown.
    current: Option<String>,
    lib: Rc<NodeLibrary>,
    ir: Rc<dyn GraphIr>,
    services: Rc<EditorServices>,
    canvas: WidgetId,
    list: WidgetId,
    /// Graph list rows: key → graph id.
    list_rows: BTreeMap<u64, String>,
    diag_list: WidgetId,
    /// Diagnostic rows: key → node.
    diag_rows: BTreeMap<u64, Option<u64>>,
    text_col: WidgetId,
    text_shown: bool,
    text: Signal<String>,
    /// The text last generated (a text that differs was edited by the user).
    generated: String,
    status: Signal<String>,
    problems: Signal<String>,
    /// Errors of the last compile, by node (cleared on a node when it changes).
    errors: BTreeMap<u64, String>,
    /// Nodes with values skipped while reading them (the problems line).
    problem_nodes: std::collections::BTreeSet<u64>,
    /// Select these nodes once they arrive (a paste, a new node).
    pending_select: Vec<u64>,
    /// Open this graph once it arrives (a new graph).
    pending_open: Option<String>,
    seen_rev: u64,
    seen_reveal: u64,
}

impl Ge {
    fn doc(&self) -> Option<&GraphDoc> {
        self.current.as_ref().and_then(|g| self.graphs.get(g))
    }
    fn say(&self, ui: &mut Ui, s: impl Into<String>) {
        self.status.set(ui.rt_mut(), s.into());
    }
    fn split(&self) -> bool {
        self.services.faults.graph_edits_not_grouped()
    }

    /// The input pin a canvas pin is.
    fn pin_name(&self, p: PinRef) -> Option<String> {
        let doc = self.doc()?;
        let n = doc.nodes.get(&p.node)?;
        if n.op == REROUTE {
            return Some(if p.output { "out" } else { "in" }.to_string());
        }
        let ln = self.lib.get(&n.op)?;
        let pins = if p.output { &ln.outputs } else { &ln.inputs };
        pins.get(usize::from(p.index)).map(|p| p.name.to_string())
    }

    fn pin_index(&self, doc: &GraphDoc, node: u64, output: bool, name: &str) -> Option<u16> {
        let n = doc.nodes.get(&node)?;
        if n.op == REROUTE {
            return Some(0);
        }
        let ln = self.lib.get(&n.op)?;
        let pins = if output { &ln.outputs } else { &ln.inputs };
        pins.iter()
            .position(|p| p.name == name)
            .and_then(|i| u16::try_from(i).ok())
    }

    /// The canvas's check: the same one the command goes through.
    fn check(&self, from: PinRef, to: PinRef) -> Result<(), String> {
        if self.services.faults.graph_skips_unit_check() {
            return Ok(());
        }
        let doc = self.doc().ok_or(forge_ui::tr!("No graph is open."))?;
        let (Some(fp), Some(tp)) = (self.pin_name(from), self.pin_name(to)) else {
            return Err(forge_ui::tr!("That pin is not on a node the library knows.").into());
        };
        check_connect(doc, &self.lib, &Source::new(from.node, &fp), to.node, &tp).map(|_| ())
    }

    fn canvas_node(&self, doc: &GraphDoc, n: &GraphNode) -> CanvasNode {
        let pos = Point::new(n.x as f32, n.y as f32);
        let mut c = if n.op == REROUTE {
            let (role, detail) = match gm::output_pin(doc, &self.lib, &Source::new(n.id, "out")) {
                Ok(Some(p)) => (pin_role(p.kind), pin_detail(p)),
                _ => (ColorRole::FgMuted, forge_ui::tr!("any value").to_string()),
            };
            CanvasNode::reroute(pos, role, &detail)
        } else {
            match self.lib.get(&n.op) {
                Some(ln) => CanvasNode::new(&ln.title, pos)
                    .with_inputs(canvas_pins(ln, false))
                    .with_outputs(canvas_pins(ln, true))
                    .with_doc(&ln.doc),
                None => {
                    let mut c = CanvasNode::new(
                        &format!("? {}", n.op.rsplit("::").next().unwrap_or(&n.op)),
                        pos,
                    );
                    c.error = Some(forge_ui::trf!(
                        "`{op}` is not a node the library provides",
                        op = n.op
                    ));
                    c
                }
            }
        };
        if let Some(e) = self.errors.get(&n.id) {
            c.error = Some(e.clone());
        }
        c
    }

    fn wires_into(&self, doc: &GraphDoc, n: &GraphNode) -> Vec<CanvasWire> {
        let mut out = Vec::new();
        for (pin, src) in &n.inputs {
            let (Some(ti), Some(fi)) = (
                self.pin_index(doc, n.id, false, pin),
                self.pin_index(doc, src.node, true, &src.pin),
            ) else {
                continue;
            };
            let role = match gm::output_pin(doc, &self.lib, src) {
                Ok(Some(p)) => pin_role(p.kind),
                _ => ColorRole::FgMuted,
            };
            out.push(CanvasWire {
                from: PinRef::output(src.node, fi),
                to: PinRef::input(n.id, ti),
                role,
            });
        }
        out
    }

    /// Rebuild the canvas from the current graph (a graph switch or a full re-read).
    fn show_all(&mut self, ui: &mut Ui, frame: bool) {
        let doc = self.doc().cloned();
        let canvas = self.canvas;
        NodeCanvas::edit(ui, canvas, |e| {
            e.clear();
            let Some(doc) = &doc else { return };
            for n in doc.nodes.values() {
                e.set_node(n.id, self.canvas_node(doc, n));
            }
            for n in doc.nodes.values() {
                for w in self.wires_into(doc, n) {
                    e.set_wire(w);
                }
            }
            for c in doc.comments.values() {
                e.set_comment(c.id, canvas_comment(c));
            }
            if frame {
                e.frame_nodes(&[]);
            }
        });
    }

    /// Apply what changed to the canvas: only the touched nodes, their wires and comments.
    fn show_changed(&mut self, ui: &mut Ui, ch: &gm::Changed) {
        let Some(g) = self.current.clone() else {
            return;
        };
        if self.graphs.get(&g).is_none() {
            let canvas = self.canvas;
            NodeCanvas::edit(ui, canvas, |e| e.clear());
            return;
        }
        let nodes: Vec<u64> = ch
            .nodes
            .iter()
            .filter(|(x, _)| *x == g)
            .map(|(_, n)| *n)
            .collect();
        for n in &nodes {
            self.errors.remove(n);
        }
        let comments: Vec<u64> = ch
            .comments
            .iter()
            .filter(|(x, _)| *x == g)
            .map(|(_, c)| *c)
            .collect();
        // Problems: kept per node, so an edit re-checks only the nodes it touched.
        let mut problems_changed = false;
        if let Some(doc) = self.graphs.get(&g) {
            for n in &nodes {
                let has = doc.nodes.get(n).is_some_and(|x| !x.problems.is_empty());
                problems_changed |= if has {
                    self.problem_nodes.insert(*n)
                } else {
                    self.problem_nodes.remove(n)
                };
            }
        }
        if problems_changed {
            self.show_problems(ui);
        }
        let Some(doc) = self.graphs.get(&g) else {
            return;
        };
        let canvas = self.canvas;
        NodeCanvas::edit(ui, canvas, |e| {
            for n in &nodes {
                match doc.nodes.get(n) {
                    Some(node) => {
                        e.set_node(*n, self.canvas_node(doc, node));
                        e.remove_wires_into(*n);
                        for w in self.wires_into(doc, node) {
                            e.set_wire(w);
                        }
                    }
                    None => e.remove_node(*n),
                }
            }
            for c in &comments {
                match doc.comments.get(c) {
                    Some(cm) => e.set_comment(*c, canvas_comment(cm)),
                    None => e.remove_comment(*c),
                }
            }
        });
    }

    fn show_list(&mut self, ui: &mut Ui) {
        self.list_rows.clear();
        let rows: Vec<(u64, RowItem, String)> = self
            .graphs
            .graphs
            .values()
            .enumerate()
            .map(|(i, d)| {
                (
                    i as u64 + 1,
                    RowItem::new(format!(
                        "{} ({})",
                        d.name,
                        forge_ui::l10n::tr(d.kind.title())
                    )),
                    d.id.clone(),
                )
            })
            .collect();
        let sel = rows
            .iter()
            .find(|(_, _, g)| Some(g) == self.current.as_ref())
            .map(|(k, _, _)| *k);
        VirtualTree::edit(ui, self.list, |t| {
            t.clear();
            for (k, item, _) in &rows {
                t.push(None, *k, item.clone());
            }
            if let Some(k) = sel {
                t.select(&[k]);
            }
        });
        for (k, _, g) in rows {
            self.list_rows.insert(k, g);
        }
        self.problem_nodes = self
            .doc()
            .map(|d| {
                d.nodes
                    .values()
                    .filter(|n| !n.problems.is_empty())
                    .map(|n| n.id)
                    .collect()
            })
            .unwrap_or_default();
        self.show_problems(ui);
    }

    fn show_problems(&mut self, ui: &mut Ui) {
        let mut problems = self
            .doc()
            .map(|d| d.header_problems.clone())
            .unwrap_or_default();
        if let Some(d) = self.doc() {
            for n in &self.problem_nodes {
                if let Some(node) = d.nodes.get(n) {
                    problems.extend(node.problems.iter().cloned());
                }
            }
        }
        self.problems.set(
            ui.rt_mut(),
            if problems.is_empty() {
                String::new()
            } else {
                forge_ui::trf!(
                    "\u{26a0} {problems_count} value(s) skipped: {items}",
                    problems_count = problems.len(),
                    items = problems.join("; ")
                )
            },
        );
    }

    /// Regenerate the text view (when shown and not being edited by the user).
    fn refresh_text(&mut self, ui: &mut Ui) {
        if !self.text_shown {
            return;
        }
        let current = self.text.get(ui.rt());
        if current != self.generated {
            self.say(
                ui,
                forge_ui::tr!("The text was edited: Apply text to change the graph, or Compile to regenerate it."),
            );
            return;
        }
        let text = match self.doc() {
            Some(doc) => self.ir.compile(doc, &self.lib).text,
            None => String::new(),
        };
        self.generated = text.clone();
        self.text.set(ui.rt_mut(), text);
    }

    fn open(&mut self, ui: &mut Ui, g: Option<String>) {
        self.current = g;
        self.errors.clear();
        self.show_all(ui, true);
        self.show_list(ui);
        self.generated = self.text.get(ui.rt());
        self.refresh_text(ui);
        let diag_list = self.diag_list;
        VirtualTree::edit(ui, diag_list, |t| t.clear());
        self.diag_rows.clear();
    }

    fn compile(&mut self, ui: &mut Ui) -> Option<Compiled> {
        let doc = self.doc()?.clone();
        let mut c = self.ir.compile(&doc, &self.lib);
        // A clean generator graph also lowers to the work it describes (Ch.13); what does not
        // lower lands on its nodes like any compile error. Lowering needs the generator backend
        // a plugin provides; with none loaded the library has no stage nodes to lower.
        let mut solve_cells = None;
        let generator = self.services.generator.clone();
        if let Some(backend) = generator.filter(|_| c.ok() && doc.kind == GraphKind::Generator) {
            match lower_generator(&doc, &self.lib, backend.as_ref()) {
                Ok(lowered) => solve_cells = Some(lowered.cells),
                Err(ds) => c.diagnostics.extend(ds),
            }
        }
        self.errors = c.errors_by_node();
        let canvas = self.canvas;
        let errors = self.errors.clone();
        NodeCanvas::edit(ui, canvas, |e| {
            e.clear_errors();
            for (n, msg) in &errors {
                e.set_error(*n, Some(msg.clone()));
            }
        });
        self.diag_rows.clear();
        let mut rows = Vec::new();
        for (i, d) in c.diagnostics.iter().enumerate() {
            let k = i as u64 + 1;
            rows.push((k, RowItem::new(d.to_string())));
            self.diag_rows.insert(k, d.node);
            let src = match d.node {
                Some(node) => LogSource::GraphNode {
                    graph: doc.id.clone(),
                    node,
                },
                None => LogSource::None,
            };
            self.services.log.borrow_mut().push(
                LogLevel::Error,
                "forge.graph",
                &format!("{}: {d}", doc.name),
                src,
            );
        }
        let diag_list = self.diag_list;
        VirtualTree::edit(ui, diag_list, |t| {
            t.clear();
            for (k, item) in rows {
                t.push(None, k, item);
            }
        });
        self.generated = c.text.clone();
        self.text.set(ui.rt_mut(), c.text.clone());
        self.say(
            ui,
            if let Some(cells) = solve_cells.filter(|_| c.ok()) {
                forge_ui::trf!("Compiled `{name}`: no errors, {ops_count} ops; it lowers to a global solve of {cells} cells.", name = doc.name, ops_count = c.ops.len(), cells)
            } else if c.ok() {
                forge_ui::trf!("Compiled `{name}`: no errors, {ops_count} ops.", name = doc.name, ops_count = c.ops.len())
            } else {
                forge_ui::trf!("Compiled `{name}`: {diagnostics_count} error(s), shown on the nodes and in the console.", name = doc.name, diagnostics_count = c.diagnostics.len())
            },
        );
        Some(c)
    }

    fn focus_node(&mut self, ui: &mut Ui, n: u64) {
        let canvas = self.canvas;
        NodeCanvas::edit(ui, canvas, |e| {
            e.select(&[n]);
            e.frame_nodes(&[n]);
        });
        ui.set_focus(Some(canvas), true);
    }

    fn emit(&self, act: &mut PanelAct, label: &str, cmds: Vec<EditorCommand>) {
        if cmds.is_empty() {
            return;
        }
        if self.split() {
            for c in cmds {
                act.cmd.emit(c);
            }
        } else {
            act.cmd.emit_all(label, cmds);
        }
    }

    /// The node search's items: the nodes this graph may use (and, from a pin, only those
    /// with a pin that pin can be wired to).
    fn search_items(&self, from: Option<PinRef>) -> Vec<PickItem> {
        let Some(doc) = self.doc() else {
            return Vec::new();
        };
        let mut items: Vec<PickItem> = Vec::new();
        let from_pin = from.and_then(|p| Some((p, self.pin_name(p)?)));
        for ln in self.lib.for_kind(doc.kind) {
            if let Some((p, name)) = &from_pin
                && self.auto_pin(doc, &ln.op, *p, name).is_none()
            {
                continue;
            }
            items.push(
                PickItem::new(&ln.op, &ln.title)
                    .hint(&ln.category)
                    .detail(&ln.doc.chars().take(48).collect::<String>()),
            );
        }
        if from.is_none() || from.is_some_and(|p| p.output) {
            items.push(
                PickItem::new(REROUTE, forge_ui::tr!("Reroute")).hint(forge_ui::tr!("Wires")),
            );
        }
        items
    }

    /// Which pin of a new `op` node a wire from `p` would go to.
    fn auto_pin(&self, doc: &GraphDoc, op: &str, p: PinRef, name: &str) -> Option<String> {
        let mut d = doc.clone();
        let id = d.next_node_id();
        d.nodes.insert(
            id,
            GraphNode {
                id,
                op: op.to_string(),
                ..GraphNode::default()
            },
        );
        if op == REROUTE {
            return p.output.then(|| "in".to_string());
        }
        let ln = self.lib.get(op)?;
        if p.output {
            ln.inputs
                .iter()
                .find(|i| {
                    check_connect(&d, &self.lib, &Source::new(p.node, name), id, i.name).is_ok()
                })
                .map(|i| i.name.to_string())
        } else {
            ln.outputs
                .iter()
                .find(|o| {
                    check_connect(&d, &self.lib, &Source::new(id, o.name), p.node, name).is_ok()
                })
                .map(|o| o.name.to_string())
        }
    }
}

fn canvas_comment(c: &gm::GraphComment) -> CanvasComment {
    CanvasComment {
        rect: Rect::new(c.x as f32, c.y as f32, c.w as f32, c.h as f32),
        text: c.text.clone(),
    }
}

fn refuse(session: &mut SessionState, what: &str, why: &str) {
    session.refuse(what, why);
}

fn watch(m: &ProjectMirror) -> u64 {
    m.watch(Watch::SettingPrefix(PREFIX.to_string()))
}

pub fn build(cx: &mut PanelCx) {
    cx.empty_state(forge_ui::tr!(
        "No graphs yet: name one and press New graph."
    ));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space[1];
        pb.want_turn();
        let services = pb.services();
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space).padding(space).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Graph editor actions")),
        )?;
        let name = pb.b.signal(String::new());
        pb.b.add(
            bar,
            "name",
            NodeStyle::leaf().width(160.0),
            TextField::new(name, forge_ui::tr!("Graph name")).placeholder(forge_ui::tr!("Motion")),
        )?;
        let kind = pb.b.signal(0usize);
        let kinds: Vec<&str> = GraphKind::ALL.iter().map(|k| forge_ui::l10n::tr(k.title())).collect();
        pb.b.add(
            bar,
            "kind",
            NodeStyle::leaf(),
            RadioGroup::new(forge_ui::tr!("Graph kind"), &kinds, kind),
        )?;
        let new_graph = pb.b.add(
            bar,
            "new",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New graph")),
        )?;
        let delete_graph = pb.b.add(
            bar,
            "delete",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Delete graph")),
        )?;
        let compile = pb.b.add(
            bar,
            "compile",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Compile")).primary(),
        )?;
        let text_toggle = pb.b.add(
            bar,
            "text",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Graph \u{2194} text")),
        )?;
        let apply = pb.b.add(
            bar,
            "apply",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Apply text")),
        )?;
        let body = pb.b.add(
            pb.parent,
            "body",
            NodeStyle::row(space).grow(1.0).padding(space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Graph")),
        )?;
        let list = pb.b.add(
            body,
            "graphs",
            NodeStyle::leaf().width(170.0).min_size(0.0, 80.0),
            VirtualTree::list(forge_ui::tr!("Graphs")).single_select(),
        )?;
        let canvas = NodeCanvas::build(
            &mut *pb.b,
            body,
            "canvas",
            NodeStyle::default().grow(1.0).min_size(240.0, 200.0),
            forge_ui::tr!("Graph canvas"),
        )?;
        let text_col = pb.b.add(
            body,
            "text_col",
            NodeStyle::column(space).width(380.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Graph as text")),
        )?;
        let text = pb.b.signal(String::new());
        pb.b.add(
            text_col,
            "text",
            NodeStyle::leaf().grow(1.0).min_size(0.0, 160.0),
            MultilineEditor::new(text, forge_ui::tr!("Generated Rust")).mono(),
        )?;
        let diag_list = pb.b.add(
            text_col,
            "diagnostics",
            NodeStyle::leaf().min_size(0.0, 90.0),
            VirtualTree::list(forge_ui::tr!("Compile errors"))
                .read_only()
                .single_select(),
        )?;
        pb.b.hide(text_col, true);
        let status = pb.b.signal(
            forge_ui::tr!("Space adds a node; drag between pins to wire them; Compile checks the graph.")
                .to_string(),
        );
        pb.b.add(
            pb.parent,
            "status",
            NodeStyle::leaf().padding(space),
            Label::new(status).wrapping(),
        )?;
        let problems = pb.b.signal(String::new());
        pb.b.add(
            pb.parent,
            "problems",
            NodeStyle::leaf().padding(space),
            Label::new(problems).kind(LabelKind::Warning).wrapping(),
        )?;
        pb.b.add(
            pb.parent,
            "backend",
            NodeStyle::leaf().padding(space),
            Label::new(forge_ui::l10n::tr_str(&services.graph.backend().note).into_owned())
                .kind(LabelKind::Small)
                .wrapping(),
        )?;

        let st = Rc::new(RefCell::new(Ge {
            graphs: Graphs::default(),
            current: None,
            lib: services.graph_library.clone(),
            ir: services.graph.clone(),
            services: services.clone(),
            canvas,
            list,
            list_rows: BTreeMap::new(),
            diag_list,
            diag_rows: BTreeMap::new(),
            text_col,
            text_shown: false,
            text,
            generated: String::new(),
            status,
            problems,
            errors: BTreeMap::new(),
            problem_nodes: std::collections::BTreeSet::new(),
            pending_select: Vec::new(),
            pending_open: None,
            seen_rev: u64::MAX,
            seen_reveal: pb.session().pending_reveal().map_or(0, |(s, _)| *s),
        }));

        // The canvas asks the panel about wires and search items (read-only).
        {
            let s = st.clone();
            let s2 = st.clone();
            // The canvas lives in the tree already; its hooks are set through the handles'
            // first sync (below), when the UI is at hand.
            let hooks = Rc::new(RefCell::new(Some((
                Rc::new(move |from: PinRef, to: PinRef| s.borrow().check(from, to))
                    as forge_ui::widgets::ConnectCheck,
                Rc::new(move |from: Option<PinRef>| Rc::new(s2.borrow().search_items(from)))
                    as forge_ui::widgets::SearchItems,
            ))));
            let s = st.clone();
            pb.sync(canvas, move |sy| {
                if let Some((check, search)) = hooks.borrow_mut().take()
                    && let Some(c) = sy.ui.widget_mut::<NodeCanvas>(canvas)
                {
                    c.set_connect_check(check);
                    c.set_search(search);
                }
                let mut ge = s.borrow_mut();
                let rev = watch(sy.mirror);
                if rev != ge.seen_rev {
                    ge.seen_rev = rev;
                    let ch = ge.graphs.follow(sy.mirror);
                    if let Some(g) = ge.pending_open.clone()
                        && ge.graphs.get(&g).is_some()
                    {
                        ge.pending_open = None;
                        ge.open(sy.ui, Some(g));
                    } else if ge
                        .current
                        .as_ref()
                        .is_none_or(|g| ge.graphs.get(g).is_none())
                    {
                        let first = ge.graphs.graphs.keys().next().cloned();
                        if first != ge.current || ch.full {
                            ge.open(sy.ui, first);
                        }
                    } else if ch.full {
                        let cur = ge.current.clone();
                        ge.open(sy.ui, cur);
                    } else {
                        let touches = ge.current.as_ref().is_some_and(|g| ch.touches(g));
                        if touches {
                            ge.show_changed(sy.ui, &ch);
                            ge.refresh_text(sy.ui);
                        }
                        if !ch.graphs.is_empty() {
                            ge.show_list(sy.ui);
                        }
                    }
                    if !ge.pending_select.is_empty() {
                        let sel = std::mem::take(&mut ge.pending_select);
                        let canvas = ge.canvas;
                        NodeCanvas::edit(sy.ui, canvas, |e| e.select(&sel));
                    }
                }
                // A console entry's click-through (or any other reveal) of one of our nodes.
                if let Some((seq, Reveal::GraphNode { graph, node })) =
                    sy.session.pending_reveal().cloned()
                    && seq > ge.seen_reveal
                {
                    ge.seen_reveal = seq;
                    if ge.graphs.get(&graph).is_some() {
                        if ge.current.as_deref() != Some(graph.as_str()) {
                            ge.open(sy.ui, Some(graph));
                        }
                        ge.focus_node(sy.ui, node);
                    }
                }
                Ok(())
            });
        }

        let s = st.clone();
        pb.on(new_graph, move |act, _: &Pressed| {
            let mut ge = s.borrow_mut();
            let n = name.get(act.ui.rt());
            let k = GraphKind::ALL
                .get(kind.get(act.ui.rt()))
                .copied()
                .unwrap_or_default();
            let n = if n.trim().is_empty() {
                forge_ui::trf!("New {kind}", kind = forge_ui::l10n::tr(k.title()).to_lowercase())
            } else {
                n.trim().to_string()
            };
            let (id, cmds) = gm::new_graph(&ge.graphs, &n, k);
            ge.emit(act, forge_ui::tr!("New graph"), cmds);
            ge.pending_open = Some(id);
            ge.say(
                act.ui,
                forge_ui::trf!("Created {title} `{n}`.", title = forge_ui::l10n::tr(k.title()).to_lowercase(), n),
            );
        });
        let s = st.clone();
        pb.on(delete_graph, move |act, _: &Pressed| {
            let ge = s.borrow();
            let Some(g) = ge.current.clone() else {
                refuse(
                    act.session,
                    forge_ui::tr!("Delete graph"),
                    forge_ui::tr!("No graph is open."),
                );
                return;
            };
            let cmds = gm::delete_graph(act.mirror, &g);
            ge.emit(act, forge_ui::tr!("Delete graph"), cmds);
        });
        let s = st.clone();
        pb.on(list, move |act, e: &SelectionChanged| {
            let mut ge = s.borrow_mut();
            if let Some(g) = e.keys.first().and_then(|k| ge.list_rows.get(k).cloned())
                && ge.current.as_deref() != Some(g.as_str())
            {
                ge.open(act.ui, Some(g));
            }
        });
        let s = st.clone();
        pb.on(list, move |act, e: &RowRenamed| {
            let ge = s.borrow();
            if let Some(g) = ge.list_rows.get(&e.key) {
                act.cmd.emit(forge_editor::domain::set(
                    gm::key(g, "name"),
                    forge_cmd::Value::Text(e.name.clone()),
                ));
            }
        });
        let s = st.clone();
        pb.on(compile, move |act, _: &Pressed| {
            let mut ge = s.borrow_mut();
            if ge.compile(act.ui).is_none() {
                refuse(
                    act.session,
                    forge_ui::tr!("Compile"),
                    forge_ui::tr!("No graph is open."),
                );
            }
        });
        let s = st.clone();
        pb.on(text_toggle, move |act, _: &Pressed| {
            let mut ge = s.borrow_mut();
            ge.text_shown = !ge.text_shown;
            let (col, shown) = (ge.text_col, ge.text_shown);
            let _ = act.ui.set_hidden(col, !shown);
            if shown {
                ge.generated = ge.text.get(act.ui.rt());
                ge.refresh_text(act.ui);
            }
        });
        let s = st.clone();
        pb.on(apply, move |act, _: &Pressed| {
            let mut ge = s.borrow_mut();
            let Some(doc) = ge.doc().cloned() else {
                refuse(
                    act.session,
                    forge_ui::tr!("Apply text"),
                    forge_ui::tr!("No graph is open."),
                );
                return;
            };
            let text = ge.text.get(act.ui.rt());
            match ge.ir.parse_text(&text, &ge.lib) {
                Ok(tg) => {
                    let cmds = apply_text(&doc, &ge.lib, &tg);
                    let n = cmds.len();
                    ge.emit(act, forge_ui::tr!("Edit graph as text"), cmds);
                    // The graph now says what the text says: the text follows it again.
                    ge.generated = text;
                    ge.say(act.ui, forge_ui::trf!("Applied the text: {n} change(s).", n));
                }
                Err(diags) => {
                    let rows: Vec<(u64, RowItem)> = diags
                        .iter()
                        .enumerate()
                        .map(|(i, d)| (i as u64 + 1, RowItem::new(d.to_string())))
                        .collect();
                    ge.diag_rows = rows.iter().map(|(k, _)| (*k, None)).collect();
                    let dl = ge.diag_list;
                    VirtualTree::edit(act.ui, dl, |t| {
                        t.clear();
                        for (k, item) in rows {
                            t.push(None, k, item);
                        }
                    });
                    refuse(
                        act.session,
                        forge_ui::tr!("The text was not applied"),
                        &diags.first().map(ToString::to_string).unwrap_or_default(),
                    );
                    ge.say(
                        act.ui,
                        forge_ui::trf!("\u{26a0} The text does not parse ({diags_count} problem(s)); nothing changed.", diags_count = diags.len()),
                    );
                }
            }
        });
        let s = st.clone();
        pb.on(diag_list, move |act, e: &RowActivated| {
            let mut ge = s.borrow_mut();
            if let Some(Some(n)) = ge.diag_rows.get(&e.key).copied() {
                ge.focus_node(act.ui, n);
            }
        });

        // ---- the canvas's edits, as commands -------------------------------------------
        let s = st.clone();
        pb.on(canvas, move |act, e: &ConnectRequested| {
            let ge = s.borrow();
            let Some(doc) = ge.doc() else { return };
            let (Some(fp), Some(tp)) = (ge.pin_name(e.from), ge.pin_name(e.to)) else {
                return;
            };
            let src = Source::new(e.from.node, &fp);
            let cmds = if ge.services.faults.graph_skips_unit_check() {
                // The control: wire it with no check at all.
                vec![forge_editor::domain::set(
                    gm::node_key(&doc.id, e.to.node, &format!("in.{tp}")),
                    forge_cmd::Value::Text(src.to_string()),
                )]
            } else {
                match gm::connect(doc, &ge.lib, &src, e.to.node, &tp) {
                    Ok(c) => c,
                    Err(why) => {
                        refuse(act.session, forge_ui::tr!("Connection refused"), &why);
                        ge.say(act.ui, format!("\u{26a0} {why}"));
                        return;
                    }
                }
            };
            ge.emit(act, forge_ui::tr!("Connect"), cmds);
            ge.say(act.ui, forge_ui::trf!("Connected {fp} to {tp}.", fp, tp));
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &ConnectRefused| {
            let ge = s.borrow();
            refuse(act.session, forge_ui::tr!("Connection refused"), &e.reason);
            ge.say(act.ui, format!("\u{26a0} {}", e.reason));
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &DisconnectRequested| {
            let ge = s.borrow();
            let (Some(doc), Some(tp)) = (ge.doc(), ge.pin_name(e.to)) else {
                return;
            };
            let cmds = gm::disconnect(&doc.id, e.to.node, &tp);
            ge.emit(act, forge_ui::tr!("Disconnect"), cmds);
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &NodesMoved| {
            let ge = s.borrow();
            let Some(doc) = ge.doc() else { return };
            let moves: Vec<(u64, f64, f64)> = e
                .moves
                .iter()
                .map(|(n, p)| (*n, f64::from(p.x), f64::from(p.y)))
                .collect();
            let label = if moves.len() == 1 {
                forge_ui::tr!("Move node")
            } else {
                forge_ui::tr!("Move nodes")
            };
            ge.emit(act, label, gm::move_nodes(&doc.id, &moves));
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &DeleteRequested| {
            let ge = s.borrow();
            let Some(doc) = ge.doc() else { return };
            let mut cmds = gm::delete_nodes(act.mirror, doc, &e.nodes);
            for c in &e.comments {
                cmds.extend(gm::delete_comment(act.mirror, &doc.id, *c));
            }
            for w in &e.wires {
                if let Some(p) = ge.pin_name(*w) {
                    cmds.extend(gm::disconnect(&doc.id, w.node, &p));
                }
            }
            ge.emit(act, forge_ui::tr!("Delete"), cmds);
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &RerouteRequested| {
            let ge = s.borrow();
            let (Some(doc), Some(p)) = (ge.doc(), ge.pin_name(e.to)) else {
                return;
            };
            let id = doc.next_node_id();
            match gm::insert_reroute(
                doc,
                e.to.node,
                &p,
                id,
                f64::from(e.at.x) - 7.0,
                f64::from(e.at.y) - 7.0,
            ) {
                Ok(cmds) => ge.emit(act, forge_ui::tr!("Add reroute"), cmds),
                Err(why) => refuse(act.session, forge_ui::tr!("Reroute"), &why),
            }
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &NodeChosen| {
            let mut ge = s.borrow_mut();
            let Some(doc) = ge.doc().cloned() else { return };
            let id = doc.next_node_id();
            let mut cmds = gm::add_node(&doc.id, id, &e.op, f64::from(e.at.x), f64::from(e.at.y));
            if let Some(p) = e.from
                && let Some(name) = ge.pin_name(p)
                && let Some(auto) = ge.auto_pin(&doc, &e.op, p, &name)
            {
                let (src, to, pin) = if p.output {
                    (Source::new(p.node, &name), id, auto)
                } else {
                    (Source::new(id, &auto), p.node, name)
                };
                cmds.push(forge_editor::domain::set(
                    gm::node_key(&doc.id, to, &format!("in.{pin}")),
                    forge_cmd::Value::Text(src.to_string()),
                ));
            }
            let title = ge
                .lib
                .get(&e.op)
                .map_or(forge_ui::tr!("Reroute").to_string(), |l| l.title.clone());
            ge.emit(act, &forge_ui::trf!("Add {title}", title), cmds);
            ge.pending_select = vec![id];
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &ClipboardRequested| {
            let mut ge = s.borrow_mut();
            let Some(doc) = ge.doc().cloned() else { return };
            match e.op {
                ClipOp::Copy | ClipOp::Cut => {
                    let Some(text) = gm::copy(&doc, &e.nodes, &e.comments) else {
                        return;
                    };
                    act.ui.clipboard().set_text(&text);
                    if e.op == ClipOp::Cut {
                        let mut cmds = gm::delete_nodes(act.mirror, &doc, &e.nodes);
                        for c in &e.comments {
                            cmds.extend(gm::delete_comment(act.mirror, &doc.id, *c));
                        }
                        ge.emit(act, forge_ui::tr!("Cut"), cmds);
                    }
                    ge.say(act.ui, forge_ui::trf!("Copied {nodes_count} node(s).", nodes_count = e.nodes.len()));
                }
                ClipOp::Paste | ClipOp::Duplicate => {
                    let text = if e.op == ClipOp::Paste {
                        act.ui.clipboard().get_text().unwrap_or_default()
                    } else {
                        gm::copy(&doc, &e.nodes, &e.comments).unwrap_or_default()
                    };
                    let (x, y) = if e.op == ClipOp::Paste {
                        (f64::from(e.at.x), f64::from(e.at.y))
                    } else {
                        let x0 = e
                            .nodes
                            .iter()
                            .filter_map(|n| doc.nodes.get(n))
                            .map(|n| (n.x, n.y))
                            .fold((f64::MAX, f64::MAX), |a, b| (a.0.min(b.0), a.1.min(b.1)));
                        (x0.0 + 40.0, x0.1 + 40.0)
                    };
                    match gm::paste(&doc, &text, x, y) {
                        Ok((ids, cmds)) => {
                            let n = ids.len();
                            let (label, said) = if e.op == ClipOp::Paste {
                                (forge_ui::tr!("Paste"), forge_ui::trf!("Pasted {n} node(s).", n))
                            } else {
                                (
                                    forge_ui::tr!("Duplicate"),
                                    forge_ui::trf!("Duplicated {n} node(s).", n),
                                )
                            };
                            ge.emit(act, label, cmds);
                            ge.say(act.ui, said);
                            ge.pending_select = ids;
                        }
                        Err(why) => refuse(act.session, forge_ui::tr!("Paste"), &why),
                    }
                }
            }
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &CommentRequested| {
            let ge = s.borrow();
            let Some(doc) = ge.doc() else { return };
            let r = e.rect;
            let cmds = gm::add_comment(
                &doc.id,
                doc.next_comment_id(),
                [
                    f64::from(r.x),
                    f64::from(r.y),
                    f64::from(r.w),
                    f64::from(r.h),
                ],
                forge_ui::tr!("Comment"),
            );
            ge.emit(act, forge_ui::tr!("Add comment"), cmds);
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &CommentEdited| {
            let ge = s.borrow();
            let Some(doc) = ge.doc() else { return };
            let r = e.rect;
            let mut cmds = gm::set_comment_rect(
                &doc.id,
                e.comment,
                [
                    f64::from(r.x),
                    f64::from(r.y),
                    f64::from(r.w),
                    f64::from(r.h),
                ],
            );
            let moves: Vec<(u64, f64, f64)> = e
                .moved
                .iter()
                .map(|(n, p)| (*n, f64::from(p.x), f64::from(p.y)))
                .collect();
            cmds.extend(gm::move_nodes(&doc.id, &moves));
            ge.emit(act, forge_ui::tr!("Move comment"), cmds);
        });
        let s = st;
        pb.on(canvas, move |act, e: &CanvasSelection| {
            let ge = s.borrow();
            let Some(doc) = ge.doc() else { return };
            if let [n] = e.nodes.as_slice()
                && let Some(node) = doc.nodes.get(n)
            {
                let about = match ge.lib.get(&node.op) {
                    Some(ln) => format!("{} \u{2014} {}", ln.title, ln.doc),
                    None if node.op == REROUTE => forge_ui::tr!("Reroute").to_string(),
                    None => forge_ui::trf!("`{op}` is not in the library", op = node.op),
                };
                let err = ge
                    .errors
                    .get(n)
                    .map(|e| format!(" \u{26a0} {e}"))
                    .unwrap_or_default();
                ge.say(act.ui, forge_ui::trf!("Node {n}: {about}{err}", n, about, err));
            }
        });
        Ok(())
    });
}
