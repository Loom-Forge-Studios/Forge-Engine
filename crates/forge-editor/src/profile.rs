//! What the profiler panel reads (Ch.21 §21.21 "Profiler", Ch.29): a frame timeline,
//! named budgets that turn red when over, adapter-pool lanes (Ch.26) and replication
//! statistics (Ch.25), through the [`CounterSource`] trait.
//!
//! **A live-data source (§21.11).** A source is a [`LiveSource`]: it bumps a generation when
//! its data changes and wakes the UI only while the profiler is visible, and the panel
//! refreshes at most 10 Hz. The editor's own UI counters (`ui.self`: frames drawn, frame
//! time) are a separate source registered as `self_ui`, so they are shown but never
//! schedule a frame — the profiler cannot keep the editor awake by measuring it.
//!
//! **The backend.** The editor's source is `forge-trace` (M2-11, WP-13):
//! [`crate::sim_bridge::TraceCounters`] reads the process tracer's live feed — the play
//! core's `sim.step` zones, the viewport host's render time and adapter lane, counters — and
//! maps its snapshot to [`ProfileSnapshot`] ([`crate::sim_bridge::profile_snapshot`]); the
//! named budgets come from `tests/perf/budgets.ron`, the same names the perf gate
//! (`forge-perf-gate`) enforces. [`MemoryCounters`] is a labelled in-memory source (D-4) a test pushes samples
//! into by hand.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use forge_ui::{LiveCell, LiveSource, UiWaker};

/// Frames the timeline keeps.
pub const TIMELINE_FRAMES: usize = 240;

/// One frame on the timeline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameSample {
    /// Whole-frame time, ms.
    pub total_ms: f64,
    /// Named parts of the frame (`render.frame.gpu.opaque`, `sim.step`), ms.
    pub parts: Vec<(String, f64)>,
}

/// A named budget and its latest measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    pub name: String,
    /// Latest measured value (ms, or a count).
    pub value: f64,
    /// The allowance.
    pub budget: f64,
    /// `"ms"` or `""` (a counter).
    pub unit: &'static str,
}

impl Budget {
    /// Over its allowance: the panel shows it red.
    pub fn over(&self) -> bool {
        self.value > self.budget
    }
}

/// Work on one adapter of the pool (Ch.26): spans within the last frame, ms from its start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lane {
    pub adapter: String,
    pub spans: Vec<(f64, f64, String)>,
}

/// Replication statistics (Ch.25).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Replication {
    pub bytes_in_per_s: f64,
    pub bytes_out_per_s: f64,
    pub entities: u64,
}

/// Everything the profiler shows, at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileSnapshot {
    pub frames: Vec<FrameSample>,
    pub budgets: Vec<Budget>,
    pub lanes: Vec<Lane>,
    /// `None`: no replication source (and why, in `replication_note`).
    pub replication: Option<Replication>,
    pub replication_note: String,
    /// Counters, sorted by name: a source's latest raw values (`sim.bodies`, draw counts).
    /// Empty for a source without them.
    pub counters: Vec<(String, f64)>,
    /// Trace events the source dropped at its capacity (0: nothing lost).
    pub dropped_events: u64,
}

/// A profiler data source (see the module docs).
pub trait CounterSource: LiveSource {
    /// What this source is (the panel shows it; an in-memory one says so).
    fn backend(&self) -> String;
    /// The current data.
    fn snapshot(&self) -> ProfileSnapshot;
}

#[derive(Default)]
struct State {
    frames: VecDeque<FrameSample>,
    budgets: BTreeMap<String, Budget>,
    lanes: BTreeMap<String, Lane>,
    replication: Option<Replication>,
    counters: Vec<(String, f64)>,
    dropped_events: u64,
}

/// The in-memory counter source (D-4; see the module docs). Thread-safe: producers on any
/// thread push, the UI reads.
pub struct MemoryCounters {
    state: Mutex<State>,
    cell: Arc<LiveCell>,
    label: String,
}

impl std::fmt::Debug for MemoryCounters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryCounters")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl MemoryCounters {
    /// What the default source is.
    pub const BACKEND: &'static str =
        "in-memory counters (D-4, pushed by hand) \u{2014} the editor reads forge-trace";

    pub fn new() -> Self {
        Self::labelled(Self::BACKEND)
    }

    pub fn labelled(label: &str) -> Self {
        Self {
            state: Mutex::new(State::default()),
            cell: LiveCell::new(),
            label: label.to_string(),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut g)
    }

    /// Declare a named budget (its allowance).
    pub fn set_budget(&self, name: &str, budget: f64, unit: &'static str) {
        self.with(|s| {
            s.budgets
                .entry(name.to_string())
                .and_modify(|b| {
                    b.budget = budget;
                    b.unit = unit;
                })
                .or_insert(Budget {
                    name: name.to_string(),
                    value: 0.0,
                    budget,
                    unit,
                });
        });
    }

    /// Push one frame: its total and named parts. Parts that name a budget update it.
    pub fn push_frame(&self, total_ms: f64, parts: &[(&str, f64)]) {
        let changed = self.with(|s| {
            for (n, v) in parts {
                if let Some(b) = s.budgets.get_mut(*n) {
                    b.value = *v;
                }
            }
            s.frames.push_back(FrameSample {
                total_ms,
                parts: parts.iter().map(|(n, v)| ((*n).to_string(), *v)).collect(),
            });
            while s.frames.len() > TIMELINE_FRAMES {
                s.frames.pop_front();
            }
            true
        });
        if changed {
            self.cell.bump();
        }
    }

    /// Replace one adapter's lane.
    pub fn set_lane(&self, adapter: &str, spans: Vec<(f64, f64, String)>) {
        self.with(|s| {
            s.lanes.insert(
                adapter.to_string(),
                Lane {
                    adapter: adapter.to_string(),
                    spans,
                },
            );
        });
        self.cell.bump();
    }

    pub fn set_replication(&self, r: Option<Replication>) {
        self.with(|s| s.replication = r);
        self.cell.bump();
    }

    /// Set raw counters (the source may have them; keys sorted on output).
    pub fn set_counters(&self, counters: Vec<(&str, f64)>) {
        let changed = self.with(|s| {
            s.counters = counters
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            true
        });
        if changed {
            self.cell.bump();
        }
    }

    /// Set the dropped-event count (events the source dropped at capacity).
    pub fn set_dropped_events(&self, n: u64) {
        self.with(|s| s.dropped_events = n);
        self.cell.bump();
    }

    /// Mark the producer running (engine playing) or stopped.
    pub fn set_live(&self, live: bool) {
        self.cell.set_live(live);
    }

    /// Load the named budgets of `tests/perf/budgets.ron` (their names and, for the dev box
    /// class `class`, their allowance). Returns how many were declared.
    pub fn declare_budgets_ron(&self, text: &str, class: &str) -> usize {
        #[derive(serde::Deserialize)]
        struct Row {
            name: String,
            kind: Kind,
            #[serde(default)]
            baseline: BTreeMap<String, f64>,
            #[serde(default)]
            max: Option<f64>,
            #[serde(default)]
            ratio: Option<f64>,
        }
        #[derive(serde::Deserialize)]
        enum Kind {
            GpuMs,
            CpuRatio,
            Counter,
        }
        #[derive(serde::Deserialize)]
        struct File {
            band: f64,
            #[serde(default)]
            gpu_slack_ms: BTreeMap<String, f64>,
            rows: Vec<Row>,
        }
        let opts = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let Ok(f) = opts.from_str::<File>(text) else {
            return 0;
        };
        let slack = f.gpu_slack_ms.get(class).copied().unwrap_or(0.0);
        let mut n = 0;
        for r in f.rows {
            match r.kind {
                Kind::GpuMs => {
                    if let Some(b) = r.baseline.get(class) {
                        self.set_budget(&r.name, (b * (1.0 + f.band)).max(b + slack), "ms");
                        n += 1;
                    }
                }
                Kind::Counter => {
                    if let Some(m) = r.max {
                        self.set_budget(&r.name, m, "");
                        n += 1;
                    }
                }
                Kind::CpuRatio => {
                    let _ = r.ratio;
                }
            }
        }
        n
    }
}

impl Default for MemoryCounters {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveSource for MemoryCounters {
    fn generation(&self) -> u64 {
        self.cell.generation()
    }
    fn is_live(&self) -> bool {
        self.cell.is_live()
    }
    fn set_waker(&self, waker: Option<Arc<dyn UiWaker>>) {
        self.cell.set_waker(waker);
    }
    fn consumed(&self) {
        self.cell.consumed();
    }
}

impl CounterSource for MemoryCounters {
    fn backend(&self) -> String {
        self.label.clone()
    }
    fn snapshot(&self) -> ProfileSnapshot {
        self.with(|s| ProfileSnapshot {
            frames: s.frames.iter().cloned().collect(),
            budgets: s.budgets.values().cloned().collect(),
            lanes: s.lanes.values().cloned().collect(),
            replication: s.replication.clone(),
            replication_note: if s.replication.is_none() {
                "No replication: forge-net (M4-7) is not built, so there is no session to measure."
                    .into()
            } else {
                String::new()
            },
            counters: s.counters.clone(),
            dropped_events: s.dropped_events,
        })
    }
}

/// The editor's own UI counters (`ui.self`): frames drawn and the last frame's time. The
/// shell updates them every rendered frame; the profiler registers this source as a
/// `self_ui` feed, so updating it never wakes the loop (§21.11 rule 4).
#[derive(Default)]
pub struct SelfUiCounters {
    cell: Arc<LiveCell>,
    frames: std::sync::atomic::AtomicU64,
    last_frame_us: std::sync::atomic::AtomicU64,
    draw_calls: std::sync::atomic::AtomicU64,
}

impl SelfUiCounters {
    pub fn new() -> Self {
        Self::default()
    }
    /// Record one rendered frame.
    pub fn frame(&self, took: std::time::Duration) {
        use std::sync::atomic::Ordering::Relaxed;
        self.frames.fetch_add(1, Relaxed);
        self.last_frame_us
            .store(took.as_micros().min(u128::from(u64::MAX)) as u64, Relaxed);
        self.cell.bump();
    }

    /// Follow a window's frame counter and its last frame's draw calls (the shell calls
    /// this every loop turn; it bumps only when a frame was drawn).
    pub fn observe(&self, frames_rendered: u64, draw_calls: u32) {
        use std::sync::atomic::Ordering::Relaxed;
        if self.frames.swap(frames_rendered, Relaxed) != frames_rendered {
            self.draw_calls.store(u64::from(draw_calls), Relaxed);
            self.cell.bump();
        }
    }
    /// The last frame's draw calls.
    pub fn draw_calls(&self) -> u64 {
        self.draw_calls.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn frames(&self) -> u64 {
        self.frames.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn last_frame_ms(&self) -> f64 {
        self.last_frame_us
            .load(std::sync::atomic::Ordering::Relaxed) as f64
            / 1000.0
    }
    pub fn cell(&self) -> Arc<LiveCell> {
        Arc::clone(&self.cell)
    }
}

impl std::fmt::Debug for SelfUiCounters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfUiCounters")
            .field("frames", &self.frames())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_turn_over_and_frames_are_bounded() {
        let c = MemoryCounters::new();
        c.set_budget("render.frame.gpu.opaque", 1.0, "ms");
        let g0 = c.generation();
        c.push_frame(4.0, &[("render.frame.gpu.opaque", 0.5)]);
        assert!(c.generation() > g0);
        assert!(!c.snapshot().budgets[0].over());
        c.push_frame(4.0, &[("render.frame.gpu.opaque", 1.5)]);
        assert!(c.snapshot().budgets[0].over());
        for _ in 0..1000 {
            c.push_frame(1.0, &[]);
        }
        assert_eq!(c.snapshot().frames.len(), TIMELINE_FRAMES);
        assert!(c.snapshot().replication_note.contains("forge-net"));
    }

    #[test]
    fn the_committed_budget_file_declares_its_gpu_rows() {
        let text = include_str!("../../../tests/perf/budgets.ron");
        let c = MemoryCounters::new();
        let n = c.declare_budgets_ron(text, "rtx3080");
        assert!(n >= 4, "{n}");
        // A GPU row's allowance is its dev-box baseline widened by the band and the slack.
        assert!(
            c.snapshot()
                .budgets
                .iter()
                .any(|b| b.name == "render2d.frame.gpu.sprites" && b.budget > 0.071)
        );
    }
}
