//! The audio mixer (`forge.audio_mixer`, DoD M2-67) through the running shell: buses and
//! routing, faders as gestures, mute/solo, sends, meters from the backend's live feed
//! (zero redraws once they settle), and the spatial preview.

mod common;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use common::{history_len, last_notice, part, press, raise, rows_with, select_row, setting, undo};
use forge_cmd::Value;
use forge_editor::domain::audio::{AudioBuses, MemoryAudio, TONE_PEAK_DB};
use forge_editor::testing::Rig;
use forge_panels_domain::widgets::MeterBridge;
use forge_ui::widgets::{
    DropTarget, NumericCommitted, RowActivated, RowRenamed, RowsDropped, SliderEdit, SliderPhase,
};
use forge_ui::{KeyCode, LiveCell, LiveSource, Modifiers, WidgetId};

const P: &str = "forge.audio_mixer";

fn w(rig: &Rig, path: &[&str]) -> WidgetId {
    part(rig, P, path)
}

fn tap(rig: &mut Rig, path: &[&str]) {
    let b = w(rig, path);
    press(rig, b);
}

/// Toggle a checkbox from the keyboard.
fn toggle(rig: &mut Rig, path: &[&str]) {
    let id = w(rig, path);
    rig.h.ui.set_focus(Some(id), true);
    rig.chord(KeyCode::Space, Modifiers::NONE);
    rig.settle();
}

fn select_bus(rig: &mut Rig, name: &str) {
    let tree = w(rig, &["body", "buses"]);
    let k = rows_with(rig, tree, name);
    let k = *k.first().unwrap_or_else(|| panic!("no bus row {name:?}"));
    select_row(rig, tree, k);
}

fn drag(rig: &mut Rig, slider: WidgetId, values: &[f32]) {
    let n = values.len();
    for (i, v) in values.iter().enumerate() {
        let phase = if i == 0 {
            SliderPhase::Begin
        } else {
            SliderPhase::Update
        };
        rig.h.ui.raise(
            slider,
            SliderEdit {
                slider,
                phase,
                value: *v,
            },
        );
        rig.turn();
        if i + 1 == n {
            rig.h.ui.raise(
                slider,
                SliderEdit {
                    slider,
                    phase: SliderPhase::End,
                    value: *v,
                },
            );
            rig.turn();
        }
    }
    rig.settle();
}

#[test]
fn buses_route_faders_are_gestures_and_loops_are_refused() {
    let mut rig = common::rig(&[P]);
    let start = rig.state_hash();
    // Master exists without any setting.
    let tree = w(&rig, &["body", "buses"]);
    assert_eq!(rows_with(&mut rig, tree, "Master").len(), 1);
    tap(&mut rig, &["bar", "add"]);
    assert_eq!(
        setting(&rig, "audio.bus.bus_1.output"),
        Some(Value::Text("master".into()))
    );
    let row = rows_with(&mut rig, tree, "Bus 1");
    raise(
        &mut rig,
        tree,
        RowRenamed {
            view: tree,
            key: row[0],
            name: "Music".into(),
        },
    );
    assert_eq!(
        setting(&rig, "audio.bus.bus_1.name"),
        Some(Value::Text("Music".into()))
    );
    select_bus(&mut rig, "Music");
    // A fader drag is one undo entry, the last value kept (tenths of a dB).
    let vol = w(&rig, &["body", "strip", "volume"]);
    let before = history_len(&rig);
    drag(&mut rig, vol, &[-1.0, -3.0, -6.04]);
    assert_eq!(history_len(&rig), before + 1);
    assert_eq!(
        setting(&rig, "audio.bus.bus_1.volume_db"),
        Some(Value::Float(-6.0))
    );
    // Mute and solo are commands.
    toggle(&mut rig, &["body", "strip", "mute"]);
    assert_eq!(
        setting(&rig, "audio.bus.bus_1.mute"),
        Some(Value::Bool(true))
    );
    undo(&mut rig);
    assert_eq!(setting(&rig, "audio.bus.bus_1.mute"), None);
    undo(&mut rig);
    assert_eq!(
        setting(&rig, "audio.bus.bus_1.volume_db"),
        Some(Value::Float(0.0))
    );
    // A child bus under Music; routing Music into it would loop: refused, named.
    tap(&mut rig, &["bar", "add"]);
    assert_eq!(
        setting(&rig, "audio.bus.bus_2.output"),
        Some(Value::Text("bus_1".into()))
    );
    let music = rows_with(&mut rig, tree, "Music");
    let child = rows_with(&mut rig, tree, "Bus 2");
    let before = history_len(&rig);
    raise(
        &mut rig,
        tree,
        RowsDropped {
            view: tree,
            keys: vec![music[0]],
            target: DropTarget {
                parent: Some(child[0]),
                index: 0,
            },
        },
    );
    assert_eq!(history_len(&rig), before, "the loop was not written");
    let (_, why) = last_notice(&rig).unwrap_or_default();
    assert!(why.contains("loop"), "{why}");
    // Dropping the child on the top level routes it to master.
    raise(
        &mut rig,
        tree,
        RowsDropped {
            view: tree,
            keys: vec![child[0]],
            target: DropTarget {
                parent: None,
                index: 0,
            },
        },
    );
    assert_eq!(
        setting(&rig, "audio.bus.bus_2.output"),
        Some(Value::Text("master".into()))
    );
    // Remove: Music's keys go; master cannot be removed.
    select_bus(&mut rig, "Music");
    tap(&mut rig, &["bar", "remove"]);
    assert_eq!(setting(&rig, "audio.bus.bus_1.name"), None);
    select_bus(&mut rig, "Master");
    let before = history_len(&rig);
    tap(&mut rig, &["bar", "remove"]);
    assert_eq!(history_len(&rig), before);
    // Every edit undoes.
    while rig.state_hash() != start && history_len(&rig) > 0 {
        let h = rig.state_hash();
        undo(&mut rig);
        if rig.state_hash() == h {
            break;
        }
    }
    assert_eq!(rig.state_hash(), start);
}

#[test]
fn sends_offer_only_loop_free_targets_and_edit_as_commands() {
    let mut rig = common::rig(&[P]);
    tap(&mut rig, &["bar", "add"]); // bus (under master)
    select_bus(&mut rig, "Master");
    tap(&mut rig, &["bar", "add"]); // bus_2 (under master)
    select_bus(&mut rig, "Bus 2");
    let targets = w(&rig, &["body", "strip", "targets"]);
    // Bus 2 may send to Bus 1, never to itself.
    assert!(
        rows_with(&mut rig, targets, "Bus 2").is_empty(),
        "no send to itself"
    );
    let to_bus = rows_with(&mut rig, targets, "Send to Bus 1");
    let to_bus: Vec<u64> = to_bus.into_iter().collect();
    assert!(!to_bus.is_empty());
    raise(
        &mut rig,
        targets,
        RowActivated {
            view: targets,
            key: to_bus[0],
        },
    );
    assert_eq!(
        setting(&rig, "audio.bus.bus_2.send.bus_1.db"),
        Some(Value::Float(0.0))
    );
    // Now bus may not send back to bus_2 (a loop).
    select_bus(&mut rig, "Bus 1");
    assert!(
        rows_with(&mut rig, targets, "Bus 2").is_empty(),
        "a looping send is not offered"
    );
    // Send level: a gesture.
    select_bus(&mut rig, "Bus 2");
    let sends = w(&rig, &["body", "strip", "sends"]);
    let s = rows_with(&mut rig, sends, "post-fader");
    select_row(&mut rig, sends, s[0]);
    let level = w(&rig, &["body", "strip", "send_level"]);
    let before = history_len(&rig);
    drag(&mut rig, level, &[-2.0, -9.0, -12.0]);
    assert_eq!(history_len(&rig), before + 1);
    assert_eq!(
        setting(&rig, "audio.bus.bus_2.send.bus_1.db"),
        Some(Value::Float(-12.0))
    );
    toggle(&mut rig, &["body", "strip", "send_pre"]);
    assert_eq!(
        setting(&rig, "audio.bus.bus_2.send.bus_1.pre"),
        Some(Value::Bool(true))
    );
    tap(&mut rig, &["body", "strip", "remove_send"]);
    assert_eq!(setting(&rig, "audio.bus.bus_2.send.bus_1.db"), None);
}

/// `test_mixer_meters_idle` (gate row `C-mixer-meters-idle`, D-5): meters follow the
/// backend's live feed, and a steady tone costs 0 frames and 0 wakeups once settled. The
/// positive control is a backend that bumps its feed on every read (it keeps the editor
/// awake at the feed's rate cap).
fn meters_idle(audio: Option<Rc<dyn AudioBuses>>) -> Result<(), String> {
    let mut rig = common::rig_with(&[P], Default::default(), move |sv| {
        if let Some(a) = audio {
            sv.audio = a;
        }
    });
    tap(&mut rig, &["bar", "add"]);
    select_bus(&mut rig, "Bus 1");
    let vol = w(&rig, &["body", "strip", "volume"]);
    drag(&mut rig, vol, &[-6.0]);
    toggle(&mut rig, &["bar", "tone"]);
    assert_eq!(
        history_len(&rig),
        2,
        "a test tone is an audition, not an edit"
    );
    // Let the meters and the shell's debounced savers settle.
    rig.advance(Duration::from_secs(5));
    let bridge = w(&rig, &["body", "right", "meters"]);
    let shown = |rig: &Rig, bus: &str| {
        rig.h
            .ui
            .widget::<MeterBridge>(bridge)
            .and_then(|b| b.shown(bus))
    };
    let m = shown(&rig, "bus_1").unwrap_or_else(|| panic!("no meter for the bus"));
    assert!((m.peak_db() - (TONE_PEAK_DB - 6.0)).abs() < 1e-6, "{m:?}");
    let master = shown(&rig, "master").unwrap_or_else(|| panic!("no master meter"));
    assert!(
        (master.peak_db() - (TONE_PEAK_DB - 6.0)).abs() < 1e-6,
        "{master:?}"
    );
    // A steady tone: nothing changes, so nothing redraws and the loop sleeps (D-5).
    let refreshes = rig
        .h
        .ui
        .widget::<MeterBridge>(bridge)
        .map_or(0, MeterBridge::refreshes);
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    if (frames, wakeups) != (0, 0) {
        return Err(format!(
            "a steady meter kept the editor awake: {frames} frames, {wakeups} wakeups in 10 s"
        ));
    }
    assert_eq!(
        rig.h
            .ui
            .widget::<MeterBridge>(bridge)
            .map_or(0, MeterBridge::refreshes),
        refreshes
    );
    // Muting the bus changes the meters: they follow (one refresh).
    toggle(&mut rig, &["body", "strip", "mute"]);
    rig.advance(Duration::from_millis(300));
    let m = shown(&rig, "bus_1").unwrap_or_else(|| panic!("no meter"));
    assert!(m.peak_db() < -100.0, "muted: {m:?}");
    Ok(())
}

#[test]
fn test_mixer_meters_idle() {
    meters_idle(None).unwrap_or_else(|e| panic!("{e}"));
}

/// A backend that reports a change on every read.
struct Chatty {
    inner: MemoryAudio,
    cell: Arc<LiveCell>,
}

impl AudioBuses for Chatty {
    fn backend(&self) -> forge_editor::domain::BackendInfo {
        self.inner.backend()
    }
    fn set_mix(&self, m: &forge_editor::domain::audio::Mixer) {
        self.inner.set_mix(m);
        self.cell.bump();
    }
    fn set_preview(&self, bus: &str, on: bool) {
        self.inner.set_preview(bus, on);
        self.cell.bump();
    }
    fn previews(&self) -> std::collections::BTreeSet<String> {
        self.inner.previews()
    }
    fn meters(&self) -> std::collections::BTreeMap<String, forge_editor::domain::audio::Meter> {
        self.cell.bump();
        self.inner.meters()
    }
    fn feed(&self) -> Arc<dyn LiveSource> {
        self.cell.clone()
    }
    fn spatialize(
        &self,
        s: &forge_editor::domain::audio::Spatial,
        at: forge_editor::domain::audio::PreviewOffset,
    ) -> forge_editor::domain::audio::SpatialOut {
        self.inner.spatialize(s, at)
    }
}

#[test]
fn positive_control_a_backend_bumping_on_every_read_keeps_the_editor_awake() {
    let chatty: Rc<dyn AudioBuses> = Rc::new(Chatty {
        inner: MemoryAudio::new(),
        cell: LiveCell::new(),
    });
    let e = meters_idle(Some(chatty)).err().unwrap_or_default();
    assert!(
        e.contains("kept the editor awake"),
        "a chatty meter feed passed: {e:?}"
    );
}

#[test]
fn spatial_settings_are_commands_and_the_preview_is_session_state() {
    let mut rig = common::rig(&[P]);
    let min = w(&rig, &["body", "right", "spatial", "min_distance"]);
    raise(
        &mut rig,
        min,
        NumericCommitted {
            field: min,
            value: 2.0,
        },
    );
    assert_eq!(
        setting(&rig, "audio.spatial.min_distance"),
        Some(Value::Float(2.0))
    );
    let max = w(&rig, &["body", "right", "spatial", "max_distance"]);
    raise(
        &mut rig,
        max,
        NumericCommitted {
            field: max,
            value: 1.0,
        },
    );
    assert_eq!(
        setting(&rig, "audio.spatial.max_distance"),
        None,
        "max below min is refused"
    );
    let rolloff = w(&rig, &["body", "right", "spatial", "rolloff"]);
    rig.h.ui.set_focus(Some(rolloff), true);
    rig.chord(KeyCode::Down, Modifiers::NONE);
    rig.settle();
    assert_eq!(
        setting(&rig, "audio.spatial.rolloff"),
        Some(Value::Text("Linear".into()))
    );
    let n = history_len(&rig);
    let pad = w(&rig, &["body", "right", "pad"]);
    let before = rig
        .h
        .node(pad)
        .and_then(|x| x.value().map(str::to_string))
        .unwrap_or_default();
    rig.h.ui.set_focus(Some(pad), true);
    rig.chord(KeyCode::Right, Modifiers::NONE);
    rig.settle();
    let after = rig
        .h
        .node(pad)
        .and_then(|x| x.value().map(str::to_string))
        .unwrap_or_default();
    assert_ne!(
        before, after,
        "moving the source changes what the listener hears"
    );
    assert!(
        after.contains("distance") && after.contains("pan"),
        "{after}"
    );
    assert_eq!(history_len(&rig), n, "the preview source is session state");
}
