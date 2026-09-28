//! The **Animation state machine** editor (`forge.anim_graph`, Ch.21 §21.21, DoD M2-64;
//! Ch.19) over `forge_editor::authoring::anim`, on the node-graph widget (ADR 0034):
//!
//! * **States and transitions** on the canvas: a state is a node, a transition a wire from
//!   its source's output to the target; each target input is one transition, labelled with
//!   its conditions (typed in the Conditions field before wiring: `speed > 0.5, grounded`).
//!   The *Any state* node's wires leave any state. Dragging states is one transaction;
//!   Delete removes states (with their transitions) or wires; the entry state is marked.
//! * **Parameters** (float, bool, trigger) and **1D/2D blend spaces**: the blend view shows
//!   the samples and a preview point with each motion's weight; dragging a sample is one
//!   gesture, one undo entry.
//! * **Bone masks**: a mask per skeleton; toggle a bone (and everything below it) in or out;
//!   assign a mask to a state.
//! * **Preview**: the machine runs (`MachineSim`, the evaluation a game runs) from its
//!   entry with the preview parameter values — session state, never written to the project;
//!   stepping shows the state, the motion weights and the transitions taken.
//! * Everything that would not run is listed and shown on its node ([`validate`]).
//!
//! Every project edit is a command (I7).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use forge_editor::authoring::anim::{
    self as an, ANY, AnimDoc, BlendKind, Machine, MachineSim, ParamKind, blend_weights_1d,
    blend_weights_2d, validate,
};
use forge_editor::emitter::Gesture;
use forge_editor::mirror::ProjectMirror;
use forge_editor::panel_rt::PanelAct;
use forge_editor::panels::PanelCx;
use forge_editor::services::EditorServices;
use forge_ui::widgets::{
    Button, CanvasNode, CanvasPin, CanvasSelection, CanvasWire, ConnectRequested, Container,
    DeleteRequested, DisconnectRequested, Label, LabelKind, NodeCanvas, NodesMoved,
    NumericCommitted, NumericField, PinRef, Pressed, RadioGroup, RowActivated, RowItem,
    SelectionChanged, TextField, VirtualTree,
};
use forge_ui::{ColorRole, NodeStyle, Point, Role, Signal, Ui, WidgetId};

use crate::common::{Row, Rows, key_of, refuse, select, selected, show_rows, watch};
use crate::widgets::{BlendData, BlendPointMoved, BlendSpaceView, DragPhase, SampleMoved};

const PREFIX: &str = "anim.";
/// The canvas key of the *Any state* node.
fn any_node() -> u64 {
    key_of(&["any"])
}

struct Ag {
    doc: AnimDoc,
    seen: u64,
    sel_machine: Option<String>,
    pending_machine: Option<String>,
    sel_state: Option<String>,
    sel_mask: Option<String>,
    machine_rows: Rows<String>,
    param_rows: Rows<String>,
    motion_rows: Rows<String>,
    mask_rows: Rows<String>,
    bone_rows: Rows<usize>,
    /// Canvas node key → state id.
    nodes: HashMap<u64, String>,
    /// Canvas input pin → transition id.
    wires: HashMap<PinRef, String>,
    canvas: WidgetId,
    machines: WidgetId,
    /// The machines list's empty state (M2-70).
    machines_empty: WidgetId,
    params: WidgetId,
    masks: WidgetId,
    bones: WidgetId,
    blend: Rc<RefCell<BlendData>>,
    blend_w: WidgetId,
    sim: Option<MachineSim>,
    status: Signal<String>,
    problems: Signal<String>,
    preview: Signal<String>,
    gesture: Option<Gesture>,
    /// The machine the canvas was last framed on.
    framed: Option<String>,
}

impl Ag {
    fn say(&self, ui: &mut Ui, s: impl Into<String>) {
        self.status.set(ui.rt_mut(), s.into());
    }
    fn machine(&self) -> Option<&Machine> {
        self.sel_machine
            .as_ref()
            .and_then(|m| self.doc.machines.get(m))
    }

    fn reread(&mut self, ui: &mut Ui, m: &ProjectMirror, sv: &EditorServices) {
        self.doc = AnimDoc::read(m);
        if let Some(x) = self.pending_machine.take()
            && self.doc.machines.contains_key(&x)
        {
            self.sel_machine = Some(x);
        }
        if self
            .sel_machine
            .as_ref()
            .is_none_or(|s| !self.doc.machines.contains_key(s))
        {
            self.sel_machine = self.doc.machines.keys().next().cloned();
        }
        if let Some(mc) = self.machine()
            && self
                .sel_state
                .as_ref()
                .is_some_and(|s| !mc.states.contains_key(s))
        {
            self.sel_state = None;
        }
        if self
            .sel_mask
            .as_ref()
            .is_none_or(|k| !self.doc.masks.contains_key(k))
        {
            self.sel_mask = self.doc.masks.keys().next().cloned();
        }
        // A preview of a machine whose shape changed restarts at its entry.
        if let Some(mc) = self.machine().cloned() {
            let keep = self
                .sim
                .as_ref()
                .is_some_and(|s| mc.states.contains_key(&s.state));
            if !keep {
                self.sim = Some(MachineSim::new(&mc));
            } else if let Some(s) = self.sim.as_mut() {
                for p in mc.params.values() {
                    s.params.entry(p.id.clone()).or_insert(p.default);
                }
                s.params.retain(|k, _| mc.params.contains_key(k));
            }
        } else {
            self.sim = None;
        }
        self.show(ui, sv);
    }

    fn show(&mut self, ui: &mut Ui, sv: &EditorServices) {
        // Machines.
        let rows: Vec<Row> = self
            .doc
            .machines
            .values()
            .map(|mc| {
                (
                    None,
                    key_of(&["machine", &mc.id]),
                    RowItem::new(forge_ui::trf!(
                        "{name} ({states_count} states)",
                        name = mc.name,
                        states_count = mc.states.len()
                    )),
                )
            })
            .collect();
        let map = self
            .doc
            .machines
            .keys()
            .map(|id| (key_of(&["machine", id]), id.clone()))
            .collect();
        show_rows(ui, self.machines, rows, map, &mut self.machine_rows);
        let _ = ui.set_hidden(self.machines_empty, !self.doc.machines.is_empty());
        if let Some(s) = &self.sel_machine {
            select(ui, self.machines, key_of(&["machine", s]));
        }
        // Parameters, with the preview's values.
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        if let Some(mc) = self.machine().cloned() {
            for p in mc.params.values() {
                let v = self
                    .sim
                    .as_ref()
                    .and_then(|s| s.params.get(&p.id))
                    .copied()
                    .unwrap_or(p.default);
                let shown = match p.kind {
                    ParamKind::Float => format!("{v:.2}"),
                    _ => (v != 0.0).to_string(),
                };
                let k = key_of(&["param", &mc.id, &p.id]);
                rows.push((
                    None,
                    k,
                    RowItem::new(format!("{} ({}) = {shown}", p.id, p.kind.name())),
                ));
                map.insert(k, p.id.clone());
            }
        }
        show_rows(ui, self.params, rows, map, &mut self.param_rows);
        // Masks and the selected mask's bones.
        let rows: Vec<Row> = self
            .doc
            .masks
            .values()
            .map(|mk| {
                (
                    None,
                    key_of(&["mask", &mk.id]),
                    RowItem::new(forge_ui::trf!(
                        "{name} ({skeleton}, {weights_count} bones)",
                        name = mk.name,
                        skeleton = mk.skeleton,
                        weights_count = mk.weights.len()
                    )),
                )
            })
            .collect();
        let map = self
            .doc
            .masks
            .keys()
            .map(|id| (key_of(&["mask", id]), id.clone()))
            .collect();
        show_rows(ui, self.masks, rows, map, &mut self.mask_rows);
        if let Some(s) = &self.sel_mask {
            select(ui, self.masks, key_of(&["mask", s]));
        }
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        if let Some(mk) = self.sel_mask.as_ref().and_then(|k| self.doc.masks.get(k))
            && let Some(sk) = sv.anim.skeleton(&mk.skeleton)
        {
            for (i, b) in sk.bones.iter().enumerate() {
                let w = mk.weights.get(&i).copied().unwrap_or(0.0);
                let k = key_of(&["bone", &mk.id, &i.to_string()]);
                let parent = b.parent.map(|p| key_of(&["bone", &mk.id, &p.to_string()]));
                let mark = if w >= 1.0 {
                    "\u{2611}".to_string()
                } else if w > 0.0 {
                    format!("{:.0}%", w * 100.0)
                } else {
                    "\u{2610}".to_string()
                };
                let mut item = RowItem::new(format!("{mark} {}", b.name));
                item.muted = w == 0.0;
                rows.push((parent, k, item));
                map.insert(k, i);
            }
        }
        show_rows(ui, self.bones, rows, map, &mut self.bone_rows);
        self.show_canvas(ui, sv);
        self.show_blend(ui);
        self.show_preview(ui);
    }

    fn show_canvas(&mut self, ui: &mut Ui, sv: &EditorServices) {
        let Some(mc) = self.machine().cloned() else {
            NodeCanvas::edit(ui, self.canvas, |e| e.clear());
            self.nodes.clear();
            self.wires.clear();
            self.problems.set(ui.rt_mut(), String::new());
            return;
        };
        let problems = validate(&mc, &*sv.anim, &self.doc.masks);
        let mut nodes = HashMap::new();
        let mut wires = HashMap::new();
        // Incoming transitions per state, in id order: each is one input pin.
        let mut incoming: BTreeMap<&str, Vec<&an::Transition>> = BTreeMap::new();
        for t in mc.transitions.values() {
            incoming.entry(t.to.as_str()).or_default().push(t);
        }
        let min_x = mc.states.values().map(|s| s.x).fold(0.0f64, f64::min);
        let min_y = mc.states.values().map(|s| s.y).fold(0.0f64, f64::min);
        let current = self.sim.as_ref().map(|s| s.state.clone());
        NodeCanvas::edit(ui, self.canvas, |e| {
            let stale: Vec<u64> = e.model().nodes().map(|(k, _)| *k).collect();
            for k in stale {
                e.remove_node(k);
            }
            e.set_node(
                any_node(),
                CanvasNode::new(
                    forge_ui::tr!("Any state"),
                    Point::new(min_x as f32 - 240.0, min_y as f32),
                )
                .with_outputs(vec![CanvasPin::new(
                    "\u{2192}",
                    forge_ui::tr!("leaves any state"),
                    ColorRole::Warning,
                )])
                .with_doc(forge_ui::tr!(
                    "Transitions from here leave whichever state is playing."
                )),
            );
            for s in mc.states.values() {
                let k = key_of(&["state", &mc.id, &s.id]);
                nodes.insert(k, s.id.clone());
                let mut title = s.name.clone();
                if s.id == mc.entry {
                    title.push_str(forge_ui::tr!(" \u{25b6} entry"));
                }
                if current.as_deref() == Some(s.id.as_str()) {
                    title.push_str(" \u{25cf}");
                }
                let what = match s.blend {
                    BlendKind::None => s
                        .motion
                        .clone()
                        .unwrap_or_else(|| forge_ui::tr!("no motion").into()),
                    b => forge_ui::trf!(
                        "{kind} blend of {n} motions",
                        kind = forge_ui::l10n::tr(b.name()),
                        n = s.samples.len()
                    ),
                };
                let mut inputs: Vec<CanvasPin> = incoming
                    .get(s.id.as_str())
                    .map(|v| {
                        v.iter()
                            .map(|t| {
                                let conds: Vec<String> =
                                    t.conds.iter().map(ToString::to_string).collect();
                                let label = if conds.is_empty() {
                                    match t.exit {
                                        Some(x) => {
                                            forge_ui::trf!("at {x}", x = format!("{x:.2}"))
                                        }
                                        None => forge_ui::tr!("always").into(),
                                    }
                                } else {
                                    conds.join(" & ")
                                };
                                let from = if t.from == ANY {
                                    forge_ui::tr!("any state").to_string()
                                } else {
                                    t.from.clone()
                                };
                                CanvasPin::new(
                                    &label,
                                    &forge_ui::trf!(
                                        "from {from}, fade {fade} s",
                                        from,
                                        fade = format!("{:.2}", t.duration)
                                    ),
                                    ColorRole::Accent,
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                inputs.push(CanvasPin::new(
                    forge_ui::tr!("+ enter"),
                    forge_ui::tr!("wire a transition here"),
                    ColorRole::FgMuted,
                ));
                e.set_node(
                    k,
                    CanvasNode::new(&title, Point::new(s.x as f32, s.y as f32))
                        .with_inputs(inputs)
                        .with_outputs(vec![CanvasPin::new(
                            "\u{2192}",
                            forge_ui::tr!("transition out"),
                            ColorRole::Accent,
                        )])
                        .with_doc(&what),
                );
                let err: Vec<&String> = problems
                    .iter()
                    // l10n: matches the validator's problem prefix (its diagnostic detail)
                    .filter(|p| p.starts_with(&format!("state {}", s.name)))
                    .collect();
                e.set_error(
                    k,
                    (!err.is_empty()).then(|| {
                        err.iter()
                            .map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    }),
                );
            }
            for (to, ts) in &incoming {
                let tk = key_of(&["state", &mc.id, to]);
                for (i, t) in ts.iter().enumerate() {
                    let fk = if t.from == ANY {
                        any_node()
                    } else {
                        key_of(&["state", &mc.id, &t.from])
                    };
                    let pin = PinRef::input(tk, i as u16);
                    wires.insert(pin, t.id.clone());
                    e.set_wire(CanvasWire {
                        from: PinRef::output(fk, 0),
                        to: pin,
                        role: if t.from == ANY {
                            ColorRole::Warning
                        } else {
                            ColorRole::Accent
                        },
                    });
                }
            }
        });
        // Show the machine from its top-left (the Any state node included) once per machine
        // shown; after that the view is the user's.
        if self.framed.as_deref() != Some(mc.id.as_str()) {
            NodeCanvas::edit(ui, self.canvas, |e| {
                let mut v = e.model().view();
                v.pan = Point::new(min_x as f32 - 260.0, min_y as f32 - 40.0);
                v.zoom = 1.0;
                e.set_view(v);
            });
            self.framed = Some(mc.id.clone());
        }
        self.nodes = nodes;
        self.wires = wires;
        let p = if problems.is_empty() {
            forge_ui::tr!("\u{2714} The machine is valid.").to_string()
        } else {
            forge_ui::trf!("\u{26a0} {problems}", problems = problems.join("; "))
        };
        self.problems.set(ui.rt_mut(), p);
    }

    fn show_blend(&mut self, ui: &mut Ui) {
        let data = match (self.machine(), &self.sel_state) {
            (Some(mc), Some(s)) => {
                mc.states
                    .get(s)
                    .filter(|s| s.blend != BlendKind::None)
                    .map(|st| {
                        let param = |p: &Option<String>| {
                            p.as_ref()
                                .and_then(|k| self.sim.as_ref().and_then(|s| s.params.get(k)))
                                .copied()
                                .unwrap_or(0.0)
                        };
                        let point = (param(&st.px), param(&st.py));
                        let two_d = st.blend == BlendKind::TwoD;
                        let weights = if two_d {
                            blend_weights_2d(
                                &st.samples.iter().map(|x| (x.x, x.y)).collect::<Vec<_>>(),
                                point,
                            )
                        } else {
                            blend_weights_1d(
                                &st.samples.iter().map(|x| x.x).collect::<Vec<_>>(),
                                point.0,
                            )
                        };
                        let selected = self
                            .blend
                            .borrow()
                            .selected
                            .filter(|i| *i < st.samples.len());
                        BlendData {
                            two_d,
                            samples: st
                                .samples
                                .iter()
                                .map(|x| {
                                    (
                                        x.id.clone(),
                                        x.motion.clone(),
                                        x.x,
                                        if two_d { x.y } else { 0.0 },
                                    )
                                })
                                .collect(),
                            point,
                            weights,
                            selected,
                        }
                    })
            }
            _ => None,
        };
        let shown = data.is_some();
        *self.blend.borrow_mut() = data.unwrap_or_default();
        let _ = ui.set_hidden(self.blend_w, !shown);
        ui.invalidate(self.blend_w, forge_ui::Dirty::PAINT | forge_ui::Dirty::A11Y);
    }

    fn show_preview(&mut self, ui: &mut Ui) {
        let text = match (self.machine(), &self.sim) {
            (Some(mc), Some(s)) => {
                let weights: Vec<String> = s
                    .weights(mc)
                    .iter()
                    .map(|(m, w)| format!("{m} {:.0}%", w * 100.0))
                    .collect();
                let name = mc
                    .states
                    .get(&s.state)
                    .map_or(s.state.as_str(), |x| x.name.as_str());
                let log = s
                    .log
                    .iter()
                    .rev()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                let playing = if weights.is_empty() {
                    forge_ui::tr!("nothing playing").into()
                } else {
                    weights.join(", ")
                };
                let time = format!("{:.2}", s.time);
                if log.is_empty() {
                    forge_ui::trf!(
                        "Preview: in {name} for {time} s \u{2014} {playing}",
                        name,
                        time,
                        playing
                    )
                } else {
                    forge_ui::trf!(
                        "Preview: in {name} for {time} s \u{2014} {playing} \u{2014} recent: {log}",
                        name,
                        time,
                        playing,
                        log
                    )
                }
            }
            _ => forge_ui::tr!("Create a machine to preview it.").into(),
        };
        self.preview.set(ui.rt_mut(), text);
    }
}

fn motion_rows(sv: &EditorServices) -> (Vec<Row>, HashMap<u64, String>) {
    let mut rows = Vec::new();
    let mut map = HashMap::new();
    for m in sv.anim.motions() {
        let k = key_of(&["motion", &m.name]);
        rows.push((
            None,
            k,
            RowItem::new(format!(
                "{} ({:.2} s{})",
                m.name,
                m.length,
                if m.looping {
                    forge_ui::tr!(", loop")
                } else {
                    ""
                }
            )),
        ));
        map.insert(k, m.name);
    }
    (rows, map)
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space[1];
        pb.want_turn();
        let sv = pb.services();
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space).padding(space).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("State machine actions")),
        )?;
        let name = pb.b.signal(String::new());
        pb.b.add(
            bar,
            "name",
            NodeStyle::leaf().width(140.0),
            TextField::new(name, forge_ui::tr!("Name")).placeholder(forge_ui::tr!("Locomotion")),
        )?;
        let new_machine = pb.b.add(
            bar,
            "new_machine",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New machine")),
        )?;
        let add_state = pb.b.add(
            bar,
            "add_state",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add state")),
        )?;
        let add_1d = pb.b.add(
            bar,
            "add_1d",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add 1D blend")),
        )?;
        let add_2d = pb.b.add(
            bar,
            "add_2d",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add 2D blend")),
        )?;
        let kind = pb.b.signal(0usize);
        let kinds: Vec<&str> = ParamKind::ALL.iter().map(|k| forge_ui::l10n::tr(k.name())).collect();
        pb.b.add(
            bar,
            "kind",
            NodeStyle::leaf(),
            RadioGroup::new(forge_ui::tr!("Parameter kind"), &kinds, kind),
        )?;
        let add_param = pb.b.add(
            bar,
            "add_param",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add parameter")),
        )?;
        let conds = pb.b.signal(String::new());
        pb.b.add(
            bar,
            "conds",
            NodeStyle::leaf().width(200.0),
            TextField::new(conds, forge_ui::tr!("Conditions for new transitions"))
                .placeholder(forge_ui::tr!("speed > 0.5, grounded")),
        )?;
        let set_entry = pb.b.add(
            bar,
            "set_entry",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Set entry")),
        )?;
        let delete = pb.b.add(
            bar,
            "delete",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Delete state")),
        )?;
        let body = pb.b.add(
            pb.parent,
            "body",
            crate::common::body_row(space),
            Container::new(Role::Group).labelled(forge_ui::tr!("State machine")),
        )?;
        let side = pb.b.add(
            body,
            "side",
            NodeStyle::column(space).width(220.0),
            Container::group(),
        )?;
        let machines_empty = pb.b.add(
            side,
            "machines_empty",
            NodeStyle::leaf(),
            forge_ui::widgets::EmptyState::new(forge_ui::tr!(
                "No state machines yet: name one above and press New machine."
            )),
        )?;
        let machines = pb.b.add(
            side,
            "machines",
            crate::common::fill(0.0, 70.0),
            VirtualTree::list(forge_ui::tr!("Machines")).single_select(),
        )?;
        let params = pb.b.add(
            side,
            "params",
            crate::common::fill(0.0, 80.0),
            VirtualTree::list(forge_ui::tr!("Parameters (preview values)"))
                .read_only()
                .single_select(),
        )?;
        let mut ml = VirtualTree::list(forge_ui::tr!(
            "Motions (Enter adds a state or a blend sample)"
        ))
        .read_only()
        .single_select();
        let (mrows, mmap) = motion_rows(&sv);
        for (p, k, item) in mrows {
            ml.push(p, k, item);
        }
        let motions =
            pb.b.add(side, "motions", crate::common::fill(0.0, 100.0), ml)?;
        let canvas = NodeCanvas::build(
            &mut *pb.b,
            body,
            "canvas",
            crate::common::fill(320.0, 240.0),
            forge_ui::tr!("State machine graph"),
        )?;
        let right = pb.b.add(
            body,
            "right",
            NodeStyle::column(space).width(260.0),
            Container::group(),
        )?;
        let blend = Rc::new(RefCell::new(BlendData::default()));
        let blend_w = pb.b.add(
            right,
            "blend",
            NodeStyle::leaf().min_size(240.0, 200.0),
            BlendSpaceView::new(blend.clone()),
        )?;
        pb.b.hide(blend_w, true);
        let masks = pb.b.add(
            right,
            "masks",
            NodeStyle::leaf().min_size(0.0, 60.0),
            VirtualTree::list(forge_ui::tr!("Bone masks")).single_select(),
        )?;
        let mask_bar = pb.b.add(
            right,
            "mask_bar",
            NodeStyle::row(space).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Mask actions")),
        )?;
        let new_mask = pb.b.add(
            mask_bar,
            "new_mask",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New mask")),
        )?;
        let assign = pb.b.add(
            mask_bar,
            "assign",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Assign to state")),
        )?;
        let unassign = pb.b.add(
            mask_bar,
            "unassign",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Clear state mask")),
        )?;
        let bones = pb.b.add(
            right,
            "bones",
            crate::common::fill(0.0, 140.0),
            VirtualTree::tree(forge_ui::tr!(
                "Bones (Enter toggles a bone and everything below it)"
            ))
            .read_only()
            .single_select(),
        )?;
        let prev_bar = pb.b.add(
            pb.parent,
            "preview_bar",
            NodeStyle::row(space).padding(space).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Preview")),
        )?;
        let value = pb.b.signal(0.0f64);
        let value_f = pb.b.add(
            prev_bar,
            "value",
            NodeStyle::leaf().width(110.0),
            NumericField::new(
                value,
                forge_ui::tr!("Preview value of the selected parameter"),
            ),
        )?;
        let fire = pb.b.add(
            prev_bar,
            "fire",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Fire / toggle")),
        )?;
        let step = pb.b.add(
            prev_bar,
            "step",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Step 0.1 s")),
        )?;
        let run = pb.b.add(
            prev_bar,
            "run",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Run 1 s")),
        )?;
        let reset = pb.b.add(
            prev_bar,
            "reset",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Reset preview")),
        )?;
        let preview = pb.b.signal(forge_ui::tr!("Create a machine to preview it.").to_string());
        pb.b.add(
            pb.parent,
            "preview",
            NodeStyle::leaf().padding(space),
            Label::new(preview).wrapping(),
        )?;
        let status =
            pb.b.signal(forge_ui::tr!("States, transitions, blend spaces and bone masks.").to_string());
        pb.b.add(
            pb.parent,
            "status",
            NodeStyle::leaf().padding(space),
            Label::new(status).kind(LabelKind::Warning).wrapping(),
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
            Label::new(forge_ui::l10n::tr_str(&sv.anim.backend().note).into_owned())
                .kind(LabelKind::Small)
                .wrapping(),
        )?;
        let mut motion_map = Rows::default();
        motion_map.map = mmap;

        let st = Rc::new(RefCell::new(Ag {
            doc: AnimDoc::default(),
            seen: u64::MAX,
            sel_machine: None,
            pending_machine: None,
            sel_state: None,
            sel_mask: None,
            machine_rows: Rows::default(),
            param_rows: Rows::default(),
            motion_rows: motion_map,
            mask_rows: Rows::default(),
            bone_rows: Rows::default(),
            nodes: HashMap::new(),
            wires: HashMap::new(),
            canvas,
            machines,
            machines_empty,
            params,
            masks,
            bones,
            blend,
            blend_w,
            sim: None,
            status,
            problems,
            preview,
            gesture: None,
            framed: None,
        }));

        let nm = move |act: &PanelAct, fallback: &str| {
            let n = name.get(act.ui.rt());
            if n.trim().is_empty() {
                fallback.to_string()
            } else {
                n.trim().to_string()
            }
        };
        let s = st.clone();
        pb.on(new_machine, move |act, _: &Pressed| {
            let mut ag = s.borrow_mut();
            let n = nm(act, forge_ui::tr!("Locomotion"));
            let sk = act
                .services
                .anim
                .skeletons()
                .first()
                .map(|x| x.name.clone())
                .unwrap_or_default();
            let first = act
                .services
                .anim
                .motions()
                .first()
                .map(|x| x.name.clone())
                // l10n: a motion's name in the animation library (data)
                .unwrap_or_else(|| "Idle".into());
            let (id, cmds) = an::new_machine(&ag.doc, &n, &sk, &first);
            act.cmd.emit_all(forge_ui::tr!("New state machine"), cmds);
            ag.pending_machine = Some(id.clone());
            ag.say(act.ui, forge_ui::trf!("Created state machine {id} on {sk}.", id, sk));
        });
        // The motion the motions list has selected (or the first).
        let pick_motion =
            |ag: &Ag, ui: &mut Ui, motions: WidgetId, sv: &EditorServices| -> String {
                selected(ui, motions)
                    .and_then(|k| ag.motion_rows.get(k))
                    .or_else(|| sv.anim.motions().first().map(|m| m.name.clone()))
                    // l10n: a motion's name in the animation library (data)
                    .unwrap_or_else(|| "Idle".into())
            };
        let s = st.clone();
        pb.on(add_state, move |act, _: &Pressed| {
            let ag = s.borrow();
            let Some(mc) = ag.machine() else {
                refuse(
                    act.session,
                    forge_ui::tr!("Add state"),
                    forge_ui::tr!("Create or select a state machine first."),
                );
                return;
            };
            let motion = pick_motion(&ag, act.ui, motions, act.services);
            let n = nm(act, &motion);
            let x = mc.states.values().map(|s| s.x).fold(0.0f64, f64::max) + 220.0;
            let (id, cmds) = an::add_state(mc, &n, &motion, x, 0.0);
            act.cmd.emit_all(forge_ui::tr!("Add state"), cmds);
            ag.say(act.ui, forge_ui::trf!("Added state {id} playing {motion}.", id, motion));
        });
        for (b, blend_kind) in [(add_1d, BlendKind::OneD), (add_2d, BlendKind::TwoD)] {
            let s = st.clone();
            pb.on(b, move |act, _: &Pressed| {
                let ag = s.borrow();
                let Some(mc) = ag.machine() else {
                    refuse(
                        act.session,
                        forge_ui::tr!("Add blend"),
                        forge_ui::tr!("Create or select a state machine first."),
                    );
                    return;
                };
                // Blend axes: the float parameters, in order (x, then y).
                let floats: Vec<&str> = mc
                    .params
                    .values()
                    .filter(|p| p.kind == ParamKind::Float)
                    .map(|p| p.id.as_str())
                    .collect();
                let need = if blend_kind == BlendKind::TwoD { 2 } else { 1 };
                if floats.len() < need {
                    refuse(
                        act.session,
                        forge_ui::tr!("Add blend"),
                        &forge_ui::trf!("A {blend_kind} blend space needs {need} float parameter(s): add them first.", blend_kind = forge_ui::l10n::tr(blend_kind.name()), need),
                    );
                    return;
                }
                let names: Vec<String> = act
                    .services
                    .anim
                    .motions()
                    .into_iter()
                    .map(|m| m.name)
                    .collect();
                let pick = |n: &str| {
                    names
                        .iter()
                        .find(|m| *m == n)
                        .cloned()
                        .unwrap_or_else(|| names.first().cloned().unwrap_or_default())
                };
                let samples: Vec<(String, f64, f64)> = if blend_kind == BlendKind::OneD {
                    vec![
                        (pick("Idle"), 0.0, 0.0), // l10n: a motion's name in the library (data)
                        (pick("Walk"), 1.0, 0.0), // l10n: a motion's name in the library (data)
                        (pick("Run"), 3.0, 0.0), // l10n: a motion's name in the library (data)
                    ]
                } else {
                    vec![
                        (pick("Idle"), 0.0, 0.0), // l10n: a motion's name in the library (data)
                        (pick("Walk"), 0.0, 1.0), // l10n: a motion's name in the library (data)
                        (pick("WalkBack"), 0.0, -1.0), // l10n: a motion's name in the library (data)
                        (pick("StrafeLeft"), -1.0, 0.0), // l10n: a motion's name in the library (data)
                        (pick("StrafeRight"), 1.0, 0.0), // l10n: a motion's name in the library (data)
                    ]
                };
                let refs: Vec<(&str, f64, f64)> = samples
                    .iter()
                    .map(|(m, x, y)| (m.as_str(), *x, *y))
                    .collect();
                let n = nm(
                    act,
                    if blend_kind == BlendKind::OneD {
                        forge_ui::tr!("Locomotion 1D")
                    } else {
                        forge_ui::tr!("Locomotion 2D")
                    },
                );
                let x = mc.states.values().map(|s| s.x).fold(0.0f64, f64::max) + 220.0;
                match an::add_blend_state(
                    mc,
                    &n,
                    blend_kind,
                    floats[0],
                    floats.get(1).copied(),
                    &refs,
                    (x, 140.0),
                ) {
                    Ok((id, cmds)) => {
                        act.cmd.emit_all(forge_ui::tr!("Add blend space"), cmds);
                        ag.say(
                            act.ui,
                            forge_ui::trf!("Added {blend_kind} blend space {id}.", blend_kind = forge_ui::l10n::tr(blend_kind.name()), id),
                        );
                    }
                    Err(why) => refuse(act.session, forge_ui::tr!("Add blend"), &why),
                }
            });
        }
        let s = st.clone();
        pb.on(add_param, move |act, _: &Pressed| {
            let ag = s.borrow();
            let Some(mc) = ag.machine() else {
                refuse(
                    act.session,
                    forge_ui::tr!("Add parameter"),
                    forge_ui::tr!("Create or select a state machine first."),
                );
                return;
            };
            let k = ParamKind::ALL
                .get(kind.get(act.ui.rt()))
                .copied()
                .unwrap_or(ParamKind::Float);
            let n = nm(act, "speed");
            let (id, cmds) = an::add_param(mc, &n, k);
            act.cmd.emit_all(forge_ui::tr!("Add parameter"), cmds);
            ag.say(act.ui, forge_ui::trf!("Added {k} parameter {id}.", k = forge_ui::l10n::tr(k.name()), id));
        });
        let s = st.clone();
        pb.on(set_entry, move |act, _: &Pressed| {
            let ag = s.borrow();
            let (Some(mc), Some(state)) = (ag.machine(), ag.sel_state.clone()) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Set entry"),
                    forge_ui::tr!("Select a state on the graph first."),
                );
                return;
            };
            match an::set_entry(mc, &state) {
                Ok(cmds) => {
                    act.cmd.emit_all(forge_ui::tr!("Set entry state"), cmds);
                }
                Err(why) => refuse(act.session, forge_ui::tr!("Set entry"), &why),
            }
        });
        let delete_states = {
            let s = st.clone();
            Rc::new(
                move |act: &mut PanelAct, states: Vec<String>, wires: Vec<String>| {
                    let ag = s.borrow();
                    let Some(mc) = ag.machine() else { return };
                    let mut cmds = an::delete_states(act.mirror, mc, &states);
                    for t in wires {
                        if !states.iter().any(|x| {
                            mc.transitions
                                .get(&t)
                                .is_some_and(|tt| &tt.from == x || &tt.to == x)
                        }) {
                            cmds.extend(an::delete_transition(act.mirror, mc, &t));
                        }
                    }
                    if !cmds.is_empty() {
                        act.cmd.emit_all(forge_ui::tr!("Delete from state machine"), cmds);
                    }
                },
            )
        };
        let s = st.clone();
        let d = delete_states.clone();
        pb.on(delete, move |act, _: &Pressed| {
            let sel = s.borrow().sel_state.clone();
            match sel {
                Some(x) => d(act, vec![x], Vec::new()),
                None => refuse(
                    act.session,
                    forge_ui::tr!("Delete"),
                    forge_ui::tr!("Select a state on the graph first."),
                ),
            }
        });
        let s = st.clone();
        let d = delete_states;
        pb.on(canvas, move |act, e: &DeleteRequested| {
            let (states, wires) = {
                let ag = s.borrow();
                (
                    e.nodes
                        .iter()
                        .filter_map(|k| ag.nodes.get(k).cloned())
                        .collect::<Vec<_>>(),
                    e.wires
                        .iter()
                        .filter_map(|p| ag.wires.get(p).cloned())
                        .collect::<Vec<_>>(),
                )
            };
            d(act, states, wires);
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &DisconnectRequested| {
            let ag = s.borrow();
            if let (Some(mc), Some(t)) = (ag.machine(), ag.wires.get(&e.to)) {
                act.cmd.emit_all(
                    forge_ui::tr!("Delete transition"),
                    an::delete_transition(act.mirror, mc, t),
                );
            }
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &ConnectRequested| {
            let ag = s.borrow();
            let Some(mc) = ag.machine() else { return };
            let from = if e.from.node == any_node() {
                Some(ANY.to_string())
            } else {
                ag.nodes.get(&e.from.node).cloned()
            };
            let (Some(from), Some(to)) = (from, ag.nodes.get(&e.to.node).cloned()) else {
                return;
            };
            let text = conds.get(act.ui.rt());
            let list: Vec<&str> = text
                .split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .collect();
            match an::add_transition(mc, &from, &to, &list, 0.2) {
                Ok((id, cmds)) => {
                    act.cmd.emit_all(forge_ui::tr!("Add transition"), cmds);
                    ag.say(act.ui, forge_ui::trf!("Added transition {id}.", id));
                }
                Err(why) => {
                    refuse(act.session, forge_ui::tr!("Add transition"), &why);
                    ag.say(act.ui, format!("\u{26a0} {why}"));
                }
            }
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &NodesMoved| {
            let ag = s.borrow();
            let Some(mc) = ag.machine() else { return };
            let moves: Vec<(String, f64, f64)> = e
                .moves
                .iter()
                .filter_map(|(k, p)| {
                    ag.nodes
                        .get(k)
                        .map(|s| (s.clone(), f64::from(p.x), f64::from(p.y)))
                })
                .collect();
            let cmds = an::move_states(mc, &moves);
            if !cmds.is_empty() {
                act.cmd.emit_all(forge_ui::tr!("Move states"), cmds);
            }
        });
        let s = st.clone();
        pb.on(canvas, move |act, e: &CanvasSelection| {
            let mut ag = s.borrow_mut();
            let st_id = e.nodes.first().and_then(|k| ag.nodes.get(k).cloned());
            if st_id != ag.sel_state {
                ag.sel_state = st_id;
                ag.blend.borrow_mut().selected = None;
                ag.show_blend(act.ui);
            }
        });
        let s = st.clone();
        pb.on(motions, move |act, e: &RowActivated| {
            let ag = s.borrow();
            let Some(motion) = ag.motion_rows.get(e.key) else {
                return;
            };
            let Some(mc) = ag.machine() else {
                refuse(
                    act.session,
                    forge_ui::tr!("Add"),
                    forge_ui::tr!("Create or select a state machine first."),
                );
                return;
            };
            // On a selected blend state: a new sample at the preview point; else a state.
            if let Some(stt) = ag.sel_state.as_ref().and_then(|x| mc.states.get(x))
                && stt.blend != BlendKind::None
            {
                let (x, y) = ag.blend.borrow().point;
                act.cmd
                    .emit_all(forge_ui::tr!("Add blend sample"), an::add_sample(mc, stt, &motion, x, y));
                ag.say(
                    act.ui,
                    forge_ui::trf!("Added {motion} to {name} at ({x}, {y}).", motion, name = stt.name, x = format!("{:.2}", x), y = format!("{:.2}", y)),
                );
                return;
            }
            let x = mc.states.values().map(|s| s.x).fold(0.0f64, f64::max) + 220.0;
            let (id, cmds) = an::add_state(mc, &motion, &motion, x, 0.0);
            act.cmd.emit_all(forge_ui::tr!("Add state"), cmds);
            ag.say(act.ui, forge_ui::trf!("Added state {id}.", id));
        });
        let s = st.clone();
        pb.on(blend_w, move |act, e: &SampleMoved| {
            let mut ag = s.borrow_mut();
            let Some((mc, stt)) = ag.machine().and_then(|mc| {
                ag.sel_state
                    .as_ref()
                    .and_then(|x| mc.states.get(x))
                    .map(|s| (mc.clone(), s.clone()))
            }) else {
                return;
            };
            let cmds = an::move_sample(&mc, &stt, &e.sample, e.x, e.y);
            match e.phase {
                DragPhase::Begin => {
                    let mut g = act.cmd.gesture(forge_ui::tr!("Move blend sample"));
                    g.update_all(cmds);
                    ag.gesture = Some(g);
                }
                DragPhase::Move => {
                    if let Some(g) = ag.gesture.as_mut() {
                        g.update_all(cmds);
                    }
                }
                DragPhase::End => {
                    if let Some(mut g) = ag.gesture.take() {
                        g.update_all(cmds);
                        g.commit();
                    }
                }
                DragPhase::Cancel => {
                    if let Some(g) = ag.gesture.take() {
                        g.cancel();
                    }
                }
                DragPhase::Nudge => {
                    act.cmd.emit_all(forge_ui::tr!("Move blend sample"), cmds);
                }
            }
        });
        let s = st.clone();
        pb.on(blend_w, move |act, e: &BlendPointMoved| {
            let mut ag = s.borrow_mut();
            let Some((px, py)) = ag
                .machine()
                .and_then(|mc| ag.sel_state.as_ref().and_then(|x| mc.states.get(x)))
                .map(|s| (s.px.clone(), s.py.clone()))
            else {
                return;
            };
            if let Some(sim) = ag.sim.as_mut() {
                if let Some(p) = &px {
                    sim.set_param(p, e.x);
                }
                if let Some(p) = &py {
                    sim.set_param(p, e.y);
                }
            }
            ag.show_blend(act.ui);
            ag.show_preview(act.ui);
            let sv = act.services;
            ag.show(act.ui, sv);
        });
        let s = st.clone();
        pb.on(value_f, move |act, e: &NumericCommitted| {
            let mut ag = s.borrow_mut();
            let Some(p) = selected(act.ui, params).and_then(|k| ag.param_rows.get(k)) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Preview value"),
                    forge_ui::tr!("Select a parameter first."),
                );
                return;
            };
            if let Some(sim) = ag.sim.as_mut() {
                sim.set_param(&p, e.value);
            }
            let sv = act.services;
            ag.show(act.ui, sv);
        });
        let s = st.clone();
        pb.on(fire, move |act, _: &Pressed| {
            let mut ag = s.borrow_mut();
            let Some(p) = selected(act.ui, params).and_then(|k| ag.param_rows.get(k)) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Fire"),
                    forge_ui::tr!("Select a bool or trigger parameter first."),
                );
                return;
            };
            let kind = ag
                .machine()
                .and_then(|mc| mc.params.get(&p))
                .map(|x| x.kind);
            if let Some(sim) = ag.sim.as_mut() {
                match kind {
                    Some(ParamKind::Trigger) => sim.set_param(&p, 1.0),
                    Some(ParamKind::Bool) => {
                        let v = sim.params.get(&p).copied().unwrap_or(0.0);
                        sim.set_param(&p, if v == 0.0 { 1.0 } else { 0.0 });
                    }
                    _ => {}
                }
            }
            let sv = act.services;
            ag.show(act.ui, sv);
        });
        for (b, secs) in [(step, 0.1f64), (run, 1.0)] {
            let s = st.clone();
            pb.on(b, move |act, _: &Pressed| {
                let mut ag = s.borrow_mut();
                let Some(mc) = ag.machine().cloned() else {
                    return;
                };
                if let Some(sim) = ag.sim.as_mut() {
                    let n = (secs / 0.1).round() as usize;
                    for _ in 0..n {
                        sim.step(&mc, &*act.services.anim, 0.1);
                    }
                }
                let sv = act.services;
                ag.show(act.ui, sv);
            });
        }
        let s = st.clone();
        pb.on(reset, move |act, _: &Pressed| {
            let mut ag = s.borrow_mut();
            ag.sim = ag.machine().map(MachineSim::new);
            let sv = act.services;
            ag.show(act.ui, sv);
        });
        let s = st.clone();
        pb.on(machines, move |act, e: &SelectionChanged| {
            let mut ag = s.borrow_mut();
            if let Some(m) = e.keys.first().and_then(|k| ag.machine_rows.get(*k))
                && ag.sel_machine.as_deref() != Some(m.as_str())
            {
                ag.sel_machine = Some(m);
                ag.sel_state = None;
                ag.sim = ag.machine().map(MachineSim::new);
                let sv = act.services;
                ag.show(act.ui, sv);
            }
        });
        let s = st.clone();
        pb.on(masks, move |act, e: &SelectionChanged| {
            let mut ag = s.borrow_mut();
            if let Some(m) = e.keys.first().and_then(|k| ag.mask_rows.get(*k)) {
                ag.sel_mask = Some(m);
                let sv = act.services;
                ag.show(act.ui, sv);
            }
        });
        let s = st.clone();
        pb.on(new_mask, move |act, _: &Pressed| {
            let mut ag = s.borrow_mut();
            let sk = ag
                .machine()
                .map(|m| m.skeleton.clone())
                .filter(|x| !x.is_empty())
                .or_else(|| {
                    act.services
                        .anim
                        .skeletons()
                        .first()
                        .map(|x| x.name.clone())
                })
                .unwrap_or_default();
            let n = nm(act, forge_ui::tr!("Upper body"));
            let (id, cmds) = an::new_mask(&ag.doc, &n, &sk, &[]);
            act.cmd.emit_all(forge_ui::tr!("New bone mask"), cmds);
            ag.sel_mask = Some(id.clone());
            ag.say(
                act.ui,
                forge_ui::trf!("Created mask {id} on {sk}: toggle bones below.", id, sk),
            );
        });
        let s = st.clone();
        pb.on(bones, move |act, e: &RowActivated| {
            let ag = s.borrow();
            let (Some(i), Some(mk)) = (
                ag.bone_rows.get(e.key),
                ag.sel_mask.as_ref().and_then(|k| ag.doc.masks.get(k)),
            ) else {
                return;
            };
            let Some(sk) = act.services.anim.skeleton(&mk.skeleton) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Bone mask"),
                    &forge_ui::trf!("Skeleton {skeleton} is not in the animation library.", skeleton = mk.skeleton),
                );
                return;
            };
            let on = mk.weights.get(&i).copied().unwrap_or(0.0) > 0.0;
            let sub = sk.subtree(i);
            match an::set_mask_weights(mk, &sub, if on { 0.0 } else { 1.0 }) {
                Ok(cmds) => {
                    act.cmd.emit_all(
                        if on {
                            forge_ui::tr!("Remove bones from mask")
                        } else {
                            forge_ui::tr!("Add bones to mask")
                        },
                        cmds,
                    );
                }
                Err(why) => refuse(act.session, forge_ui::tr!("Bone mask"), &why),
            }
        });
        for (b, clear) in [(assign, false), (unassign, true)] {
            let s = st.clone();
            pb.on(b, move |act, _: &Pressed| {
                let ag = s.borrow();
                let (Some(mc), Some(state)) = (ag.machine(), ag.sel_state.clone()) else {
                    refuse(
                        act.session,
                        forge_ui::tr!("State mask"),
                        forge_ui::tr!("Select a state on the graph first."),
                    );
                    return;
                };
                let mask = if clear { None } else { ag.sel_mask.clone() };
                if !clear && mask.is_none() {
                    refuse(
                        act.session,
                        forge_ui::tr!("State mask"),
                        forge_ui::tr!("Create or select a mask first."),
                    );
                    return;
                }
                act.cmd.emit_all(
                    forge_ui::tr!("State mask"),
                    an::set_state_mask(mc, &state, mask.as_deref()),
                );
            });
        }

        let s = st;
        pb.sync(machines, move |sy| {
            let rev = watch(sy.mirror, PREFIX);
            let mut ag = s.borrow_mut();
            if rev != ag.seen {
                ag.seen = rev;
                ag.reread(sy.ui, sy.mirror, sy.services);
            }
            Ok(())
        });
        Ok(())
    });
}
