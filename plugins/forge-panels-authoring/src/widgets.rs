//! The authoring editors' custom widgets. Each paints from a small shared view model the
//! panel fills from the project (`Rc<RefCell<…>>`), raises **actions** for what the user
//! did (the panel turns them into commands — a widget never edits the project), is fully
//! keyboard-operable, and gives assistive technology a role, a name and a value.
//!
//! * [`TimelineView`] — the sequencer's tracks and keys over a time ruler with the
//!   playhead. Click or drag the ruler to scrub; click a key to select it (Shift adds),
//!   drag keys to retime them (one gesture), double-click a lane to key it there. Keys:
//!   Left/Right move the playhead a frame (Shift: a second), Home/End jump, Up/Down pick the
//!   track, `[`/`]` select the previous/next key (Shift adds), K keys the track at the
//!   playhead, Ctrl+Left/Right nudge the selected keys a
//!   frame, Delete removes them, Space plays or pauses. Only the rows in view are painted, and in
//!   each only the keys in the visible time range, one mark per pixel column.
//! * [`BlendSpaceView`] — a 1D or 2D blend space: its samples, and the preview point whose
//!   weights the panel shows. Drag a sample to move it (one gesture), drag elsewhere (or
//!   use the arrows) to move the preview point; Tab-free: `[`/`]` pick the sample the arrow
//!   keys move with Ctrl.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use accesskit::{Action, Node};
use forge_ui::input::PointerButton;
use forge_ui::text::TextStyle;
use forge_ui::widget::{A11yCx, EventCx, PaintCx};
use forge_ui::{ColorRole, Handled, KeyCode, Point, Rect, Role, UiEvent, Widget};

fn small(cx: &PaintCx) -> TextStyle {
    TextStyle::body(cx.theme().type_scale.small)
}

// ---- timeline -------------------------------------------------------------------------------

/// The track header column's width.
pub const HEADER_W: f32 = 170.0;
/// The time ruler's height.
pub const RULER_H: f32 = 22.0;
/// One track row.
pub const ROW_H: f32 = 24.0;
/// Pointer tolerance for a key (px).
const KEY_HIT: f32 = 7.0;

/// One row of the timeline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackRow {
    pub id: String,
    pub label: String,
    /// `(key id, time)`.
    pub keys: Vec<(String, f64)>,
    pub muted: bool,
}

/// What the timeline shows (the panel fills it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimelineData {
    pub length: f64,
    pub fps: i64,
    pub tracks: Vec<TrackRow>,
    /// The playhead.
    pub time: f64,
    pub track: Option<String>,
    pub keys: Vec<String>,
    pub playing: bool,
}

impl TimelineData {
    fn frame(&self) -> f64 {
        1.0 / self.fps.clamp(1, 240) as f64
    }
    fn track_index(&self) -> Option<usize> {
        self.track
            .as_ref()
            .and_then(|t| self.tracks.iter().position(|r| &r.id == t))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragPhase {
    Begin,
    Move,
    End,
    Cancel,
    /// A one-step move from the keyboard (its own transaction).
    Nudge,
}

/// The playhead was moved to `t` (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scrubbed {
    pub t: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackPicked {
    pub track: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeysPicked {
    pub track: String,
    pub keys: Vec<String>,
}

/// Keys moved by `dt` seconds from where the gesture began.
#[derive(Clone, Debug, PartialEq)]
pub struct KeysDragged {
    pub track: String,
    pub keys: Vec<String>,
    pub dt: f64,
    pub phase: DragPhase,
}

/// Key the track at `t`.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyAt {
    pub track: String,
    pub t: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeleteKeysRequested;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayToggled;

/// A playing timeline's frame (the panel advances the preview).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayTick;

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    None,
    Scrub,
    Keys {
        track: String,
        keys: Vec<String>,
        x0: f32,
        begun: bool,
        dt: f64,
    },
}

/// See the module docs.
pub struct TimelineView {
    data: Rc<RefCell<TimelineData>>,
    drag: Drag,
    /// The first row shown (wheel scrolling).
    scroll: usize,
    animating: bool,
    painted_rows: Cell<usize>,
    painted_keys: Cell<usize>,
    /// W2 fault: ask for frames while nothing plays (`timeline_always_animating`).
    always_animating: bool,
    /// W2 fault: paint every key of every visible track (`timeline_paint_every_key`).
    paint_every_key: bool,
}

impl TimelineView {
    pub fn new(data: Rc<RefCell<TimelineData>>) -> Self {
        Self {
            data,
            drag: Drag::None,
            scroll: 0,
            animating: false,
            painted_rows: Cell::new(0),
            painted_keys: Cell::new(0),
            always_animating: false,
            paint_every_key: false,
        }
    }
    /// The idle guard's positive control (never set outside it).
    #[doc(hidden)]
    pub fn with_fault_always_animating(mut self, on: bool) -> Self {
        self.always_animating = on;
        self
    }
    /// `test_sequencer_drag_cost`'s paint control (never set outside it).
    #[doc(hidden)]
    pub fn with_fault_paint_every_key(mut self, on: bool) -> Self {
        self.paint_every_key = on;
        self
    }
    /// Rows the last paint visited (only the rows in view).
    pub fn painted_rows(&self) -> usize {
        self.painted_rows.get()
    }
    /// Key marks the last paint drew: keys in the visible time range, one per pixel column
    /// (plus the selected ones).
    pub fn painted_keys(&self) -> usize {
        self.painted_keys.get()
    }
    fn lanes(r: Rect) -> Rect {
        Rect::new(
            r.x + HEADER_W,
            r.y + RULER_H,
            (r.w - HEADER_W).max(1.0),
            (r.h - RULER_H).max(0.0),
        )
    }
    fn px_per_s(r: Rect, length: f64) -> f64 {
        f64::from(Self::lanes(r).w - 8.0).max(1.0) / length.max(1e-3)
    }
    fn x_of(r: Rect, length: f64, t: f64) -> f32 {
        Self::lanes(r).x + 4.0 + (t * Self::px_per_s(r, length)) as f32
    }
    fn t_of(r: Rect, length: f64, x: f32) -> f64 {
        (f64::from(x - Self::lanes(r).x - 4.0) / Self::px_per_s(r, length)).clamp(0.0, length)
    }
    fn visible_rows(r: Rect) -> usize {
        ((Self::lanes(r).h / ROW_H).ceil() as usize).max(1)
    }
    fn row_at(&self, r: Rect, y: f32) -> Option<usize> {
        if y < r.y + RULER_H {
            return None;
        }
        let i = ((y - r.y - RULER_H) / ROW_H) as usize + self.scroll;
        (i < self.data.borrow().tracks.len()).then_some(i)
    }
    fn snap(d: &TimelineData, t: f64) -> f64 {
        let f = d.frame();
        ((t / f).round() * f).clamp(0.0, d.length)
    }
    fn scrub(&self, cx: &mut EventCx, t: f64) {
        let t = {
            let d = self.data.borrow();
            Self::snap(&d, t)
        };
        self.data.borrow_mut().time = t;
        cx.action(Scrubbed { t });
        cx.request_paint();
        cx.request_a11y();
    }
    fn pick_track(&self, cx: &mut EventCx, i: usize) {
        let id = self.data.borrow().tracks.get(i).map(|r| r.id.clone());
        if let Some(id) = id {
            {
                let mut d = self.data.borrow_mut();
                if d.track.as_ref() != Some(&id) {
                    d.keys.clear();
                }
                d.track = Some(id.clone());
            }
            cx.action(TrackPicked { track: id });
            cx.request_paint();
            cx.request_a11y();
        }
    }
    fn key_at(&self, r: Rect, row: usize, x: f32) -> Option<String> {
        let d = self.data.borrow();
        let tr = d.tracks.get(row)?;
        tr.keys
            .iter()
            .map(|(id, t)| (id, (Self::x_of(r, d.length, *t) - x).abs()))
            .filter(|(_, dx)| *dx <= KEY_HIT)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id.clone())
    }
    fn describe(&self) -> String {
        let d = self.data.borrow();
        let track = d.track_index().and_then(|i| d.tracks.get(i)).map_or_else(
            || forge_ui::tr!("no track").to_string(),
            |t| {
                forge_ui::trf!(
                    "track {track} ({keys} keys)",
                    track = t.label,
                    keys = t.keys.len()
                )
            },
        );
        forge_ui::trf!(
            "{time} s of {length} s, {track}, {keys} key(s) selected{playing}",
            time = format!("{:.2}", d.time),
            length = format!("{:.2}", d.length),
            track,
            keys = d.keys.len(),
            playing = if d.playing {
                forge_ui::tr!(", playing")
            } else {
                ""
            }
        )
    }
}

impl Widget for TimelineView {
    fn role(&self) -> Role {
        Role::Group
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let r = cx.rect();
        let length = self.data.borrow().length;
        // A playing timeline asks for frames only while it plays (zero idle, D-5).
        let wants = self.data.borrow().playing || self.always_animating;
        if wants && !self.animating {
            self.animating = true;
            cx.request_anim_frame();
        }
        match ev {
            UiEvent::AnimFrame => {
                if self.always_animating && !self.data.borrow().playing {
                    cx.request_anim_frame();
                    cx.request_paint();
                } else if self.data.borrow().playing {
                    cx.action(PlayTick);
                    cx.request_anim_frame();
                    cx.request_paint();
                } else {
                    self.animating = false;
                    cx.stop_anim();
                }
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
            } => {
                cx.request_focus();
                if pos.y < r.y + RULER_H && pos.x >= r.x + HEADER_W {
                    self.drag = Drag::Scrub;
                    cx.capture_pointer();
                    self.scrub(cx, Self::t_of(r, length, pos.x));
                    return Handled::Yes;
                }
                let Some(row) = self.row_at(r, pos.y) else {
                    return Handled::Yes;
                };
                self.pick_track(cx, row);
                if pos.x < r.x + HEADER_W {
                    return Handled::Yes;
                }
                let track = self.data.borrow().tracks[row].id.clone();
                match self.key_at(r, row, pos.x) {
                    Some(k) => {
                        let keys = {
                            let mut d = self.data.borrow_mut();
                            if cx.modifiers().shift {
                                if !d.keys.contains(&k) {
                                    d.keys.push(k.clone());
                                }
                            } else if !d.keys.contains(&k) {
                                d.keys = vec![k.clone()];
                            }
                            d.keys.clone()
                        };
                        cx.action(KeysPicked {
                            track: track.clone(),
                            keys: keys.clone(),
                        });
                        self.drag = Drag::Keys {
                            track,
                            keys,
                            x0: pos.x,
                            begun: false,
                            dt: 0.0,
                        };
                        cx.capture_pointer();
                    }
                    None if *clicks >= 2 => {
                        let t = {
                            let d = self.data.borrow();
                            Self::snap(&d, Self::t_of(r, length, pos.x))
                        };
                        cx.action(KeyAt { track, t });
                    }
                    None => {
                        self.data.borrow_mut().keys.clear();
                        cx.action(KeysPicked {
                            track,
                            keys: Vec::new(),
                        });
                        self.drag = Drag::Scrub;
                        cx.capture_pointer();
                        self.scrub(cx, Self::t_of(r, length, pos.x));
                    }
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => match &mut self.drag {
                Drag::Scrub => {
                    let x = pos.x;
                    self.scrub(cx, Self::t_of(r, length, x));
                    Handled::Yes
                }
                Drag::Keys {
                    track,
                    keys,
                    x0,
                    begun,
                    dt,
                } => {
                    if !*begun && (pos.x - *x0).abs() < 2.0 {
                        return Handled::Yes;
                    }
                    let phase = if *begun {
                        DragPhase::Move
                    } else {
                        DragPhase::Begin
                    };
                    *begun = true;
                    let raw = f64::from(pos.x - *x0) / Self::px_per_s(r, length);
                    let f = self.data.borrow().frame();
                    *dt = (raw / f).round() * f;
                    cx.action(KeysDragged {
                        track: track.clone(),
                        keys: keys.clone(),
                        dt: *dt,
                        phase,
                    });
                    cx.request_paint();
                    Handled::Yes
                }
                Drag::None => Handled::No,
            },
            UiEvent::PointerUp { .. } => {
                let d = std::mem::replace(&mut self.drag, Drag::None);
                cx.release_pointer();
                if let Drag::Keys {
                    track,
                    keys,
                    begun: true,
                    dt,
                    ..
                } = d
                {
                    cx.action(KeysDragged {
                        track,
                        keys,
                        dt,
                        phase: DragPhase::End,
                    });
                }
                Handled::Yes
            }
            UiEvent::Wheel { dy, .. } => {
                let n = self.data.borrow().tracks.len();
                let vis = Self::visible_rows(r);
                let max = n.saturating_sub(vis);
                let s = if *dy < 0.0 {
                    (self.scroll + 1).min(max)
                } else {
                    self.scroll.saturating_sub(1)
                };
                if s != self.scroll {
                    self.scroll = s;
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => {
                if k.code == KeyCode::Escape
                    && let Drag::Keys {
                        track,
                        keys,
                        begun: true,
                        ..
                    } = std::mem::replace(&mut self.drag, Drag::None)
                {
                    cx.release_pointer();
                    cx.action(KeysDragged {
                        track,
                        keys,
                        dt: 0.0,
                        phase: DragPhase::Cancel,
                    });
                    return Handled::Yes;
                }
                let (time, frame, length, track_i, n, track, keys) = {
                    let d = self.data.borrow();
                    (
                        d.time,
                        d.frame(),
                        d.length,
                        d.track_index(),
                        d.tracks.len(),
                        d.track.clone(),
                        d.keys.clone(),
                    )
                };
                let step = if k.mods.shift { 1.0 } else { frame };
                match (&k.code, k.mods.ctrl) {
                    (KeyCode::Left, false) => self.scrub(cx, time - step),
                    (KeyCode::Right, false) => self.scrub(cx, time + step),
                    (KeyCode::Home, _) => self.scrub(cx, 0.0),
                    (KeyCode::End, _) => self.scrub(cx, length),
                    (KeyCode::Up, false) if n > 0 => {
                        let i = track_i.map_or(0, |i| i.saturating_sub(1));
                        self.scroll = self.scroll.min(i);
                        self.pick_track(cx, i);
                    }
                    (KeyCode::Down, false) if n > 0 => {
                        let i = track_i.map_or(0, |i| (i + 1).min(n - 1));
                        let vis = Self::visible_rows(cx.rect());
                        if i >= self.scroll + vis {
                            self.scroll = i + 1 - vis;
                        }
                        self.pick_track(cx, i);
                    }
                    (KeyCode::Left | KeyCode::Right, true) => {
                        if let (Some(track), false) = (track, keys.is_empty()) {
                            let dt = if k.code == KeyCode::Left { -step } else { step };
                            cx.action(KeysDragged {
                                track,
                                keys,
                                dt,
                                phase: DragPhase::Nudge,
                            });
                        }
                    }
                    (KeyCode::Char('[') | KeyCode::Char(']'), false) => {
                        // The previous / next key of the track: select it (Shift adds) and
                        // move the playhead onto it.
                        let next = k.code == KeyCode::Char(']');
                        let found = {
                            let d = self.data.borrow();
                            d.track_index().and_then(|i| {
                                let keys = &d.tracks[i].keys;
                                let half = d.frame() * 0.5;
                                if next {
                                    keys.iter()
                                        .filter(|(_, t)| *t > time + half)
                                        .min_by(|a, b| a.1.total_cmp(&b.1))
                                } else {
                                    keys.iter()
                                        .filter(|(_, t)| *t < time - half)
                                        .max_by(|a, b| a.1.total_cmp(&b.1))
                                }
                                .map(|(id, t)| (id.clone(), *t, d.tracks[i].id.clone()))
                            })
                        };
                        if let Some((id, t, track)) = found {
                            let keys = {
                                let mut d = self.data.borrow_mut();
                                if k.mods.shift {
                                    if !d.keys.contains(&id) {
                                        d.keys.push(id);
                                    }
                                } else {
                                    d.keys = vec![id];
                                }
                                d.keys.clone()
                            };
                            cx.action(KeysPicked { track, keys });
                            self.scrub(cx, t);
                        }
                    }
                    (KeyCode::Char('k'), false) | (KeyCode::Enter, false) => {
                        if let Some(track) = track {
                            cx.action(KeyAt { track, t: time });
                        }
                    }
                    (KeyCode::Delete, _) => cx.action(DeleteKeysRequested),
                    (KeyCode::Space, false) => cx.action(PlayToggled),
                    _ => return Handled::No,
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::A11yAction(Action::Increment) => {
                let (t, f) = {
                    let d = self.data.borrow();
                    (d.time, d.frame())
                };
                self.scrub(cx, t + f);
                Handled::Yes
            }
            UiEvent::A11yAction(Action::Decrement) => {
                let (t, f) = {
                    let d = self.data.borrow();
                    (d.time, d.frame())
                };
                self.scrub(cx, t - f);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let d = self.data.borrow();
        let lanes = Self::lanes(r);
        cx.fill(r, ColorRole::BgSunken, 0.0);
        cx.fill(Rect::new(r.x, r.y, HEADER_W, r.h), ColorRole::BgRaised, 0.0);
        let st = small(cx);
        // The ruler: a tick per second, per frame when there is room.
        let pps = Self::px_per_s(r, d.length);
        let frame = d.frame();
        if pps * frame >= 6.0 {
            let n = (d.length / frame).round() as i64;
            for i in 0..=n {
                let x = Self::x_of(r, d.length, i as f64 * frame);
                cx.mark(
                    Rect::new(x, r.y + RULER_H - 5.0, 1.0, 5.0),
                    ColorRole::Border,
                    ColorRole::BgSunken,
                    0.0,
                );
            }
        }
        let secs_step = if pps >= 40.0 {
            1.0
        } else {
            (40.0 / pps).ceil()
        };
        let mut s = 0.0;
        while s <= d.length + 1e-9 {
            let x = Self::x_of(r, d.length, s);
            cx.mark(
                Rect::new(x, r.y + 4.0, 1.0, RULER_H - 4.0),
                ColorRole::FgMuted,
                ColorRole::BgSunken,
                0.0,
            );
            cx.text(
                &format!("{s:.0}s"),
                &st,
                Point::new(x + 3.0, r.y + 2.0),
                ColorRole::FgMuted,
            );
            s += secs_step;
        }
        // The rows in view only.
        let vis = Self::visible_rows(r);
        let mut painted = 0;
        let mut keys_painted = 0;
        // The time range the lanes show (`x_of` inverted at their edges, unclamped), with
        // half a key mark (4 px) of slack on each side.
        let slack = 4.0 / pps;
        let t0 = -4.0 / pps - slack;
        let t1 = f64::from(lanes.w - 4.0) / pps + slack;
        let track_i = d.track_index();
        for (i, tr) in d.tracks.iter().enumerate().skip(self.scroll).take(vis) {
            painted += 1;
            let y = r.y + RULER_H + (i - self.scroll) as f32 * ROW_H;
            if y > r.bottom() {
                break;
            }
            let row = Rect::new(r.x, y, r.w, ROW_H);
            let bg = if Some(i) == track_i {
                ColorRole::Selection
            } else if i % 2 == 1 {
                ColorRole::BgBase
            } else {
                ColorRole::BgSunken
            };
            cx.fill(row, bg, 0.0);
            let fg = if tr.muted {
                ColorRole::FgMuted
            } else {
                ColorRole::FgPrimary
            };
            let label = if tr.muted {
                forge_ui::trf!("{track} (muted)", track = tr.label)
            } else {
                tr.label.clone()
            };
            cx.text(&label, &st, Point::new(r.x + 6.0, y + 5.0), fg);
            // Only the keys in the visible time range (keys are in time order: two binary
            // searches), and one mark per pixel column — a dense track paints what can be
            // seen, not every key. A selected key is always drawn (it shows its colour).
            let (from, to) = if self.paint_every_key {
                (0, tr.keys.len())
            } else {
                (
                    tr.keys.partition_point(|(_, t)| *t < t0),
                    tr.keys.partition_point(|(_, t)| *t <= t1),
                )
            };
            let mut last_px = i64::MIN;
            for (kid, t) in &tr.keys[from..to.max(from)] {
                let x = Self::x_of(r, d.length, *t);
                let sel = Some(i) == track_i && d.keys.contains(kid);
                let px = x.floor() as i64;
                if px == last_px && !sel && !self.paint_every_key {
                    continue;
                }
                last_px = px;
                keys_painted += 1;
                let role = if sel {
                    ColorRole::Accent
                } else if tr.muted {
                    ColorRole::FgMuted
                } else {
                    ColorRole::FgPrimary
                };
                cx.mark(
                    Rect::new(x - 4.0, y + ROW_H * 0.5 - 4.0, 8.0, 8.0),
                    role,
                    bg,
                    2.0,
                );
            }
        }
        self.painted_rows.set(painted);
        self.painted_keys.set(keys_painted);
        // The playhead.
        let x = Self::x_of(r, d.length, d.time);
        cx.mark(
            Rect::new(x - 1.0, r.y, 2.0, lanes.bottom() - r.y),
            ColorRole::Accent,
            ColorRole::BgSunken,
            0.0,
        );
        if d.tracks.is_empty() {
            cx.text(
                forge_ui::tr!("No tracks: select an entity and pick a property to animate."),
                &st,
                Point::new(lanes.x + 8.0, lanes.y + 6.0),
                ColorRole::FgMuted,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut Node) {
        node.set_label(forge_ui::tr!("Timeline"));
        node.set_value(self.describe());
        node.set_description(
            forge_ui::tr!("Left/Right move the playhead a frame (Shift: a second); Up/Down pick a track; [ and ] select keys; K keys it; Ctrl+Left/Right nudge keys; Delete removes them; Space plays"),
        );
        node.add_action(Action::Focus);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    }
}

// ---- blend space ------------------------------------------------------------------------------

/// A blend space as the view shows it (the panel fills it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlendData {
    pub two_d: bool,
    /// `(sample id, label, x, y)`.
    pub samples: Vec<(String, String, f64, f64)>,
    /// The preview point.
    pub point: (f64, f64),
    /// Each sample's weight at the preview point (aligned with `samples`).
    pub weights: Vec<f64>,
    pub selected: Option<usize>,
}

impl BlendData {
    /// The value range shown: the samples' bounds padded, at least -1..1.
    fn bounds(&self) -> ((f64, f64), (f64, f64)) {
        let (mut x0, mut x1, mut y0, mut y1) = (-1.0f64, 1.0f64, -1.0f64, 1.0f64);
        for (_, _, x, y) in &self.samples {
            x0 = x0.min(*x);
            x1 = x1.max(*x);
            y0 = y0.min(*y);
            y1 = y1.max(*y);
        }
        let px = (x1 - x0) * 0.1;
        let py = (y1 - y0) * 0.1;
        ((x0 - px, x1 + px), (y0 - py, y1 + py))
    }
}

/// A sample was dragged to `(x, y)`.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleMoved {
    pub sample: String,
    pub x: f64,
    pub y: f64,
    pub phase: DragPhase,
}

/// The preview point moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendPointMoved {
    pub x: f64,
    pub y: f64,
}

/// See the module docs.
pub struct BlendSpaceView {
    data: Rc<RefCell<BlendData>>,
    drag: Option<(usize, bool)>,
    moving_point: bool,
}

impl BlendSpaceView {
    pub fn new(data: Rc<RefCell<BlendData>>) -> Self {
        Self {
            data,
            drag: None,
            moving_point: false,
        }
    }
    fn plot(r: Rect) -> Rect {
        Rect::new(
            r.x + 12.0,
            r.y + 12.0,
            (r.w - 24.0).max(1.0),
            (r.h - 24.0).max(1.0),
        )
    }
    fn to_px(d: &BlendData, r: Rect, x: f64, y: f64) -> Point {
        let p = Self::plot(r);
        let ((x0, x1), (y0, y1)) = d.bounds();
        let u = ((x - x0) / (x1 - x0)) as f32;
        let v = if d.two_d {
            ((y - y0) / (y1 - y0)) as f32
        } else {
            0.5
        };
        Point::new(p.x + u * p.w, p.bottom() - v * p.h)
    }
    fn from_px(d: &BlendData, r: Rect, pt: Point) -> (f64, f64) {
        let p = Self::plot(r);
        let ((x0, x1), (y0, y1)) = d.bounds();
        let u = f64::from(((pt.x - p.x) / p.w).clamp(0.0, 1.0));
        let v = f64::from(((p.bottom() - pt.y) / p.h).clamp(0.0, 1.0));
        let round = |a: f64| (a * 100.0).round() / 100.0;
        (
            round(x0 + u * (x1 - x0)),
            if d.two_d {
                round(y0 + v * (y1 - y0))
            } else {
                0.0
            },
        )
    }
    fn set_point(&self, cx: &mut EventCx, x: f64, y: f64) {
        self.data.borrow_mut().point = (x, y);
        cx.action(BlendPointMoved { x, y });
        cx.request_paint();
        cx.request_a11y();
    }
    fn readout(&self) -> String {
        let d = self.data.borrow();
        let mut parts: Vec<String> = d
            .samples
            .iter()
            .zip(&d.weights)
            .filter(|(_, w)| **w >= 0.005)
            .map(|((_, l, _, _), w)| format!("{l} {:.0}%", w * 100.0))
            .collect();
        if parts.is_empty() {
            parts.push(forge_ui::tr!("no samples").into());
        }
        let pt = if d.two_d {
            format!("({:.2}, {:.2})", d.point.0, d.point.1)
        } else {
            format!("{:.2}", d.point.0)
        };
        forge_ui::trf!("at {pt}: {weights}", pt, weights = parts.join(", "))
    }
}

impl Widget for BlendSpaceView {
    fn role(&self) -> Role {
        Role::Canvas
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let r = cx.rect();
        match ev {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                cx.request_focus();
                cx.capture_pointer();
                let hit = {
                    let d = self.data.borrow();
                    d.samples
                        .iter()
                        .enumerate()
                        .map(|(i, (_, _, x, y))| {
                            let p = Self::to_px(&d, r, *x, *y);
                            (i, (p.x - pos.x).hypot(p.y - pos.y))
                        })
                        .filter(|(_, dist)| *dist <= 9.0)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(i, _)| i)
                };
                match hit {
                    Some(i) => {
                        self.data.borrow_mut().selected = Some(i);
                        self.drag = Some((i, false));
                    }
                    None => {
                        self.moving_point = true;
                        let (x, y) = Self::from_px(&self.data.borrow(), r, *pos);
                        self.set_point(cx, x, y);
                    }
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                if self.moving_point {
                    let (x, y) = Self::from_px(&self.data.borrow(), r, *pos);
                    self.set_point(cx, x, y);
                    return Handled::Yes;
                }
                if let Some((i, begun)) = self.drag {
                    let (x, y) = Self::from_px(&self.data.borrow(), r, *pos);
                    let id = {
                        let mut d = self.data.borrow_mut();
                        if let Some(s) = d.samples.get_mut(i) {
                            s.2 = x;
                            s.3 = y;
                        }
                        d.samples.get(i).map(|s| s.0.clone())
                    };
                    if let Some(sample) = id {
                        cx.action(SampleMoved {
                            sample,
                            x,
                            y,
                            phase: if begun {
                                DragPhase::Move
                            } else {
                                DragPhase::Begin
                            },
                        });
                    }
                    self.drag = Some((i, true));
                    cx.request_paint();
                    return Handled::Yes;
                }
                Handled::No
            }
            UiEvent::PointerUp { .. } => {
                cx.release_pointer();
                self.moving_point = false;
                if let Some((i, true)) = self.drag.take() {
                    let s = self.data.borrow().samples.get(i).cloned();
                    if let Some((sample, _, x, y)) = s {
                        cx.action(SampleMoved {
                            sample,
                            x,
                            y,
                            phase: DragPhase::End,
                        });
                    }
                }
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => {
                let (two_d, n) = {
                    let d = self.data.borrow();
                    (d.two_d, d.samples.len())
                };
                match k.code {
                    KeyCode::Char('[') | KeyCode::Char(']') if n > 0 => {
                        let mut d = self.data.borrow_mut();
                        let cur = d.selected.unwrap_or(0);
                        d.selected = Some(if k.code == KeyCode::Char(']') {
                            (cur + 1) % n
                        } else {
                            (cur + n - 1) % n
                        });
                        drop(d);
                        cx.request_paint();
                        cx.request_a11y();
                        return Handled::Yes;
                    }
                    KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {}
                    _ => return Handled::No,
                }
                if !two_d && matches!(k.code, KeyCode::Up | KeyCode::Down) {
                    return Handled::No;
                }
                let step = if k.mods.shift { 0.01 } else { 0.1 };
                let (dx, dy) = match k.code {
                    KeyCode::Left => (-step, 0.0),
                    KeyCode::Right => (step, 0.0),
                    KeyCode::Up => (0.0, step),
                    _ => (0.0, -step),
                };
                let round = |a: f64| (a * 100.0).round() / 100.0;
                if k.mods.ctrl {
                    // Move the selected sample: one step, one transaction.
                    let s = {
                        let d = self.data.borrow();
                        d.selected.and_then(|i| d.samples.get(i).cloned())
                    };
                    if let Some((sample, _, x, y)) = s {
                        cx.action(SampleMoved {
                            sample,
                            x: round(x + dx),
                            y: round(y + dy),
                            phase: DragPhase::Nudge,
                        });
                    }
                } else {
                    let (x, y) = self.data.borrow().point;
                    self.set_point(cx, round(x + dx), round(y + dy));
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgSunken, 0.0);
        let d = self.data.borrow();
        let p = Self::plot(r);
        // Axes through zero.
        let o = Self::to_px(&d, r, 0.0, 0.0);
        cx.mark(
            Rect::new(p.x, o.y, p.w, 1.0),
            ColorRole::Border,
            ColorRole::BgSunken,
            0.0,
        );
        if d.two_d {
            cx.mark(
                Rect::new(o.x, p.y, 1.0, p.h),
                ColorRole::Border,
                ColorRole::BgSunken,
                0.0,
            );
        }
        let st = small(cx);
        for (i, ((_, label, x, y), w)) in d
            .samples
            .iter()
            .zip(d.weights.iter().chain(std::iter::repeat(&0.0)))
            .enumerate()
        {
            let c = Self::to_px(&d, r, *x, *y);
            let size = 8.0 + 8.0 * (*w as f32).clamp(0.0, 1.0);
            let role = if Some(i) == d.selected {
                ColorRole::Accent
            } else {
                ColorRole::FgPrimary
            };
            cx.mark(
                Rect::new(c.x - size * 0.5, c.y - size * 0.5, size, size),
                role,
                ColorRole::BgSunken,
                size * 0.5,
            );
            cx.text(
                label,
                &st,
                Point::new(c.x + 8.0, c.y - 16.0),
                ColorRole::FgMuted,
            );
        }
        let c = Self::to_px(&d, r, d.point.0, d.point.1);
        cx.mark(
            Rect::new(c.x - 1.0, c.y - 7.0, 2.0, 14.0),
            ColorRole::Warning,
            ColorRole::BgSunken,
            0.0,
        );
        cx.mark(
            Rect::new(c.x - 7.0, c.y - 1.0, 14.0, 2.0),
            ColorRole::Warning,
            ColorRole::BgSunken,
            0.0,
        );
        drop(d);
        let text = self.readout();
        cx.text(
            &text,
            &st,
            Point::new(r.x + 6.0, r.bottom() - 18.0),
            ColorRole::FgPrimary,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut Node) {
        node.set_label(forge_ui::tr!("Blend space"));
        node.set_value(self.readout());
        node.set_description(
            forge_ui::tr!("Arrows move the preview point (Shift: finely); [ and ] pick a sample, Ctrl+arrows move it"),
        );
        node.add_action(Action::Focus);
    }
}
