//! The sequencer (`forge.sequencer`, DoD M2-63) through the running shell: a clip keys any
//! reflected property of any entity (a float and a vector, picked from the selected
//! entity's properties), keys at the playhead, curve edits, interpolation, mute, and every
//! edit a command with its undo entry; the preview scrubs and plays through the play core.

mod common;

use std::time::Duration;

use common::{
    activate_row, history_len, last_notice, part, press, raise, rows_with, set_prop, setting,
    settings_under, spawn, text_of, type_into, undo,
};
use forge_cmd::{EntityKey, Value};
use forge_editor::testing::Rig;
use forge_panels_authoring::widgets::{KeyAt, Scrubbed};
use forge_ui::widgets::{Curve, CurveEdited, CurveKey, Interp as CurveInterp};
use forge_ui::{KeyCode, Modifiers, WidgetId};

const P: &str = "forge.sequencer";

fn w(rig: &Rig, path: &[&str]) -> WidgetId {
    part(rig, P, path)
}

fn tap(rig: &mut Rig, path: &[&str]) {
    let b = w(rig, path);
    press(rig, b);
}

/// An entity with a float and a vector property, selected; a clip "Walk".
fn setup() -> (Rig, EntityKey) {
    let mut rig = common::rig(&[P]);
    let e = spawn(&mut rig, "Crate");
    set_prop(
        &mut rig,
        e,
        "transform.position.local",
        Value::Vec3([0.0, 0.0, 0.0]),
    );
    set_prop(&mut rig, e, "light.intensity", Value::Float(1.0));
    rig.shell.session_mut().set_selection(vec![e]);
    rig.settle();
    let name = w(&rig, &["bar", "name"]);
    type_into(&mut rig, name, "Walk");
    tap(&mut rig, &["bar", "new_clip"]);
    (rig, e)
}

fn add_track(rig: &mut Rig, prop: &str) {
    let props = w(rig, &["body", "side", "props"]);
    let row = rows_with(rig, props, prop);
    let row = *row
        .first()
        .unwrap_or_else(|| panic!("no property row {prop}"));
    activate_row(rig, props, row);
}

fn timeline(rig: &Rig) -> WidgetId {
    w(rig, &["body", "timeline"])
}

fn scrub_to(rig: &mut Rig, t: f64) {
    let tl = timeline(rig);
    // The widget moves its own playhead before it raises; mirror that with keys.
    rig.h.ui.set_focus(Some(tl), true);
    rig.chord(KeyCode::Home, Modifiers::NONE);
    let frames = (t * 30.0).round() as usize;
    for _ in 0..frames {
        rig.h
            .ui
            .handle(forge_ui::InputEvent::Key(forge_ui::KeyEvent::press(
                KeyCode::Right,
                Modifiers::NONE,
            )));
    }
    rig.turn();
    rig.settle();
}

fn keys_of(rig: &Rig, prefix: &str) -> Vec<(String, Value)> {
    settings_under(rig, prefix)
}

#[test]
fn any_reflected_property_is_keyed_through_commands_and_undoes() {
    let (mut rig, e) = setup();
    assert_eq!(
        setting(&rig, "seq.clip.walk.name"),
        Some(Value::Text("Walk".into()))
    );
    // The selected entity's properties are listed, each one a track to add.
    let props = w(&rig, &["body", "side", "props"]);
    assert!(!rows_with(&mut rig, props, "transform.position.local (3D vector)").is_empty());
    assert!(!rows_with(&mut rig, props, "light.intensity (number)").is_empty());
    let before = history_len(&rig);
    add_track(&mut rig, "light.intensity");
    assert_eq!(
        history_len(&rig),
        before + 1,
        "adding a track is one undo entry"
    );
    let t = "seq.clip.walk.track.crate_light_intensity";
    assert_eq!(
        setting(&rig, &format!("{t}.entity")),
        Some(Value::Entity(e))
    );
    assert_eq!(
        setting(&rig, &format!("{t}.path")),
        Some(Value::Text("light.intensity".into()))
    );
    assert_eq!(
        setting(&rig, &format!("{t}.key.k0.v")),
        Some(Value::Float(1.0))
    );
    assert_eq!(
        setting(&rig, &format!("{t}.key.k0.t")),
        Some(Value::Float(0.0))
    );
    // The same property again is refused, with the reason.
    add_track(&mut rig, "light.intensity");
    assert_eq!(history_len(&rig), before + 1);
    let (_, why) = last_notice(&rig).unwrap_or_default();
    assert!(why.contains("already animates"), "{why}");
    // Move the playhead to 1 s, change the property, key it (the Key button).
    scrub_to(&mut rig, 1.0);
    set_prop(&mut rig, e, "light.intensity", Value::Float(5.0));
    let h = history_len(&rig);
    tap(&mut rig, &["bar", "key"]);
    assert_eq!(history_len(&rig), h + 1, "a key is one undo entry");
    assert_eq!(
        setting(&rig, &format!("{t}.key.k1.t")),
        Some(Value::Float(1.0))
    );
    assert_eq!(
        setting(&rig, &format!("{t}.key.k1.v")),
        Some(Value::Float(5.0))
    );
    // Keying the same frame again changes that key, it adds none.
    set_prop(&mut rig, e, "light.intensity", Value::Float(6.0));
    tap(&mut rig, &["bar", "key"]);
    assert_eq!(
        setting(&rig, &format!("{t}.key.k1.v")),
        Some(Value::Float(6.0))
    );
    assert!(setting(&rig, &format!("{t}.key.k2.t")).is_none());
    // A vector property: a second track, keyed with K at 2 s.
    add_track(&mut rig, "transform.position.local");
    let v = "seq.clip.walk.track.crate_transform_position_local";
    scrub_to(&mut rig, 2.0);
    set_prop(
        &mut rig,
        e,
        "transform.position.local",
        Value::Vec3([4.0, 0.0, -2.0]),
    );
    let tl = timeline(&rig);
    raise(
        &mut rig,
        tl,
        KeyAt {
            track: "crate_transform_position_local".into(),
            t: 2.0,
        },
    );
    assert_eq!(
        setting(&rig, &format!("{v}.key.k1.v")),
        Some(Value::Vec3([4.0, 0.0, -2.0]))
    );
    // Undo removes that key and nothing else.
    let n = keys_of(&rig, "seq.").len();
    undo(&mut rig);
    assert!(setting(&rig, &format!("{v}.key.k1.v")).is_none());
    assert_eq!(keys_of(&rig, "seq.").len(), n - 3);
    // Mute is a command.
    tap(&mut rig, &["bar", "mute"]);
    let muted = keys_of(&rig, "seq.")
        .iter()
        .any(|(k, v)| k.ends_with(".muted") && *v == Value::Bool(true));
    assert!(muted, "the selected track is muted");
}

#[test]
fn the_change_log_reader_equals_a_fresh_read() {
    use forge_editor::authoring::timeline::TimelineDoc;
    let (mut rig, e) = setup();
    let (mut doc, mut seq) = {
        let m = rig.shell.mirror();
        (TimelineDoc::read(&m), m.setting_change_seq())
    };
    let mut step = |rig: &mut Rig| {
        let m = rig.shell.mirror();
        let keys = m
            .setting_changes_since(seq)
            .unwrap_or_else(|| panic!("the log reaches back"));
        doc.apply_changes(&m, keys);
        seq = m.setting_change_seq();
        assert_eq!(doc, TimelineDoc::read(&m), "incremental == fresh");
    };
    add_track(&mut rig, "light.intensity");
    step(&mut rig);
    add_track(&mut rig, "transform.position.local");
    step(&mut rig);
    scrub_to(&mut rig, 1.0);
    set_prop(&mut rig, e, "light.intensity", Value::Float(2.0));
    tap(&mut rig, &["bar", "key"]);
    step(&mut rig);
    tap(&mut rig, &["bar", "mute"]);
    step(&mut rig);
    tap(&mut rig, &["bar", "loop"]);
    step(&mut rig);
    tap(&mut rig, &["bar", "delete_track"]);
    step(&mut rig);
    undo(&mut rig);
    step(&mut rig);
    // Another clip, then the first one deleted from the clip list.
    let name = w(&rig, &["bar", "name"]);
    type_into(&mut rig, name, "Run");
    tap(&mut rig, &["bar", "new_clip"]);
    step(&mut rig);
    let clips = w(&rig, &["body", "side", "clips"]);
    let row = rows_with(&mut rig, clips, "Walk (");
    common::select_row(&mut rig, clips, row[0]);
    raise(
        &mut rig,
        clips,
        forge_ui::widgets::RowsDeleteRequested {
            view: clips,
            keys: vec![row[0]],
        },
    );
    step(&mut rig);
    assert!(setting(&rig, "seq.clip.walk.name").is_none());
}

#[test]
fn a_key_drag_is_one_undo_entry_and_a_curve_edit_one_transaction() {
    let (mut rig, e) = setup();
    add_track(&mut rig, "light.intensity");
    scrub_to(&mut rig, 1.0);
    set_prop(&mut rig, e, "light.intensity", Value::Float(3.0));
    tap(&mut rig, &["bar", "key"]);
    let t = "seq.clip.walk.track.crate_light_intensity";
    // Select the key at 1 s from the keyboard ([ ] walk the keys) and drag it +0.5 s.
    let tl = timeline(&rig);
    rig.h.ui.set_focus(Some(tl), true);
    rig.chord(KeyCode::Home, Modifiers::NONE);
    rig.chord(KeyCode::Char(']'), Modifiers::NONE);
    rig.settle();
    let before = history_len(&rig);
    let drag = |rig: &mut Rig, dt: f64, phase| {
        raise(
            rig,
            tl,
            forge_panels_authoring::widgets::KeysDragged {
                track: "crate_light_intensity".into(),
                keys: vec!["k1".into()],
                dt,
                phase,
            },
        );
    };
    use forge_panels_authoring::widgets::DragPhase::{Begin, End, Move};
    drag(&mut rig, 0.1, Begin);
    for i in 2..=15 {
        drag(&mut rig, f64::from(i) * 0.1 / 3.0, Move);
    }
    drag(&mut rig, 0.5, End);
    assert_eq!(history_len(&rig), before + 1, "the whole drag is one entry");
    assert_eq!(
        setting(&rig, &format!("{t}.key.k1.t")),
        Some(Value::Float(1.5))
    );
    undo(&mut rig);
    assert_eq!(
        setting(&rig, &format!("{t}.key.k1.t")),
        Some(Value::Float(1.0))
    );
    // The curve editor: the track's curve; one edit raises values and adds a key.
    let ed = w(&rig, &["curves", "editor"]);
    let edited = Curve::new(vec![
        CurveKey {
            t: 0.0,
            v: 2.0,
            interp: CurveInterp::Linear,
        },
        CurveKey {
            t: 1.0,
            v: 3.0,
            interp: CurveInterp::Smooth,
        },
        CurveKey {
            t: 2.0,
            v: 0.5,
            interp: CurveInterp::Smooth,
        },
    ]);
    let sig = rig
        .h
        .ui
        .widget::<forge_ui::widgets::CurveEditor>(ed)
        .map(|_| ())
        .is_some();
    assert!(sig, "the curve editor is shown");
    let before = history_len(&rig);
    // What the editor writes to its signal, then its once-per-gesture action.
    set_curve(&mut rig, ed, edited);
    raise(&mut rig, ed, CurveEdited { editor: ed });
    assert_eq!(
        history_len(&rig),
        before + 1,
        "a curve edit is one transaction"
    );
    assert_eq!(
        setting(&rig, &format!("{t}.key.k0.v")),
        Some(Value::Float(2.0))
    );
    assert_eq!(
        setting(&rig, &format!("{t}.key.k0.interp")),
        Some(Value::Text("Linear".into()))
    );
    assert_eq!(
        setting(&rig, &format!("{t}.key.k2.t")),
        Some(Value::Float(2.0))
    );
    assert_eq!(
        setting(&rig, &format!("{t}.key.k2.v")),
        Some(Value::Float(0.5))
    );
    // Interpolation of the selected key.
    rig.h.ui.set_focus(Some(tl), true);
    rig.chord(KeyCode::Home, Modifiers::NONE);
    rig.chord(KeyCode::Char(']'), Modifiers::NONE);
    rig.settle();
    tap(&mut rig, &["curves", "Constant"]);
    assert_eq!(
        setting(&rig, &format!("{t}.key.k1.interp")),
        Some(Value::Text("Constant".into()))
    );
}

/// Write the curve editor's model as a pointer edit does (the editor writes its signal live
/// and raises `CurveEdited` once per gesture).
fn set_curve(rig: &mut Rig, ed: WidgetId, c: Curve) {
    let s = rig
        .h
        .ui
        .widget::<forge_ui::widgets::CurveEditor>(ed)
        .map(forge_ui::widgets::CurveEditor::model)
        .unwrap_or_else(|| panic!("curve editor"));
    s.set(rig.h.ui.rt_mut(), c);
    rig.turn();
}

#[test]
fn the_preview_scrubs_and_plays_through_the_play_core() {
    let (mut rig, e) = setup();
    add_track(&mut rig, "transform.position.local");
    scrub_to(&mut rig, 2.0);
    set_prop(
        &mut rig,
        e,
        "transform.position.local",
        Value::Vec3([4.0, 0.0, 0.0]),
    );
    tap(&mut rig, &["bar", "key"]);
    // Linear from the first key so the halfway point is exact: select it with `[`.
    let tl = timeline(&rig);
    scrub_to(&mut rig, 0.5);
    rig.h.ui.set_focus(Some(tl), true);
    rig.chord(KeyCode::Char('['), Modifiers::NONE);
    rig.settle();
    tap(&mut rig, &["curves", "Linear"]);
    assert_eq!(
        setting(
            &rig,
            "seq.clip.walk.track.crate_transform_position_local.key.k0.interp"
        ),
        Some(Value::Text("Linear".into()))
    );
    let hash = rig.state_hash();
    scrub_to(&mut rig, 1.0);
    let sv = rig.shell.handles().services_rc();
    {
        let pv = sv.anim_preview.borrow();
        let (over, _) = pv.overrides().unwrap_or_else(|| panic!("a preview"));
        let (pos, _, _) = over
            .get(&e)
            .unwrap_or_else(|| panic!("the entity is previewed"));
        assert_eq!(
            pos.local.x, 2.0,
            "halfway between the keys, from the play core"
        );
        assert!(pv.forks() >= 1);
    }
    assert_eq!(
        rig.state_hash(),
        hash,
        "scrubbing writes nothing to the project"
    );
    // An event track keyed at 0.5 s; play from 0 fires it on the play core's steps.
    let name = w(&rig, &["bar", "name"]);
    type_into(&mut rig, name, "Footsteps");
    tap(&mut rig, &["bar", "new_event"]);
    raise(
        &mut rig,
        tl,
        KeyAt {
            track: "footsteps".into(),
            t: 0.5,
        },
    );
    let hash = rig.state_hash();
    rig.h.ui.set_focus(Some(tl), true);
    rig.chord(KeyCode::Home, Modifiers::NONE);
    tap(&mut rig, &["bar", "play"]);
    assert!(sv.anim_preview.borrow().is_playing());
    let (frames, _) = rig.advance(Duration::from_millis(1000));
    assert!(frames > 0, "a playing clip draws");
    let t = sv.anim_preview.borrow().time();
    let n = (t * 60.0).round() as u64;
    assert_eq!(
        forge_editor::play::tick_of_step(n).0 as f64 / 1e6,
        t,
        "the playhead moves in whole play-core steps"
    );
    assert!(t > 0.5 && t <= 1.1, "{t}");
    let status = text_of(&rig, w(&rig, &["status"]));
    assert!(status.contains("Event Footsteps"), "{status}");
    tap(&mut rig, &["bar", "play"]);
    assert!(!sv.anim_preview.borrow().is_playing(), "paused");
    assert_eq!(
        rig.state_hash(),
        hash,
        "playing writes nothing to the project"
    );
    // Paused or stopped: the editor idles (after the frame already asked for and the
    // shell's own one-shot deadlines).
    rig.advance(Duration::from_secs(3));
    assert_eq!(
        rig.h.ui.debug_counts().get("anims"),
        Some(&0),
        "no frame loop"
    );
    let (frames, wakes) = rig.advance(Duration::from_secs(5));
    assert_eq!((frames, wakes), (0, 0), "a paused preview costs nothing");
    tap(&mut rig, &["bar", "stop"]);
    assert!(sv.anim_preview.borrow().overrides().is_none());
    let _ = Scrubbed { t: 0.0 };
}
