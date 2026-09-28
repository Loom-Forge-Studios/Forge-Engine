//! The inspector through the running shell (DoD M2-37): generated from reflection, every
//! reflected primitive and composite kind gets an editor (`every_reflected_kind_gets_an_editor`
//! with a W2 positive control that removes one kind's editor), units and ranges shown,
//! multi-object editing with mixed values, add/remove component, enum variants, slider
//! gestures, `InspectorWidget` overrides, touch-to-promote, and another issuer's edits
//! refreshing rows without a rebuild.

mod common;

use std::rc::Rc;
use std::sync::Arc;

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_core::EntityId;
use forge_editor::client::BusClient;
use forge_editor::inspect::{ALL_PIN_KINDS, ComponentCatalog, InspectorCx, Tag};
use forge_editor::services::PanelFaults;
use forge_editor::session::{Reveal, SeedPathPanel};
use forge_editor::testing::Rig;
use forge_frames::{DVec3, FrameId, FramePos, FrameVel, Tick};
use forge_plugin::points::{InspectorWidget, InspectorWidgetDescriptor, WidgetTarget};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use forge_reflect::bevy_reflect::TypePath;
use forge_reflect::{PinKind, forge_api};
use forge_ui::widgets::{
    Button, CheckState, IntegerCommitted, IntegerField, Label, NumericCommitted, NumericField,
    Pressed, SliderEdit, SliderPhase, TriCheckbox,
};
use forge_ui::{KeyCode, Modifiers, NodeStyle};

const P: &str = "forge.inspector";
/// Above both 2^53 (f64 exact) and i64::MAX (stored as its bit pattern).
const SEED: u64 = u64::MAX - 2;

/// A nested struct.
#[forge_api]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Inner {
    /// Width.
    #[forge(units = "m", min = 0.0)]
    pub w: f64,
    /// A note.
    pub note: String,
}

/// A data enum.
#[forge_api]
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// Nothing.
    Off,
    /// A pulse.
    Pulse {
        /// How often.
        #[forge(units = "Hz", range = 0.1..=10.0, step = 0.1)]
        rate: f64,
    },
}

/// A component with a field of every reflected kind.
#[forge_api(name = "Everything", category = "Test")]
#[derive(Clone, Debug, PartialEq)]
pub struct Everything {
    /// A flag.
    pub flag: bool,
    /// A count.
    #[forge(range = 0..=100, step = 1)]
    pub count: i32,
    /// A byte.
    pub small: u8,
    /// A seed (all 64 bits matter).
    pub seed: u64,
    /// A signed counter.
    pub offset: i64,
    /// A speed.
    #[forge(units = "m/s", range = 0.0..=50.0, step = 0.5)]
    pub speed: f64,
    /// Power (a slider).
    #[forge(units = "W", range = 0.0..=10.0, step = 0.5, widget = "slider")]
    pub power: f64,
    /// A name.
    pub name: String,
    /// Something to follow.
    #[forge(entity)]
    pub target: EntityId,
    /// A frame.
    pub frame: FrameId,
    /// When.
    pub when: Tick,
    /// Where.
    #[forge(units = "m")]
    pub at: FramePos,
    /// How fast.
    #[forge(units = "m/s")]
    pub vel: FrameVel,
    /// Nested.
    pub inner: Inner,
    /// A mode.
    pub mode: Mode,
    /// Shown, not edited.
    #[forge(read_only)]
    pub serial: String,
    /// Not shown.
    #[forge(hidden)]
    pub secret: f64,
}

impl Default for Everything {
    fn default() -> Self {
        Self {
            flag: true,
            count: 3,
            small: 7,
            seed: SEED,
            offset: i64::MIN + 1,
            speed: 2.5,
            power: 1.0,
            name: "thing".into(),
            target: EntityId::from_bits(1).unwrap_or_else(|| panic!("an entity id")),
            frame: FrameId(2),
            when: Tick(1_500_000),
            at: FramePos::new(
                FrameId(1),
                DVec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
            ),
            vel: FrameVel::at_rest(FrameId(1)),
            inner: Inner {
                w: 4.0,
                note: "n".into(),
            },
            mode: Mode::Pulse { rate: 2.0 },
            serial: "SN-1".into(),
            secret: 42.0,
        }
    }
}

fn catalog() -> ComponentCatalog {
    let mut c = ComponentCatalog::builtin().unwrap_or_else(|e| panic!("{e}"));
    c.register_type::<Inner>().unwrap_or_else(|e| panic!("{e}"));
    c.register_type::<Mode>().unwrap_or_else(|e| panic!("{e}"));
    c.register::<Everything>("everything")
        .unwrap_or_else(|e| panic!("{e}"));
    c
}

fn rig_with(faults: PanelFaults, extra: &[&dyn SourcePlugin]) -> Rig {
    common::rig_with(
        &[P, "forge.hierarchy"],
        faults,
        |s| s.components = Rc::new(catalog()),
        extra,
    )
}

/// Add component `key` (every default) to `e` through a script, as an automation session would.
fn add_component(rig: &mut Rig, e: EntityKey, key: &str) {
    let cmds = rig
        .shell
        .handles()
        .services()
        .components
        .get(key)
        .unwrap_or_else(|| panic!("{key}"))
        .add_commands(e);
    let mut s = rig.connect(Issuer::Script { path: "s".into() });
    for c in cmds {
        s.apply(c, None);
    }
    let _ = s.pump();
    rig.turn();
}

fn select(rig: &mut Rig, keys: &[EntityKey]) {
    rig.shell.session_mut().set_selection(keys.to_vec());
    rig.turn();
}

fn row(rig: &Rig, comp: &str, path: &str, part: &str) -> Option<forge_ui::WidgetId> {
    common::try_part(
        rig,
        P,
        &[
            "content",
            &format!("comp:{comp}"),
            &format!("row:{path}"),
            part,
        ],
    )
}

fn text_of(rig: &Rig, id: forge_ui::WidgetId) -> String {
    rig.h
        .ui
        .widget::<Label>(id)
        .map(|l| l.text(rig.h.ui.rt()))
        .unwrap_or_default()
}

fn prop(rig: &Rig, e: EntityKey, p: &str) -> Option<Value> {
    rig.shell.mirror().property(e, p).cloned()
}

/// Every shown row of `everything` has an editor; every pin kind is among them.
fn check_every_kind(faults: PanelFaults) -> Result<Vec<String>, String> {
    let mut rig = rig_with(faults, &[]);
    let e = common::spawn(&mut rig, "Probe", None);
    add_component(&mut rig, e, "everything");
    select(&mut rig, &[e]);
    let services = rig.shell.handles().services_rc();
    let comp = services
        .components
        .get("everything")
        .cloned()
        .ok_or("registered")?;
    let mut kinds = Vec::new();
    let mut shown = Vec::new();
    for r in &comp.rows {
        let me = rig.shell.mirror().entity(e).cloned().ok_or("entity")?;
        if !comp.row_active(r, &me) {
            continue;
        }
        if row(&rig, "everything", &r.path, "label").is_none() {
            return Err(format!("row {} is not shown", r.path));
        }
        if row(&rig, "everything", &r.path, "edit").is_none() {
            return Err(format!("row {} ({:?}) has no editor", r.path, r.field.kind));
        }
        kinds.push(r.field.kind);
        shown.push(r.path.clone());
    }
    for k in ALL_PIN_KINDS {
        if !kinds.contains(&k) {
            return Err(format!("no row of kind {k:?} was checked"));
        }
    }
    Ok(shown)
}

#[test]
fn every_reflected_kind_gets_an_editor() {
    let shown = check_every_kind(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        shown.contains(&"mode.rate".to_string()),
        "the active data variant's field: {shown:?}"
    );
    assert!(
        shown.contains(&"inner.w".to_string()),
        "nested struct fields: {shown:?}"
    );
    assert!(
        !shown.iter().any(|p| p == "secret"),
        "hidden fields are omitted"
    );
}

#[test]
fn positive_control_a_kind_without_an_editor_fails() {
    for k in [PinKind::Time, PinKind::Vector, PinKind::Enum] {
        let e = check_every_kind(PanelFaults {
            inspector_skip_kind: Some(k),
            ..PanelFaults::default()
        })
        .err()
        .unwrap_or_default();
        assert!(
            e.contains("has no editor"),
            "{k:?} without an editor passed: {e:?}"
        );
    }
}

#[test]
fn rows_show_units_ranges_defaults_and_read_only_values() {
    let mut rig = rig_with(PanelFaults::default(), &[]);
    let e = common::spawn(&mut rig, "Probe", None);
    add_component(&mut rig, e, "everything");
    select(&mut rig, &[e]);
    // Defaults came from reflection of `Everything::default()`.
    assert_eq!(prop(&rig, e, "everything.speed"), Some(Value::Float(2.5)));
    assert_eq!(
        prop(&rig, e, "everything.at.local"),
        Some(Value::Vec3([1.0, 2.0, 3.0]))
    );
    assert_eq!(
        prop(&rig, e, "everything.when"),
        Some(Value::Int(1_500_000))
    );
    assert_eq!(
        prop(&rig, e, "everything.mode"),
        Some(Value::Text("Pulse".into()))
    );
    assert_eq!(
        prop(&rig, e, "everything.mode.rate"),
        Some(Value::Float(2.0))
    );
    assert_eq!(
        prop(&rig, e, "everything.target"),
        None,
        "no reference until one is picked"
    );
    assert_eq!(
        prop(&rig, e, "everything.secret"),
        Some(Value::Float(42.0)),
        "hidden: still stored"
    );
    let label = row(&rig, "everything", "speed", "label").unwrap_or_else(|| panic!("label"));
    let a11y = text_of(&rig, label);
    assert_eq!(a11y, "Speed (m/s)", "units are shown with the label");
    let speed = row(&rig, "everything", "speed", "edit").unwrap_or_else(|| panic!("edit"));
    let v = rig
        .h
        .ui
        .widget::<NumericField>(speed)
        .map(|f| f.value(rig.h.ui.rt()));
    assert_eq!(v, Some(2.5));
    // A committed value outside the range is clamped (range from `#[forge(range)]`).
    rig.h.ui.raise(
        speed,
        NumericCommitted {
            field: speed,
            value: 99.0,
        },
    );
    rig.turn();
    assert_eq!(prop(&rig, e, "everything.speed"), Some(Value::Float(50.0)));
    // Read-only: a label, not an editor.
    let ro = row(&rig, "everything", "serial", "edit").unwrap_or_else(|| panic!("serial"));
    assert!(rig.h.ui.widget::<Label>(ro).is_some());
    // Time shows seconds, stores microseconds — exactly (a fixed-point integer field).
    let when = row(&rig, "everything", "when", "edit").unwrap_or_else(|| panic!("when"));
    let shown = rig.h.ui.widget::<IntegerField>(when).map(|f| {
        let v = f.value(rig.h.ui.rt());
        (v, f.format_number(v))
    });
    assert_eq!(shown, Some((1_500_000, "1.5".to_string())));
    rig.h.ui.raise(
        when,
        IntegerCommitted {
            field: when,
            value: 2_250_000,
        },
    );
    rig.turn();
    assert_eq!(
        prop(&rig, e, "everything.when"),
        Some(Value::Int(2_250_000))
    );
}

/// Integer rows round-trip every bit and hold only what their type holds. Returns the
/// first disagreement.
fn check_integer_rows(faults: PanelFaults) -> Result<(), String> {
    let mut rig = rig_with(faults, &[]);
    let a = common::spawn(&mut rig, "A", None);
    let b = common::spawn(&mut rig, "B", None);
    add_component(&mut rig, a, "everything");
    add_component(&mut rig, b, "everything");
    select(&mut rig, &[a]);
    let field = |rig: &Rig, path: &str| {
        let id = row(rig, "everything", path, "edit").unwrap_or_else(|| panic!("{path}"));
        let v = rig
            .h
            .ui
            .widget::<IntegerField>(id)
            .map(|f| f.value(rig.h.ui.rt()));
        (id, v)
    };
    let commit = |rig: &mut Rig, id, value: i128| {
        rig.h.ui.raise(id, IntegerCommitted { field: id, value });
        rig.turn();
    };
    // The reflected default, all 64 bits of it, shown exactly.
    let (seed, shown) = field(&rig, "seed");
    if shown != Some(i128::from(SEED)) {
        return Err(format!("the seed default shows {shown:?}, expected {SEED}"));
    }
    let (offset, shown) = field(&rig, "offset");
    if shown != Some(i128::from(i64::MIN + 1)) {
        return Err(format!("the i64 default shows {shown:?}"));
    }
    // 2^53 + 1 (the first integer f64 cannot hold) and u64::MAX, stored and shown exactly.
    for v in [(1u64 << 53) + 1, u64::MAX, SEED] {
        commit(&mut rig, seed, i128::from(v));
        let stored = prop(&rig, a, "everything.seed");
        if stored != Some(Value::Int(v as i64)) {
            return Err(format!("committing {v} stored {stored:?}"));
        }
        let (_, shown) = field(&rig, "seed");
        if shown != Some(i128::from(v)) {
            return Err(format!("committing {v} shows {shown:?}"));
        }
    }
    commit(&mut rig, offset, i128::from(i64::MAX - 1));
    if prop(&rig, a, "everything.offset") != Some(Value::Int(i64::MAX - 1)) {
        return Err("an i64 near its maximum was not stored exactly".into());
    }
    // A u8 with no #[forge(range)] still holds only 0..=255: the editor refuses 300 when
    // typed, and a value past the limit is clamped, never stored as-is.
    let (small, _) = field(&rig, "small");
    let refused = rig
        .h
        .ui
        .widget::<IntegerField>(small)
        .map(|f| (f.parse("300"), f.parse("-1")));
    if !matches!(refused, Some((Err(_), Err(_)))) {
        return Err(format!("the u8 field accepted 300 or -1: {refused:?}"));
    }
    for (v, want) in [(300, 255), (-1, 0), (200, 200)] {
        commit(&mut rig, small, v);
        let stored = prop(&rig, a, "everything.small");
        if stored != Some(Value::Int(want)) {
            return Err(format!("committing {v} to a u8 stored {stored:?}"));
        }
    }
    // Multi-object: two different seeds (mixed); one edit sets both, exactly.
    common::set_prop(&mut rig, b, "everything.seed", Value::Int(12_345));
    select(&mut rig, &[a, b]);
    let mixed = row(&rig, "everything", "seed", "mixed").unwrap_or_else(|| panic!("mixed"));
    if text_of(&rig, mixed) != "mixed" {
        return Err("two different seeds do not show as mixed".into());
    }
    let (seed, _) = field(&rig, "seed");
    let v = (1u64 << 60) + 7;
    commit(&mut rig, seed, i128::from(v));
    for e in [a, b] {
        let stored = prop(&rig, e, "everything.seed");
        if stored != Some(Value::Int(v as i64)) {
            return Err(format!("a multi-object edit of {v} stored {stored:?}"));
        }
    }
    Ok(())
}

#[test]
fn integer_rows_round_trip_every_bit_and_hold_only_their_type_range() {
    check_integer_rows(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_integers_through_f64_fail() {
    let e = check_integer_rows(PanelFaults {
        inspector_int_through_f64: true,
        ..PanelFaults::default()
    })
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("committing") || e.contains("stored"),
        "integers passed through f64 kept every bit: {e:?}"
    );
}

#[test]
fn multi_object_editing_shows_mixed_values_and_edits_every_entity_at_once() {
    let mut rig = rig_with(PanelFaults::default(), &[]);
    let es: Vec<EntityKey> = (0..3)
        .map(|i| common::spawn(&mut rig, &format!("L{i}"), None))
        .collect();
    for e in &es {
        add_component(&mut rig, *e, "light");
    }
    common::set_prop(&mut rig, es[1], "light.range", Value::Float(25.0));
    common::set_prop(&mut rig, es[2], "light.enabled", Value::Bool(false));
    select(&mut rig, &es);
    let mixed = row(&rig, "light", "range", "mixed").unwrap_or_else(|| panic!("mixed"));
    assert_eq!(text_of(&rig, mixed), "mixed");
    let enabled = row(&rig, "light", "enabled", "edit").unwrap_or_else(|| panic!("enabled"));
    assert_eq!(
        rig.h
            .ui
            .widget::<TriCheckbox>(enabled)
            .map(|t| t.state(rig.h.ui.rt())),
        Some(CheckState::Mixed),
        "a mixed boolean is a tri-state box"
    );
    let h = common::history_len(&rig);
    let range = row(&rig, "light", "range", "edit").unwrap_or_else(|| panic!("range"));
    rig.h.ui.raise(
        range,
        NumericCommitted {
            field: range,
            value: 12.0,
        },
    );
    rig.turn();
    assert_eq!(
        common::history_len(&rig),
        h + 1,
        "one transaction for the whole selection"
    );
    for e in &es {
        assert_eq!(prop(&rig, *e, "light.range"), Some(Value::Float(12.0)));
    }
    assert_eq!(text_of(&rig, mixed), "", "no longer mixed");
    // Clicking the mixed box turns every entity on.
    rig.h.click(enabled);
    rig.turn();
    for e in &es {
        assert_eq!(prop(&rig, *e, "light.enabled"), Some(Value::Bool(true)));
    }
    rig.shell.emitter().undo();
    rig.turn();
    assert_eq!(
        prop(&rig, es[2], "light.enabled"),
        Some(Value::Bool(false)),
        "undo restores each one"
    );
}

#[test]
fn a_slider_drag_is_one_gesture_across_the_selection() {
    let mut rig = rig_with(PanelFaults::default(), &[]);
    let es: Vec<EntityKey> = (0..2)
        .map(|i| common::spawn(&mut rig, &format!("L{i}"), None))
        .collect();
    for e in &es {
        add_component(&mut rig, *e, "light");
    }
    select(&mut rig, &es);
    let before = rig.state_hash();
    let h = common::history_len(&rig);
    let power = row(&rig, "light", "power", "edit").unwrap_or_else(|| panic!("power"));
    let send = |rig: &mut Rig, phase: SliderPhase, value: f32| {
        rig.h.ui.raise(
            power,
            SliderEdit {
                slider: power,
                phase,
                value,
            },
        );
        rig.turn();
    };
    send(&mut rig, SliderPhase::Begin, 900.0);
    for i in 0..30 {
        send(&mut rig, SliderPhase::Update, 1000.0 + i as f32 * 100.0);
    }
    send(&mut rig, SliderPhase::End, 3900.0);
    assert_eq!(
        common::history_len(&rig),
        h + 1,
        "one undo entry for the drag"
    );
    for e in &es {
        assert_eq!(prop(&rig, *e, "light.power"), Some(Value::Float(3900.0)));
    }
    rig.shell.emitter().undo();
    rig.turn();
    assert_eq!(rig.state_hash(), before);
    // Esc (cancel) reverts a drag entirely.
    send(&mut rig, SliderPhase::Begin, 100.0);
    send(&mut rig, SliderPhase::Update, 200.0);
    send(&mut rig, SliderPhase::Cancel, 200.0);
    assert_eq!(rig.state_hash(), before);
}

#[test]
fn add_and_remove_component_are_one_undo_entry_each() {
    let mut rig = rig_with(PanelFaults::default(), &[]);
    let e = common::spawn(&mut rig, "Box", None);
    select(&mut rig, &[e]);
    let h = common::history_len(&rig);
    // The combo lists components the selection lacks; the first is "Everything".
    rig.h
        .click(common::part(&rig, P, &["content", "add", "button"]));
    rig.turn();
    assert_eq!(common::history_len(&rig), h + 1);
    assert!(
        common::last_entry(&rig).0.starts_with("Add"),
        "{:?}",
        common::last_entry(&rig)
    );
    assert!(
        row(&rig, "everything", "speed", "edit").is_some(),
        "the group appears"
    );
    let n_props = rig
        .shell
        .mirror()
        .entity(e)
        .map_or(0, |m| m.properties.len());
    assert!(n_props > 10, "every default was set ({n_props})");
    let remove = common::part(&rig, P, &["content", "comp:everything", "head", "remove"]);
    rig.h.click(remove);
    rig.turn();
    assert_eq!(
        rig.shell.mirror().entity(e).map(|m| m.properties.len()),
        Some(0)
    );
    assert_eq!(common::history_len(&rig), h + 2);
    assert!(common::try_part(&rig, P, &["content", "comp:everything"]).is_none());
    rig.shell.emitter().undo();
    rig.turn();
    assert_eq!(
        rig.shell.mirror().entity(e).map(|m| m.properties.len()),
        Some(n_props)
    );
}

#[test]
fn switching_an_enum_variant_swaps_its_fields() {
    let mut rig = rig_with(PanelFaults::default(), &[]);
    let e = common::spawn(&mut rig, "Lamp", None);
    add_component(&mut rig, e, "light");
    select(&mut rig, &[e]);
    assert!(
        row(&rig, "light", "kind.cone", "edit").is_none(),
        "Point has no cone"
    );
    let kind = row(&rig, "light", "kind", "edit").unwrap_or_else(|| panic!("kind"));
    rig.h.ui.set_focus(Some(kind), true);
    rig.turn();
    rig.chord(KeyCode::Char('s'), Modifiers::NONE);
    rig.turn();
    assert_eq!(
        prop(&rig, e, "light.kind"),
        Some(Value::Text("Spot".into()))
    );
    assert_eq!(
        prop(&rig, e, "light.kind.cone"),
        Some(Value::Float(1.0)),
        "the new variant's default"
    );
    assert!(
        row(&rig, "light", "kind.cone", "edit").is_some(),
        "its field is shown"
    );
    rig.shell.emitter().undo();
    rig.turn();
    assert_eq!(
        prop(&rig, e, "light.kind"),
        Some(Value::Text("Point".into()))
    );
    assert_eq!(prop(&rig, e, "light.kind.cone"), None);
}

/// A plugin replacing every `widget = "angle"` row and the whole Tag component.
struct Widgets {
    m: Manifest,
}

impl Widgets {
    fn new() -> Self {
        Self {
            m: Manifest::parse(
                "Plugin(id: \"com.test.widgets\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [InspectorWidget(\"com.test.angle\"), InspectorWidget(\"com.test.tag\")])",
            )
            .unwrap_or_else(|e| panic!("{e}")),
        }
    }
}

impl SourcePlugin for Widgets {
    fn manifest(&self) -> &Manifest {
        &self.m
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<InspectorWidget<InspectorCx>>(
            "com.test.angle",
            InspectorWidgetDescriptor {
                applies_to: WidgetTarget::Attribute("angle".into()),
                build: Arc::new(|cx: &mut InspectorCx| {
                    let (comp, row, ents) =
                        (cx.component.clone(), cx.row.clone(), cx.entities.clone());
                    cx.add_live(move |pb| {
                        let b =
                            pb.b.add(pb.parent, "dial", NodeStyle::leaf(), Button::new("Zero"))?;
                        let path = format!("{comp}.{}", row.map(|r| r.path).unwrap_or_default());
                        pb.on(b, move |act, _: &Pressed| {
                            let cmds = ents
                                .iter()
                                .map(|e| EditorCommand::SetProperty {
                                    entity: *e,
                                    path: path.clone(),
                                    value: Value::Float(0.0),
                                })
                                .collect();
                            act.cmd.emit_all("Zero angle", cmds);
                        });
                        Ok(())
                    });
                }),
            },
            Order::Last,
        )?;
        cx.add::<InspectorWidget<InspectorCx>>(
            "com.test.tag",
            InspectorWidgetDescriptor {
                applies_to: WidgetTarget::Type(Tag::type_path().to_string()),
                build: Arc::new(|cx: &mut InspectorCx| {
                    cx.add_live(|pb| {
                        pb.b.add(
                            pb.parent,
                            "custom_tag",
                            NodeStyle::leaf(),
                            Label::new("A custom tag editor"),
                        )?;
                        Ok(())
                    });
                }),
            },
            Order::Last,
        )?;
        Ok(())
    }
}

#[test]
fn inspector_widgets_replace_rows_and_components() {
    let w = Widgets::new();
    let mut rig = rig_with(PanelFaults::default(), &[&w]);
    let e = common::spawn(&mut rig, "Probe", None);
    add_component(&mut rig, e, "transform");
    add_component(&mut rig, e, "tag");
    common::set_prop(&mut rig, e, "transform.yaw", Value::Float(45.0));
    select(&mut rig, &[e]);
    // The `angle` hint rows use the plugin's editor; others keep the generated one.
    assert!(row(&rig, "transform", "yaw", "edit").is_none());
    let dial = row(&rig, "transform", "yaw", "dial").unwrap_or_else(|| panic!("the custom editor"));
    assert!(row(&rig, "transform", "scale", "edit").is_some());
    rig.h.click(dial);
    rig.turn();
    assert_eq!(
        prop(&rig, e, "transform.yaw"),
        Some(Value::Float(0.0)),
        "its edit is a command"
    );
    assert_eq!(common::last_entry(&rig).0, "Zero angle");
    // The Tag component's whole body is the plugin's.
    assert!(common::try_part(&rig, P, &["content", "comp:tag", "custom_tag"]).is_some());
    assert!(row(&rig, "tag", "label", "edit").is_none());
}

#[test]
fn generated_content_shows_its_seed_path_and_promotes_on_touch() {
    // What a plugin whose panel shows seed paths declares (the hierarchy stands in for it).
    let mut rig = common::rig_with(
        &[P, "forge.hierarchy"],
        PanelFaults::default(),
        |s| {
            s.components = Rc::new(catalog());
            s.extensions.insert(Rc::new(SeedPathPanel {
                panel: forge_ui::dock::PanelId::new("forge.hierarchy"),
                title: "Seeds".into(),
            }));
        },
        &[],
    );
    let e = common::spawn(&mut rig, "Boulder", None);
    add_component(&mut rig, e, "transform");
    let seed = "universe/world:3/zone:118/tree:2/region:14";
    common::set_prop(&mut rig, e, "gen.seed_path", Value::Text(seed.into()));
    select(&mut rig, &[e]);
    let text = common::part(&rig, P, &["content", "gen", "text"]);
    assert!(text_of(&rig, text).contains(seed));
    let h = common::history_len(&rig);
    let scale = row(&rig, "transform", "scale", "edit").unwrap_or_else(|| panic!("scale"));
    rig.h.ui.raise(
        scale,
        NumericCommitted {
            field: scale,
            value: 2.0,
        },
    );
    rig.turn();
    assert_eq!(
        common::history_len(&rig),
        h + 1,
        "the edit and the promotion are one transaction"
    );
    assert_eq!(prop(&rig, e, "gen.promoted"), Some(Value::Bool(true)));
    assert_eq!(prop(&rig, e, "transform.scale"), Some(Value::Float(2.0)));
    rig.h.click(common::part(
        &rig,
        P,
        &["content", "gen", "actions", "reveal"],
    ));
    rig.turn();
    let revealed = rig.shell.session().pending_reveal().map(|(_, r)| r.clone());
    assert_eq!(revealed, Some(Reveal::SeedPath(seed.into())));
}

/// No panel declared for seed paths: generated content still shows its seed path and can
/// be promoted, and there is no reveal button that would open nothing.
#[test]
fn generated_content_has_no_reveal_button_without_a_seed_path_panel() {
    let mut rig = rig_with(PanelFaults::default(), &[]);
    let e = common::spawn(&mut rig, "Boulder", None);
    let seed = "universe/world:3/zone:118";
    common::set_prop(&mut rig, e, "gen.seed_path", Value::Text(seed.into()));
    select(&mut rig, &[e]);
    let text = common::part(&rig, P, &["content", "gen", "text"]);
    assert!(text_of(&rig, text).contains(seed));
    assert!(common::try_part(&rig, P, &["content", "gen", "actions", "promote"]).is_some());
    assert!(
        common::try_part(&rig, P, &["content", "gen", "actions", "reveal"]).is_none(),
        "a reveal button with no panel to show the seed path"
    );
}

#[test]
fn another_issuers_edit_refreshes_the_row_without_a_rebuild() {
    let mut rig = rig_with(PanelFaults::default(), &[]);
    let e = common::spawn(&mut rig, "Probe", None);
    add_component(&mut rig, e, "transform");
    select(&mut rig, &[e]);
    let scale = row(&rig, "transform", "scale", "edit").unwrap_or_else(|| panic!("scale"));
    let automation_issuer = Issuer::Automation {
        session: "s1".into(),
        tool: "set_property".into(),
    };
    let mut auto = rig.connect(automation_issuer);
    auto.apply(
        EditorCommand::SetProperty {
            entity: e,
            path: "transform.scale".into(),
            value: Value::Float(3.5),
        },
        None,
    );
    let _ = auto.pump();
    rig.turn();
    assert!(
        rig.h.ui.contains(scale),
        "the same editor widget: no rebuild"
    );
    assert_eq!(
        rig.h
            .ui
            .widget::<NumericField>(scale)
            .map(|f| f.value(rig.h.ui.rt())),
        Some(3.5)
    );
    assert_eq!(common::last_entry(&rig).1, "automation:s1");
}

/// The components the inspector's "Add component" button adds to a fresh entity, pressed
/// `n` times (each press adds the list's first entry, and the list then offers the rest),
/// in a project whose settings are `preset`'s defaults (what opening a project made from
/// that preset gives the mirror).
fn offered_order(preset: &str, n: usize) -> Vec<String> {
    let mut rig = common::rig_with(&[P, "forge.hierarchy"], PanelFaults::default(), |_| {}, &[]);
    let defaults = forge_editor::project::preset_defaults(preset).unwrap_or_else(|e| panic!("{e}"));
    {
        let mut s = rig.connect(Issuer::Script { path: "s".into() });
        for (key, value) in defaults {
            s.apply(
                EditorCommand::SetSetting {
                    key,
                    value: Some(value),
                },
                None,
            );
        }
        let _ = s.pump();
    }
    rig.turn();
    let e = common::spawn(&mut rig, "Thing", None);
    select(&mut rig, &[e]);
    let present = |rig: &Rig| -> Vec<String> {
        let sv = rig.shell.handles().services();
        let m = rig.shell.mirror().entity(e).cloned();
        sv.components
            .components()
            .filter(|c| m.as_ref().is_some_and(|m| c.present_on(m)))
            .map(|c| c.key.clone())
            .collect()
    };
    let mut order = Vec::new();
    for _ in 0..n {
        let before = present(&rig);
        // Pressed on the button itself: the inspector grows with every component, and a
        // pointer click would land wherever the button has scrolled to.
        let b = common::part(&rig, P, &["content", "add", "button"]);
        rig.h.ui.raise(b, Pressed(b));
        rig.turn();
        let added: Vec<String> = present(&rig)
            .into_iter()
            .filter(|k| !before.contains(k))
            .collect();
        assert_eq!(
            added.len(),
            1,
            "{preset}: one component per press: {added:?} after {order:?}"
        );
        order.extend(added);
    }
    order
}

/// Ch.35 §35.2: in a project made from the 2D preset (`render.path` "2d") the 2D pipeline's
/// components are offered first by the inspector's add-component list: pressing "Add
/// component" once per 2D component adds exactly the 2D components. Control: the same editor
/// with the 3D preset's settings offers the catalogue's own order, which interleaves other
/// components, so the order really comes from the project's render path.
#[test]
fn a_2d_project_offers_the_2d_components_first() {
    let catalog = ComponentCatalog::editor().unwrap_or_else(|e| panic!("{e}"));
    let two_d_keys: Vec<String> = catalog
        .components()
        .filter(|c| c.type_path.starts_with("forge_2d::"))
        .map(|c| c.key.clone())
        .collect();
    assert!(two_d_keys.len() >= 2, "{two_d_keys:?}");
    let n = two_d_keys.len();
    let mut two_d = offered_order("2d", n);
    two_d.sort();
    let mut want = two_d_keys.clone();
    want.sort();
    assert_eq!(two_d, want, "the 2D preset offers every 2D component first");
    let three_d = offered_order("3d", n);
    assert!(
        three_d.iter().any(|k| !two_d_keys.contains(k)),
        "control: the 3D preset offered only 2D components first: {three_d:?}"
    );
}
