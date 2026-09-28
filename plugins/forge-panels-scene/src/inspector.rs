//! The **Inspector** (`forge.inspector`, Ch.21 §21.21, DoD M2-37), generated from
//! reflection (`forge_editor::inspect`).
//!
//! * Every component on the selection gets a group whose rows come from its type's
//!   `#[forge_api]` metadata: label and unit, tooltip (the doc comment), range, step,
//!   editor hint, category headers, nested structs indented, an enum's data-variant fields
//!   under it. Every reflected kind has an editor ([`forge_editor::inspect::EditorKind`]).
//! * **Multi-object editing**: rows read across the selection; a row whose values differ
//!   shows "mixed" (a tri-state box for booleans) and an edit sets it on every selected
//!   entity in one transaction. A slider drag is one gesture: one command step per frame
//!   across the selection, one undo entry, Esc cancels.
//! * Add component (every default, one transaction) and Remove component (every property
//!   under its key, one transaction).
//! * **`InspectorWidget`**: a registered custom editor replaces a row (by `widget` hint or
//!   type) or a whole component (by type) — the plugin's steps run here with a
//!   [`PanelBuilder`], so they emit commands like any panel.
//! * **Generated content** (`gen.seed_path`) shows its seed path — "Show in …" reveals it
//!   in the panel a plugin declared for seed paths ([`SeedPathPanel`]), when one did — and
//!   "Promote"; any edit also promotes it (touch to promote, Ch.14).
//! * It follows the mirror: the content is rebuilt only when the *shape* of what is shown
//!   changes (selection, components, active variants); otherwise only the rows whose
//!   properties changed are refreshed, so an automation session's edit shows next frame at the cost
//!   of those rows.
//!
//! [`PanelBuilder`]: forge_editor::panel_rt::PanelBuilder

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::emitter::Gesture;
use forge_editor::inspect::{
    ComponentDef, EditorKind, FieldState, GEN_PROMOTED, GEN_SEED_PATH, InspectorCx, InspectorRow,
    component_widget, field_state, int_of_value, int_range, int_value, promote_command, row_widget,
    set_commands,
};
use forge_editor::mirror::ProjectMirror;
use forge_editor::panel_rt::{PanelBuilder, ShellHandles, WindowKey};
use forge_editor::panels::PanelCx;
use forge_editor::services::EditorServices;
use forge_editor::session::SeedPathPanel;
use forge_ui::widgets::{
    Button, CheckState, Checkbox, ComboBox, Container, IntegerCommitted, IntegerField, Label,
    LabelKind, NumericCommitted, NumericField, Pressed, SignalChanged, SignalRelay, Slider,
    SliderEdit, SliderPhase, Submitted, TextField, TriCheckbox, vector_editor,
};
use forge_ui::{Key, NodeStyle, Role, Ui, UiError, WidgetId};

/// Entities listed in an entity picker (plus the current value).
pub const PICKER_CAP: usize = 1000;

type Refresher = Box<dyn FnMut(&mut Ui, &ProjectMirror)>;

/// What the inspector keeps between rebuilds.
struct Insp {
    content: WidgetId,
    window: WindowKey,
    /// Hash of what the content shows (selection, components, variants, raw paths).
    shape: u64,
    seen: u64,
    selection: Vec<EntityKey>,
    refreshers: Vec<Refresher>,
    signals: Vec<forge_ui::state::AnySignal>,
    rebuilds: u64,
    /// A teammate claimed what is selected: every row is read-only (the core refuses the
    /// edits too; the inspector does not just hide them, WP-U10).
    locked: bool,
    /// The one selected entity when it is linked to a base scene (WP-U20): its rows show
    /// inherited or overridden, with Revert.
    linked: Option<EntityKey>,
    /// Its base node: a change there can change what the entity overrides.
    base: Option<EntityKey>,
}

fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::Text(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}
fn f64_of(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Float(x)) => *x,
        Some(Value::Int(i)) => *i as f64,
        _ => 0.0,
    }
}
fn vec3_of(v: Option<&Value>) -> [f64; 3] {
    match v {
        Some(Value::Vec3(a)) => *a,
        _ => [0.0; 3],
    }
}

/// The shape of what the inspector shows (rebuild when it changes).
fn shape_of(m: &ProjectMirror, sel: &[EntityKey], services: &EditorServices) -> u64 {
    let mut h = DefaultHasher::new();
    sel.hash(&mut h);
    for e in sel {
        let Some(me) = m.entity(*e) else {
            0u8.hash(&mut h);
            continue;
        };
        for c in services.components.components_of(me) {
            c.key.hash(&mut h);
            for r in c
                .rows
                .iter()
                .filter(|r| r.field.kind == forge_reflect::PinKind::Enum)
            {
                c.variant_on(me, &r.path).hash(&mut h);
            }
        }
        for (k, _) in services.components.unclaimed(me) {
            k.hash(&mut h);
        }
        me.properties.contains_key(GEN_SEED_PATH).hash(&mut h);
        (me.properties.get(GEN_PROMOTED) == Some(&Value::Bool(true))).hash(&mut h);
    }
    // Scene composition (WP-U20): what the one selected entity is (instance, inherited node,
    // scene) and what it is linked to. What it overrides is refreshed row by row, not here,
    // so a drag that starts overriding a value never rebuilds the slider under it.
    if let [one] = sel
        && let Some(l) = forge_editor::composition::link(m, *one)
    {
        format!("{:?}", l.role).hash(&mut h);
        l.base_scene.hash(&mut h);
        l.base.hash(&mut h);
        l.missing.hash(&mut h);
    }
    // Teammates (WP-U10): a claim on the selection makes it read-only; who else is here.
    let (claim, viewers) = collab_state(m, sel, services);
    claim.hash(&mut h);
    viewers.hash(&mut h);
    h.finish()
}

/// Another teammate's claim covering the selection (`(holder, path)`), and the teammates
/// looking at it (WP-U10, Ch.37 §37.4 / §37.7).
fn collab_state(
    m: &ProjectMirror,
    sel: &[EntityKey],
    services: &EditorServices,
) -> (Option<(String, String)>, Vec<String>) {
    let c = &services.collab;
    if !c.attached() || sel.is_empty() {
        return (None, Vec::new());
    }
    let me = c.me();
    let claim = sel.iter().find_map(|e| {
        c.claim_over(e.0, |k| {
            m.entity(EntityKey(k)).and_then(|x| x.parent).map(|p| p.0)
        })
        .filter(|cl| cl.holder != me)
        .map(|cl| (cl.holder, cl.label))
    });
    let viewers = c.viewers();
    let mut who: Vec<String> = sel
        .iter()
        .filter_map(|e| viewers.get(&e.0))
        .flatten()
        .cloned()
        .collect();
    who.sort();
    who.dedup();
    (claim, who)
}

/// A float row's value at its step precision, clamped to its range (integer rows go
/// through `int_value`, exactly).
fn number_value(row: &InspectorRow, x: f64) -> Value {
    let x = match row.field.step.filter(|s| *s > 0.0) {
        Some(step) => {
            let base = row.field.min.unwrap_or(0.0);
            let snapped = ((x - base) / step).round() * step + base;
            let decimals = (-step.log10()).ceil().max(0.0) as usize;
            format!("{snapped:.decimals$}").parse().unwrap_or(snapped)
        }
        None => x,
    };
    let x = row.field.min.map_or(x, |m| x.max(m));
    let x = row.field.max.map_or(x, |m| x.min(m));
    Value::Float(x)
}

/// Everything a row editor needs.
#[derive(Clone)]
struct RowCx {
    comp: Rc<ComponentDef>,
    row: InspectorRow,
    carriers: Rc<Vec<EntityKey>>,
}

impl RowCx {
    fn state(&self, m: &ProjectMirror) -> FieldState {
        field_state(m, &self.carriers, &self.comp, &self.row)
    }
    fn commands(&self, m: &ProjectMirror, storage: usize, v: &Value) -> Vec<EditorCommand> {
        match self.row.storage().get(storage) {
            Some(s) => set_commands(m, &self.carriers, &self.comp, s, v),
            None => Vec::new(),
        }
    }
    fn label(&self) -> String {
        forge_ui::trf!(
            "Set {component} {field}",
            component = forge_ui::l10n::tr_str(self.comp.title),
            field = forge_ui::l10n::tr_str(self.row.field.label)
        )
    }
}

/// Build the editor of one row under `line`; returns the widget keyed "edit".
fn row_editor(
    pb: &mut PanelBuilder,
    line: WidgetId,
    rc: &RowCx,
    insp: &Rc<RefCell<Insp>>,
) -> Result<Option<WidgetId>, UiError> {
    let label = rc.row.label();
    let st = rc.state(&pb.mirror());
    let mixed_sig = pb.b.signal(if st.mixed { "mixed" } else { "" }.to_string());
    insp.borrow_mut().signals.push(mixed_sig.any());
    let read_only = rc.row.field.read_only || insp.borrow().locked;
    let kind = rc.row.editor();
    let edit: WidgetId = if read_only && kind != EditorKind::Group {
        let s = pb.b.signal(display_value(&st));
        insp.borrow_mut().signals.push(s.any());
        let id = pb.b.add(
            line,
            "edit",
            NodeStyle::leaf(),
            Label::new(s).kind(LabelKind::Mono),
        )?;
        let rc2 = rc.clone();
        insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
            let st = rc2.state(m);
            s.set(ui.rt_mut(), display_value(&st));
            mixed_sig.set(
                ui.rt_mut(),
                if st.mixed { forge_ui::tr!("mixed") } else { "" }.into(),
            );
        }));
        id
    } else {
        match kind {
            EditorKind::Toggle => {
                let cs = |st: &FieldState| {
                    if st.mixed {
                        CheckState::Mixed
                    } else if st.first() == Some(&Value::Bool(true)) {
                        CheckState::On
                    } else {
                        CheckState::Off
                    }
                };
                let sig = pb.b.signal(cs(&st));
                insp.borrow_mut().signals.push(sig.any());
                let id = pb.b.add(
                    line,
                    "edit",
                    NodeStyle::leaf(),
                    TriCheckbox::new(sig, &label),
                )?;
                let known = Rc::new(Cell::new(cs(&st)));
                let relay = pb.b.add(
                    line,
                    "relay",
                    NodeStyle::leaf(),
                    SignalRelay::new(sig.any()),
                )?;
                let (k, rc2) = (known.clone(), rc.clone());
                pb.on(relay, move |act, _: &SignalChanged| {
                    let v = sig.get(act.ui.rt());
                    if v == k.get() {
                        return;
                    }
                    k.set(v);
                    let on = v == CheckState::On;
                    let cmds = rc2.commands(act.mirror, 0, &Value::Bool(on));
                    act.cmd.emit_all(&rc2.label(), cmds);
                });
                let rc2 = rc.clone();
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    let st = rc2.state(m);
                    let v = cs(&st);
                    if v != known.get() {
                        known.set(v);
                        sig.set(ui.rt_mut(), v);
                    }
                }));
                id
            }
            EditorKind::Integer | EditorKind::Id | EditorKind::Time => {
                // Exact: i128 end to end, bounded by the field's type (a u8 row takes
                // 0..=255, a u64 seed all 64 bits).
                let field = rc.row.field.clone();
                let (lo, hi) = int_range(&field).unwrap_or((i64::MIN.into(), i64::MAX.into()));
                let of = move |f: &forge_reflect::InspectorField, v: Option<&Value>| {
                    v.and_then(|v| int_of_value(f, v)).unwrap_or(0)
                };
                let x0 = of(&field, st.first());
                let sig = pb.b.signal(x0);
                insp.borrow_mut().signals.push(sig.any());
                let mut f = IntegerField::new(sig, &label).range(lo, hi);
                if kind == EditorKind::Time {
                    // Microseconds, shown in seconds with every digit kept.
                    f = f.unit("s").decimals(6);
                } else {
                    if let Some(u) = field.units {
                        f = f.unit(u);
                    }
                    if let Some(s) = field.step.filter(|s| *s >= 1.0) {
                        f = f.step(s.round() as i128);
                    }
                }
                let id = pb.b.add(line, "edit", NodeStyle::leaf().width(160.0), f)?;
                let known = Rc::new(Cell::new(x0));
                let (k, rc2) = (known.clone(), rc.clone());
                // Positive control for the round-trip guard: the old lossy path.
                let lossy = pb.services().faults.inspector_int_through_f64();
                pb.on(id, move |act, e: &IntegerCommitted| {
                    let x = if lossy {
                        e.value as f64 as i128
                    } else {
                        e.value
                    };
                    let v = int_value(&rc2.row.field, x);
                    // Show what is stored (snapped and clamped into the type's range).
                    let shown = int_of_value(&rc2.row.field, &v).unwrap_or(e.value);
                    k.set(shown);
                    sig.set(act.ui.rt_mut(), shown);
                    let cmds = rc2.commands(act.mirror, 0, &v);
                    act.cmd.emit_all(&rc2.label(), cmds);
                });
                let rc2 = rc.clone();
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    let st = rc2.state(m);
                    let x = of(&rc2.row.field, st.first());
                    if x != known.get() {
                        known.set(x);
                        sig.set(ui.rt_mut(), x);
                    }
                    mixed_sig.set(
                        ui.rt_mut(),
                        if st.mixed { forge_ui::tr!("mixed") } else { "" }.into(),
                    );
                }));
                id
            }
            EditorKind::Number => {
                let x0 = f64_of(st.first());
                let sig = pb.b.signal(x0);
                insp.borrow_mut().signals.push(sig.any());
                let mut f = NumericField::new(sig, &label);
                if let Some(u) = rc.row.field.units {
                    f = f.unit(u);
                }
                if let (Some(a), Some(b)) = (rc.row.field.min, rc.row.field.max) {
                    f = f.range(a, b);
                }
                if let Some(s) = rc.row.field.step {
                    f = f.step(s);
                }
                let id = pb.b.add(line, "edit", NodeStyle::leaf().width(160.0), f)?;
                let known = Rc::new(Cell::new(x0));
                let (k, rc2) = (known.clone(), rc.clone());
                pb.on(id, move |act, e: &NumericCommitted| {
                    k.set(e.value);
                    let v = number_value(&rc2.row, e.value);
                    let cmds = rc2.commands(act.mirror, 0, &v);
                    act.cmd.emit_all(&rc2.label(), cmds);
                });
                let rc2 = rc.clone();
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    let st = rc2.state(m);
                    let x = f64_of(st.first());
                    if x.to_bits() != known.get().to_bits() {
                        known.set(x);
                        sig.set(ui.rt_mut(), x);
                    }
                    mixed_sig.set(
                        ui.rt_mut(),
                        if st.mixed { forge_ui::tr!("mixed") } else { "" }.into(),
                    );
                }));
                id
            }
            EditorKind::Slider => {
                let (min, max) = (
                    rc.row.field.min.unwrap_or(0.0),
                    rc.row.field.max.unwrap_or(1.0),
                );
                let step = rc.row.field.step.unwrap_or((max - min) / 100.0);
                let x0 = f64_of(st.first());
                let sig = pb.b.signal(x0 as f32);
                insp.borrow_mut().signals.push(sig.any());
                let id = pb.b.add(
                    line,
                    "edit",
                    NodeStyle::leaf().width(200.0),
                    Slider::new(sig, &label, min as f32, max as f32, step as f32),
                )?;
                let unit = rc.row.field.units.unwrap_or("");
                let readout = pb.b.signal(format!("{x0} {unit}"));
                insp.borrow_mut().signals.push(readout.any());
                pb.b.add(
                    line,
                    "value",
                    NodeStyle::leaf().width(90.0),
                    Label::new(readout),
                )?;
                let gesture: Rc<RefCell<Option<Gesture>>> = Rc::new(RefCell::new(None));
                let known = Rc::new(Cell::new(x0));
                let (k, rc2, g) = (known.clone(), rc.clone(), gesture.clone());
                pb.on(id, move |act, e: &SliderEdit| {
                    let v = number_value(&rc2.row, f64::from(e.value));
                    k.set(f64_of(Some(&v)));
                    readout.set(act.ui.rt_mut(), format!("{} {unit}", f64_of(Some(&v))));
                    let cmds = rc2.commands(act.mirror, 0, &v);
                    match e.phase {
                        SliderPhase::Begin => {
                            let mut gg = act.cmd.gesture(&forge_ui::trf!(
                                "Drag {label}",
                                label = forge_ui::l10n::tr(rc2.row.field.label)
                            ));
                            gg.update_all(cmds);
                            *g.borrow_mut() = Some(gg);
                        }
                        SliderPhase::Update => match g.borrow_mut().as_mut() {
                            Some(gg) => gg.update_all(cmds),
                            None => {
                                act.cmd.emit_all(&rc2.label(), cmds);
                            }
                        },
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
                            act.cmd.emit_all(&rc2.label(), cmds);
                        }
                    }
                });
                let rc2 = rc.clone();
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    if gesture.borrow().is_some() {
                        return; // mid-drag: the thumb is the truth until release
                    }
                    let st = rc2.state(m);
                    let x = f64_of(st.first());
                    if x.to_bits() != known.get().to_bits() {
                        known.set(x);
                        sig.set(ui.rt_mut(), x as f32);
                        readout.set(ui.rt_mut(), format!("{x} {unit}"));
                    }
                    mixed_sig.set(
                        ui.rt_mut(),
                        if st.mixed { forge_ui::tr!("mixed") } else { "" }.into(),
                    );
                }));
                id
            }
            EditorKind::Text => {
                let s0 = text_of(st.first());
                let sig = pb.b.signal(s0.clone());
                insp.borrow_mut().signals.push(sig.any());
                let id = pb.b.add(
                    line,
                    "edit",
                    NodeStyle::leaf().width(220.0),
                    TextField::new(sig, &label),
                )?;
                let known = Rc::new(RefCell::new(s0));
                let (k, rc2) = (known.clone(), rc.clone());
                pb.on(id, move |act, e: &Submitted| {
                    if *k.borrow() == e.text {
                        return;
                    }
                    *k.borrow_mut() = e.text.clone();
                    let cmds = rc2.commands(act.mirror, 0, &Value::Text(e.text.clone()));
                    act.cmd.emit_all(&rc2.label(), cmds);
                });
                let rc2 = rc.clone();
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    let st = rc2.state(m);
                    let t = text_of(st.first());
                    if t != *known.borrow() {
                        *known.borrow_mut() = t.clone();
                        sig.set(ui.rt_mut(), t);
                    }
                    mixed_sig.set(
                        ui.rt_mut(),
                        if st.mixed { forge_ui::tr!("mixed") } else { "" }.into(),
                    );
                }));
                id
            }
            EditorKind::EntityPicker => {
                let (options, keys) = {
                    let m = pb.mirror();
                    let mut keys: Vec<Option<EntityKey>> = vec![None];
                    let mut names = vec![forge_ui::tr!("None").to_string()];
                    for (k, e) in m.entities().take(PICKER_CAP) {
                        keys.push(Some(*k));
                        names.push(format!("{} ({k})", e.name));
                    }
                    if let Some(Value::Entity(cur)) = st.first()
                        && !keys.contains(&Some(*cur))
                    {
                        keys.push(Some(*cur));
                        names.push(
                            m.entity(*cur)
                                .map_or(format!("{cur}"), |e| format!("{} ({cur})", e.name)),
                        );
                    }
                    (names, keys)
                };
                let index_of = {
                    let keys = keys.clone();
                    move |v: Option<&Value>| match v {
                        Some(Value::Entity(e)) => {
                            keys.iter().position(|k| *k == Some(*e)).unwrap_or(0)
                        }
                        _ => 0,
                    }
                };
                let i0 = index_of(st.first());
                let sig = pb.b.signal(i0);
                insp.borrow_mut().signals.push(sig.any());
                let opts: Vec<&str> = options.iter().map(String::as_str).collect();
                let id = pb.b.add(
                    line,
                    "edit",
                    NodeStyle::leaf().width(220.0),
                    ComboBox::new(&label, &opts, sig),
                )?;
                let relay = pb.b.add(
                    line,
                    "relay",
                    NodeStyle::leaf(),
                    SignalRelay::new(sig.any()),
                )?;
                let known = Rc::new(Cell::new(i0));
                let (k, rc2, ks) = (known.clone(), rc.clone(), keys);
                pb.on(relay, move |act, _: &SignalChanged| {
                    let i = sig.get(act.ui.rt());
                    if i == k.get() {
                        return;
                    }
                    k.set(i);
                    let cmds = match ks.get(i).copied().flatten() {
                        Some(e) => rc2.commands(act.mirror, 0, &Value::Entity(e)),
                        None => rc2
                            .carriers
                            .iter()
                            .filter(|c| {
                                act.mirror
                                    .property(**c, &rc2.comp.prop(&rc2.row.path))
                                    .is_some()
                            })
                            .map(|c| EditorCommand::RemoveProperty {
                                entity: *c,
                                path: rc2.comp.prop(&rc2.row.path),
                            })
                            .collect(),
                    };
                    act.cmd.emit_all(&rc2.label(), cmds);
                });
                let rc2 = rc.clone();
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    let st = rc2.state(m);
                    let i = index_of(st.first());
                    if i != known.get() {
                        known.set(i);
                        sig.set(ui.rt_mut(), i);
                    }
                    mixed_sig.set(
                        ui.rt_mut(),
                        if st.mixed { forge_ui::tr!("mixed") } else { "" }.into(),
                    );
                }));
                id
            }
            EditorKind::FramePos => {
                let frame0 = f64_of(st.values.first().and_then(Option::as_ref));
                let local0 = vec3_of(st.values.get(1).and_then(Option::as_ref));
                let edit = pb.b.add(
                    line,
                    "edit",
                    NodeStyle::row(4.0),
                    Container::new(Role::Group).labelled(&label),
                )?;
                let fsig = pb.b.signal(frame0);
                insp.borrow_mut().signals.push(fsig.any());
                let frame = pb.b.add(
                    edit,
                    "frame",
                    NodeStyle::leaf().width(80.0),
                    NumericField::new(fsig, &forge_ui::trf!("{label} frame", label))
                        .range(0.0, f64::from(u32::MAX))
                        .step(1.0)
                        .decimals(0),
                )?;
                let vsig = pb.b.signal(local0);
                insp.borrow_mut().signals.push(vsig.any());
                vector_editor::<3>(pb.b, edit, "local", vsig, &label, rc.row.field.units)?;
                let relay = pb.b.add(
                    edit,
                    "relay",
                    NodeStyle::leaf(),
                    SignalRelay::new(vsig.any()),
                )?;
                let known_f = Rc::new(Cell::new(frame0));
                let known_v = Rc::new(Cell::new(local0));
                let (k, rc2) = (known_f.clone(), rc.clone());
                pb.on(frame, move |act, e: &NumericCommitted| {
                    k.set(e.value);
                    let cmds =
                        rc2.commands(act.mirror, 0, &Value::Int(e.value.round().max(0.0) as i64));
                    act.cmd.emit_all(&rc2.label(), cmds);
                });
                let (k, rc2) = (known_v.clone(), rc.clone());
                pb.on(relay, move |act, _: &SignalChanged| {
                    let v = vsig.get(act.ui.rt());
                    if v == k.get() {
                        return;
                    }
                    k.set(v);
                    let cmds = rc2.commands(act.mirror, 1, &Value::Vec3(v));
                    act.cmd.emit_all(&rc2.label(), cmds);
                });
                let rc2 = rc.clone();
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    let st = rc2.state(m);
                    let f = f64_of(st.values.first().and_then(Option::as_ref));
                    let l = vec3_of(st.values.get(1).and_then(Option::as_ref));
                    if f.to_bits() != known_f.get().to_bits() {
                        known_f.set(f);
                        fsig.set(ui.rt_mut(), f);
                    }
                    if l != known_v.get() {
                        known_v.set(l);
                        vsig.set(ui.rt_mut(), l);
                    }
                    mixed_sig.set(
                        ui.rt_mut(),
                        if st.mixed { forge_ui::tr!("mixed") } else { "" }.into(),
                    );
                }));
                edit
            }
            EditorKind::Variant => {
                let variants: Vec<String> = rc
                    .row
                    .field
                    .variants
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let index_of = {
                    let vs = variants.clone();
                    move |v: Option<&Value>| {
                        let t = text_of(v);
                        vs.iter().position(|n| *n == t).unwrap_or(0)
                    }
                };
                let i0 = index_of(st.first());
                let sig = pb.b.signal(i0);
                insp.borrow_mut().signals.push(sig.any());
                // Shown by their looked-up names; the commands carry the variant's own name.
                let shown: Vec<String> = variants
                    .iter()
                    .map(|v| forge_ui::l10n::tr_str(v).into_owned())
                    .collect();
                let opts: Vec<&str> = shown.iter().map(String::as_str).collect();
                let id = pb.b.add(
                    line,
                    "edit",
                    NodeStyle::leaf().width(200.0),
                    ComboBox::new(&label, &opts, sig),
                )?;
                let relay = pb.b.add(
                    line,
                    "relay",
                    NodeStyle::leaf(),
                    SignalRelay::new(sig.any()),
                )?;
                let known = Rc::new(Cell::new(i0));
                let (k, rc2) = (known.clone(), rc.clone());
                pb.on(relay, move |act, _: &SignalChanged| {
                    let i = sig.get(act.ui.rt());
                    if i == k.get() {
                        return;
                    }
                    k.set(i);
                    let Some(name) = variants.get(i) else { return };
                    let mut cmds = Vec::new();
                    for c in rc2.carriers.iter() {
                        let Some(me) = act.mirror.entity(*c) else {
                            continue;
                        };
                        if rc2.comp.variant_on(me, &rc2.row.path).as_deref() == Some(name.as_str())
                        {
                            continue;
                        }
                        cmds.extend(rc2.comp.variant_commands(*c, me, &rc2.row.path, name));
                        cmds.extend(promote_command(*c, me));
                    }
                    act.cmd.emit_all(
                        &forge_ui::trf!(
                            "Make {label} {name}",
                            label = forge_ui::l10n::tr(rc2.row.field.label),
                            name = forge_ui::l10n::tr_str(name)
                        ),
                        cmds,
                    );
                });
                let rc2 = rc.clone();
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    let st = rc2.state(m);
                    let i = index_of(st.first());
                    if i != known.get() {
                        known.set(i);
                        sig.set(ui.rt_mut(), i);
                    }
                    mixed_sig.set(
                        ui.rt_mut(),
                        if st.mixed { forge_ui::tr!("mixed") } else { "" }.into(),
                    );
                }));
                id
            }
            EditorKind::Group => pb.b.add(
                line,
                "edit",
                NodeStyle::leaf(),
                Label::new(rc.row.field.type_path.rsplit("::").next().unwrap_or(""))
                    .kind(LabelKind::Muted),
            )?,
        }
    };
    pb.b.add(
        line,
        "mixed",
        NodeStyle::leaf(),
        Label::new(mixed_sig).kind(LabelKind::Small),
    )?;
    Ok(Some(edit))
}

fn display_value(st: &FieldState) -> String {
    if st.mixed {
        return "mixed".into();
    }
    st.values
        .iter()
        .map(|v| {
            v.as_ref()
                .map_or(forge_ui::tr!("None").to_string(), ToString::to_string)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One component's group.
fn build_component(
    pb: &mut PanelBuilder,
    services: &EditorServices,
    comp: Rc<ComponentDef>,
    carriers: Rc<Vec<EntityKey>>,
    cache: &OverrideCache,
    insp: &Rc<RefCell<Insp>>,
) -> Result<(), UiError> {
    let space = pb.b.theme_ref().space;
    let group = pb.b.add(
        pb.parent,
        Key::Str(format!("comp:{}", comp.key).into()),
        NodeStyle::column(space[1]).padding(space[1]),
        Container::new(Role::Group).labelled(forge_ui::l10n::tr(comp.title)),
    )?;
    let head = pb.b.add(
        group,
        "head",
        NodeStyle::row(space[2]),
        Container::new(Role::Group).labelled(forge_ui::l10n::tr(comp.title)),
    )?;
    pb.b.add(
        head,
        "title",
        NodeStyle::leaf().grow(1.0),
        Label::new(forge_ui::l10n::tr(comp.title))
            .kind(LabelKind::Heading)
            .tooltip(forge_ui::l10n::tr(comp.doc)),
    )?;
    let remove = pb.b.add(
        head,
        "remove",
        NodeStyle::leaf(),
        Button::new(forge_ui::tr!("Remove")),
    )?;
    let (c, cs) = (comp.clone(), carriers.clone());
    pb.on(remove, move |act, _: &Pressed| {
        let mut cmds = Vec::new();
        for e in cs.iter() {
            if let Some(me) = act.mirror.entity(*e) {
                cmds.extend(c.remove_commands(*e, me));
            }
        }
        act.cmd.emit_all(
            &forge_ui::trf!("Remove {title}", title = forge_ui::l10n::tr(c.title)),
            cmds,
        );
    });
    if let Some(desc) = component_widget(&services.inspector_widgets, &comp) {
        let mut icx = InspectorCx::new(&comp.key, &comp.type_path, None, carriers.to_vec());
        (desc.build)(&mut icx);
        let parent = pb.parent;
        pb.parent = group;
        for step in icx.take_steps() {
            step(pb)?;
        }
        pb.parent = parent;
        return Ok(());
    }
    let skip = services.faults.inspector_skip_kind();
    let mut last_cat: Option<&'static str> = None;
    let rows = comp.rows.clone();
    for row in rows {
        // A row inside a data variant shows only where every carrier holds that variant.
        let active = {
            let m = pb.mirror();
            carriers
                .iter()
                .filter_map(|e| m.entity(*e))
                .all(|me| comp.row_active(&row, me))
        };
        if !active {
            continue;
        }
        if row.depth == 0 && row.field.category != last_cat {
            last_cat = row.field.category;
            if let Some(cat) = last_cat {
                pb.b.add(
                    group,
                    Key::Str(format!("cat:{cat}").into()),
                    NodeStyle::leaf(),
                    Label::new(forge_ui::l10n::tr(cat)).kind(LabelKind::Small),
                )?;
            }
        }
        let line = pb.b.add(
            group,
            Key::Str(format!("row:{}", row.path).into()),
            NodeStyle::row(space[2])
                .padding(space[1])
                .indent(row.depth as f32 * 16.0),
            Container::new(Role::Group).labelled(forge_ui::l10n::tr(row.field.label)),
        )?;
        pb.b.add(
            line,
            "label",
            NodeStyle::leaf().width(150.0),
            Label::new(row.label()).tooltip(forge_ui::l10n::tr(row.field.tooltip)),
        )?;
        let props: Vec<String> = row.storage().iter().map(|s| comp.prop(s)).collect();
        row_mark(pb, line, props, cache, insp)?;
        if skip == Some(row.field.kind) {
            continue; // W2 positive control: this kind gets no editor
        }
        if let Some(desc) = row_widget(&services.inspector_widgets, &row) {
            let mut icx = InspectorCx::new(
                &comp.key,
                &comp.type_path,
                Some(row.clone()),
                carriers.to_vec(),
            );
            (desc.build)(&mut icx);
            let parent = pb.parent;
            pb.parent = line;
            for step in icx.take_steps() {
                step(pb)?;
            }
            pb.parent = parent;
            continue;
        }
        let rc = RowCx {
            comp: comp.clone(),
            row: row.clone(),
            carriers: carriers.clone(),
        };
        row_editor(pb, line, &rc, insp)?;
    }
    Ok(())
}

/// Properties no component claims: generic editors by value type.
fn build_raw(
    pb: &mut PanelBuilder,
    sel: &[EntityKey],
    cache: &OverrideCache,
    insp: &Rc<RefCell<Insp>>,
) -> Result<(), UiError> {
    let services = pb.services();
    let paths: Vec<(String, Value)> = {
        let m = pb.mirror();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for e in sel {
            if let Some(me) = m.entity(*e) {
                for (k, v) in services.components.unclaimed(me) {
                    // `scene.*` is composition bookkeeping (WP-U20): shown as the Scene
                    // group, never as raw rows.
                    if k.starts_with("gen.") || forge_scene_meta(k) || !seen.insert(k.clone()) {
                        continue;
                    }
                    out.push((k.clone(), v.clone()));
                }
            }
        }
        out
    };
    if paths.is_empty() {
        return Ok(());
    }
    let space = pb.b.theme_ref().space;
    let group = pb.b.add(
        pb.parent,
        "raw",
        NodeStyle::column(space[1]).padding(space[1]),
        Container::new(Role::Group).labelled(forge_ui::tr!("Other properties")),
    )?;
    pb.b.add(
        group,
        "title",
        NodeStyle::leaf(),
        Label::new(forge_ui::tr!("Other properties")).kind(LabelKind::Heading),
    )?;
    let carriers = Rc::new(sel.to_vec());
    for (path, v0) in paths {
        let line = pb.b.add(
            group,
            Key::Str(format!("row:{path}").into()),
            NodeStyle::row(space[2]).padding(space[1]),
            Container::new(Role::Group).labelled(&path),
        )?;
        pb.b.add(
            line,
            "label",
            NodeStyle::leaf().width(150.0),
            Label::new(path.as_str()),
        )?;
        row_mark(pb, line, vec![path.clone()], cache, insp)?;
        let cmds_for = {
            let (p, cs) = (path.clone(), carriers.clone());
            move |m: &ProjectMirror, v: Value| -> Vec<EditorCommand> {
                cs.iter()
                    .filter(|e| m.property(**e, &p).is_some())
                    .map(|e| EditorCommand::SetProperty {
                        entity: *e,
                        path: p.clone(),
                        value: v.clone(),
                    })
                    .collect()
            }
        };
        let label = forge_ui::trf!("Set {path}", path);
        match v0 {
            Value::Bool(b) => {
                let sig = pb.b.signal(b);
                insp.borrow_mut().signals.push(sig.any());
                pb.b.add(line, "edit", NodeStyle::leaf(), Checkbox::new(sig, &path))?;
                let relay = pb.b.add(
                    line,
                    "relay",
                    NodeStyle::leaf(),
                    SignalRelay::new(sig.any()),
                )?;
                let known = Rc::new(Cell::new(b));
                let k = known.clone();
                pb.on(relay, move |act, _: &SignalChanged| {
                    let v = sig.get(act.ui.rt());
                    if v != k.get() {
                        k.set(v);
                        act.cmd
                            .emit_all(&label, cmds_for(act.mirror, Value::Bool(v)));
                    }
                });
                let (p, first) = (path.clone(), carriers.first().copied());
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    if let Some(Value::Bool(v)) = first.and_then(|e| m.property(e, &p))
                        && *v != known.get()
                    {
                        known.set(*v);
                        sig.set(ui.rt_mut(), *v);
                    }
                }));
            }
            Value::Int(i0) => {
                // Exact over the whole i64 range (no f64 on the way).
                let sig = pb.b.signal(i128::from(i0));
                insp.borrow_mut().signals.push(sig.any());
                let f = IntegerField::new(sig, &path);
                let id = pb.b.add(line, "edit", NodeStyle::leaf().width(160.0), f)?;
                pb.on(id, move |act, e: &IntegerCommitted| {
                    let v = i64::try_from(e.value).map_or(Value::Int(i0), Value::Int);
                    act.cmd.emit_all(&label, cmds_for(act.mirror, v));
                });
                let (p, first) = (path.clone(), carriers.first().copied());
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    if let Some(Value::Int(x)) = first.and_then(|e| m.property(e, &p)) {
                        sig.set(ui.rt_mut(), i128::from(*x));
                    }
                }));
            }
            Value::Float(x0) => {
                let sig = pb.b.signal(x0);
                insp.borrow_mut().signals.push(sig.any());
                let f = NumericField::new(sig, &path);
                let id = pb.b.add(line, "edit", NodeStyle::leaf().width(160.0), f)?;
                pb.on(id, move |act, e: &NumericCommitted| {
                    act.cmd
                        .emit_all(&label, cmds_for(act.mirror, Value::Float(e.value)));
                });
                let (p, first) = (path.clone(), carriers.first().copied());
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    let x = f64_of(first.and_then(|e| m.property(e, &p)));
                    sig.set(ui.rt_mut(), x);
                }));
            }
            Value::Text(t) => {
                let sig = pb.b.signal(t);
                insp.borrow_mut().signals.push(sig.any());
                let id = pb.b.add(
                    line,
                    "edit",
                    NodeStyle::leaf().width(220.0),
                    TextField::new(sig, &path),
                )?;
                pb.on(id, move |act, e: &Submitted| {
                    act.cmd
                        .emit_all(&label, cmds_for(act.mirror, Value::Text(e.text.clone())));
                });
                let (p, first) = (path.clone(), carriers.first().copied());
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    sig.set(ui.rt_mut(), text_of(first.and_then(|e| m.property(e, &p))));
                }));
            }
            Value::Vec3(a) => {
                let sig = pb.b.signal(a);
                insp.borrow_mut().signals.push(sig.any());
                vector_editor::<3>(pb.b, line, "edit", sig, &path, None)?;
                let relay = pb.b.add(
                    line,
                    "relay",
                    NodeStyle::leaf(),
                    SignalRelay::new(sig.any()),
                )?;
                let known = Rc::new(Cell::new(a));
                let k = known.clone();
                pb.on(relay, move |act, _: &SignalChanged| {
                    let v = sig.get(act.ui.rt());
                    if v != k.get() {
                        k.set(v);
                        act.cmd
                            .emit_all(&label, cmds_for(act.mirror, Value::Vec3(v)));
                    }
                });
                let (p, first) = (path.clone(), carriers.first().copied());
                insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
                    let v = vec3_of(first.and_then(|e| m.property(e, &p)));
                    if v != known.get() {
                        known.set(v);
                        sig.set(ui.rt_mut(), v);
                    }
                }));
            }
            Value::Entity(e) => {
                pb.b.add(
                    line,
                    "edit",
                    NodeStyle::leaf(),
                    Label::new(format!("{e}")).kind(LabelKind::Mono),
                )?;
            }
        }
    }
    Ok(())
}

/// Is `path` scene-composition bookkeeping (`scene.*`)?
fn forge_scene_meta(path: &str) -> bool {
    path == "scene" || path.starts_with("scene.")
}

/// The overridden paths of `e` as of the mirror's change `seq`, shared by every row's
/// refresher so a change costs one diff, not one per row.
type OverrideCache = Rc<RefCell<Option<(u64, BTreeSet<String>)>>>;

fn overridden_paths(cache: &OverrideCache, m: &ProjectMirror, e: EntityKey) -> BTreeSet<String> {
    let seq = m.change_seq();
    if let Some((s, paths)) = &*cache.borrow()
        && *s == seq
    {
        return paths.clone();
    }
    let paths: BTreeSet<String> = forge_editor::composition::link(m, e)
        .map(|l| l.overrides.paths().map(str::to_string).collect())
        .unwrap_or_default();
    *cache.borrow_mut() = Some((seq, paths.clone()));
    paths
}

/// A row's inherited / overridden mark and its Revert (WP-U20), for a linked entity. The
/// mark updates as the row's value does, without rebuilding the row.
fn row_mark(
    pb: &mut PanelBuilder,
    line: WidgetId,
    paths: Vec<String>,
    cache: &OverrideCache,
    insp: &Rc<RefCell<Insp>>,
) -> Result<(), UiError> {
    let (Some(e), locked) = ({
        let i = insp.borrow();
        (i.linked, i.locked)
    }) else {
        return Ok(());
    };
    if paths.is_empty() {
        return Ok(());
    }
    let word = |over: bool| -> String {
        if over {
            forge_ui::tr!("\u{25cf} overridden").to_string()
        } else {
            forge_ui::tr!("\u{25cb} inherited").to_string()
        }
    };
    let over0 = {
        let m = pb.mirror();
        let o = overridden_paths(cache, &m, e);
        paths.iter().any(|p| o.contains(p))
    };
    let sig = pb.b.signal(word(over0));
    insp.borrow_mut().signals.push(sig.any());
    pb.b.add(
        line,
        "inherit",
        NodeStyle::leaf(),
        Label::new(sig).kind(LabelKind::Small),
    )?;
    let revert = if locked {
        None
    } else {
        let id = pb.b.add(
            line,
            "revert",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Revert")),
        )?;
        pb.b.hide(id, !over0);
        let (ps, c) = (paths.clone(), Rc::clone(cache));
        pb.on(id, move |act, _: &Pressed| {
            let o = overridden_paths(&c, act.mirror, e);
            let cmds: Vec<EditorCommand> = ps
                .iter()
                .filter(|p| o.contains(*p))
                .map(|p| forge_editor::composition::revert_command(e, Some(p)))
                .collect();
            act.cmd.emit_all(forge_ui::tr!("Revert to base"), cmds);
        });
        Some(id)
    };
    let c = Rc::clone(cache);
    let shown = Cell::new(over0);
    insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
        let o = overridden_paths(&c, m, e);
        let over = paths.iter().any(|p| o.contains(p));
        if over != shown.get() {
            shown.set(over);
            sig.set(ui.rt_mut(), word(over));
            if let Some(r) = revert {
                let _ = ui.set_hidden(r, !over);
            }
        }
    }));
    Ok(())
}

/// The Scene group (WP-U20): what the selected entity is in scene composition, and its
/// actions — Open base scene, Revert node to base, Make Local (naming what it loses), and
/// for a scene: open it, place it, derive from it. A plain entity can be saved as a scene.
/// Every change is a command (I7).
fn build_scene(
    pb: &mut PanelBuilder,
    sel: &[EntityKey],
    cache: &OverrideCache,
    insp: &Rc<RefCell<Insp>>,
) -> Result<(), UiError> {
    use forge_editor::composition::{self as comp, SceneRole};
    {
        let mut i = insp.borrow_mut();
        i.linked = None;
        i.base = None;
    }
    let [e] = sel else {
        return Ok(());
    };
    let e = *e;
    let locked = insp.borrow().locked;
    let space = pb.b.theme_ref().space;
    let (link, loss, users, in_lib) = {
        let m = pb.mirror();
        let link = comp::link(&m, e);
        let users = match &link {
            Some(l) if matches!(l.role, SceneRole::SceneRoot { .. }) => comp::users(&m, e),
            _ => 0,
        };
        (
            link,
            comp::make_local_loss(&m, e),
            users,
            comp::in_library(&m, e),
        )
    };
    let Some(link) = link else {
        // A plain entity in the world can become a scene (an instance takes its place).
        if in_lib || locked {
            return Ok(());
        }
        let row = pb.b.add(
            pb.parent,
            "scene",
            NodeStyle::row(space[1]).padding(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Scene")),
        )?;
        let pack = pb.b.add(
            row,
            "pack",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Save as scene")),
        )?;
        pb.on(pack, move |act, _: &Pressed| {
            act.cmd.emit(comp::pack_command(e));
        });
        return Ok(());
    };
    if link.inherits() {
        let mut i = insp.borrow_mut();
        i.linked = Some(e);
        i.base = link.base;
    }
    let group = pb.b.add(
        pb.parent,
        "scene",
        NodeStyle::column(space[1]).padding(space[1]),
        Container::new(Role::Group).labelled(forge_ui::tr!("Scene")),
    )?;
    let scene = link.scene.clone();
    // l10n-block: each arm is a whole trf!/tr! key
    let text = match (&link.role, link.missing) {
        (_, true) => forge_ui::trf!(
            "\u{26a0} Its base scene \u{201c}{scene}\u{201d} is missing: it keeps the values it has.",
            scene
        ),
        (SceneRole::Library, _) => forge_ui::tr!(
            "The scene library. Each scene in it can be placed anywhere; the world shows its instances, not the scenes."
        )
        .to_string(),
        (SceneRole::SceneRoot { inherits: None, .. }, _) => forge_ui::trf!(
            "The scene \u{201c}{scene}\u{201d}, placed {users} time(s).",
            scene,
            users
        ),
        (SceneRole::SceneRoot { inherits: Some(_), .. }, _) => forge_ui::trf!(
            "A scene derived from \u{201c}{scene}\u{201d}: it keeps only what it changes and adds, and follows the rest.",
            scene
        ),
        (SceneRole::InstanceRoot { .. }, _) => forge_ui::trf!(
            "An instance of the scene \u{201c}{scene}\u{201d}: what you change here overrides it; everything else follows the scene.",
            scene
        ),
        (SceneRole::Inherited, _) => forge_ui::trf!(
            "Part of an instance of \u{201c}{scene}\u{201d}: it follows the scene except where overridden.",
            scene
        ),
        (SceneRole::Plain, _) => String::new(),
    };
    pb.b.add(
        group,
        "what",
        NodeStyle::leaf(),
        Label::new(text).wrapping(),
    )?;
    let bar = pb.b.add(
        group,
        "actions",
        NodeStyle::row(space[1]).wrap(),
        Container::new(Role::Toolbar).labelled(forge_ui::tr!("Scene actions")),
    )?;
    if let Some(root) = link.base_scene {
        let base = link.base;
        let open = pb.b.add(
            bar,
            "open_base",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Open base scene")),
        )?;
        pb.on(open, move |act, _: &Pressed| {
            act.session.open_scene(Some(root));
            if let Some(b) = base {
                act.session.set_selection(vec![b]);
            }
        });
    }
    if let SceneRole::SceneRoot { id, .. } = &link.role {
        let open = pb.b.add(
            bar,
            "open_scene",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Open scene")),
        )?;
        pb.on(open, move |act, _: &Pressed| {
            act.session.open_scene(Some(e))
        });
        if !locked {
            let place = pb.b.add(
                bar,
                "place",
                NodeStyle::leaf(),
                Button::new(forge_ui::tr!("Place in the world")),
            )?;
            let sid = id.clone();
            pb.on(place, move |act, _: &Pressed| {
                act.cmd.emit(comp::instance_command(&sid, None));
            });
            let derive = pb.b.add(
                bar,
                "inherit",
                NodeStyle::leaf(),
                Button::new(forge_ui::tr!("New inherited scene")),
            )?;
            let sid = id.clone();
            pb.on(derive, move |act, _: &Pressed| {
                act.cmd.emit(comp::new_inherited_command(&sid));
            });
        }
    }
    if matches!(link.role, SceneRole::Library) && !locked {
        let new = pb.b.add(
            bar,
            "new_scene",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New scene")),
        )?;
        pb.on(new, |act, _: &Pressed| {
            act.cmd
                .emit(comp::new_scene_command(forge_ui::tr!("Scene")));
        });
    }
    if link.inherits() && !locked {
        let over0 = !overridden_paths(cache, &pb.mirror(), e).is_empty();
        let revert = pb.b.add(
            bar,
            "revert_node",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Revert node to base")),
        )?;
        pb.b.hide(revert, !over0);
        pb.on(revert, move |act, _: &Pressed| {
            act.cmd.emit(comp::revert_command(e, None));
        });
        let c = Rc::clone(cache);
        insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
            let _ = ui.set_hidden(revert, overridden_paths(&c, m, e).is_empty());
        }));
    }
    if let Some(loss) = loss.filter(|_| !locked) {
        pb.b.add(
            group,
            "loss",
            NodeStyle::leaf(),
            Label::new(forge_ui::trf!(
                "\u{26a0} Make Local keeps every value but stops following \u{201c}{scene}\u{201d}: {nodes} node(s) become this project's own ({overridden} with overrides); {nested} scene(s) nested in it stay linked. Ctrl+Z undoes it.",
                scene = loss.scene,
                nodes = loss.nodes,
                overridden = loss.overridden,
                nested = loss.nested
            ))
            .kind(LabelKind::Warning)
            .wrapping(),
        )?;
        let local = pb.b.add(
            bar,
            "make_local",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Make local")),
        )?;
        pb.on(local, move |act, _: &Pressed| {
            act.cmd.emit(comp::make_local_command(e));
        });
    }
    Ok(())
}

/// (Re)build everything under the content container for the selection.
fn build_content(pb: &mut PanelBuilder, insp: &Rc<RefCell<Insp>>) -> Result<(), UiError> {
    let services = pb.services();
    let space = pb.b.theme_ref().space;
    let sel: Vec<EntityKey> = {
        let m = pb.mirror();
        pb.session()
            .selection
            .iter()
            .copied()
            .filter(|e| m.entity(*e).is_some())
            .collect()
    };
    if sel.is_empty() {
        pb.b.add(
            pb.parent,
            "none",
            NodeStyle::leaf().padding(space[2]),
            forge_ui::widgets::EmptyState::new(forge_ui::tr!(
                "Select an entity in the hierarchy to inspect it."
            )),
        )?;
        return Ok(());
    }
    // Teammates (WP-U10): a claim held by someone else makes the selection read-only here,
    // and the core refuses edits to it anyway; who else is looking at it.
    let (claim, viewers) = {
        let m = pb.mirror();
        collab_state(&m, &sel, &services)
    };
    let locked = claim.is_some();
    insp.borrow_mut().locked = locked;
    if claim.is_some() || !viewers.is_empty() {
        let team = pb.b.add(
            pb.parent,
            "collab",
            NodeStyle::column(space[1]).padding(space[1]),
            Container::new(Role::Group).labelled(forge_ui::tr!("Teammates")),
        )?;
        if let Some((holder, path)) = &claim {
            pb.b.add(
                team,
                "claimed",
                NodeStyle::leaf(),
                Label::new(forge_ui::trf!("Claimed by {holder} (\u{201c}{path}\u{201d}): read-only for you until they release it.", holder, path))
                .kind(LabelKind::Warning)
                .wrapping(),
            )?;
        }
        if !viewers.is_empty() {
            pb.b.add(
                team,
                "here",
                NodeStyle::leaf(),
                Label::new(forge_ui::trf!(
                    "\u{25cf} Also here: {items}",
                    items = viewers.join(", ")
                ))
                .kind(LabelKind::Muted)
                .wrapping(),
            )?;
        }
    }
    // Header: the name (one entity: editable) or the count.
    let head = pb.b.add(
        pb.parent,
        "head",
        NodeStyle::row(space[2]).padding(space[1]),
        Container::new(Role::Group).labelled(forge_ui::tr!("Selection")),
    )?;
    if sel.len() == 1 && locked {
        let name = pb
            .mirror()
            .entity(sel[0])
            .map(|e| e.name.clone())
            .unwrap_or_default();
        pb.b.add(
            head,
            "name",
            NodeStyle::leaf(),
            Label::new(name).kind(LabelKind::Heading),
        )?;
    } else if sel.len() == 1 {
        let (name, entity) = {
            let m = pb.mirror();
            (
                m.entity(sel[0]).map(|e| e.name.clone()).unwrap_or_default(),
                sel[0],
            )
        };
        let sig = pb.b.signal(name.clone());
        insp.borrow_mut().signals.push(sig.any());
        let field = pb.b.add(
            head,
            "name",
            NodeStyle::leaf().width(240.0),
            TextField::new(sig, forge_ui::tr!("Name")),
        )?;
        pb.on(field, move |act, e: &Submitted| {
            let n = e.text.trim();
            if !n.is_empty() && act.mirror.entity(entity).is_some_and(|me| me.name != n) {
                act.cmd.emit(EditorCommand::Rename {
                    entity,
                    name: n.to_string(),
                });
            }
        });
        insp.borrow_mut().refreshers.push(Box::new(move |ui, m| {
            if let Some(me) = m.entity(entity) {
                sig.set(ui.rt_mut(), me.name.clone());
            }
        }));
    } else {
        pb.b.add(
            head,
            "count",
            NodeStyle::leaf(),
            Label::new(forge_ui::trf!(
                "{sel_count} entities selected",
                sel_count = sel.len()
            ))
            .kind(LabelKind::Heading),
        )?;
    }
    // Scene composition (WP-U20): what it is, what it follows, and its actions.
    let cache: OverrideCache = Rc::new(RefCell::new(None));
    build_scene(pb, &sel, &cache, insp)?;
    // Generated content: seed path, reveal, promote.
    let generated: Vec<(EntityKey, String, bool)> = {
        let m = pb.mirror();
        sel.iter()
            .filter_map(|e| {
                let me = m.entity(*e)?;
                let Some(Value::Text(p)) = me.properties.get(GEN_SEED_PATH) else {
                    return None;
                };
                Some((
                    *e,
                    p.clone(),
                    me.properties.get(GEN_PROMOTED) == Some(&Value::Bool(true)),
                ))
            })
            .collect()
    };
    if let Some((_, path, _)) = generated.first() {
        let unpromoted = generated.iter().filter(|g| !g.2).count();
        let gen_box = pb.b.add(
            pb.parent,
            "gen",
            NodeStyle::column(space[1]).padding(space[1]),
            Container::new(Role::Group).labelled(forge_ui::tr!("Generated content")),
        )?;
        let text = if unpromoted == 0 {
            forge_ui::trf!(
                "Generated from {path} \u{2014} promoted to authored content.",
                path
            )
        } else {
            forge_ui::trf!(
                "Generated from {path}. Editing it promotes it to authored content (it stops following its generator).",
                path
            )
        };
        pb.b.add(
            gen_box,
            "text",
            NodeStyle::leaf(),
            Label::new(text).wrapping(),
        )?;
        let row = pb.b.add(
            gen_box,
            "actions",
            NodeStyle::row(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Generated content actions")),
        )?;
        // Only a panel a plugin declared can show a seed path: without one there is no
        // button that would open nothing.
        if let Some(target) = SeedPathPanel::of(&pb.services()) {
            let reveal = pb.b.add(
                row,
                "reveal",
                NodeStyle::leaf(),
                Button::new(forge_ui::trf!("Show in {title}", title = target.title)),
            )?;
            let p = path.clone();
            pb.on(reveal, move |act, _: &Pressed| {
                target.reveal(act.session, p.clone());
            });
        }
        if unpromoted > 0 && !locked {
            let promote = pb.b.add(
                row,
                "promote",
                NodeStyle::leaf(),
                Button::new(forge_ui::tr!("Promote")),
            )?;
            let ents: Vec<EntityKey> = generated.iter().map(|g| g.0).collect();
            pb.on(promote, move |act, _: &Pressed| {
                let cmds: Vec<EditorCommand> = ents
                    .iter()
                    .filter_map(|e| act.mirror.entity(*e).and_then(|me| promote_command(*e, me)))
                    .collect();
                act.cmd
                    .emit_all(forge_ui::tr!("Promote generated content"), cmds);
            });
        }
    }
    // Components, in key order; a component shows if any selected entity carries it.
    let comps: Vec<(Rc<ComponentDef>, Rc<Vec<EntityKey>>)> = {
        let m = pb.mirror();
        services
            .components
            .components()
            .filter_map(|c| {
                let carriers: Vec<EntityKey> = sel
                    .iter()
                    .copied()
                    .filter(|e| m.entity(*e).is_some_and(|me| c.present_on(me)))
                    .collect();
                (!carriers.is_empty()).then(|| (Rc::new(c.clone()), Rc::new(carriers)))
            })
            .collect()
    };
    for (c, carriers) in comps {
        build_component(pb, &services, c, carriers, &cache, insp)?;
    }
    build_raw(pb, &sel, &cache, insp)?;
    // Add component: every component not on every selected entity.
    let addable: Vec<(String, String)> = {
        let m = pb.mirror();
        let mut v: Vec<(String, String, bool)> = services
            .components
            .components()
            .filter(|c| {
                sel.iter()
                    .any(|e| m.entity(*e).is_some_and(|me| !c.present_on(me)))
            })
            .map(|c| {
                (
                    c.key.clone(),
                    forge_ui::l10n::tr(c.title).to_string(),
                    c.type_path.starts_with("forge_2d::"),
                )
            })
            .collect();
        // The 2D preset's inspector (Ch.35 §35.2): in a 2D project (`render.path` is "2d",
        // the 2D preset's default) the 2D pipeline's components are offered first. An
        // order, never a filter: every component stays addable under every preset (I15).
        if matches!(m.setting("render.path"), Some(Value::Text(t)) if t == "2d") {
            v.sort_by_key(|(_, _, two_d)| !*two_d);
        }
        v.into_iter().map(|(k, t, _)| (k, t)).collect()
    };
    if !addable.is_empty() && !locked {
        let foot = pb.b.add(
            pb.parent,
            "add",
            NodeStyle::row(space[1]).padding(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Add component")),
        )?;
        let choice = pb.b.signal(0usize);
        insp.borrow_mut().signals.push(choice.any());
        let titles: Vec<&str> = addable.iter().map(|(_, t)| t.as_str()).collect();
        pb.b.add(
            foot,
            "kind",
            NodeStyle::leaf().width(180.0),
            ComboBox::new(forge_ui::tr!("Component to add"), &titles, choice),
        )?;
        let add = pb.b.add(
            foot,
            "button",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add component")),
        )?;
        let sel2 = sel.clone();
        let services2 = services.clone();
        pb.on(add, move |act, _: &Pressed| {
            let i = choice.get(act.ui.rt());
            let Some((key, title)) = addable.get(i) else {
                return;
            };
            let Some(c) = services2.components.get(key) else {
                return;
            };
            let mut cmds = Vec::new();
            for e in &sel2 {
                if let Some(me) = act.mirror.entity(*e)
                    && !c.present_on(me)
                {
                    cmds.extend(c.add_commands(*e));
                    cmds.extend(promote_command(*e, me));
                }
            }
            act.cmd
                .emit_all(&forge_ui::trf!("Add {title}", title), cmds);
        });
    }
    Ok(())
}

fn rebuild(ui: &mut Ui, handles: &ShellHandles, insp: &Rc<RefCell<Insp>>) -> Result<(), UiError> {
    let (content, window, old_signals) = {
        let mut i = insp.borrow_mut();
        i.refreshers.clear();
        i.rebuilds += 1;
        (i.content, i.window, std::mem::take(&mut i.signals))
    };
    for c in ui.children(content) {
        ui.remove(c)?;
    }
    for s in old_signals {
        ui.rt_mut().dispose(s);
    }
    let mut pb = handles.builder(ui, content, window);
    build_content(&mut pb, insp)
}

/// Does any logged change since `seen` touch the selection? (`true` when unknown.)
fn touches(m: &ProjectMirror, seen: u64, sel: &[EntityKey]) -> bool {
    use forge_editor::mirror::MirrorChange;
    let Some(changes) = m.changes_since(seen) else {
        return true;
    };
    let set: HashSet<&EntityKey> = sel.iter().collect();
    let mut any = false;
    for c in changes {
        let e = match c {
            MirrorChange::Created(e) | MirrorChange::Renamed(e) | MirrorChange::Property(e, _) => e,
            MirrorChange::Removed { entity, .. } | MirrorChange::Reparented { entity, .. } => {
                entity
            }
        };
        if set.contains(e) {
            any = true;
            break;
        }
    }
    any
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let content = pb.b.add(
            pb.parent,
            "content",
            NodeStyle::column(space[1])
                .padding(space[1])
                .grow(1.0)
                .scrollable(),
            Container::new(Role::Group).labelled(forge_ui::tr!("Inspector")),
        )?;
        let (selection, shape, seen) = {
            let m = pb.mirror();
            let s = pb.session();
            let sel = s.selection.clone();
            (
                sel.clone(),
                shape_of(&m, &sel, &pb.services()),
                m.change_seq(),
            )
        };
        let insp = Rc::new(RefCell::new(Insp {
            content,
            window: pb.window(),
            shape,
            seen,
            selection,
            refreshers: Vec::new(),
            signals: Vec::new(),
            rebuilds: 0,
            locked: false,
            linked: None,
            base: None,
        }));
        let parent = pb.parent;
        pb.parent = content;
        build_content(pb, &insp)?;
        pb.parent = parent;
        let handles = pb.handles();
        // Teammates (WP-U10): claims and presence change off the bus; a live feed (≤ 10 Hz,
        // only while visible) says when to look again.
        let relay = pb.b.add(
            pb.parent,
            "collab_feed",
            NodeStyle::leaf(),
            forge_editor::feed::FeedRelay::new(),
        )?;
        pb.on(relay, |act, _: &forge_editor::feed::FeedTicked| {
            act.want_turn()
        });
        let mut feed = false;
        pb.sync(content, move |s| {
            if !feed && s.services.collab.attached() {
                feed = true;
                s.ui.add_feed(
                    relay,
                    forge_ui::LiveFeed {
                        source: s.services.collab.feed(),
                        max_hz: crate::hierarchy::COLLAB_HZ,
                        self_ui: false,
                    },
                );
            }
            let sel: Vec<EntityKey> = s.session.selection.clone();
            let shape = shape_of(s.mirror, &sel, s.services);
            let (old_shape, seen) = {
                let i = insp.borrow();
                (i.shape, i.seen)
            };
            let seq = s.mirror.change_seq();
            if shape != old_shape || sel != insp.borrow().selection {
                {
                    let mut i = insp.borrow_mut();
                    i.shape = shape;
                    i.selection = sel;
                    i.seen = seq;
                }
                return rebuild(s.ui, &handles, &insp);
            }
            if seq != seen {
                let mut watched = sel.clone();
                watched.extend(insp.borrow().base);
                let hit = touches(s.mirror, seen, &watched);
                insp.borrow_mut().seen = seq;
                if hit {
                    let mut refreshers = std::mem::take(&mut insp.borrow_mut().refreshers);
                    for r in &mut refreshers {
                        r(s.ui, s.mirror);
                    }
                    let mut i = insp.borrow_mut();
                    refreshers.append(&mut i.refreshers);
                    i.refreshers = refreshers;
                }
            }
            Ok(())
        });
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_editor::inspect::ComponentCatalog;

    #[test]
    fn numbers_snap_to_the_step_and_clamp_to_the_range() {
        let c = ComponentCatalog::builtin().unwrap_or_else(|e| panic!("{e}"));
        let t = c.get("transform").unwrap_or_else(|| panic!("transform"));
        let yaw = t
            .rows
            .iter()
            .find(|r| r.path == "yaw")
            .unwrap_or_else(|| panic!("yaw"));
        assert_eq!(number_value(yaw, 12.26), Value::Float(12.5));
        assert_eq!(number_value(yaw, 999.0), Value::Float(180.0));
        let tag = c.get("tag").unwrap_or_else(|| panic!("tag"));
        let layer = tag
            .rows
            .iter()
            .find(|r| r.path == "layer")
            .unwrap_or_else(|| panic!("layer"));
        // An i32 row: exact, and never outside what an i32 holds (or its range).
        assert_eq!(int_value(&layer.field, 3), Value::Int(3));
        let (lo, hi) = int_range(&layer.field).unwrap_or_default();
        assert!(
            lo >= i128::from(i32::MIN) && hi <= i128::from(i32::MAX),
            "{lo}..={hi}"
        );
        assert_eq!(
            int_value(&layer.field, i128::from(i64::MAX)),
            Value::Int(hi as i64)
        );
    }
}
