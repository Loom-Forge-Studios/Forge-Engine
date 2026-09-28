//! `forge-trace` — profiling and observability (Ch.29, M2-11).
//!
//! **One API, three sinks.** Code opens [`Zone`]s (`let _z = forge_trace::zone!("sim.step");`),
//! reports [`Tracer::counter`]s and marks frames ([`Tracer::frame_mark`]). Where that goes is
//! a runtime choice ([`Sinks`]):
//!
//! * [`Sinks::COUNTERS`] — the **live feed**: per-frame zone totals on a bounded timeline,
//!   counters, and **named budgets** that turn red when over ([`budget`]). This is what the
//!   editor's profiler panel reads ([`Tracer::snapshot`], [`feed::FeedCell`]) through
//!   `forge_editor::sim_bridge::TraceCounters`, the panel's `CounterSource` (WP-U6).
//! * [`Sinks::PERFETTO`] — a **Perfetto** trace: zones become nested slices on per-thread
//!   tracks, counters become counter tracks, frame marks and budget overruns become instant
//!   events; [`Tracer::take_perfetto`] encodes the native protobuf format ([`perfetto`]), which
//!   <https://ui.perfetto.dev> opens directly.
//! * [`Sinks::TRACY`] — **Tracy** (cargo feature `tracy`): zones, plots and frame marks go to
//!   a Tracy viewer connected over loopback.
//!
//! **Disabled costs nothing measurable.** Every sink off, opening and closing a zone is one
//! relaxed atomic load and a branch: no clock read, no lock, no allocation
//! (`tests/test_trace_overhead.rs` measures it and fails a zone that reads the clock while
//! disabled). A zone captures the sinks at open, so a sink switched on mid-zone never sees a
//! half zone.
//!
//! **Names are budgets.** A zone or counter name is dotted, `subsystem.metric...`
//! (`sim.step`, `render.frame.gpu.opaque`); a [`budget::Budget`] with the same name is
//! measured by it, and [`Tracer::budgets_by_subsystem`] groups them by their first segment.
//! `tests/perf/budgets.ron` (the M1 perf gate's file) declares the render budgets through
//! [`Tracer::declare_budgets_ron`].
//!
//! Memory is bounded: the timeline keeps [`TIMELINE_FRAMES`] frames, and a Perfetto
//! recording keeps at most its capacity of events and counts the rest as dropped.

#![forbid(unsafe_code)]

pub mod budget;
pub mod controls;
mod error;
pub mod feed;
pub mod perfetto;
pub mod timed;
#[cfg(feature = "tracy")]
mod tracy;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

pub use budget::Budget;
pub use error::TraceError;
pub use feed::{FeedCell, FrameSample, Lane, TraceSnapshot};

/// Frames the live timeline keeps (four seconds at 60 Hz; the profiler panel's window).
pub const TIMELINE_FRAMES: usize = 240;

/// Events a Perfetto recording keeps before it starts counting drops (≈ 64 MB of slices).
pub const DEFAULT_PERFETTO_CAPACITY: usize = 2_000_000;

/// Which sinks are on: a bit set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Sinks(pub u32);

impl Sinks {
    pub const NONE: Sinks = Sinks(0);
    /// The live feed and budgets (the profiler panel).
    pub const COUNTERS: Sinks = Sinks(1);
    /// A Perfetto recording.
    pub const PERFETTO: Sinks = Sinks(2);
    /// Tracy (only with the `tracy` feature; otherwise enabling it is refused).
    pub const TRACY: Sinks = Sinks(4);
    pub const ALL: Sinks = Sinks(7);

    #[must_use]
    pub const fn contains(self, o: Sinks) -> bool {
        self.0 & o.0 == o.0 && o.0 != 0
    }
    #[must_use]
    pub const fn union(self, o: Sinks) -> Sinks {
        Sinks(self.0 | o.0)
    }
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Whether this build has the Tracy sink.
pub const TRACY_BUILT: bool = cfg!(feature = "tracy");

/// A small per-thread index (Perfetto thread tracks), assigned on a thread's first recorded
/// event.
fn thread_index() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    thread_local! {
        static TID: Cell<u32> = const { Cell::new(0) };
    }
    TID.with(|t| {
        let v = t.get();
        if v != 0 {
            return v;
        }
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        t.set(n);
        n
    })
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One zone name's total in the frame in progress.
#[derive(Clone, Copy, Default)]
struct PartAcc {
    /// ns spent since the last frame mark (summed over threads and calls).
    ns: u64,
    /// Zones closed since the last frame mark (0: the zone did not run this frame).
    calls: u32,
}

/// Zone totals of the frame in progress.
///
/// A zone name, once seen, keeps its entry: a frame mark zeroes it instead of removing it,
/// so a steady frame (the same zones every frame) inserts nothing and allocates nothing.
/// The names are `&'static str`, so the table is bounded by the zone names in the code.
#[derive(Default)]
struct FrameAccum {
    parts: BTreeMap<&'static str, PartAcc>,
    /// When the last frame mark happened (ns since the epoch); `None` before the first.
    last_mark: Option<u64>,
}

/// A frame on the live timeline as the tracer keeps it: part names are the zones' own
/// `&'static str`, so recording a frame copies no name; [`Tracer::snapshot`] turns it into
/// the public [`FrameSample`] when the panel reads it.
#[derive(Default)]
struct RawFrame {
    total_ms: f64,
    parts: Vec<(&'static str, f64)>,
}

/// The live feed's state: what [`Tracer::snapshot`] returns.
///
/// Counters are keyed by their `&'static str` name, so reporting a counter that exists
/// allocates nothing (WP-19); the timeline's oldest frame is recycled once it is full.
#[derive(Default)]
struct Live {
    frames: std::collections::VecDeque<RawFrame>,
    budgets: BTreeMap<String, Budget>,
    counters: BTreeMap<&'static str, f64>,
    lanes: BTreeMap<String, Lane>,
}

/// A Perfetto recording in progress.
pub(crate) struct Recording {
    pub(crate) slices: Vec<perfetto::Slice>,
    pub(crate) counters: Vec<perfetto::CounterSample>,
    pub(crate) instants: Vec<perfetto::Instant>,
    pub(crate) threads: BTreeMap<u32, String>,
    pub(crate) dropped: u64,
    pub(crate) capacity: usize,
}

impl Recording {
    const fn new() -> Self {
        Self {
            slices: Vec::new(),
            counters: Vec::new(),
            instants: Vec::new(),
            threads: BTreeMap::new(),
            dropped: 0,
            capacity: DEFAULT_PERFETTO_CAPACITY,
        }
    }
    fn len(&self) -> usize {
        self.slices.len() + self.counters.len() + self.instants.len()
    }
    fn room(&mut self) -> bool {
        if self.len() < self.capacity {
            true
        } else {
            self.dropped += 1;
            false
        }
    }
    fn thread(&mut self, tid: u32) {
        self.threads.entry(tid).or_insert_with(|| {
            std::thread::current()
                .name()
                .map_or_else(|| format!("thread {tid}"), str::to_owned)
        });
    }
}

/// The tracer: sinks, the live feed, budgets and the Perfetto recording (see the crate docs).
///
/// There is one process-wide tracer, [`global()`], which the [`zone!`] macro uses; a tracer
/// can also be created on its own (tests, a tool that profiles one run in isolation).
pub struct Tracer {
    sinks: AtomicU32,
    epoch: OnceLock<Instant>,
    frame: Mutex<FrameAccum>,
    live: Mutex<Live>,
    record: Mutex<Recording>,
    feed: FeedCell,
}

impl Default for Tracer {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Tracer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracer")
            .field("sinks", &self.sinks())
            .finish_non_exhaustive()
    }
}

static GLOBAL: Tracer = Tracer::new();

/// The process-wide tracer (what [`zone!`] and the engine's subsystems report to).
#[inline]
#[must_use]
pub fn global() -> &'static Tracer {
    &GLOBAL
}

/// Open a zone on the [`global()`] tracer: `let _z = forge_trace::zone!("sim.step");`. The
/// zone closes when the guard drops. The name must be a `&'static str`.
#[macro_export]
macro_rules! zone {
    ($name:expr) => {
        $crate::global().zone_at($name, file!(), line!())
    };
}

impl Tracer {
    /// A tracer with every sink off.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sinks: AtomicU32::new(0),
            epoch: OnceLock::new(),
            frame: Mutex::new(FrameAccum {
                parts: BTreeMap::new(),
                last_mark: None,
            }),
            live: Mutex::new(Live {
                frames: std::collections::VecDeque::new(),
                budgets: BTreeMap::new(),
                counters: BTreeMap::new(),
                lanes: BTreeMap::new(),
            }),
            record: Mutex::new(Recording::new()),
            feed: FeedCell::new(),
        }
    }

    /// The sinks currently on.
    #[inline]
    #[must_use]
    pub fn sinks(&self) -> Sinks {
        Sinks(self.sinks.load(Ordering::Relaxed))
    }

    /// Turn sinks on. Tracy in a build without the `tracy` feature is refused
    /// (`TRACE-0003`) rather than silently doing nothing.
    pub fn enable(&self, s: Sinks) -> Result<(), TraceError> {
        if s.contains(Sinks::TRACY) && !TRACY_BUILT {
            return Err(TraceError::TracyNotBuilt);
        }
        #[cfg(feature = "tracy")]
        if s.contains(Sinks::TRACY) {
            tracy::start();
        }
        let _ = self.now_ns(); // fix the epoch before the first event
        self.sinks.fetch_or(s.0, Ordering::Relaxed);
        Ok(())
    }

    /// Turn sinks off. Zones already open finish into the sinks they opened with.
    pub fn disable(&self, s: Sinks) {
        self.sinks.fetch_and(!s.0, Ordering::Relaxed);
    }

    /// Nanoseconds since this tracer's epoch (its first use).
    #[must_use]
    pub fn now_ns(&self) -> u64 {
        let e = self.epoch.get_or_init(Instant::now);
        u64::try_from(e.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// Open a zone named `name` (dotted: `subsystem.metric`). See [`zone!`].
    #[inline]
    pub fn zone(&self, name: &'static str) -> Zone<'_> {
        self.zone_at(name, "", 0)
    }

    /// [`Tracer::zone`] with a source location (Tracy shows it; the other sinks ignore it).
    #[inline]
    pub fn zone_at(&self, name: &'static str, file: &'static str, line: u32) -> Zone<'_> {
        let sinks = self.sinks.load(Ordering::Relaxed);
        if sinks == 0 {
            return Zone {
                tracer: self,
                name,
                sinks: 0,
                start: 0,
                #[cfg(feature = "tracy")]
                _tracy: None,
                _not_send: PhantomData,
            };
        }
        self.open(sinks, name, file, line)
    }

    #[cold]
    #[inline(never)]
    fn open(&self, sinks: u32, name: &'static str, file: &'static str, line: u32) -> Zone<'_> {
        let _ = (file, line);
        Zone {
            tracer: self,
            name,
            sinks,
            #[cfg(feature = "tracy")]
            _tracy: (sinks & Sinks::TRACY.0 != 0).then(|| tracy::span(name, file, line)),
            start: self.now_ns(),
            _not_send: PhantomData,
        }
    }

    #[cold]
    #[inline(never)]
    fn close(&self, sinks: u32, name: &'static str, start: u64) {
        let end = self.now_ns().max(start);
        if sinks & Sinks::COUNTERS.0 != 0 {
            let mut f = lock(&self.frame);
            let acc = f.parts.entry(name).or_default();
            acc.ns += end - start;
            acc.calls = acc.calls.saturating_add(1);
        }
        if sinks & Sinks::PERFETTO.0 != 0 {
            let tid = thread_index();
            let mut r = lock(&self.record);
            if r.room() {
                r.thread(tid);
                r.slices.push(perfetto::Slice {
                    tid,
                    name,
                    start,
                    end,
                });
            }
        }
    }

    /// Report a counter (a count, bytes, a millisecond figure measured elsewhere such as a
    /// GPU timestamp). A budget with the same name is measured by it.
    #[inline]
    pub fn counter(&self, name: &'static str, value: f64) {
        let sinks = self.sinks.load(Ordering::Relaxed);
        if sinks != 0 {
            self.counter_slow(sinks, name, value);
        }
    }

    #[cold]
    #[inline(never)]
    fn counter_slow(&self, sinks: u32, name: &'static str, value: f64) {
        if sinks & Sinks::COUNTERS.0 != 0 {
            let mut l = lock(&self.live);
            // An existing key is updated in place: no allocation (the steady case).
            match l.counters.get_mut(name) {
                Some(v) => *v = value,
                None => {
                    l.counters.insert(name, value);
                }
            }
            if let Some(b) = l.budgets.get_mut(name) {
                b.observe(value);
            }
        }
        if sinks & Sinks::PERFETTO.0 != 0 {
            let ts = self.now_ns();
            let mut r = lock(&self.record);
            if r.room() {
                r.counters.push(perfetto::CounterSample { name, ts, value });
            }
        }
        #[cfg(feature = "tracy")]
        if sinks & Sinks::TRACY.0 != 0 {
            tracy::plot(name, value);
        }
    }

    /// An instant event (a named moment: "play", "stop", "hitch") on this thread's track.
    pub fn instant(&self, name: &str) {
        let sinks = self.sinks.load(Ordering::Relaxed);
        if sinks & Sinks::PERFETTO.0 != 0 {
            self.instant_at(name, self.now_ns());
        }
        #[cfg(feature = "tracy")]
        if sinks & Sinks::TRACY.0 != 0 {
            tracy::message(name);
        }
    }

    fn instant_at(&self, name: &str, ts: u64) {
        let tid = thread_index();
        let mut r = lock(&self.record);
        if r.room() {
            r.thread(tid);
            r.instants.push(perfetto::Instant {
                tid,
                name: name.to_owned(),
                ts,
            });
        }
    }

    /// End a frame: the zone totals since the last mark become one [`FrameSample`] on the
    /// live timeline (its parts, ms), budgets named by a part are measured, a budget over its
    /// allowance counts an over-budget frame (and marks the Perfetto trace), and the feed is
    /// bumped. Call it once per frame (the editor loop) or per simulation step batch.
    pub fn frame_mark(&self) {
        let sinks = self.sinks.load(Ordering::Relaxed);
        if sinks == 0 {
            return;
        }
        let now = self.now_ns();
        if sinks & Sinks::COUNTERS.0 != 0 {
            // The sample to fill: the timeline's oldest frame once it is full (its parts'
            // vector is reused), so a steady frame allocates nothing (WP-19, measured by
            // `tests/test_trace_alloc.rs`).
            let mut sample = {
                let mut l = lock(&self.live);
                if l.frames.len() >= TIMELINE_FRAMES {
                    l.frames.pop_front().unwrap_or_default()
                } else {
                    RawFrame::default()
                }
            };
            sample.parts.clear();
            let since = {
                let mut f = lock(&self.frame);
                for (name, acc) in &mut f.parts {
                    if acc.calls > 0 {
                        sample.parts.push((*name, ns_to_ms(acc.ns)));
                        *acc = PartAcc::default();
                    }
                }
                f.last_mark.replace(now)
            };
            sample.total_ms = since.map_or(0.0, |s| ns_to_ms(now.saturating_sub(s)));
            let perfetto = sinks & Sinks::PERFETTO.0 != 0;
            // Filled only when a Perfetto recording marks the overruns.
            let mut over: Vec<&'static str> = Vec::new();
            {
                let mut l = lock(&self.live);
                for &(n, ms) in &sample.parts {
                    if let Some(b) = l.budgets.get_mut(n)
                        && b.observe(ms)
                        && perfetto
                    {
                        over.push(n);
                    }
                }
                l.frames.push_back(sample);
                while l.frames.len() > TIMELINE_FRAMES {
                    l.frames.pop_front();
                }
            }
            for n in over {
                self.instant_at(&format!("over budget: {n}"), now);
            }
            self.feed.bump();
        }
        if sinks & Sinks::PERFETTO.0 != 0 {
            self.instant_at("frame", now);
        }
        #[cfg(feature = "tracy")]
        if sinks & Sinks::TRACY.0 != 0 {
            tracy::frame_mark();
        }
    }

    // ---- budgets --------------------------------------------------------------------------

    /// Declare (or re-declare) a named budget: its allowance and unit (`"ms"`, or `""` for a
    /// count). Its measured value is kept across a re-declaration.
    pub fn declare_budget(&self, name: &str, allowance: f64, unit: &'static str) {
        let mut l = lock(&self.live);
        l.budgets
            .entry(name.to_owned())
            .and_modify(|b| {
                b.budget = allowance;
                b.unit = unit;
            })
            .or_insert_with(|| Budget::new(name, allowance, unit));
    }

    /// Declare the budgets of a `tests/perf/budgets.ron` file (the M1 perf gate's) for the
    /// device class `class` (`rtx3080`, `warp`): GPU rows get `max(base x (1 + gpu_band),
    /// base + slack[class])`, counters their `max`. CPU-ratio rows are relative to an
    /// in-process calibration and have no absolute allowance, so they are skipped. Returns
    /// how many budgets were declared.
    pub fn declare_budgets_ron(&self, text: &str, class: &str) -> Result<usize, TraceError> {
        let rows = budget::parse_budgets_ron(text, class)?;
        let n = rows.len();
        for (name, allowance, unit) in rows {
            self.declare_budget(&name, allowance, unit);
        }
        Ok(n)
    }

    /// Every budget, grouped by subsystem (the name's first segment).
    #[must_use]
    pub fn budgets_by_subsystem(&self) -> BTreeMap<String, Vec<Budget>> {
        let l = lock(&self.live);
        let mut out: BTreeMap<String, Vec<Budget>> = BTreeMap::new();
        for b in l.budgets.values() {
            out.entry(b.subsystem().to_owned())
                .or_default()
                .push(b.clone());
        }
        out
    }

    // ---- the live feed ----------------------------------------------------------------------

    /// The feed the profiler panel follows: bumped on every frame mark with counters on.
    #[must_use]
    pub fn feed(&self) -> &FeedCell {
        &self.feed
    }

    /// Mark the producer running (a simulation playing, the engine rendering) or stopped.
    pub fn set_live(&self, live: bool) {
        self.feed.set_live(live);
    }

    /// Replace one adapter's lane (Ch.26): spans within the last frame, ms from its start.
    pub fn set_lane(&self, adapter: &str, spans: Vec<(f64, f64, String)>) {
        lock(&self.live).lanes.insert(
            adapter.to_owned(),
            Lane {
                adapter: adapter.to_owned(),
                spans,
            },
        );
        self.feed.bump();
    }

    /// Everything the profiler shows, now.
    #[must_use]
    pub fn snapshot(&self) -> TraceSnapshot {
        let l = lock(&self.live);
        TraceSnapshot {
            frames: l
                .frames
                .iter()
                .map(|f| FrameSample {
                    total_ms: f.total_ms,
                    parts: f
                        .parts
                        .iter()
                        .map(|(n, ms)| ((*n).to_owned(), *ms))
                        .collect(),
                })
                .collect(),
            budgets: l.budgets.values().cloned().collect(),
            counters: l
                .counters
                .iter()
                .map(|(k, v)| ((*k).to_owned(), *v))
                .collect(),
            lanes: l.lanes.values().cloned().collect(),
            dropped_events: lock(&self.record).dropped,
        }
    }

    /// Forget the timeline, counters and measured values (budgets stay declared).
    pub fn clear_live(&self) {
        {
            let mut l = lock(&self.live);
            l.frames.clear();
            l.counters.clear();
            for b in l.budgets.values_mut() {
                b.reset();
            }
        }
        let mut f = lock(&self.frame);
        for acc in f.parts.values_mut() {
            *acc = PartAcc::default();
        }
        f.last_mark = None;
        drop(f);
        self.feed.bump();
    }

    // ---- Perfetto -------------------------------------------------------------------------

    /// Cap the Perfetto recording at `events` (the rest are counted as dropped).
    pub fn set_perfetto_capacity(&self, events: usize) {
        lock(&self.record).capacity = events;
    }

    /// Events recorded for Perfetto so far, and how many were dropped at the cap.
    #[must_use]
    pub fn perfetto_len(&self) -> (usize, u64) {
        let r = lock(&self.record);
        (r.len(), r.dropped)
    }

    /// Take the Perfetto recording so far (it restarts empty) and encode it as a Perfetto
    /// protobuf trace (`.pftrace`) for `process_name`.
    #[must_use]
    pub fn take_perfetto(&self, process_name: &str) -> Vec<u8> {
        let rec = {
            let mut r = lock(&self.record);
            let cap = r.capacity;
            let mut fresh = Recording::new();
            fresh.capacity = cap;
            std::mem::replace(&mut *r, fresh)
        };
        perfetto::encode(&rec, process_name, std::process::id())
    }

    /// [`Tracer::take_perfetto`] into a file.
    pub fn write_perfetto(
        &self,
        path: &std::path::Path,
        process_name: &str,
    ) -> Result<usize, TraceError> {
        let bytes = self.take_perfetto(process_name);
        std::fs::write(path, &bytes).map_err(|e| TraceError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        Ok(bytes.len())
    }
}

fn ns_to_ms(ns: u64) -> f64 {
    ns as f64 / 1.0e6
}

/// An open zone; it closes when dropped (see [`Tracer::zone`]). Not `Send`: a zone opens
/// and closes on one thread, which is what makes per-thread slices nest.
#[must_use = "a zone measures until it is dropped; bind it: `let _z = zone!(..)`"]
pub struct Zone<'t> {
    tracer: &'t Tracer,
    name: &'static str,
    /// The sinks captured at open (0: inert).
    sinks: u32,
    start: u64,
    #[cfg(feature = "tracy")]
    _tracy: Option<tracy_client::Span>,
    _not_send: PhantomData<*const ()>,
}

impl Zone<'_> {
    /// The zone's name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }
    /// Whether the zone is recording (some sink was on when it opened).
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.sinks != 0
    }
}

impl Drop for Zone<'_> {
    #[inline]
    fn drop(&mut self) {
        if self.sinks != 0 {
            self.tracer.close(self.sinks, self.name, self.start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_zones_record_nothing() {
        let t = Tracer::new();
        {
            let z = t.zone("sim.step");
            assert!(!z.is_recording());
        }
        t.counter("sim.bodies", 3.0);
        t.frame_mark();
        let s = t.snapshot();
        assert!(s.frames.is_empty() && s.counters.is_empty());
        assert_eq!(t.perfetto_len(), (0, 0));
    }

    #[test]
    fn frame_mark_turns_zone_totals_into_parts_and_budgets() {
        let t = Tracer::new();
        t.enable(Sinks::COUNTERS).unwrap();
        t.declare_budget("sim.step", 0.000_001, "ms");
        t.declare_budget("sim.bodies", 10.0, "");
        t.frame_mark(); // the first mark only starts the clock
        {
            let _a = t.zone("sim.step");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        {
            let _b = t.zone("sim.step");
        }
        t.counter("sim.bodies", 12.0);
        let g = t.feed().generation();
        t.frame_mark();
        assert!(t.feed().generation() > g);
        let s = t.snapshot();
        assert_eq!(s.frames.len(), 2);
        let f = &s.frames[1];
        assert!(f.total_ms >= 2.0, "{f:?}");
        assert_eq!(f.parts.len(), 1);
        assert_eq!(f.parts[0].0, "sim.step");
        assert!(f.parts[0].1 >= 2.0);
        let by = t.budgets_by_subsystem();
        let sim = &by["sim"];
        assert_eq!(sim.len(), 2);
        assert!(sim.iter().all(Budget::over), "{sim:?}");
        assert_eq!(s.counters, vec![("sim.bodies".to_string(), 12.0)]);
    }

    #[test]
    fn the_timeline_is_bounded() {
        let t = Tracer::new();
        t.enable(Sinks::COUNTERS).unwrap();
        for _ in 0..(TIMELINE_FRAMES + 50) {
            t.frame_mark();
        }
        assert_eq!(t.snapshot().frames.len(), TIMELINE_FRAMES);
    }

    #[test]
    fn the_perfetto_recording_is_bounded_and_counts_drops() {
        let t = Tracer::new();
        t.set_perfetto_capacity(10);
        t.enable(Sinks::PERFETTO).unwrap();
        for _ in 0..25 {
            let _z = t.zone("x.y");
        }
        assert_eq!(t.perfetto_len(), (10, 15));
        let bytes = t.take_perfetto("t");
        assert!(!bytes.is_empty());
        assert_eq!(t.perfetto_len(), (0, 0));
    }

    #[test]
    fn a_zone_keeps_the_sinks_it_opened_with() {
        let t = Tracer::new();
        let z = t.zone("a.b");
        t.enable(Sinks::PERFETTO).unwrap();
        drop(z);
        assert_eq!(t.perfetto_len().0, 0, "a zone opened while off stays inert");
        let z = t.zone("a.b");
        t.disable(Sinks::PERFETTO);
        drop(z);
        assert_eq!(t.perfetto_len().0, 1, "a zone opened while on finishes");
    }

    #[test]
    fn tracy_is_refused_without_the_feature() {
        let t = Tracer::new();
        let r = t.enable(Sinks::TRACY);
        assert_eq!(r.is_err(), !TRACY_BUILT);
    }

    #[test]
    fn every_trace_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in TraceError::all_variants_for_tests() {
            let code = e.code();
            assert!(
                md.contains(&format!("| {code} | forge-trace |")),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code));
        }
    }
}
