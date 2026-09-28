//! The **Audio mixer** (`forge.audio_mixer`, Ch.21 §21.21, DoD M2-67; Ch.20): buses,
//! sends, meters and a spatial preview over `forge_editor::domain::audio`.
//!
//! * **Buses.** The routing tree (each bus under the bus it feeds, `master` at the root):
//!   add, remove (the buses it fed move to its output), rename in place, and drag onto
//!   another bus to re-route — a route that would loop is refused with the reason.
//! * **Strip.** Volume and pan faders (a drag is **one gesture, one undo entry**), mute and
//!   solo, and a test tone (an audition: session state, not an edit).
//! * **Sends.** Per bus, pre- or post-fader, with a level fader; add one by activating a
//!   target (targets that would loop are not offered).
//! * **Meters.** One bridge for every bus, fed by the audio backend's live feed (§21.11):
//!   it redraws only when a meter changed, at most 10 Hz, only while visible.
//! * **Spatial preview.** Drag the source around the listener to hear (read) the distance
//!   gain and pan the rolloff settings give; the settings themselves are project state.
//!
//! Every project edit is a command (I7).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use forge_cmd::Value;
use forge_editor::domain::audio::{
    self as au, MASTER, Mixer, PreviewOffset, Rolloff, SPATIAL, Spatial, VOLUME_MAX_DB,
    VOLUME_MIN_DB, bus_key,
};
use forge_editor::domain::{clear_under, set};
use forge_editor::emitter::Gesture;
use forge_editor::mirror::ProjectMirror;
use forge_editor::panel_rt::PanelAct;
use forge_editor::panels::PanelCx;
use forge_editor::services::EditorServices;
use forge_ui::widgets::{
    Button, Checkbox, Container, Label, LabelKind, NumericCommitted, NumericField, Pressed,
    RadioGroup, RowActivated, RowItem, RowRenamed, RowsDeleteRequested, RowsDropped,
    SelectionChanged, SignalChanged, SignalRelay, Slider, SliderEdit, SliderPhase, VirtualTree,
};
use forge_ui::{Dirty, LiveFeed, NodeStyle, Role, Signal, Ui, WidgetId};

use crate::common::{Row, Rows, key_of, refuse, select, show_rows, watch};
use crate::widgets::{MeterBridge, PadData, SpatialPad};

const PREFIX: &str = "audio.";
/// The meters' refresh cap (a live-data panel, §21.11).
pub const METER_HZ: u8 = 10;

struct Mx {
    mixer: Mixer,
    spatial: Spatial,
    seen: u64,
    status: Signal<String>,
    problems: Signal<String>,
    tree: WidgetId,
    rows: Rows<String>,
    sends: WidgetId,
    send_rows: Rows<String>,
    targets: WidgetId,
    target_rows: Rows<String>,
    sel: String,
    sel_send: Option<String>,
    title: Signal<String>,
    volume: Signal<f32>,
    pan: Signal<f32>,
    mute: Signal<bool>,
    solo: Signal<bool>,
    tone: Signal<bool>,
    send_db: Signal<f32>,
    send_pre: Signal<bool>,
    gesture: Option<Gesture>,
    order: Rc<RefCell<Vec<(String, String)>>>,
    bridge: WidgetId,
    feed: bool,
    pad: WidgetId,
    pad_data: Rc<RefCell<PadData>>,
    min_d: Signal<f64>,
    max_d: Signal<f64>,
    rolloff: Signal<usize>,
}

impl Mx {
    fn say(&self, ui: &mut Ui, s: impl Into<String>) {
        self.status.set(ui.rt_mut(), s.into());
    }

    fn reread(&mut self, ui: &mut Ui, m: &ProjectMirror, sv: &EditorServices) {
        self.mixer = Mixer::read(m);
        let (spatial, sp) = Spatial::read(m);
        self.spatial = spatial;
        sv.audio.set_mix(&self.mixer);
        let mut all = self.mixer.problems.clone();
        all.extend(sp);
        let p = if all.is_empty() {
            String::new()
        } else {
            format!("\u{26a0} {}", all.join("; "))
        };
        self.problems.set(ui.rt_mut(), p);
        if !self.mixer.buses.contains_key(&self.sel) {
            self.sel = MASTER.to_string();
        }
        self.show(ui, sv);
    }

    /// The routing tree, the strip, the sends, the meters' order and the spatial settings.
    fn show(&mut self, ui: &mut Ui, sv: &EditorServices) {
        let mx = &self.mixer;
        // Routing tree: depth-first from master; buses in a loop listed at the top level.
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        let mut order = Vec::new();
        let mut placed = std::collections::BTreeSet::new();
        let mut stack: Vec<(Option<u64>, String)> = vec![(None, MASTER.to_string())];
        while let Some((parent, id)) = stack.pop() {
            if !placed.insert(id.clone()) {
                continue;
            }
            let Some(b) = mx.buses.get(&id) else { continue };
            let k = key_of(&["bus", &id]);
            let flags = format!(
                "{}{}",
                if b.mute { "  [muted]" } else { "" },
                if b.solo { "  [solo]" } else { "" }
            );
            rows.push((
                parent,
                k,
                RowItem::new(forge_ui::trf!(
                    "{name}  {volume_db} dB{flags}",
                    name = b.name,
                    volume_db = format!("{:+.1}", b.volume_db),
                    flags
                ))
                .muted(b.mute),
            ));
            map.insert(k, id.clone());
            order.push((id.clone(), b.name.clone()));
            let mut kids: Vec<String> = mx
                .buses
                .values()
                .filter(|c| {
                    c.output.as_deref() == Some(id.as_str())
                        || (id == MASTER
                            && c.output.as_ref().is_some_and(|o| !mx.buses.contains_key(o)))
                })
                .map(|c| c.id.clone())
                .collect();
            kids.reverse();
            for c in kids {
                stack.push((Some(k), c));
            }
        }
        for b in mx.buses.values() {
            if !placed.contains(&b.id) {
                let k = key_of(&["bus", &b.id]);
                rows.push((
                    None,
                    k,
                    RowItem::new(forge_ui::trf!("{name} (in a routing loop)", name = b.name))
                        .muted(true),
                ));
                map.insert(k, b.id.clone());
                order.push((b.id.clone(), b.name.clone()));
            }
        }
        show_rows(ui, self.tree, rows, map, &mut self.rows);
        *self.order.borrow_mut() = order;
        ui.invalidate(self.bridge, Dirty::PAINT | Dirty::A11Y);

        // The strip of the selected bus.
        let b = mx.buses.get(&self.sel);
        self.title.set(
            ui.rt_mut(),
            b.map_or(String::new(), |b| {
                format!(
                    "{} \u{2192} {}",
                    b.name,
                    b.output
                        .as_deref()
                        .map_or(forge_ui::tr!("the device").to_string(), |o| mx
                            .buses
                            .get(o)
                            .map_or(o.to_string(), |x| x.name.clone()))
                )
            }),
        );
        if self.gesture.is_none() {
            set_f32(ui, self.volume, b.map_or(0.0, |b| b.volume_db) as f32);
            set_f32(ui, self.pan, b.map_or(0.0, |b| b.pan) as f32);
        }
        set_bool(ui, self.mute, b.is_some_and(|b| b.mute));
        set_bool(ui, self.solo, b.is_some_and(|b| b.solo));
        set_bool(ui, self.tone, sv.audio.previews().contains(&self.sel));

        // Sends of the selected bus, and the targets a new send may use.
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        if let Some(b) = b {
            for s in b.sends.values() {
                let k = key_of(&["send", &b.id, &s.target]);
                let tn = mx
                    .buses
                    .get(&s.target)
                    .map_or(s.target.as_str(), |t| t.name.as_str());
                rows.push((
                    None,
                    k,
                    RowItem::new(forge_ui::trf!(
                        "\u{2192} {tn}  {db} dB  {fader}",
                        tn,
                        db = format!("{:+.1}", s.db),
                        fader = if s.pre {
                            forge_ui::tr!("pre-fader")
                        } else {
                            forge_ui::tr!("post-fader")
                        }
                    )),
                ));
                map.insert(k, s.target.clone());
            }
        }
        show_rows(ui, self.sends, rows, map, &mut self.send_rows);
        if self
            .sel_send
            .as_ref()
            .is_some_and(|t| b.is_none_or(|b| !b.sends.contains_key(t)))
        {
            self.sel_send = None;
        }
        let send = b.and_then(|b| self.sel_send.as_ref().and_then(|t| b.sends.get(t)));
        if self.gesture.is_none() {
            set_f32(ui, self.send_db, send.map_or(0.0, |s| s.db) as f32);
        }
        set_bool(ui, self.send_pre, send.is_some_and(|s| s.pre));
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        if let Some(b) = b {
            for t in mx.buses.values() {
                if b.sends.contains_key(&t.id) || mx.would_loop(&b.id, &t.id) {
                    continue;
                }
                let k = key_of(&["target", &b.id, &t.id]);
                rows.push((
                    None,
                    k,
                    RowItem::new(forge_ui::trf!("Send to {name}", name = t.name)),
                ));
                map.insert(k, t.id.clone());
            }
        }
        show_rows(ui, self.targets, rows, map, &mut self.target_rows);

        // Spatial settings.
        let s = self.spatial;
        if self.min_d.get(ui.rt()).to_bits() != s.min_distance.to_bits() {
            self.min_d.set(ui.rt_mut(), s.min_distance);
        }
        if self.max_d.get(ui.rt()).to_bits() != s.max_distance.to_bits() {
            self.max_d.set(ui.rt_mut(), s.max_distance);
        }
        let ri = Rolloff::ALL
            .iter()
            .position(|r| *r == s.rolloff)
            .unwrap_or(0);
        if self.rolloff.get(ui.rt()) != ri {
            self.rolloff.set(ui.rt_mut(), ri);
        }
        self.pad_data.borrow_mut().spatial = s;
        ui.invalidate(self.pad, Dirty::PAINT | Dirty::A11Y);
    }
}

fn set_f32(ui: &mut Ui, s: Signal<f32>, v: f32) {
    if s.get(ui.rt()).to_bits() != v.to_bits() {
        s.set(ui.rt_mut(), v);
    }
}
fn set_bool(ui: &mut Ui, s: Signal<bool>, v: bool) {
    if s.get(ui.rt()) != v {
        s.set(ui.rt_mut(), v);
    }
}

/// A fader value as stored: tenths of a dB (or hundredths of pan), no `f32` residue.
fn tenths(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}
fn hundredths(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

/// A fader edit: one gesture per drag (one undo entry), one command per keyboard step.
fn fader(
    act: &mut PanelAct,
    g: &mut Option<Gesture>,
    e: &SliderEdit,
    label: &str,
    key: String,
    v: f64,
) {
    let cmd = set(key, Value::Float(v));
    match e.phase {
        SliderPhase::Begin => {
            let mut gg = act.cmd.gesture(label);
            gg.update(cmd);
            *g = Some(gg);
        }
        SliderPhase::Update => match g.as_mut() {
            Some(gg) => gg.update(cmd),
            None => {
                act.cmd.emit(cmd);
            }
        },
        SliderPhase::End => {
            if let Some(gg) = g.take() {
                gg.commit();
            }
        }
        SliderPhase::Cancel => {
            if let Some(gg) = g.take() {
                gg.cancel();
            }
        }
        SliderPhase::Step => {
            act.cmd.emit(cmd);
        }
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("the master bus, which every project has"));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space[1];
        // The sync step fills the lists from the mirror: run it right after the build.
        pb.want_turn();
        let sv = pb.services();
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space).padding(space).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Mixer actions")),
        )?;
        let add = pb.b.add(
            bar,
            "add",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add bus")),
        )?;
        let remove = pb.b.add(
            bar,
            "remove",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Remove bus")),
        )?;
        let tone = pb.b.signal(false);
        pb.b.add(
            bar,
            "tone",
            NodeStyle::leaf(),
            Checkbox::new(tone, forge_ui::tr!("Test tone on this bus")),
        )?;
        let tone_relay = pb.b.add(
            bar,
            "tone.relay",
            NodeStyle::leaf(),
            SignalRelay::new(tone.any()),
        )?;
        let body = pb.b.add(
            pb.parent,
            "body",
            NodeStyle::row(space).grow(1.0).padding(space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Mixer")),
        )?;
        let tree = pb.b.add(
            body,
            "buses",
            NodeStyle::leaf().width(240.0).min_size(0.0, 120.0),
            VirtualTree::tree(forge_ui::tr!("Bus routing")).single_select(),
        )?;

        let strip = pb.b.add(
            body,
            "strip",
            NodeStyle::column(space).width(280.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Bus strip")),
        )?;
        let title = pb.b.signal(String::new());
        pb.b.add(
            strip,
            "title",
            NodeStyle::leaf(),
            Label::new(title).kind(LabelKind::Heading),
        )?;
        let volume = pb.b.signal(0.0f32);
        let vol = pb.b.add(
            strip,
            "volume",
            NodeStyle::leaf(),
            Slider::new(
                volume,
                forge_ui::tr!("Volume (dB)"),
                VOLUME_MIN_DB as f32,
                VOLUME_MAX_DB as f32,
                0.5,
            ),
        )?;
        let pan = pb.b.signal(0.0f32);
        let pan_w = pb.b.add(
            strip,
            "pan",
            NodeStyle::leaf(),
            Slider::new(pan, forge_ui::tr!("Pan"), -1.0, 1.0, 0.05),
        )?;
        let mute = pb.b.signal(false);
        pb.b.add(
            strip,
            "mute",
            NodeStyle::leaf(),
            Checkbox::new(mute, forge_ui::tr!("Mute")),
        )?;
        let mute_relay = pb.b.add(
            strip,
            "mute.relay",
            NodeStyle::leaf(),
            SignalRelay::new(mute.any()),
        )?;
        let solo = pb.b.signal(false);
        pb.b.add(
            strip,
            "solo",
            NodeStyle::leaf(),
            Checkbox::new(solo, forge_ui::tr!("Solo")),
        )?;
        let solo_relay = pb.b.add(
            strip,
            "solo.relay",
            NodeStyle::leaf(),
            SignalRelay::new(solo.any()),
        )?;
        pb.b.add(
            strip,
            "sends_title",
            NodeStyle::leaf(),
            Label::new(forge_ui::tr!("Sends")).kind(LabelKind::Heading),
        )?;
        let sends = pb.b.add(
            strip,
            "sends",
            NodeStyle::leaf().min_size(0.0, 60.0).grow(1.0),
            VirtualTree::list(forge_ui::tr!("Sends"))
                .read_only()
                .single_select(),
        )?;
        let send_db = pb.b.signal(0.0f32);
        let send_level = pb.b.add(
            strip,
            "send_level",
            NodeStyle::leaf(),
            Slider::new(
                send_db,
                forge_ui::tr!("Send level (dB)"),
                VOLUME_MIN_DB as f32,
                VOLUME_MAX_DB as f32,
                0.5,
            ),
        )?;
        let send_pre = pb.b.signal(false);
        pb.b.add(
            strip,
            "send_pre",
            NodeStyle::leaf(),
            Checkbox::new(send_pre, forge_ui::tr!("Pre-fader")),
        )?;
        let pre_relay = pb.b.add(
            strip,
            "send_pre.relay",
            NodeStyle::leaf(),
            SignalRelay::new(send_pre.any()),
        )?;
        let remove_send = pb.b.add(
            strip,
            "remove_send",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Remove send")),
        )?;
        let targets = pb.b.add(
            strip,
            "targets",
            NodeStyle::leaf().min_size(0.0, 60.0).grow(1.0),
            VirtualTree::list(forge_ui::tr!("Add a send (Enter)"))
                .read_only()
                .single_select(),
        )?;

        let right = pb.b.add(
            body,
            "right",
            NodeStyle::column(space).grow(1.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Meters and spatial preview")),
        )?;
        let order = Rc::new(RefCell::new(Vec::new()));
        let bridge = pb.b.add(
            right,
            "meters",
            NodeStyle::leaf().grow(1.0).min_size(120.0, 120.0),
            MeterBridge::new(sv.audio.clone(), order.clone()),
        )?;
        let sp = pb.b.add(
            right,
            "spatial",
            NodeStyle::row(space).wrap(),
            Container::new(Role::Group).labelled(forge_ui::tr!("Spatialisation")),
        )?;
        let min_d = pb.b.signal(1.0f64);
        let min_w = pb.b.add(
            sp,
            "min_distance",
            NodeStyle::leaf(),
            NumericField::new(min_d, forge_ui::tr!("Min distance"))
                .unit("m")
                .range(0.01, 10_000.0)
                .decimals(2),
        )?;
        let max_d = pb.b.signal(50.0f64);
        let max_w = pb.b.add(
            sp,
            "max_distance",
            NodeStyle::leaf(),
            NumericField::new(max_d, forge_ui::tr!("Max distance"))
                .unit("m")
                .range(0.01, 100_000.0)
                .decimals(2),
        )?;
        let rolloff = pb.b.signal(0usize);
        let names: Vec<&str> = Rolloff::ALL
            .iter()
            .map(|r| forge_ui::l10n::tr(r.name()))
            .collect();
        pb.b.add(
            sp,
            "rolloff",
            NodeStyle::leaf(),
            RadioGroup::new(forge_ui::tr!("Rolloff"), &names, rolloff),
        )?;
        let rolloff_relay = pb.b.add(
            sp,
            "rolloff.relay",
            NodeStyle::leaf(),
            SignalRelay::new(rolloff.any()),
        )?;
        let pad_data = Rc::new(RefCell::new(PadData {
            spatial: Spatial {
                min_distance: 1.0,
                max_distance: 50.0,
                rolloff: Rolloff::Inverse,
            },
            source: PreviewOffset {
                right_m: 3.0,
                ahead_m: 4.0,
            },
        }));
        let pad = pb.b.add(
            right,
            "pad",
            NodeStyle::leaf().grow(1.0).min_size(140.0, 140.0),
            SpatialPad::new(pad_data.clone(), sv.audio.clone()),
        )?;

        let status = pb.b.signal(
            forge_ui::tr!(
                "Select a bus to edit its strip; drag a bus onto another to re-route it."
            )
            .to_string(),
        );
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
            Label::new(forge_ui::l10n::tr_str(&sv.audio.backend().note).into_owned())
                .kind(LabelKind::Small)
                .wrapping(),
        )?;

        let spatial0 = pad_data.borrow().spatial;
        let st = Rc::new(RefCell::new(Mx {
            mixer: Mixer {
                buses: Default::default(),
                problems: Vec::new(),
            },
            spatial: spatial0,
            seen: u64::MAX,
            status,
            problems,
            tree,
            rows: Rows::default(),
            sends,
            send_rows: Rows::default(),
            targets,
            target_rows: Rows::default(),
            sel: MASTER.to_string(),
            sel_send: None,
            title,
            volume,
            pan,
            mute,
            solo,
            tone,
            send_db,
            send_pre,
            gesture: None,
            order,
            bridge,
            feed: false,
            pad,
            pad_data,
            min_d,
            max_d,
            rolloff,
        }));

        let s = st.clone();
        pb.on(add, move |act, _: &Pressed| {
            let m = s.borrow();
            let name = forge_ui::trf!("Bus {n}", n = m.mixer.buses.len());
            let (id, cmds) = au::new_bus(&m.mixer, &name, &m.sel);
            act.cmd.emit_all(forge_ui::tr!("Add bus"), cmds);
            m.say(
                act.ui,
                forge_ui::trf!("Added bus {id}, feeding {sel}.", id, sel = m.sel),
            );
        });
        let s = st.clone();
        let remove_bus = Rc::new(move |act: &mut PanelAct, id: String| {
            let m = s.borrow();
            if id == MASTER {
                refuse(
                    act.session,
                    forge_ui::tr!("Remove bus"),
                    forge_ui::tr!("Master is the mix's output and cannot be removed."),
                );
                return;
            }
            act.cmd.emit_all(
                forge_ui::tr!("Remove bus"),
                au::remove_bus(act.mirror, &m.mixer, &id),
            );
        });
        let (s, rb) = (st.clone(), remove_bus.clone());
        pb.on(remove, move |act, _: &Pressed| {
            let id = s.borrow().sel.clone();
            rb(act, id);
        });
        let (s, rb) = (st.clone(), remove_bus);
        pb.on(tree, move |act, e: &RowsDeleteRequested| {
            let id = e.keys.first().and_then(|k| s.borrow().rows.get(*k));
            if let Some(id) = id {
                rb(act, id);
            }
        });
        let s = st.clone();
        pb.on(tree, move |act, e: &SelectionChanged| {
            let mut m = s.borrow_mut();
            if let Some(id) = e.keys.first().and_then(|k| m.rows.get(*k)) {
                m.sel = id;
                m.sel_send = None;
                m.show(act.ui, act.services);
            }
        });
        let s = st.clone();
        pb.on(tree, move |act, e: &RowRenamed| {
            if let Some(id) = s.borrow().rows.get(e.key) {
                act.cmd
                    .emit(set(bus_key(&id, "name"), Value::Text(e.name.clone())));
            }
        });
        let s = st.clone();
        pb.on(tree, move |act, e: &RowsDropped| {
            let m = s.borrow();
            let to = e
                .target
                .parent
                .and_then(|k| m.rows.get(k))
                .unwrap_or_else(|| MASTER.to_string());
            let mut cmds = Vec::new();
            for k in &e.keys {
                let Some(b) = m.rows.get(*k) else { continue };
                if b == MASTER {
                    refuse(
                        act.session,
                        forge_ui::tr!("Route bus"),
                        forge_ui::tr!("Master is the mix's output; it feeds no other bus."),
                    );
                    return;
                }
                if m.mixer.would_loop(&b, &to) {
                    refuse(
                        act.session,
                        forge_ui::tr!("Route bus"),
                        &forge_ui::trf!(
                            "Routing {b} into {to} would make a loop ({to} already feeds {b}).",
                            b,
                            to
                        ),
                    );
                    return;
                }
                cmds.push(set(bus_key(&b, "output"), Value::Text(to.clone())));
            }
            act.cmd.emit_all(forge_ui::tr!("Route bus"), cmds);
        });
        let s = st.clone();
        pb.on(vol, move |act, e: &SliderEdit| {
            let mut m = s.borrow_mut();
            let key = bus_key(&m.sel, "volume_db");
            let label = forge_ui::trf!("Volume of {bus}", bus = m.sel);
            fader(act, &mut m.gesture, e, &label, key, tenths(e.value));
        });
        let s = st.clone();
        pb.on(pan_w, move |act, e: &SliderEdit| {
            let mut m = s.borrow_mut();
            let key = bus_key(&m.sel, "pan");
            let label = forge_ui::trf!("Pan of {bus}", bus = m.sel);
            fader(act, &mut m.gesture, e, &label, key, hundredths(e.value));
        });
        for (relay, field, sig) in [(mute_relay, "mute", mute), (solo_relay, "solo", solo)] {
            let s = st.clone();
            pb.on(relay, move |act, _: &SignalChanged| {
                let m = s.borrow();
                let want = sig.get(act.ui.rt());
                let cur = m
                    .mixer
                    .buses
                    .get(&m.sel)
                    .is_some_and(|b| if field == "mute" { b.mute } else { b.solo });
                if want != cur {
                    act.cmd.emit(set(bus_key(&m.sel, field), Value::Bool(want)));
                }
            });
        }
        let s = st.clone();
        pb.on(tone_relay, move |act, _: &SignalChanged| {
            let m = s.borrow();
            let on = m.tone.get(act.ui.rt());
            if act.services.audio.previews().contains(&m.sel) != on {
                act.services.audio.set_preview(&m.sel, on);
                m.say(
                    act.ui,
                    if on {
                        forge_ui::trf!("Test tone playing into {sel}.", sel = m.sel)
                    } else {
                        forge_ui::tr!("Test tone stopped.").into()
                    },
                );
            }
        });
        let s = st.clone();
        pb.on(sends, move |act, e: &SelectionChanged| {
            let mut m = s.borrow_mut();
            m.sel_send = e.keys.first().and_then(|k| m.send_rows.get(*k));
            m.show(act.ui, act.services);
        });
        let s = st.clone();
        pb.on(send_level, move |act, e: &SliderEdit| {
            let mut m = s.borrow_mut();
            let Some(t) = m.sel_send.clone() else {
                if e.phase != SliderPhase::Update {
                    refuse(
                        act.session,
                        forge_ui::tr!("Send level"),
                        forge_ui::tr!("Select a send first."),
                    );
                }
                return;
            };
            let key = bus_key(&m.sel, &format!("send.{t}.db"));
            let label = forge_ui::trf!("Send {bus} \u{2192} {t}", bus = m.sel, t);
            fader(act, &mut m.gesture, e, &label, key, tenths(e.value));
        });
        let s = st.clone();
        pb.on(pre_relay, move |act, _: &SignalChanged| {
            let m = s.borrow();
            let want = m.send_pre.get(act.ui.rt());
            let Some(t) = m.sel_send.clone() else { return };
            let cur = m
                .mixer
                .buses
                .get(&m.sel)
                .and_then(|b| b.sends.get(&t))
                .is_some_and(|x| x.pre);
            if want != cur {
                act.cmd.emit(set(
                    bus_key(&m.sel, &format!("send.{t}.pre")),
                    Value::Bool(want),
                ));
            }
        });
        let s = st.clone();
        pb.on(remove_send, move |act, _: &Pressed| {
            let m = s.borrow();
            let Some(t) = m.sel_send.clone() else {
                refuse(
                    act.session,
                    forge_ui::tr!("Remove send"),
                    forge_ui::tr!("Select a send first."),
                );
                return;
            };
            act.cmd.emit_all(
                forge_ui::tr!("Remove send"),
                clear_under(act.mirror, &format!("{}.{}.send.{t}", au::BUS, m.sel)),
            );
        });
        let s = st.clone();
        pb.on(targets, move |act, e: &RowActivated| {
            let m = s.borrow();
            let Some(t) = m.target_rows.get(e.key) else {
                return;
            };
            if m.mixer.would_loop(&m.sel, &t) {
                refuse(
                    act.session,
                    forge_ui::tr!("Add send"),
                    &forge_ui::trf!(
                        "A send from {sel} to {t} would make a loop.",
                        sel = m.sel,
                        t
                    ),
                );
                return;
            }
            act.cmd.emit_all(
                forge_ui::tr!("Add send"),
                vec![
                    set(bus_key(&m.sel, &format!("send.{t}.db")), Value::Float(0.0)),
                    set(
                        bus_key(&m.sel, &format!("send.{t}.pre")),
                        Value::Bool(false),
                    ),
                ],
            );
            m.say(
                act.ui,
                forge_ui::trf!(
                    "{sel} now sends to {t} at 0 dB, post-fader.",
                    sel = m.sel,
                    t
                ),
            );
        });
        let s = st.clone();
        pb.on(min_w, move |act, e: &NumericCommitted| {
            let _ = &s;
            act.cmd.emit(set(
                format!("{SPATIAL}.min_distance"),
                Value::Float(e.value),
            ));
        });
        let s = st.clone();
        pb.on(max_w, move |act, e: &NumericCommitted| {
            let m = s.borrow();
            if e.value < m.spatial.min_distance {
                refuse(
                    act.session,
                    forge_ui::tr!("Max distance"),
                    forge_ui::tr!("The max distance must not be below the min distance."),
                );
                return;
            }
            act.cmd.emit(set(
                format!("{SPATIAL}.max_distance"),
                Value::Float(e.value),
            ));
        });
        let s = st.clone();
        pb.on(rolloff_relay, move |act, _: &SignalChanged| {
            let m = s.borrow();
            let r = Rolloff::ALL
                .get(m.rolloff.get(act.ui.rt()))
                .copied()
                .unwrap_or(Rolloff::Inverse);
            if r != m.spatial.rolloff {
                act.cmd.emit(set(
                    format!("{SPATIAL}.rolloff"),
                    Value::Text(r.name().into()),
                ));
            }
        });

        let s = st;
        pb.sync(tree, move |sy| {
            let mut m = s.borrow_mut();
            if !m.feed {
                m.feed = true;
                let bridge = m.bridge;
                let feed = sy.ui.add_feed(
                    bridge,
                    LiveFeed {
                        source: sy.services.audio.feed(),
                        max_hz: METER_HZ,
                        self_ui: false,
                    },
                );
                let _ = feed;
            }
            let rev = watch(sy.mirror, PREFIX);
            if rev != m.seen {
                m.seen = rev;
                m.reread(sy.ui, sy.mirror, sy.services);
                let (tree, key) = (m.tree, key_of(&["bus", &m.sel]));
                select(sy.ui, tree, key);
            }
            Ok(())
        });
        Ok(())
    });
}
