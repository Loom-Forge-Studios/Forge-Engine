//! The audio mixer's model (Ch.20, Ch.21 §21.21 "Audio mixer", DoD M2-67): buses routed
//! into one another and finally into `master`, each with volume, pan, mute and solo, plus
//! sends (pre- or post-fader) to other buses; meters from the audio engine; and the
//! spatialisation settings a spatial preview exercises.
//!
//! Project settings (see [`super`]):
//!
//! | Key | Value |
//! |---|---|
//! | `audio.bus.<b>.name` | text |
//! | `audio.bus.<b>.output` | text: the bus it feeds (absent: `master`; ignored on `master`) |
//! | `audio.bus.<b>.{volume_db,pan}` | number (dB in [−80, +12]; pan in [−1, 1]) |
//! | `audio.bus.<b>.{mute,solo}` | bool |
//! | `audio.bus.<b>.send.<target>.db` | number: send level |
//! | `audio.bus.<b>.send.<target>.pre` | bool: pre-fader |
//! | `audio.spatial.{min_distance,max_distance}` | number (m) |
//! | `audio.spatial.rolloff` | text: `Inverse`, `Linear` or `Logarithmic` |
//!
//! `master` always exists (it has no output). Routing must not loop: the panel refuses an
//! edit that would close one, and a loop written by another issuer is reported and meters
//! as silence on the buses in it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use forge_cmd::{EditorCommand, Value};
use forge_ui::{LiveCell, LiveSource};

use super::{BackendInfo, boolean, float, objects, set, sub_objects, text};
use crate::mirror::ProjectMirror;

pub const BUS: &str = "audio.bus";
pub const SPATIAL: &str = "audio.spatial";
pub const MASTER: &str = "master";
pub const VOLUME_MIN_DB: f64 = -80.0;
pub const VOLUME_MAX_DB: f64 = 12.0;
/// The level shown for silence.
pub const SILENCE_DB: f64 = -120.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Send {
    pub target: String,
    pub db: f64,
    pub pre: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bus {
    pub id: String,
    pub name: String,
    /// `None` only for `master`.
    pub output: Option<String>,
    pub volume_db: f64,
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
    pub sends: BTreeMap<String, Send>,
}

impl Bus {
    fn master() -> Bus {
        Bus {
            id: MASTER.into(),
            name: "Master".into(),
            output: None,
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            sends: BTreeMap::new(),
        }
    }
}

/// Every bus, read from the mirror.
#[derive(Clone, Debug, PartialEq)]
pub struct Mixer {
    pub buses: BTreeMap<String, Bus>,
    pub problems: Vec<String>,
}

impl Mixer {
    pub fn read(m: &ProjectMirror) -> Mixer {
        let mut buses = BTreeMap::new();
        buses.insert(MASTER.to_string(), Bus::master());
        let mut problems = Vec::new();
        let p = &mut problems;
        for (id, f) in objects(m, BUS) {
            let at = format!("{BUS}.{id}");
            let is_master = id == MASTER;
            let mut sends = BTreeMap::new();
            for (t, sf) in sub_objects(&f, "send") {
                let sat = format!("{at}.send.{t}");
                sends.insert(
                    t.to_string(),
                    Send {
                        target: t.to_string(),
                        db: float(&sf, "db", 0.0, p, &sat).clamp(VOLUME_MIN_DB, VOLUME_MAX_DB),
                        pre: boolean(&sf, "pre", false, p, &sat),
                    },
                );
            }
            let output = text(&f, "output", MASTER, p, &at);
            buses.insert(
                id.to_string(),
                Bus {
                    id: id.to_string(),
                    name: text(&f, "name", if is_master { "Master" } else { id }, p, &at),
                    output: (!is_master).then_some(output),
                    volume_db: float(&f, "volume_db", 0.0, p, &at)
                        .clamp(VOLUME_MIN_DB, VOLUME_MAX_DB),
                    pan: float(&f, "pan", 0.0, p, &at).clamp(-1.0, 1.0),
                    mute: boolean(&f, "mute", false, p, &at),
                    solo: boolean(&f, "solo", false, p, &at),
                    sends,
                },
            );
        }
        // Outputs and sends naming a missing bus.
        let ids: BTreeSet<String> = buses.keys().cloned().collect();
        for b in buses.values() {
            if let Some(o) = &b.output
                && !ids.contains(o)
            {
                problems.push(format!(
                    "bus {:?} feeds a missing bus {o:?} (it plays into master)",
                    b.name
                ));
            }
            for t in b.sends.keys() {
                if !ids.contains(t) {
                    problems.push(format!("bus {:?} sends to a missing bus {t:?}", b.name));
                }
            }
        }
        let mut mx = Mixer { buses, problems };
        if let Some(l) = mx.route_loop() {
            mx.problems
                .push(format!("routing loop: {}", l.join(" \u{2192} ")));
        }
        mx
    }

    /// Where `b`'s signal goes: its output (a missing one falls back to master) and its
    /// sends' targets.
    pub fn edges(&self, b: &Bus) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(o) = &b.output {
            out.push(if self.buses.contains_key(o) {
                o.clone()
            } else {
                MASTER.to_string()
            });
        }
        out.extend(
            b.sends
                .keys()
                .filter(|t| self.buses.contains_key(*t))
                .cloned(),
        );
        out
    }

    /// Is `to` reachable from `from` along outputs and sends?
    pub fn reaches(&self, from: &str, to: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut stack = vec![from.to_string()];
        while let Some(c) = stack.pop() {
            if c == to {
                return true;
            }
            if !seen.insert(c.clone()) {
                continue;
            }
            if let Some(b) = self.buses.get(&c) {
                stack.extend(self.edges(b));
            }
        }
        false
    }

    /// Would routing `from` into `to` (as its output or a send) close a loop?
    pub fn would_loop(&self, from: &str, to: &str) -> bool {
        from == to || self.reaches(to, from)
    }

    /// A loop in the routing, if an issuer wrote one.
    pub fn route_loop(&self) -> Option<Vec<String>> {
        // Depth-first with colours; report the first back edge's cycle.
        fn visit(
            mx: &Mixer,
            id: &str,
            state: &mut BTreeMap<String, u8>,
            path: &mut Vec<String>,
        ) -> Option<Vec<String>> {
            state.insert(id.to_string(), 1);
            path.push(id.to_string());
            if let Some(b) = mx.buses.get(id) {
                for n in mx.edges(b) {
                    match state.get(&n).copied().unwrap_or(0) {
                        1 => {
                            let at = path.iter().position(|p| *p == n).unwrap_or(0);
                            let mut cyc: Vec<String> = path[at..].to_vec();
                            cyc.push(n);
                            return Some(cyc);
                        }
                        0 => {
                            if let Some(c) = visit(mx, &n, state, path) {
                                return Some(c);
                            }
                        }
                        _ => {}
                    }
                }
            }
            path.pop();
            state.insert(id.to_string(), 2);
            None
        }
        let mut state = BTreeMap::new();
        for id in self.buses.keys() {
            if state.get(id).copied().unwrap_or(0) == 0
                && let Some(c) = visit(self, id, &mut state, &mut Vec::new())
            {
                return Some(c);
            }
        }
        None
    }

    /// The buses feeding `id` through their output (the routing tree's children).
    pub fn children(&self, id: &str) -> Vec<&Bus> {
        self.buses
            .values()
            .filter(|b| b.output.as_deref() == Some(id))
            .collect()
    }
}

/// How loudness falls off with distance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rolloff {
    Inverse,
    Linear,
    Logarithmic,
}

impl Rolloff {
    pub const ALL: [Rolloff; 3] = [Rolloff::Inverse, Rolloff::Linear, Rolloff::Logarithmic];
    pub fn name(self) -> &'static str {
        match self {
            Rolloff::Inverse => "Inverse",
            Rolloff::Linear => "Linear",
            Rolloff::Logarithmic => "Logarithmic",
        }
    }
    pub fn parse(s: &str) -> Option<Rolloff> {
        Rolloff::ALL.into_iter().find(|r| r.name() == s)
    }
}

/// The spatialisation settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spatial {
    pub min_distance: f64,
    pub max_distance: f64,
    pub rolloff: Rolloff,
}

impl Spatial {
    pub fn read(m: &ProjectMirror) -> (Spatial, Vec<String>) {
        let mut p = Vec::new();
        let f: BTreeMap<&str, &Value> = {
            let pre = format!("{SPATIAL}.");
            m.settings_under(&pre)
                .map(|(k, v)| (&k[pre.len()..], v))
                .collect()
        };
        let min = float(&f, "min_distance", 1.0, &mut p, SPATIAL).max(0.01);
        let max = float(&f, "max_distance", 50.0, &mut p, SPATIAL).max(min);
        let r = text(&f, "rolloff", "Inverse", &mut p, SPATIAL);
        let rolloff = Rolloff::parse(&r).unwrap_or_else(|| {
            p.push(format!(
                "{SPATIAL}.rolloff: unknown curve {r:?} (Inverse used)"
            ));
            Rolloff::Inverse
        });
        (
            Spatial {
                min_distance: min,
                max_distance: max,
                rolloff,
            },
            p,
        )
    }
}

/// Where the preview source is, relative to the listener, on the preview plane (metres;
/// `+x` right, `+y` ahead). An offset, not a world position.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PreviewOffset {
    pub right_m: f64,
    pub ahead_m: f64,
}

/// What the listener hears from the preview source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialOut {
    pub distance_m: f64,
    pub gain_db: f64,
    pub pan: f64,
}

/// One bus's meter (dBFS).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meter {
    pub peak_l_db: f64,
    pub peak_r_db: f64,
    pub rms_db: f64,
}

impl Meter {
    pub const SILENT: Meter = Meter {
        peak_l_db: SILENCE_DB,
        peak_r_db: SILENCE_DB,
        rms_db: SILENCE_DB,
    };
    pub fn peak_db(&self) -> f64 {
        self.peak_l_db.max(self.peak_r_db)
    }
}

/// What the mixer asks of the audio engine (Ch.20). `forge-audio` implements it;
/// [`MemoryAudio`] is the labelled in-memory stand-in (D-4).
///
/// `set_mix` hands the engine the project's mix as the mixer reads it (the engine follows
/// project state; this is not an edit). The meters are a live feed (§21.11): `feed` is
/// bumped when a meter changes, and the panel reads them only then.
pub trait AudioBuses {
    fn backend(&self) -> BackendInfo;
    fn set_mix(&self, mixer: &Mixer);
    /// Play (or stop) a test tone into `bus` (session state: an audition, not an edit).
    fn set_preview(&self, bus: &str, on: bool);
    fn previews(&self) -> BTreeSet<String>;
    fn meters(&self) -> BTreeMap<String, Meter>;
    fn feed(&self) -> Arc<dyn LiveSource>;
    fn spatialize(&self, s: &Spatial, at: PreviewOffset) -> SpatialOut;
}

fn db_to_power(db: f64) -> f64 {
    10f64.powf(db / 10.0)
}
fn power_to_db(p: f64) -> f64 {
    if p <= 0.0 {
        SILENCE_DB
    } else {
        (10.0 * p.log10()).max(SILENCE_DB)
    }
}

/// The level of the in-memory test tone (a sine at −12 dBFS peak).
pub const TONE_PEAK_DB: f64 = -12.0;

#[derive(Default)]
struct AudioState {
    mixer: Option<Mixer>,
    previews: BTreeSet<String>,
    meters: BTreeMap<String, Meter>,
}

/// The in-memory audio backend (D-4): no sound device. It evaluates the mix graph —
/// test tones summed through each bus's fader, mute, solo-in-place, sends and pan, in
/// routing order — and meters the result, and bumps its feed only when a meter changes,
/// so a steady tone costs nothing after it settles (zero-idle, D-5).
pub struct MemoryAudio {
    state: Mutex<AudioState>,
    cell: Arc<LiveCell>,
}

impl Default for MemoryAudio {
    fn default() -> Self {
        Self {
            state: Mutex::new(AudioState::default()),
            cell: LiveCell::new(),
        }
    }
}

impl std::fmt::Debug for MemoryAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryAudio").finish_non_exhaustive()
    }
}

impl MemoryAudio {
    pub fn new() -> Self {
        Self::default()
    }

    fn recompute(&self, st: &mut AudioState) {
        let meters = match &st.mixer {
            Some(mx) => evaluate(mx, &st.previews),
            None => BTreeMap::new(),
        };
        if meters != st.meters {
            st.meters = meters;
            self.cell.set_live(!st.previews.is_empty());
            self.cell.bump();
        }
    }
}

/// Meter every bus of `mx` with test tones on `tones` (power sums, a sine's 3 dB crest).
pub fn evaluate(mx: &Mixer, tones: &BTreeSet<String>) -> BTreeMap<String, Meter> {
    let looped: BTreeSet<String> = mx
        .route_loop()
        .map(|l| l.into_iter().collect())
        .unwrap_or_default();
    // Solo in place: a soloed bus, what feeds it, and where it plays stay audible.
    let soloed: Vec<&str> = mx
        .buses
        .values()
        .filter(|b| b.solo)
        .map(|b| b.id.as_str())
        .collect();
    let audible = |id: &str| {
        soloed.is_empty()
            || soloed
                .iter()
                .any(|s| *s == id || mx.reaches(id, s) || mx.reaches(s, id))
    };
    // Topological order over the loop-free part.
    let mut indeg: BTreeMap<&str, usize> = mx.buses.keys().map(|k| (k.as_str(), 0)).collect();
    for b in mx.buses.values() {
        if looped.contains(&b.id) {
            continue;
        }
        for n in mx.edges(b) {
            if !looped.contains(&n)
                && let Some(d) = indeg.get_mut(n.as_str())
            {
                *d += 1;
            }
        }
    }
    let mut ready: Vec<&str> = indeg
        .iter()
        .filter(|(k, d)| **d == 0 && !looped.contains(**k))
        .map(|(k, _)| *k)
        .collect();
    let mut input: BTreeMap<&str, f64> = BTreeMap::new();
    let tone_rms = db_to_power(TONE_PEAK_DB - 3.0103);
    for t in tones {
        if let Some((k, _)) = mx.buses.get_key_value(t) {
            *input.entry(k.as_str()).or_default() += tone_rms;
        }
    }
    let mut out = BTreeMap::new();
    while let Some(id) = ready.pop() {
        let Some(b) = mx.buses.get(id) else { continue };
        let pin = input.get(id).copied().unwrap_or(0.0);
        let fader = if b.mute || !audible(id) {
            0.0
        } else {
            db_to_power(b.volume_db)
        };
        let post = pin * fader;
        let rms_db = power_to_db(post);
        let theta = (b.pan + 1.0) * std::f64::consts::FRAC_PI_4;
        let lr = |g: f64| {
            if post <= 0.0 {
                SILENCE_DB
            } else {
                (rms_db + 3.0103 + 20.0 * (g * std::f64::consts::SQRT_2).max(1e-12).log10())
                    .max(SILENCE_DB)
            }
        };
        out.insert(
            id.to_string(),
            Meter {
                peak_l_db: lr(theta.cos()),
                peak_r_db: lr(theta.sin()),
                rms_db,
            },
        );
        for n in mx.edges(b) {
            if looped.contains(&n) {
                continue;
            }
            let add = if b.output.as_deref() == Some(n.as_str())
                || (b.output.as_ref().is_some_and(|o| !mx.buses.contains_key(o)) && n == MASTER)
            {
                post
            } else {
                match b.sends.get(&n) {
                    Some(s) if s.pre => pin * db_to_power(s.db),
                    Some(s) => post * db_to_power(s.db),
                    None => 0.0,
                }
            };
            if let Some((k, _)) = mx.buses.get_key_value(&n) {
                *input.entry(k.as_str()).or_default() += add;
                if let Some(d) = indeg.get_mut(k.as_str()) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        ready.push(k.as_str());
                    }
                }
            }
        }
    }
    for id in mx.buses.keys() {
        out.entry(id.clone()).or_insert(Meter::SILENT);
    }
    out
}

impl AudioBuses for MemoryAudio {
    fn backend(&self) -> BackendInfo {
        BackendInfo {
            name: "in-memory mixer".into(),
            in_memory: true,
            note: "In-memory mixer (D-4): meters show test tones through the mix graph; the audio engine forge-audio is not built yet, so nothing plays on a device.".into(),
        }
    }
    fn set_mix(&self, mixer: &Mixer) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if st.mixer.as_ref() != Some(mixer) {
            st.mixer = Some(mixer.clone());
            self.recompute(&mut st);
        }
    }
    fn set_preview(&self, bus: &str, on: bool) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let changed = if on {
            st.previews.insert(bus.to_string())
        } else {
            st.previews.remove(bus)
        };
        if changed {
            self.recompute(&mut st);
        }
    }
    fn previews(&self) -> BTreeSet<String> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .previews
            .clone()
    }
    fn meters(&self) -> BTreeMap<String, Meter> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .meters
            .clone()
    }
    fn feed(&self) -> Arc<dyn LiveSource> {
        self.cell.clone()
    }
    fn spatialize(&self, s: &Spatial, at: PreviewOffset) -> SpatialOut {
        let d = at.right_m.hypot(at.ahead_m);
        let dc = d.clamp(s.min_distance, s.max_distance);
        let gain = match s.rolloff {
            Rolloff::Inverse => s.min_distance / dc,
            Rolloff::Linear => {
                let span = (s.max_distance - s.min_distance).max(1e-9);
                (1.0 - (dc - s.min_distance) / span).max(0.0)
            }
            Rolloff::Logarithmic => {
                let r = (s.max_distance / s.min_distance).max(1.0 + 1e-9);
                (1.0 - (dc / s.min_distance).ln() / r.ln()).max(0.0)
            }
        };
        let gain_db = if gain <= 0.0 {
            SILENCE_DB
        } else {
            (20.0 * gain.log10()).max(SILENCE_DB)
        };
        let pan = if d < 1e-9 {
            0.0
        } else {
            (at.right_m / d).clamp(-1.0, 1.0)
        };
        SpatialOut {
            distance_m: d,
            gain_db,
            pan,
        }
    }
}

// ---- edits --------------------------------------------------------------------------------

pub fn bus_key(bus: &str, field: &str) -> String {
    format!("{BUS}.{bus}.{field}")
}

/// A new bus feeding `output`.
pub fn new_bus(mx: &Mixer, name: &str, output: &str) -> (String, Vec<EditorCommand>) {
    let id = super::unique_id(&super::ident_from(name), |s| mx.buses.contains_key(s));
    let cmds = vec![
        set(bus_key(&id, "name"), Value::Text(name.into())),
        set(bus_key(&id, "output"), Value::Text(output.into())),
        set(bus_key(&id, "volume_db"), Value::Float(0.0)),
    ];
    (id, cmds)
}

/// Remove a bus: its keys, the sends into it, and re-route the buses it fed to its own
/// output (so nothing goes silent by surprise). `master` cannot be removed.
pub fn remove_bus(m: &ProjectMirror, mx: &Mixer, id: &str) -> Vec<EditorCommand> {
    if id == MASTER {
        return Vec::new();
    }
    let Some(b) = mx.buses.get(id) else {
        return Vec::new();
    };
    let to = b.output.clone().unwrap_or_else(|| MASTER.to_string());
    let mut cmds = super::clear_under(m, &format!("{BUS}.{id}"));
    for c in mx.children(id) {
        cmds.push(set(bus_key(&c.id, "output"), Value::Text(to.clone())));
    }
    for o in mx.buses.values() {
        if o.sends.contains_key(id) {
            cmds.extend(super::clear_under(m, &format!("{BUS}.{}.send.{id}", o.id)));
        }
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus(id: &str, out: Option<&str>) -> Bus {
        Bus {
            id: id.into(),
            name: id.into(),
            output: out.map(str::to_string),
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            sends: BTreeMap::new(),
        }
    }

    fn mixer(bs: Vec<Bus>) -> Mixer {
        let mut buses = BTreeMap::new();
        buses.insert(MASTER.to_string(), Bus::master());
        for b in bs {
            buses.insert(b.id.clone(), b);
        }
        Mixer {
            buses,
            problems: Vec::new(),
        }
    }

    #[test]
    fn a_tone_meters_through_faders_mute_and_solo() {
        let mut music = bus("music", Some(MASTER));
        music.volume_db = -6.0;
        let sfx = bus("sfx", Some(MASTER));
        let mx = mixer(vec![music, sfx]);
        let tones: BTreeSet<String> = ["music".to_string()].into();
        let m = evaluate(&mx, &tones);
        let music_m = m.get("music").copied().unwrap_or(Meter::SILENT);
        assert!(
            (music_m.peak_db() - (TONE_PEAK_DB - 6.0)).abs() < 1e-6,
            "{music_m:?}"
        );
        let master = m.get(MASTER).copied().unwrap_or(Meter::SILENT);
        assert!((master.peak_db() - (TONE_PEAK_DB - 6.0)).abs() < 1e-6);
        assert_eq!(m.get("sfx").map(|x| x.rms_db), Some(SILENCE_DB));
        // Solo sfx: music is silenced even with its tone.
        let mut mx2 = mx.clone();
        if let Some(b) = mx2.buses.get_mut("sfx") {
            b.solo = true;
        }
        let m2 = evaluate(&mx2, &tones);
        assert_eq!(m2.get("music").map(|x| x.rms_db), Some(SILENCE_DB));
        assert_eq!(m2.get(MASTER).map(|x| x.rms_db), Some(SILENCE_DB));
    }

    #[test]
    fn sends_add_power_and_loops_are_found() {
        let mut dry = bus("dry", Some(MASTER));
        dry.sends.insert(
            "reverb".into(),
            Send {
                target: "reverb".into(),
                db: 0.0,
                pre: false,
            },
        );
        let rev = bus("reverb", Some(MASTER));
        let mx = mixer(vec![dry, rev]);
        let m = evaluate(&mx, &["dry".to_string()].into());
        let master = m.get(MASTER).copied().unwrap_or(Meter::SILENT);
        // Two equal paths sum to +3 dB.
        assert!(
            (master.rms_db - (TONE_PEAK_DB - 3.0103 + 3.0103)).abs() < 1e-3,
            "{master:?}"
        );
        assert!(mx.would_loop("reverb", "dry"));
        assert!(!mx.would_loop("dry", "reverb"));
        let mut looped = mx.clone();
        if let Some(b) = looped.buses.get_mut("reverb") {
            b.output = Some("dry".into());
        }
        assert!(looped.route_loop().is_some());
        let m = evaluate(&looped, &["dry".to_string()].into());
        assert_eq!(m.get("dry").map(|x| x.rms_db), Some(SILENCE_DB));
    }

    #[test]
    fn pan_is_equal_power_and_spatial_gain_falls_off() {
        let mut b = bus("p", Some(MASTER));
        b.pan = 1.0;
        let mx = mixer(vec![b]);
        let m = evaluate(&mx, &["p".to_string()].into());
        let p = m.get("p").copied().unwrap_or(Meter::SILENT);
        assert!(
            (p.peak_r_db - (TONE_PEAK_DB + 3.0103)).abs() < 1e-3,
            "{p:?}"
        );
        assert!(p.peak_l_db < -100.0);
        let a = MemoryAudio::new();
        let s = Spatial {
            min_distance: 1.0,
            max_distance: 100.0,
            rolloff: Rolloff::Inverse,
        };
        let near = a.spatialize(
            &s,
            PreviewOffset {
                right_m: 0.0,
                ahead_m: 1.0,
            },
        );
        let far = a.spatialize(
            &s,
            PreviewOffset {
                right_m: 10.0,
                ahead_m: 0.0,
            },
        );
        assert!(near.gain_db.abs() < 1e-9);
        assert!((far.gain_db + 20.0).abs() < 1e-9);
        assert!((far.pan - 1.0).abs() < 1e-9);
    }
}
