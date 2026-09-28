//! The sequencer's model (Ch.19 "animate any property", Ch.6; Ch.21 §21.21 "Sequencer &
//! timeline", DoD M2-63): **clips** of **tracks**; a property track keys **any reflected
//! property of any entity** (the reflect path the inspector edits, `transform.position.local`,
//! `light.intensity`, …), an event track keys named events. One timeline for everything —
//! Godot's `AnimationPlayer` model, which Ch.19 picks over UE's and Unity's split.
//!
//! Project settings (see [`crate::domain`] for the conventions):
//!
//! | Key | Value |
//! |---|---|
//! | `seq.clip.<c>.name` | text |
//! | `seq.clip.<c>.length` | seconds (float, > 0) |
//! | `seq.clip.<c>.fps` | the snapping rate (integer, 1..=240) |
//! | `seq.clip.<c>.loop` | bool |
//! | `seq.clip.<c>.track.<t>.kind` | `Property` or `Event` |
//! | `seq.clip.<c>.track.<t>.entity` | the entity (property tracks) |
//! | `seq.clip.<c>.track.<t>.path` | the reflect path (property tracks) or the track's name |
//! | `seq.clip.<c>.track.<t>.muted` | bool |
//! | `seq.clip.<c>.track.<t>.key.<k>.t` | seconds |
//! | `seq.clip.<c>.track.<t>.key.<k>.v` | the value (the property's own type; text for an event) |
//! | `seq.clip.<c>.track.<t>.key.<k>.interp` | `Constant`, `Linear` or `Smooth` |
//!
//! **Every edit is a command** (I7): adding a key, moving keys, a curve edit, retiming a
//! clip — each is one `SetSetting` transaction, one undo entry; a key drag is one gesture.
//!
//! **Evaluation** is `f64` and deterministic. Floats and each component of a 3-vector use the
//! curve editor's own evaluation ([`forge_ui::widgets::Curve::eval`]: constant, linear, or
//! Catmull-Rom cubic Hermite), so what the curve editor draws is exactly what plays;
//! integers use it too and round; booleans, text and entity references step.

use std::collections::BTreeMap;

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_ui::widgets::{Curve, CurveKey, Interp as CurveInterp};

use crate::domain::{boolean, clear, float, int, objects, set, sub_objects, text, unique_id};
use crate::mirror::ProjectMirror;

pub const CLIP: &str = "seq.clip";
/// The default clip length (seconds) and snapping rate.
pub const DEFAULT_LENGTH: f64 = 5.0;
pub const DEFAULT_FPS: i64 = 30;

/// How a key's segment interpolates to the next key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Interp {
    Constant,
    Linear,
    /// Cubic Hermite with Catmull-Rom tangents.
    #[default]
    Smooth,
}

impl Interp {
    pub const ALL: [Interp; 3] = [Interp::Constant, Interp::Linear, Interp::Smooth];
    pub fn name(self) -> &'static str {
        match self {
            Interp::Constant => "Constant",
            Interp::Linear => "Linear",
            Interp::Smooth => "Smooth",
        }
    }
    pub fn parse(s: &str) -> Option<Interp> {
        Interp::ALL.into_iter().find(|i| i.name() == s)
    }
    pub fn to_curve(self) -> CurveInterp {
        match self {
            Interp::Constant => CurveInterp::Constant,
            Interp::Linear => CurveInterp::Linear,
            Interp::Smooth => CurveInterp::Smooth,
        }
    }
    pub fn from_curve(i: CurveInterp) -> Interp {
        match i {
            CurveInterp::Constant => Interp::Constant,
            CurveInterp::Linear => Interp::Linear,
            CurveInterp::Smooth => Interp::Smooth,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrackKind {
    /// Keys a reflected property of an entity.
    Property,
    /// Keys named events (the game hears them as the playhead crosses them).
    Event,
}

impl TrackKind {
    pub fn name(self) -> &'static str {
        match self {
            TrackKind::Property => "Property",
            TrackKind::Event => "Event",
        }
    }
    pub fn parse(s: &str) -> Option<TrackKind> {
        match s {
            "Property" => Some(TrackKind::Property),
            "Event" => Some(TrackKind::Event),
            _ => None,
        }
    }
}

/// One key.
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    pub id: String,
    pub t: f64,
    pub v: Value,
    pub interp: Interp,
}

/// One track: its keys sorted by time (ties by id).
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: String,
    pub kind: TrackKind,
    pub entity: Option<EntityKey>,
    /// The reflect path (a property track) or the track's name (an event track).
    pub path: String,
    pub muted: bool,
    pub keys: Vec<Key>,
    /// Values of this track that were skipped, and why.
    pub problems: Vec<String>,
}

/// How many curve components a value has (0: it steps).
pub fn components(v: &Value) -> usize {
    match v {
        Value::Float(_) | Value::Int(_) => 1,
        Value::Vec3(_) => 3,
        _ => 0,
    }
}

fn component(v: &Value, c: usize) -> Option<f64> {
    match (v, c) {
        (Value::Float(x), 0) => Some(*x),
        (Value::Int(i), 0) => Some(*i as f64),
        (Value::Vec3(a), c) if c < 3 => Some(a[c]),
        _ => None,
    }
}

impl Track {
    /// The value kind the track keys (its first key's), if it has keys.
    pub fn value_kind(&self) -> Option<&'static str> {
        self.keys.first().map(|k| k.v.kind())
    }

    /// Component `c` of the track as a curve (for the curve editor and for evaluation).
    pub fn curve(&self, c: usize) -> Curve {
        Curve::new(
            self.keys
                .iter()
                .filter_map(|k| {
                    component(&k.v, c).map(|v| CurveKey {
                        t: k.t,
                        v,
                        interp: k.interp.to_curve(),
                    })
                })
                .collect(),
        )
    }

    /// The track's value at `t` (clamped to the first/last key outside the keyed range).
    pub fn sample(&self, t: f64) -> Option<Value> {
        let first = self.keys.first()?;
        match &first.v {
            Value::Float(_) => Some(Value::Float(self.curve(0).eval(t))),
            Value::Int(_) => Some(Value::Int(self.curve(0).eval(t).round() as i64)),
            Value::Vec3(_) => Some(Value::Vec3([
                self.curve(0).eval(t),
                self.curve(1).eval(t),
                self.curve(2).eval(t),
            ])),
            // Steps: the last key at or before `t`, else the first.
            _ => {
                let i = self.keys.partition_point(|k| k.t <= t);
                Some(self.keys[i.saturating_sub(1)].v.clone())
            }
        }
    }

    /// The key nearest `t` within `tolerance` seconds.
    pub fn key_near(&self, t: f64, tolerance: f64) -> Option<&Key> {
        self.keys
            .iter()
            .filter(|k| (k.t - t).abs() <= tolerance)
            .min_by(|a, b| (a.t - t).abs().total_cmp(&(b.t - t).abs()))
    }
}

/// One clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub id: String,
    pub name: String,
    pub length: f64,
    pub fps: i64,
    pub looping: bool,
    pub tracks: BTreeMap<String, Track>,
    /// Values of the clip's own fields that were skipped, and why.
    pub problems: Vec<String>,
}

impl Clip {
    /// The time a playhead at `t` samples: wrapped into the clip when it loops, clamped
    /// otherwise.
    pub fn local_time(&self, t: f64) -> f64 {
        if self.looping && self.length > 0.0 {
            t.rem_euclid(self.length)
        } else {
            t.clamp(0.0, self.length.max(0.0))
        }
    }
    /// One frame at the snapping rate.
    pub fn frame(&self) -> f64 {
        1.0 / self.fps.clamp(1, 240) as f64
    }
    /// `t` snapped to the nearest frame, inside the clip.
    pub fn snap(&self, t: f64) -> f64 {
        let f = self.frame();
        ((t / f).round() * f).clamp(0.0, self.length.max(0.0))
    }
    /// Every property the clip drives at `t`: `(entity, path, value)`, unmuted tracks only.
    pub fn sample(&self, t: f64) -> Vec<(EntityKey, String, Value)> {
        let lt = self.local_time(t);
        self.tracks
            .values()
            .filter(|tr| tr.kind == TrackKind::Property && !tr.muted)
            .filter_map(|tr| Some((tr.entity?, tr.path.clone(), tr.sample(lt)?)))
            .collect()
    }
    /// Event keys the playhead crossed going from `t0` to `t1` (`t0 < t ≤ t1`, in clip
    /// time; one wrap for a looping clip).
    pub fn events_between(&self, t0: f64, t1: f64) -> Vec<(f64, String, String)> {
        let mut out = Vec::new();
        let ranges: Vec<(f64, f64)> = if self.looping && t1 - t0 < self.length {
            let (a, b) = (self.local_time(t0), self.local_time(t1));
            if b >= a {
                vec![(a, b)]
            } else {
                vec![(a, self.length), (-1.0, b)]
            }
        } else {
            vec![(t0, t1)]
        };
        // In the order the playhead crosses them: range by range, by time within one.
        for (a, b) in ranges {
            let mut part = Vec::new();
            for tr in self
                .tracks
                .values()
                .filter(|tr| tr.kind == TrackKind::Event && !tr.muted)
            {
                for k in tr.keys.iter().filter(|k| k.t > a && k.t <= b) {
                    let name = match &k.v {
                        Value::Text(s) => s.clone(),
                        other => format!("{other:?}"),
                    };
                    part.push((k.t, tr.path.clone(), name));
                }
            }
            part.sort_by(|x, y| x.0.total_cmp(&y.0));
            out.extend(part);
        }
        out
    }
    /// The entities the clip animates.
    pub fn animated(&self) -> Vec<EntityKey> {
        let mut v: Vec<EntityKey> = self
            .tracks
            .values()
            .filter(|t| t.kind == TrackKind::Property)
            .filter_map(|t| t.entity)
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

/// Every clip, read from the mirror; malformed values are skipped and listed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimelineDoc {
    pub clips: BTreeMap<String, Clip>,
    pub problems: Vec<String>,
}

fn clip_key(c: &str, f: &str) -> String {
    format!("{CLIP}.{c}.{f}")
}
fn track_key(c: &str, t: &str, f: &str) -> String {
    format!("{CLIP}.{c}.track.{t}.{f}")
}
fn key_key(c: &str, t: &str, k: &str, f: &str) -> String {
    format!("{CLIP}.{c}.track.{t}.key.{k}.{f}")
}

/// The fields of one object: `prefix.field → value`, keyed by `field`.
fn fields_under<'a>(m: &'a ProjectMirror, prefix: &str) -> BTreeMap<&'a str, &'a Value> {
    let p = format!("{prefix}.");
    m.settings_under(&p)
        .map(|(k, v)| (&k[p.len()..], v))
        .collect()
}

fn parse_track(tid: &str, tf: &BTreeMap<&str, &Value>, at: &str) -> Track {
    let mut problems = Vec::new();
    let p = &mut problems;
    let tat = format!("{at}.track.{tid}");
    let kind_s = text(tf, "kind", "Property", p, &tat);
    let kind = TrackKind::parse(&kind_s).unwrap_or_else(|| {
        p.push(format!(
            "{tat}.kind: unknown kind {kind_s:?} (Property used)"
        ));
        TrackKind::Property
    });
    let entity = match tf.get("entity") {
        Some(Value::Entity(e)) => Some(*e),
        None => None,
        Some(v) => {
            p.push(format!(
                "{tat}.entity: expected an entity, found {}",
                v.kind()
            ));
            None
        }
    };
    if kind == TrackKind::Property && entity.is_none() {
        p.push(format!(
            "{tat}: a property track without an entity drives nothing"
        ));
    }
    let mut keys = Vec::new();
    let mut key_fields: BTreeMap<&str, BTreeMap<&str, &Value>> = BTreeMap::new();
    for (k, v) in tf {
        if let Some(rest) = k.strip_prefix("key.")
            && let Some((kid, field)) = rest.split_once('.')
        {
            key_fields.entry(kid).or_default().insert(field, *v);
        }
    }
    for (kid, kf) in key_fields {
        let kat = format!("{tat}.key.{kid}");
        let t = float(&kf, "t", 0.0, p, &kat);
        let Some(v) = kf.get("v").map(|v| (*v).clone()) else {
            p.push(format!("{kat}: a key without a value"));
            continue;
        };
        let is_s = text(&kf, "interp", "Smooth", p, &kat);
        let interp = Interp::parse(&is_s).unwrap_or_else(|| {
            p.push(format!("{kat}.interp: unknown {is_s:?} (Smooth used)"));
            Interp::Smooth
        });
        keys.push(Key {
            id: kid.to_string(),
            t,
            v,
            interp,
        });
    }
    keys.sort_by(|a, b| a.t.total_cmp(&b.t).then_with(|| a.id.cmp(&b.id)));
    // A track keys one value kind: keys of another kind are skipped.
    if let Some(kind0) = keys.first().map(|k| k.v.kind()) {
        let before = keys.len();
        keys.retain(|k| k.v.kind() == kind0);
        if keys.len() != before {
            p.push(format!(
                "{tat}: {} key(s) of another type than {kind0} skipped",
                before - keys.len()
            ));
        }
    }
    let path = text(tf, "path", "", p, &tat);
    let muted = boolean(tf, "muted", false, p, &tat);
    Track {
        id: tid.to_string(),
        kind,
        entity,
        path,
        muted,
        keys,
        problems,
    }
}

fn parse_clip(id: &str, f: &BTreeMap<&str, &Value>) -> Clip {
    let mut problems = Vec::new();
    let p = &mut problems;
    let at = format!("{CLIP}.{id}");
    let mut length = float(f, "length", DEFAULT_LENGTH, p, &at);
    if !(length > 0.0 && length.is_finite()) {
        p.push(format!(
            "{at}.length: {length} is not a positive length (5 s used)"
        ));
        length = DEFAULT_LENGTH;
    }
    let mut fps = int(f, "fps", DEFAULT_FPS, p, &at);
    if !(1..=240).contains(&fps) {
        p.push(format!(
            "{at}.fps: {fps} is outside 1..=240 ({DEFAULT_FPS} used)"
        ));
        fps = DEFAULT_FPS;
    }
    let tracks = sub_objects(f, "track")
        .into_iter()
        .map(|(tid, tf)| (tid.to_string(), parse_track(tid, &tf, &at)))
        .collect();
    let name = text(f, "name", id, p, &at);
    let looping = boolean(f, "loop", false, p, &at);
    Clip {
        id: id.to_string(),
        name,
        length,
        fps,
        looping,
        tracks,
        problems,
    }
}

impl TimelineDoc {
    pub fn read(m: &ProjectMirror) -> TimelineDoc {
        let mut d = TimelineDoc::default();
        for (id, f) in objects(m, CLIP) {
            d.clips.insert(id.to_string(), parse_clip(id, &f));
        }
        d.collect_problems();
        d
    }

    fn collect_problems(&mut self) {
        self.problems = self
            .clips
            .values()
            .flat_map(|c| {
                c.problems
                    .iter()
                    .chain(c.tracks.values().flat_map(|t| t.problems.iter()))
                    .cloned()
            })
            .collect();
    }

    /// Bring the doc up to date with the settings changed since it was read (`keys`, the
    /// mirror's change log): a clip whose own fields changed is re-read, otherwise only the
    /// tracks touched are — a key drag in a clip of thousands of keys re-reads one track.
    pub fn apply_changes<'k>(
        &mut self,
        m: &ProjectMirror,
        keys: impl Iterator<Item = &'k str>,
    ) -> DocChanges {
        let mut clips: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut tracks: std::collections::BTreeSet<(String, String)> =
            std::collections::BTreeSet::new();
        for k in keys {
            let Some(rest) = k.strip_prefix("seq.clip.") else {
                continue;
            };
            let mut it = rest.splitn(4, '.');
            let (Some(c), f) = (it.next(), it.next()) else {
                continue;
            };
            match (f, it.next()) {
                (Some("track"), Some(t)) => {
                    tracks.insert((c.to_string(), t.to_string()));
                }
                _ => {
                    clips.insert(c.to_string());
                }
            }
        }
        for c in &clips {
            let f = fields_under(m, &format!("{CLIP}.{c}"));
            if f.is_empty() {
                self.clips.remove(c);
            } else {
                self.clips.insert(c.clone(), parse_clip(c, &f));
            }
        }
        let mut out = DocChanges::default();
        for (c, t) in tracks {
            if clips.contains(&c) {
                continue;
            }
            let f = fields_under(m, &format!("{CLIP}.{c}.track.{t}"));
            match self.clips.get_mut(&c) {
                Some(clip) if f.is_empty() => {
                    clip.tracks.remove(&t);
                    out.tracks.insert((c, t));
                }
                Some(clip) => {
                    let at = format!("{CLIP}.{c}");
                    clip.tracks.insert(t.clone(), parse_track(&t, &f, &at));
                    out.tracks.insert((c, t));
                }
                // A track of a clip with no fields yet: read the clip whole.
                None => {
                    let f = fields_under(m, &format!("{CLIP}.{c}"));
                    if !f.is_empty() {
                        self.clips.insert(c.clone(), parse_clip(&c, &f));
                    }
                    out.clips.insert(c);
                }
            }
        }
        out.clips.extend(clips);
        self.collect_problems();
        out
    }
}

/// What [`TimelineDoc::apply_changes`] re-read: whole clips (their own fields changed, or
/// they appeared or went), and single tracks of other clips. A view redraws only these: a
/// key drag re-reads, and re-shows, one track.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocChanges {
    pub clips: std::collections::BTreeSet<String>,
    /// `(clip, track)`: re-read (or removed, when the clip no longer has it).
    pub tracks: std::collections::BTreeSet<(String, String)>,
}

impl DocChanges {
    /// Nothing was re-read.
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty() && self.tracks.is_empty()
    }
}

// ---- edits (each returns the commands of one transaction) --------------------------------

/// A new clip: `(id, commands)`.
pub fn new_clip(doc: &TimelineDoc, name: &str) -> (String, Vec<EditorCommand>) {
    let id = unique_id(&crate::domain::ident_from(name), |s| {
        doc.clips.contains_key(s)
    });
    let cmds = vec![
        set(clip_key(&id, "name"), Value::Text(name.to_string())),
        set(clip_key(&id, "length"), Value::Float(DEFAULT_LENGTH)),
        set(clip_key(&id, "fps"), Value::Int(DEFAULT_FPS)),
        set(clip_key(&id, "loop"), Value::Bool(false)),
    ];
    (id, cmds)
}

/// Retime a clip: its length (keys past the new end are kept, and flagged by the panel).
pub fn set_length(clip: &Clip, length: f64) -> Result<Vec<EditorCommand>, String> {
    if !(length > 0.0 && length.is_finite() && length <= 3600.0) {
        return Err(format!("a clip is 0..3600 s long, not {length}"));
    }
    Ok(vec![set(
        clip_key(&clip.id, "length"),
        Value::Float(length),
    )])
}

pub fn set_fps(clip: &Clip, fps: i64) -> Result<Vec<EditorCommand>, String> {
    if !(1..=240).contains(&fps) {
        return Err(format!(
            "the snapping rate is 1..=240 frames per second, not {fps}"
        ));
    }
    Ok(vec![set(clip_key(&clip.id, "fps"), Value::Int(fps))])
}

pub fn set_loop(clip: &Clip, looping: bool) -> Vec<EditorCommand> {
    vec![set(clip_key(&clip.id, "loop"), Value::Bool(looping))]
}

pub fn rename_clip(clip: &Clip, name: &str) -> Vec<EditorCommand> {
    vec![set(
        clip_key(&clip.id, "name"),
        Value::Text(name.to_string()),
    )]
}

/// Remove a clip and everything in it.
pub fn delete_clip(m: &ProjectMirror, clip: &Clip) -> Vec<EditorCommand> {
    crate::domain::clear_under(m, &format!("{CLIP}.{}", clip.id))
}

fn track_id(clip: &Clip, base: &str) -> String {
    unique_id(&crate::domain::ident_from(base), |s| {
        clip.tracks.contains_key(s)
    })
}

/// The next free key id of a track (`k0`, `k1`, …).
fn next_key_id(track: &Track) -> String {
    let mut n = track.keys.len();
    loop {
        let id = format!("k{n}");
        if !track.keys.iter().any(|k| k.id == id) {
            return id;
        }
        n += 1;
    }
}

/// A property track on `entity`'s `path`, keyed with the property's current value at `t`
/// (a track with no key would drive nothing). Refused when the entity or the property does
/// not exist, or the clip already animates that property.
pub fn add_property_track(
    m: &ProjectMirror,
    clip: &Clip,
    entity: EntityKey,
    path: &str,
    t: f64,
) -> Result<(String, Vec<EditorCommand>), String> {
    let e = m
        .entity(entity)
        .ok_or_else(|| format!("{entity} is not in the project"))?;
    let v = e
        .properties
        .get(path)
        .ok_or_else(|| format!("{} has no property {path:?}", e.name))?;
    if clip
        .tracks
        .values()
        .any(|tr| tr.entity == Some(entity) && tr.path == path)
    {
        return Err(format!("{} already animates {}.{path}", clip.name, e.name));
    }
    let id = track_id(clip, &format!("{}_{}", e.name, path.replace('.', "_")));
    let k = "k0".to_string();
    let interp = if components(v) > 0 {
        Interp::Smooth
    } else {
        Interp::Constant
    };
    let t = clip.snap(t);
    Ok((
        id.clone(),
        vec![
            set(
                track_key(&clip.id, &id, "kind"),
                Value::Text("Property".into()),
            ),
            set(track_key(&clip.id, &id, "entity"), Value::Entity(entity)),
            set(
                track_key(&clip.id, &id, "path"),
                Value::Text(path.to_string()),
            ),
            set(key_key(&clip.id, &id, &k, "t"), Value::Float(t)),
            set(key_key(&clip.id, &id, &k, "v"), v.clone()),
            set(
                key_key(&clip.id, &id, &k, "interp"),
                Value::Text(interp.name().into()),
            ),
        ],
    ))
}

/// An event track named `name`.
pub fn add_event_track(clip: &Clip, name: &str) -> (String, Vec<EditorCommand>) {
    let id = track_id(clip, name);
    (
        id.clone(),
        vec![
            set(
                track_key(&clip.id, &id, "kind"),
                Value::Text("Event".into()),
            ),
            set(
                track_key(&clip.id, &id, "path"),
                Value::Text(name.to_string()),
            ),
        ],
    )
}

pub fn set_muted(clip: &Clip, track: &Track, muted: bool) -> Vec<EditorCommand> {
    vec![set(
        track_key(&clip.id, &track.id, "muted"),
        Value::Bool(muted),
    )]
}

pub fn delete_track(m: &ProjectMirror, clip: &Clip, track: &Track) -> Vec<EditorCommand> {
    crate::domain::clear_under(m, &format!("{CLIP}.{}.track.{}", clip.id, track.id))
}

/// Key `v` at `t` (snapped to a frame): the key already on that frame changes value,
/// otherwise a new key is added. The value must be of the track's kind.
pub fn set_key(clip: &Clip, track: &Track, t: f64, v: Value) -> Result<Vec<EditorCommand>, String> {
    if !v.is_finite() {
        return Err("a key's value must be finite".into());
    }
    if let Some(kind) = track.value_kind()
        && kind != v.kind()
    {
        return Err(format!(
            "{} keys {kind} values, not {}",
            track.path,
            v.kind()
        ));
    }
    if track.kind == TrackKind::Event && !matches!(v, Value::Text(_)) {
        return Err("an event key is the event's name (text)".into());
    }
    let t = clip.snap(t);
    let half = clip.frame() * 0.5;
    if let Some(k) = track.key_near(t, half) {
        return Ok(vec![set(key_key(&clip.id, &track.id, &k.id, "v"), v)]);
    }
    let id = next_key_id(track);
    let interp = if components(&v) > 0 {
        Interp::Smooth
    } else {
        Interp::Constant
    };
    Ok(vec![
        set(key_key(&clip.id, &track.id, &id, "t"), Value::Float(t)),
        set(key_key(&clip.id, &track.id, &id, "v"), v),
        set(
            key_key(&clip.id, &track.id, &id, "interp"),
            Value::Text(interp.name().into()),
        ),
    ])
}

/// Key the property's current value (what the inspector shows) at `t`.
pub fn key_current(
    m: &ProjectMirror,
    clip: &Clip,
    track: &Track,
    t: f64,
) -> Result<Vec<EditorCommand>, String> {
    let e = track
        .entity
        .ok_or("an event track has no property to key")?;
    let v = m
        .property(e, &track.path)
        .ok_or_else(|| format!("{e} no longer has {}", track.path))?
        .clone();
    set_key(clip, track, t, v)
}

/// Move keys by `dt` seconds (snapped to frames, kept inside the clip). Two keys may not
/// land on one frame: refused with the time.
pub fn move_keys(
    clip: &Clip,
    track: &Track,
    ids: &[String],
    dt: f64,
) -> Result<Vec<EditorCommand>, String> {
    let mut new_t: BTreeMap<&str, f64> = BTreeMap::new();
    for k in &track.keys {
        let t = if ids.contains(&k.id) {
            clip.snap(k.t + dt)
        } else {
            k.t
        };
        new_t.insert(&k.id, t);
    }
    let half = clip.frame() * 0.5;
    let mut times: Vec<f64> = new_t.values().copied().collect();
    times.sort_by(f64::total_cmp);
    if let Some(w) = times.windows(2).find(|w| w[1] - w[0] < half) {
        return Err(format!("two keys would share the frame at {:.3} s", w[0]));
    }
    Ok(track
        .keys
        .iter()
        .filter(|k| ids.contains(&k.id))
        .filter(|k| new_t.get(k.id.as_str()) != Some(&k.t))
        .map(|k| {
            set(
                key_key(&clip.id, &track.id, &k.id, "t"),
                Value::Float(new_t[k.id.as_str()]),
            )
        })
        .collect())
}

pub fn delete_keys(
    m: &ProjectMirror,
    clip: &Clip,
    track: &Track,
    ids: &[String],
) -> Vec<EditorCommand> {
    ids.iter()
        .flat_map(|k| {
            crate::domain::clear_under(m, &format!("{CLIP}.{}.track.{}.key.{k}", clip.id, track.id))
        })
        .collect()
}

pub fn set_interp(
    clip: &Clip,
    track: &Track,
    ids: &[String],
    interp: Interp,
) -> Vec<EditorCommand> {
    ids.iter()
        .map(|k| {
            set(
                key_key(&clip.id, &track.id, k, "interp"),
                Value::Text(interp.name().into()),
            )
        })
        .collect()
}

/// The commands that make component `c` of `track` equal the curve editor's `curve`: keys
/// matched in time order (retimed, revalued, re-interpolated), extra curve keys added,
/// missing ones removed. One transaction.
pub fn apply_curve(
    m: &ProjectMirror,
    clip: &Clip,
    track: &Track,
    c: usize,
    curve: &Curve,
) -> Result<Vec<EditorCommand>, String> {
    let Some(first) = track.keys.first() else {
        return Err("the track has no keys to edit".into());
    };
    if c >= components(&first.v) {
        return Err(format!(
            "{} keys {} values, which have no curve {c}",
            track.path,
            first.v.kind()
        ));
    }
    let mut cmds = Vec::new();
    let with = |v: &Value, x: f64| -> Value {
        match v {
            Value::Float(_) => Value::Float(x),
            Value::Int(_) => Value::Int(x.round() as i64),
            Value::Vec3(a) => {
                let mut a = *a;
                a[c] = x;
                Value::Vec3(a)
            }
            other => other.clone(),
        }
    };
    let mut ids: Vec<String> = track.keys.iter().map(|k| k.id.clone()).collect();
    let mut probe = track.clone();
    for (i, ck) in curve.keys.iter().enumerate() {
        if !(ck.t.is_finite() && ck.v.is_finite()) {
            return Err("a curve key must be finite".into());
        }
        let t = clip.snap(ck.t);
        match track.keys.get(i) {
            Some(k) => {
                if (k.t - t).abs() > f64::EPSILON {
                    cmds.push(set(
                        key_key(&clip.id, &track.id, &k.id, "t"),
                        Value::Float(t),
                    ));
                }
                let nv = with(&k.v, ck.v);
                if nv != k.v {
                    cmds.push(set(key_key(&clip.id, &track.id, &k.id, "v"), nv));
                }
                let ni = Interp::from_curve(ck.interp);
                if ni != k.interp {
                    cmds.push(set(
                        key_key(&clip.id, &track.id, &k.id, "interp"),
                        Value::Text(ni.name().into()),
                    ));
                }
            }
            None => {
                // A new key: the other components take the track's value at that time.
                let base = track.sample(t).unwrap_or_else(|| first.v.clone());
                let id = next_key_id(&probe);
                probe.keys.push(Key {
                    id: id.clone(),
                    t,
                    v: base.clone(),
                    interp: Interp::Smooth,
                });
                ids.push(id.clone());
                cmds.push(set(key_key(&clip.id, &track.id, &id, "t"), Value::Float(t)));
                cmds.push(set(
                    key_key(&clip.id, &track.id, &id, "v"),
                    with(&base, ck.v),
                ));
                cmds.push(set(
                    key_key(&clip.id, &track.id, &id, "interp"),
                    Value::Text(Interp::from_curve(ck.interp).name().into()),
                ));
            }
        }
    }
    for k in track.keys.iter().skip(curve.keys.len()) {
        cmds.extend(crate::domain::clear_under(
            m,
            &format!("{CLIP}.{}.track.{}.key.{}", clip.id, track.id, k.id),
        ));
    }
    Ok(cmds)
}

/// Clear one key field (used by tests and the panel's "reset interpolation").
pub fn clear_key_field(clip: &Clip, track: &Track, key: &str, field: &str) -> EditorCommand {
    clear(key_key(&clip.id, &track.id, key, field))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(keys: &[(f64, Value, Interp)]) -> Track {
        Track {
            id: "t".into(),
            kind: TrackKind::Property,
            entity: Some(EntityKey(1)),
            path: "p".into(),
            muted: false,
            keys: keys
                .iter()
                .enumerate()
                .map(|(i, (t, v, interp))| Key {
                    id: format!("k{i}"),
                    t: *t,
                    v: v.clone(),
                    interp: *interp,
                })
                .collect(),
            problems: Vec::new(),
        }
    }

    #[test]
    fn floats_vectors_ints_and_steps_evaluate() {
        let f = track(&[
            (0.0, Value::Float(0.0), Interp::Linear),
            (2.0, Value::Float(10.0), Interp::Linear),
        ]);
        assert_eq!(f.sample(1.0), Some(Value::Float(5.0)));
        assert_eq!(f.sample(-1.0), Some(Value::Float(0.0)), "clamped before");
        assert_eq!(f.sample(9.0), Some(Value::Float(10.0)), "clamped after");
        let v = track(&[
            (0.0, Value::Vec3([0.0, 0.0, 0.0]), Interp::Linear),
            (1.0, Value::Vec3([2.0, 4.0, -2.0]), Interp::Linear),
        ]);
        assert_eq!(v.sample(0.5), Some(Value::Vec3([1.0, 2.0, -1.0])));
        let i = track(&[
            (0.0, Value::Int(0), Interp::Linear),
            (1.0, Value::Int(3), Interp::Linear),
        ]);
        assert_eq!(i.sample(0.5), Some(Value::Int(2)), "1.5 rounds to 2");
        let c = track(&[
            (0.0, Value::Float(1.0), Interp::Constant),
            (1.0, Value::Float(5.0), Interp::Constant),
        ]);
        assert_eq!(c.sample(0.99), Some(Value::Float(1.0)));
        let b = track(&[
            (0.0, Value::Bool(false), Interp::Constant),
            (1.0, Value::Bool(true), Interp::Constant),
        ]);
        assert_eq!(b.sample(0.5), Some(Value::Bool(false)));
        assert_eq!(b.sample(1.0), Some(Value::Bool(true)));
        // Smooth passes through its keys and matches the curve editor's evaluation.
        let s = track(&[
            (0.0, Value::Float(0.0), Interp::Smooth),
            (1.0, Value::Float(1.0), Interp::Smooth),
            (2.0, Value::Float(0.0), Interp::Smooth),
        ]);
        assert_eq!(s.sample(1.0), Some(Value::Float(1.0)));
        assert_eq!(s.sample(0.5), Some(Value::Float(s.curve(0).eval(0.5))));
    }

    #[test]
    fn looping_and_events() {
        let mut clip = Clip {
            id: "c".into(),
            name: "C".into(),
            length: 2.0,
            fps: 30,
            looping: true,
            tracks: BTreeMap::new(),
            problems: Vec::new(),
        };
        assert_eq!(clip.local_time(2.5), 0.5);
        let mut ev = track(&[
            (0.5, Value::Text("step".into()), Interp::Constant),
            (1.5, Value::Text("land".into()), Interp::Constant),
        ]);
        ev.kind = TrackKind::Event;
        ev.path = "fx".into();
        clip.tracks.insert("fx".into(), ev);
        let e = clip.events_between(1.0, 2.6);
        let names: Vec<&str> = e.iter().map(|x| x.2.as_str()).collect();
        assert_eq!(
            names,
            vec!["land", "step"],
            "wrapped: 1.5, then 0.5 after the loop"
        );
        assert_eq!(clip.snap(0.51), 0.5);
        clip.looping = false;
        assert_eq!(clip.local_time(9.0), 2.0);
    }
}
