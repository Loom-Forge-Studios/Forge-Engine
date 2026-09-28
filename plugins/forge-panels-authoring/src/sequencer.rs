//! The **Sequencer & timeline** (`forge.sequencer`, Ch.21 §21.21, DoD M2-63; Ch.19
//! animate-any-property, Ch.6): clips of tracks over `forge_editor::authoring::timeline`.
//!
//! * **Key any reflected property on any entity**: select an entity (hierarchy, viewport)
//!   and its properties are listed — every reflect path it has; activating one adds a track
//!   keyed with the current value at the playhead. **K** (or the Key button, or a
//!   double-click on the lane) keys the track's current value at the playhead.
//! * **Tracks**: property and event tracks, mute, delete; keys drag in time as one gesture
//!   (one undo entry), nudge a frame with Ctrl+arrows, delete with Delete.
//! * **Curves**: the selected track's curve (a component of a vector) in the curve
//!   editor; an edit gesture there is one transaction; interpolation per key.
//! * **Preview on the play core**: scrubbing and playing evaluate the clip and fork
//!   `forge-sim` from the animated entities (`authoring::preview`); the viewport shows the
//!   result while no play session runs. The preview is session state; the project is never
//!   written by it.
//!
//! Every project edit is a command (I7).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use forge_cmd::{EntityKey, Value};
use forge_editor::authoring::timeline::{
    self as tl, Clip, Interp, TimelineDoc, Track, TrackKind, components,
};
use forge_editor::emitter::Gesture;
use forge_editor::mirror::ProjectMirror;
use forge_editor::panel_rt::PanelAct;
use forge_editor::panels::PanelCx;
use forge_ui::widgets::{
    Button, Container, Curve, CurveEdited, CurveEditor, IntegerCommitted, IntegerField, Label,
    LabelKind, NumericCommitted, NumericField, Pressed, RadioGroup, RowActivated, RowItem,
    RowRenamed, RowsDeleteRequested, SelectionChanged, TextField, VirtualTree,
};
use forge_ui::{NodeStyle, Role, Signal, Ui, UiEvent, WidgetId};

use crate::common::{Row, Rows, key_of, refuse, select, show_rows, watch};
use crate::widgets::{
    DeleteKeysRequested, DragPhase, KeyAt, KeysDragged, KeysPicked, PlayTick, PlayToggled,
    Scrubbed, TimelineData, TimelineView, TrackPicked, TrackRow,
};

const PREFIX: &str = "seq.";

struct Seq {
    doc: TimelineDoc,
    seen: u64,
    seen_sel: u64,
    sel_clip: Option<String>,
    pending_clip: Option<String>,
    clip_rows: Rows<String>,
    prop_rows: Rows<String>,
    prop_entity: Option<EntityKey>,
    /// The mirror revision the property list was built at.
    seen_mrev: u64,
    view: Rc<RefCell<TimelineData>>,
    tl: WidgetId,
    clips: WidgetId,
    /// The clips list's empty state (M2-70).
    clips_empty: WidgetId,
    props: WidgetId,
    curve_ed: WidgetId,
    curve: Signal<Curve>,
    comp: Signal<usize>,
    shown_comp: usize,
    status: Signal<String>,
    problems: Signal<String>,
    play_label: Signal<String>,
    loop_label: Signal<String>,
    length: Signal<f64>,
    fps: Signal<i128>,
    gesture: Option<Gesture>,
    /// The clip and track as they were when a key drag began.
    drag_base: Option<(Clip, Track)>,
    /// The mirror's setting-change sequence the doc is current to (`u64::MAX`: never read).
    change_seq: u64,
    /// W2 fault (`sequencer_full_show`): re-show everything on every change.
    full_show: bool,
    /// The guard's probe: timeline rows built (`None` in the editor).
    rows_probe: Option<Rc<std::cell::Cell<usize>>>,
    /// W2 fault (`timeline_pause_keeps_playing`).
    pause_keeps_playing: bool,
}

/// The timeline row of a track (its label, its keys' ids and times).
fn track_row(m: &ProjectMirror, t: &Track) -> TrackRow {
    TrackRow {
        id: t.id.clone(),
        label: match (t.kind, t.entity) {
            (TrackKind::Property, Some(e)) => format!(
                "{}.{}",
                m.entity(e)
                    .map_or_else(|| e.to_string(), |x| x.name.clone()),
                t.path
            ),
            (TrackKind::Property, None) => format!("?.{}", t.path),
            (TrackKind::Event, _) => format!("\u{2691} {}", t.path),
        },
        keys: t.keys.iter().map(|k| (k.id.clone(), k.t)).collect(),
        muted: t.muted,
    }
}

impl Seq {
    fn say(&self, ui: &mut Ui, s: impl Into<String>) {
        self.status.set(ui.rt_mut(), s.into());
    }
    fn clip(&self) -> Option<&Clip> {
        self.sel_clip.as_ref().and_then(|c| self.doc.clips.get(c))
    }
    fn track(&self) -> Option<&Track> {
        let t = self.view.borrow().track.clone()?;
        self.clip()?.tracks.get(&t)
    }

    fn reread(
        &mut self,
        ui: &mut Ui,
        m: &ProjectMirror,
        preview: &RefCell<forge_editor::authoring::preview::TimelinePreview>,
    ) {
        // Apply the change log (a key drag re-reads one track); re-read everything only
        // the first time or when the log no longer reaches back far enough.
        let changes = if self.change_seq == u64::MAX {
            None
        } else {
            m.setting_changes_since(self.change_seq)
                .map(|keys| self.doc.apply_changes(m, keys))
        };
        if changes.is_none() {
            self.doc = TimelineDoc::read(m);
        }
        let mut sel_moved = false;
        self.change_seq = m.setting_change_seq();
        let p = if self.doc.problems.is_empty() {
            String::new()
        } else {
            forge_ui::trf!(
                "\u{26a0} {n} value(s) skipped: {why}",
                n = self.doc.problems.len(),
                why = self.doc.problems.join("; ")
            )
        };
        self.problems.set(ui.rt_mut(), p);
        if let Some(c) = self.pending_clip.take()
            && self.doc.clips.contains_key(&c)
        {
            sel_moved |= self.sel_clip.as_deref() != Some(c.as_str());
            self.sel_clip = Some(c);
        }
        if self
            .sel_clip
            .as_ref()
            .is_none_or(|s| !self.doc.clips.contains_key(s))
        {
            sel_moved = true;
            self.sel_clip = self.doc.clips.keys().next().cloned();
        }
        // Re-show only what the change log touched when it is tracks of the clip shown (a
        // key drag: one track per frame); everything else re-shows the panel.
        match changes {
            Some(ch) if !self.full_show && !sel_moved => {
                if !self.show_tracks(ui, m, &ch) {
                    self.show(ui, m);
                }
            }
            _ => self.show(ui, m),
        }
        // Keep a preview of this clip current (a key moved under the playhead).
        let mut pv = preview.borrow_mut();
        match self.clip() {
            Some(c) if pv.clip() == Some(c.id.as_str()) => pv.refresh(m, c),
            _ if pv.clip().is_some() => pv.stop(),
            _ => {}
        }
    }

    fn show(&mut self, ui: &mut Ui, m: &ProjectMirror) {
        let rows: Vec<Row> = self
            .doc
            .clips
            .values()
            .map(|c| {
                (
                    None,
                    key_of(&["clip", &c.id]),
                    RowItem::new(forge_ui::trf!(
                        "{name} ({length} s, {tracks_count} tracks)",
                        name = c.name,
                        length = format!("{:.2}", c.length),
                        tracks_count = c.tracks.len()
                    )),
                )
            })
            .collect();
        let map = self
            .doc
            .clips
            .keys()
            .map(|id| (key_of(&["clip", id]), id.clone()))
            .collect();
        show_rows(ui, self.clips, rows, map, &mut self.clip_rows);
        let _ = ui.set_hidden(self.clips_empty, !self.doc.clips.is_empty());
        if let Some(s) = &self.sel_clip {
            select(ui, self.clips, key_of(&["clip", s]));
        }
        let (length, fps, looping, tracks) = match self.clip() {
            Some(c) => (
                c.length,
                c.fps,
                c.looping,
                c.tracks
                    .values()
                    .map(|t| track_row(m, t))
                    .collect::<Vec<_>>(),
            ),
            None => (tl::DEFAULT_LENGTH, tl::DEFAULT_FPS, false, Vec::new()),
        };
        if let Some(p) = &self.rows_probe {
            p.set(p.get() + tracks.len());
        }
        {
            let mut v = self.view.borrow_mut();
            v.length = length;
            v.fps = fps;
            v.time = v.time.clamp(0.0, length);
            if v.track
                .as_ref()
                .is_some_and(|t| !tracks.iter().any(|r| &r.id == t))
            {
                v.track = None;
                v.keys.clear();
            }
            if v.track.is_none() {
                v.track = tracks.first().map(|r| r.id.clone());
            }
            let keep: Vec<String> = v
                .track
                .as_ref()
                .and_then(|t| tracks.iter().find(|r| &r.id == t))
                .map(|r| {
                    v.keys
                        .iter()
                        .filter(|k| r.keys.iter().any(|(id, _)| id == *k))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            v.keys = keep;
            v.tracks = tracks;
        }
        self.length.set(ui.rt_mut(), length);
        self.fps.set(ui.rt_mut(), i128::from(fps));
        self.loop_label.set(
            ui.rt_mut(),
            if looping {
                forge_ui::tr!("Loop: on")
            } else {
                forge_ui::tr!("Loop: off")
            }
            .to_string(),
        );
        ui.invalidate(self.tl, forge_ui::Dirty::PAINT | forge_ui::Dirty::A11Y);
        self.show_curve(ui);
    }

    /// Re-show only the tracks `ch` re-read, when that is all it touched: tracks of the clip
    /// shown, each still a row (none added or removed). A key drag re-reads one track per
    /// frame, so it rebuilds one row — not every track's row and key ids, nor the clip
    /// list. `false`: the change is more than that (the caller re-shows everything).
    fn show_tracks(&mut self, ui: &mut Ui, m: &ProjectMirror, ch: &tl::DocChanges) -> bool {
        if !ch.clips.is_empty() {
            return false;
        }
        let Some(clip) = self.sel_clip.as_ref().and_then(|c| self.doc.clips.get(c)) else {
            return false;
        };
        let mut v = self.view.borrow_mut();
        let mut at = Vec::with_capacity(ch.tracks.len());
        for (c, t) in &ch.tracks {
            let (Some(track), Some(i)) = (
                clip.tracks.get(t).filter(|_| c == &clip.id),
                v.tracks.iter().position(|r| &r.id == t),
            ) else {
                return false;
            };
            at.push((i, track));
        }
        let mut curve = false;
        for (i, track) in at {
            v.tracks[i] = track_row(m, track);
            if v.track.as_deref() == Some(track.id.as_str()) {
                // Keep the selected keys that still exist.
                let row = &v.tracks[i];
                let keep: Vec<String> = v
                    .keys
                    .iter()
                    .filter(|k| row.keys.iter().any(|(id, _)| id == *k))
                    .cloned()
                    .collect();
                v.keys = keep;
                curve = true;
            }
        }
        if let Some(p) = &self.rows_probe {
            p.set(p.get() + ch.tracks.len());
        }
        drop(v);
        if !ch.tracks.is_empty() {
            ui.invalidate(self.tl, forge_ui::Dirty::PAINT | forge_ui::Dirty::A11Y);
        }
        if curve {
            self.show_curve(ui);
        }
        true
    }

    /// The selected track's curve in the curve editor, its range fitted to the keys.
    fn show_curve(&mut self, ui: &mut Ui) {
        let length = self.view.borrow().length;
        let comp = self.comp.get(ui.rt());
        let curve = match self.track() {
            Some(t) if t.keys.first().is_some_and(|k| components(&k.v) > comp) => t.curve(comp),
            _ => Curve::default(),
        };
        let (lo, hi) = curve
            .keys
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), k| {
                (a.min(k.v), b.max(k.v))
            });
        let (lo, hi) = if lo.is_finite() {
            let pad = ((hi - lo) * 0.15).max(1.0);
            (lo - pad, hi + pad)
        } else {
            (0.0, 1.0)
        };
        if let Some(ed) = ui.widget_mut::<CurveEditor>(self.curve_ed) {
            ed.set_range((0.0, length.max(1e-3)), (lo, hi));
        }
        self.shown_comp = comp;
        self.curve.set(ui.rt_mut(), curve);
        ui.invalidate(
            self.curve_ed,
            forge_ui::Dirty::PAINT | forge_ui::Dirty::A11Y,
        );
    }

    /// The selected entity's properties, each one a track to add.
    fn show_props(&mut self, ui: &mut Ui, m: &ProjectMirror, entity: Option<EntityKey>) {
        self.prop_entity = entity;
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        if let Some(e) = entity.and_then(|k| m.entity(k).map(|x| (k, x))) {
            for (path, v) in &e.1.properties {
                let k = key_of(&["prop", &e.0.to_string(), path]);
                let kind = forge_editor::command_labels::value_kind_label(v);
                rows.push((None, k, RowItem::new(format!("{path} ({kind})"))));
                map.insert(k, path.clone());
            }
        }
        show_rows(ui, self.props, rows, map, &mut self.prop_rows);
    }
}

fn scrub(act: &mut PanelAct, s: &Seq, t: f64) {
    if let Some(c) = s.clip() {
        let mut pv = act.services.anim_preview.borrow_mut();
        pv.scrub(act.mirror, c, t);
        let now = pv.time();
        let values = pv.values().to_vec();
        drop(pv);
        s.view.borrow_mut().time = now;
        if act.services.faults.timeline_preview_writes_project() {
            // W2 fault: a preview that is not a sandbox (it edits the project).
            act.cmd.emit_all(
                forge_ui::tr!("Preview"),
                values
                    .into_iter()
                    .map(
                        |(entity, path, value)| forge_cmd::EditorCommand::SetProperty {
                            entity,
                            path,
                            value,
                        },
                    )
                    .collect(),
            );
        }
        if let Some(e) = act.services.anim_preview.borrow_mut().take_error() {
            s.say(act.ui, forge_ui::trf!("\u{26a0} Preview: {e}", e));
        }
        act.want_turn();
    }
}

fn set_playing(act: &mut PanelAct, s: &Seq, on: bool) {
    // W2 fault: a pause that leaves the timeline playing (its frame loop never stops).
    s.view.borrow_mut().playing = on || s.pause_keeps_playing;
    s.play_label.set(
        act.ui.rt_mut(),
        if on {
            forge_ui::tr!("Pause")
        } else {
            forge_ui::tr!("Play")
        }
        .to_string(),
    );
    if on {
        // Wake the timeline's frame loop (it runs only while playing, D-5).
        act.ui.send(s.tl, &UiEvent::AnimFrame);
    }
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
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Sequencer actions")),
        )?;
        let name = pb.b.signal(String::new());
        pb.b.add(
            bar,
            "name",
            NodeStyle::leaf().width(150.0),
            TextField::new(name, forge_ui::tr!("Name")).placeholder(forge_ui::tr!("Clip or event name")),
        )?;
        let new_clip = pb.b.add(bar, "new_clip", NodeStyle::leaf(), Button::new(forge_ui::tr!("New clip")))?;
        let new_event = pb.b.add(
            bar,
            "new_event",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Event track")),
        )?;
        let length = pb.b.signal(tl::DEFAULT_LENGTH);
        pb.b.add(
            bar,
            "length_label",
            NodeStyle::leaf(),
            Label::new(forge_ui::tr!("Length (s)")).kind(LabelKind::Small),
        )?;
        let length_f = pb.b.add(
            bar,
            "length",
            NodeStyle::leaf().width(110.0),
            NumericField::new(length, forge_ui::tr!("Length (s)")),
        )?;
        let fps = pb.b.signal(i128::from(tl::DEFAULT_FPS));
        pb.b.add(
            bar,
            "fps_label",
            NodeStyle::leaf(),
            Label::new(forge_ui::tr!("FPS")).kind(LabelKind::Small),
        )?;
        let fps_f = pb.b.add(
            bar,
            "fps",
            NodeStyle::leaf().width(80.0),
            IntegerField::new(fps, forge_ui::tr!("Frames per second")),
        )?;
        let loop_label = pb.b.signal(forge_ui::tr!("Loop: off").to_string());
        let loop_b = pb.b.add(bar, "loop", NodeStyle::leaf(), Button::new(loop_label))?;
        let play_label = pb.b.signal(forge_ui::tr!("Play").to_string());
        let play = pb.b.add(bar, "play", NodeStyle::leaf(), Button::new(play_label))?;
        let stop = pb.b.add(bar, "stop", NodeStyle::leaf(), Button::new(forge_ui::tr!("Stop preview")))?;
        let key_b = pb.b.add(bar, "key", NodeStyle::leaf(), Button::new(forge_ui::tr!("Key (K)")))?;
        let mute = pb.b.add(bar, "mute", NodeStyle::leaf(), Button::new(forge_ui::tr!("Mute track")))?;
        let del_track = pb.b.add(
            bar,
            "delete_track",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Delete track")),
        )?;
        let body = pb.b.add(
            pb.parent,
            "body",
            crate::common::body_row(space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Sequencer")),
        )?;
        let side = pb.b.add(
            body,
            "side",
            NodeStyle::column(space).width(230.0),
            Container::group(),
        )?;
        let clips_empty = pb.b.add(
            side,
            "clips_empty",
            NodeStyle::leaf(),
            forge_ui::widgets::EmptyState::new(forge_ui::tr!(
                "No clips yet: name one above and press New clip."
            )),
        )?;
        let clips = pb.b.add(
            side,
            "clips",
            crate::common::fill(0.0, 90.0),
            VirtualTree::list(forge_ui::tr!("Clips")).single_select(),
        )?;
        let props = pb.b.add(
            side,
            "props",
            crate::common::fill(0.0, 120.0),
            VirtualTree::list(forge_ui::tr!("Properties of the selected entity (Enter adds a track)"))
                .read_only()
                .single_select(),
        )?;
        let view = Rc::new(RefCell::new(TimelineData {
            length: tl::DEFAULT_LENGTH,
            fps: tl::DEFAULT_FPS,
            ..TimelineData::default()
        }));
        let tlw = pb.b.add(
            body,
            "timeline",
            crate::common::fill(320.0, 160.0),
            TimelineView::new(view.clone())
                .with_fault_always_animating(sv.faults.timeline_always_animating())
                .with_fault_paint_every_key(sv.faults.timeline_paint_every_key()),
        )?;
        let curve_row = pb.b.add(
            pb.parent,
            "curves",
            NodeStyle::row(space).padding(space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Curve")),
        )?;
        let comp = pb.b.signal(0usize);
        pb.b.add(
            curve_row,
            "component",
            NodeStyle::leaf(),
            RadioGroup::new(forge_ui::tr!("Component"), &[forge_ui::tr!("X"), forge_ui::tr!("Y"), forge_ui::tr!("Z")], comp),
        )?;
        let comp_relay = pb.b.add(
            curve_row,
            "component_relay",
            NodeStyle::leaf(),
            forge_ui::widgets::SignalRelay::new(comp.any()),
        )?;
        let mut interps = Vec::new();
        for i in Interp::ALL {
            let b = pb.b.add(
                curve_row,
                forge_ui::Key::Static(i.name()),
                NodeStyle::leaf(),
                Button::new(forge_ui::l10n::tr(i.name())),
            )?;
            interps.push((b, i));
        }
        let curve = pb.b.signal(Curve::default());
        let curve_ed = pb.b.add(
            curve_row,
            "editor",
            crate::common::fill(260.0, 150.0),
            CurveEditor::new(curve, forge_ui::tr!("Track curve")),
        )?;
        let status = pb.b.signal(
            forge_ui::tr!("Select an entity, pick a property, and key it over time.").to_string(),
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
            Label::new(
                forge_ui::tr!("Preview: the clip is evaluated and forked into the play core (forge-sim); the viewport shows it while no play session runs."),
            )
            .kind(LabelKind::Small)
            .wrapping(),
        )?;
        let no_gesture = sv.faults.timeline_drag_without_gesture();

        let st = Rc::new(RefCell::new(Seq {
            doc: TimelineDoc::default(),
            seen: u64::MAX,
            seen_sel: u64::MAX,
            sel_clip: None,
            pending_clip: None,
            clip_rows: Rows::default(),
            prop_rows: Rows::default(),
            prop_entity: None,
            seen_mrev: u64::MAX,
            view,
            tl: tlw,
            clips,
            clips_empty,
            props,
            curve_ed,
            curve,
            comp,
            shown_comp: 0,
            status,
            problems,
            play_label,
            loop_label,
            length,
            fps,
            gesture: None,
            drag_base: None,
            change_seq: u64::MAX,
            full_show: sv.faults.sequencer_full_show(),
            rows_probe: sv.faults.sequencer_rows_probe().clone(),
            pause_keeps_playing: sv.faults.timeline_pause_keeps_playing(),
        }));

        let s = st.clone();
        pb.on(new_clip, move |act, _: &Pressed| {
            let mut sq = s.borrow_mut();
            let n = name.get(act.ui.rt());
            let n = if n.trim().is_empty() { forge_ui::tr!("Clip").to_string() } else { n.trim().to_string() };
            let (id, cmds) = tl::new_clip(&sq.doc, &n);
            act.cmd.emit_all(forge_ui::tr!("New clip"), cmds);
            sq.pending_clip = Some(id.clone());
            sq.say(act.ui, forge_ui::trf!("Created clip {id}.", id));
        });
        let s = st.clone();
        pb.on(new_event, move |act, _: &Pressed| {
            let sq = s.borrow();
            let Some(c) = sq.clip() else {
                refuse(act.session, forge_ui::tr!("Event track"), forge_ui::tr!("Create or select a clip first."));
                return;
            };
            let n = name.get(act.ui.rt());
            let n = if n.trim().is_empty() { forge_ui::tr!("Events").to_string() } else { n.trim().to_string() };
            let (id, cmds) = tl::add_event_track(c, &n);
            act.cmd.emit_all(forge_ui::tr!("Add event track"), cmds);
            sq.say(act.ui, forge_ui::trf!("Added event track {id}.", id));
        });
        let s = st.clone();
        pb.on(length_f, move |act, e: &NumericCommitted| {
            let sq = s.borrow();
            let Some(c) = sq.clip() else { return };
            match tl::set_length(c, e.value) {
                Ok(cmds) => {
                    act.cmd.emit_all(forge_ui::tr!("Clip length"), cmds);
                }
                Err(why) => {
                    refuse(act.session, forge_ui::tr!("Clip length"), &why);
                    sq.length.set(act.ui.rt_mut(), c.length);
                }
            }
        });
        let s = st.clone();
        pb.on(fps_f, move |act, e: &IntegerCommitted| {
            let sq = s.borrow();
            let Some(c) = sq.clip() else { return };
            let v = i64::try_from(e.value).unwrap_or(i64::MAX);
            match tl::set_fps(c, v) {
                Ok(cmds) => {
                    act.cmd.emit_all(forge_ui::tr!("Clip frame rate"), cmds);
                }
                Err(why) => {
                    refuse(act.session, forge_ui::tr!("Frames per second"), &why);
                    sq.fps.set(act.ui.rt_mut(), i128::from(c.fps));
                }
            }
        });
        let s = st.clone();
        pb.on(loop_b, move |act, _: &Pressed| {
            let sq = s.borrow();
            if let Some(c) = sq.clip() {
                act.cmd.emit_all(forge_ui::tr!("Clip loop"), tl::set_loop(c, !c.looping));
            }
        });
        let s = st.clone();
        pb.on(play, move |act, _: &Pressed| toggle_play(act, &s));
        let s = st.clone();
        pb.on(tlw, move |act, _: &PlayToggled| toggle_play(act, &s));
        let s = st.clone();
        pb.on(stop, move |act, _: &Pressed| {
            let sq = s.borrow();
            act.services.anim_preview.borrow_mut().stop();
            set_playing(act, &sq, false);
            sq.say(act.ui, forge_ui::tr!("Preview stopped: the viewport shows the project."));
            act.want_turn();
        });
        let s = st.clone();
        pb.on(tlw, move |act, _: &PlayTick| {
            let sq = s.borrow();
            let Some(c) = sq.clip() else { return };
            let now = act.ui.now();
            let mut pv = act.services.anim_preview.borrow_mut();
            let moved = pv.advance(act.mirror, c, now);
            let t = pv.time();
            let still = pv.is_playing();
            let events = pv.take_events();
            drop(pv);
            if moved {
                sq.view.borrow_mut().time = t;
                act.want_turn();
            }
            if let Some((at, track, ev)) = events.last() {
                sq.say(act.ui, forge_ui::trf!("Event {ev} on {track} at {at} s.", ev, track, at = format!("{:.2}", at)));
            }
            if !still {
                set_playing(act, &sq, false);
            }
        });
        let s = st.clone();
        pb.on(tlw, move |act, e: &Scrubbed| {
            let sq = s.borrow();
            scrub(act, &sq, e.t);
        });
        let s = st.clone();
        pb.on(tlw, move |act, _: &TrackPicked| {
            s.borrow_mut().show_curve(act.ui);
        });
        let s = st.clone();
        pb.on(tlw, move |act, e: &KeysPicked| {
            let sq = s.borrow();
            if let Some(c) = sq.clip()
                && let Some(t) = c.tracks.get(&e.track)
                && let Some(k) = e.keys.first().and_then(|k| t.keys.iter().find(|x| &x.id == k))
            {
                sq.say(
                    act.ui,
                    forge_ui::trf!("Key {id} at {t} s: {v} ({interp})", id = k.id, t = format!("{:.3}", k.t), v = format!("{:?}", k.v), interp = k.interp.name()),
                );
            }
        });
        let s = st.clone();
        pb.on(tlw, move |act, e: &KeysDragged| {
            let mut sq = s.borrow_mut();
            // A drag's `dt` is from where it began: the keys are moved from the times they
            // had then (the gesture's preview moves them in the model as the drag goes).
            if e.phase == DragPhase::Begin {
                sq.drag_base = sq
                    .clip()
                    .and_then(|c| c.tracks.get(&e.track).map(|t| (c.clone(), t.clone())));
            }
            let base = match e.phase {
                DragPhase::Nudge => sq
                    .clip()
                    .and_then(|c| c.tracks.get(&e.track).map(|t| (c.clone(), t.clone()))),
                _ => sq.drag_base.clone(),
            };
            let Some((clip, track)) = base else {
                return;
            };
            if matches!(e.phase, DragPhase::End | DragPhase::Cancel) {
                sq.drag_base = None;
            }
            if no_gesture && matches!(e.phase, DragPhase::Begin | DragPhase::Move) {
                // W2 fault: every frame of the drag its own transaction. The frames are
                // relative to the start, so each re-reads the model the last one wrote.
                if let Ok(cmds) = tl::move_keys(&clip, &track, &e.keys, e.dt)
                    && !cmds.is_empty()
                {
                    act.cmd.emit_all(forge_ui::tr!("Move keys"), cmds);
                }
                return;
            }
            match e.phase {
                DragPhase::Begin => {
                    let mut g = act.cmd.gesture(forge_ui::tr!("Move keys"));
                    if let Ok(cmds) = tl::move_keys(&clip, &track, &e.keys, e.dt) {
                        g.update_all(cmds);
                    }
                    sq.gesture = Some(g);
                }
                DragPhase::Move => {
                    // Every frame re-states the whole move from the gesture's start: the
                    // gesture replaces its preview, so a drag is one transaction.
                    if let Some(g) = sq.gesture.as_mut()
                        && let Ok(cmds) = tl::move_keys(&clip, &track, &e.keys, e.dt)
                    {
                        g.update_all(cmds);
                    }
                }
                DragPhase::End => {
                    if let Some(g) = sq.gesture.take() {
                        g.commit();
                    }
                }
                DragPhase::Cancel => {
                    if let Some(g) = sq.gesture.take() {
                        g.cancel();
                    }
                }
                DragPhase::Nudge => match tl::move_keys(&clip, &track, &e.keys, e.dt) {
                    Ok(cmds) if !cmds.is_empty() => {
                        act.cmd.emit_all(forge_ui::tr!("Nudge keys"), cmds);
                    }
                    Ok(_) => {}
                    Err(why) => refuse(act.session, forge_ui::tr!("Move keys"), &why),
                },
            }
        });
        let s = st.clone();
        pb.on(tlw, move |act, e: &KeyAt| key_track(act, &s.borrow(), Some(&e.track), e.t));
        let s = st.clone();
        pb.on(key_b, move |act, _: &Pressed| {
            let sq = s.borrow();
            let t = sq.view.borrow().time;
            key_track(act, &sq, None, t);
        });
        let s = st.clone();
        pb.on(tlw, move |act, _: &DeleteKeysRequested| {
            let sq = s.borrow();
            let keys = sq.view.borrow().keys.clone();
            if let (Some(c), Some(t)) = (sq.clip(), sq.track())
                && !keys.is_empty()
            {
                act.cmd
                    .emit_all(forge_ui::tr!("Delete keys"), tl::delete_keys(act.mirror, c, t, &keys));
            }
        });
        let s = st.clone();
        pb.on(mute, move |act, _: &Pressed| {
            let sq = s.borrow();
            if let (Some(c), Some(t)) = (sq.clip(), sq.track()) {
                act.cmd.emit_all(
                    if t.muted { forge_ui::tr!("Unmute track") } else { forge_ui::tr!("Mute track") },
                    tl::set_muted(c, t, !t.muted),
                );
            }
        });
        let s = st.clone();
        pb.on(del_track, move |act, _: &Pressed| {
            let sq = s.borrow();
            if let (Some(c), Some(t)) = (sq.clip(), sq.track()) {
                act.cmd
                    .emit_all(forge_ui::tr!("Delete track"), tl::delete_track(act.mirror, c, t));
            }
        });
        for (b, i) in interps {
            let s = st.clone();
            pb.on(b, move |act, _: &Pressed| {
                let sq = s.borrow();
                let keys = sq.view.borrow().keys.clone();
                match (sq.clip(), sq.track()) {
                    (Some(c), Some(t)) if !keys.is_empty() => {
                        act.cmd
                            .emit_all(forge_ui::tr!("Key interpolation"), tl::set_interp(c, t, &keys, i));
                    }
                    _ => refuse(act.session, forge_ui::tr!("Interpolation"), forge_ui::tr!("Select keys first.")),
                }
            });
        }
        let s = st.clone();
        pb.on(curve_ed, move |act, _: &CurveEdited| {
            let sq = s.borrow();
            let (Some(c), Some(t)) = (sq.clip(), sq.track()) else {
                return;
            };
            let curve = sq.curve.get(act.ui.rt());
            match tl::apply_curve(act.mirror, c, t, sq.shown_comp, &curve) {
                Ok(cmds) if !cmds.is_empty() => {
                    act.cmd.emit_all(forge_ui::tr!("Edit curve"), cmds);
                }
                Ok(_) => {}
                Err(why) => refuse(act.session, forge_ui::tr!("Edit curve"), &why),
            }
        });
        let s = st.clone();
        pb.on(clips, move |act, e: &SelectionChanged| {
            let mut sq = s.borrow_mut();
            if let Some(c) = e.keys.first().and_then(|k| sq.clip_rows.get(*k))
                && sq.sel_clip.as_deref() != Some(c.as_str())
            {
                sq.sel_clip = Some(c);
                act.services.anim_preview.borrow_mut().stop();
                set_playing(act, &sq, false);
                sq.view.borrow_mut().track = None;
                sq.show(act.ui, act.mirror);
                act.want_turn();
            }
        });
        let s = st.clone();
        pb.on(clips, move |act, e: &RowRenamed| {
            let sq = s.borrow();
            if let Some(c) = sq.clip_rows.get(e.key).and_then(|id| sq.doc.clips.get(&id)) {
                act.cmd.emit_all(forge_ui::tr!("Rename clip"), tl::rename_clip(c, &e.name));
            }
        });
        let s = st.clone();
        pb.on(clips, move |act, _: &RowsDeleteRequested| {
            let sq = s.borrow();
            if let Some(c) = sq.clip() {
                act.cmd.emit_all(forge_ui::tr!("Delete clip"), tl::delete_clip(act.mirror, c));
            }
        });
        let s = st.clone();
        pb.on(props, move |act, e: &RowActivated| {
            let sq = s.borrow();
            let (Some(path), Some(ent)) = (sq.prop_rows.get(e.key), sq.prop_entity) else {
                return;
            };
            let Some(c) = sq.clip() else {
                refuse(act.session, forge_ui::tr!("Add track"), forge_ui::tr!("Create or select a clip first."));
                return;
            };
            let t = sq.view.borrow().time;
            match tl::add_property_track(act.mirror, c, ent, &path, t) {
                Ok((id, cmds)) => {
                    act.cmd.emit_all(forge_ui::tr!("Add property track"), cmds);
                    sq.view.borrow_mut().track = Some(id.clone());
                    sq.say(act.ui, forge_ui::trf!("Animating {path} (track {id}).", path, id));
                }
                Err(why) => {
                    refuse(act.session, forge_ui::tr!("Add track"), &why);
                    sq.say(act.ui, format!("\u{26a0} {why}"));
                }
            }
        });
        let s = st.clone();
        pb.on(comp_relay, move |act, _: &forge_ui::widgets::SignalChanged| {
            s.borrow_mut().show_curve(act.ui);
        });

        let s = st;
        pb.sync(clips, move |sy| {
            let rev = watch(sy.mirror, PREFIX);
            let mut sq = s.borrow_mut();
            if rev != sq.seen {
                let first = sq.seen == u64::MAX;
                sq.seen = rev;
                sq.reread(sy.ui, sy.mirror, &sy.services.anim_preview);
                if first {
                    // The timeline's frame loop starts (and at once stops) here: it runs
                    // only while a clip plays.
                    let t = sq.tl;
                    sy.ui.send(t, &UiEvent::AnimFrame);
                }
            }
            let sel_rev = sy.session.selection_revision();
            let ent = sy.session.selection.first().copied();
            let mrev = sy.mirror.revision();
            if sel_rev != sq.seen_sel || (ent.is_some() && mrev != sq.seen_mrev) {
                sq.seen_sel = sel_rev;
                sq.seen_mrev = mrev;
                sq.show_props(sy.ui, sy.mirror, ent);
            }
            Ok(())
        });
        Ok(())
    });
}

fn toggle_play(act: &mut PanelAct, s: &Rc<RefCell<Seq>>) {
    let sq = s.borrow();
    let Some(c) = sq.clip() else {
        refuse(
            act.session,
            forge_ui::tr!("Play"),
            forge_ui::tr!("Create or select a clip first."),
        );
        return;
    };
    let mut pv = act.services.anim_preview.borrow_mut();
    if pv.is_playing() {
        pv.pause();
        drop(pv);
        set_playing(act, &sq, false);
    } else {
        let t = sq.view.borrow().time;
        if pv.clip() != Some(c.id.as_str()) {
            pv.scrub(act.mirror, c, t);
        }
        let now = act.ui.now();
        pv.play(act.mirror, c, now);
        drop(pv);
        set_playing(act, &sq, true);
    }
    act.want_turn();
}

fn key_track(act: &mut PanelAct, sq: &Seq, track: Option<&str>, t: f64) {
    let Some(c) = sq.clip() else {
        refuse(
            act.session,
            forge_ui::tr!("Key"),
            forge_ui::tr!("Create or select a clip first."),
        );
        return;
    };
    let tr = match track {
        Some(id) => c.tracks.get(id),
        None => sq.track(),
    };
    let Some(tr) = tr else {
        refuse(
            act.session,
            forge_ui::tr!("Key"),
            forge_ui::tr!("Select a track first."),
        );
        return;
    };
    let r = match tr.kind {
        TrackKind::Property => tl::key_current(act.mirror, c, tr, t),
        TrackKind::Event => tl::set_key(c, tr, t, Value::Text(tr.path.clone())),
    };
    match r {
        Ok(cmds) => {
            act.cmd.emit_all(forge_ui::tr!("Key"), cmds);
            sq.say(
                act.ui,
                forge_ui::trf!(
                    "Keyed {path} at {snap} s.",
                    path = tr.path,
                    snap = format!("{:.3}", c.snap(t))
                ),
            );
        }
        Err(why) => refuse(act.session, forge_ui::tr!("Key"), &why),
    }
}
