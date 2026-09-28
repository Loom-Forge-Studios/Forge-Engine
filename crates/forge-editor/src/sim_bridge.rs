//! The editor's side of the play core and of `forge-trace` (WP-13): the real backends behind
//! WP-U6's two traits.
//!
//! * [`SimPlay`] — [`crate::play::PlayBackend`] over `forge_sim::PlaySession`. The shell runs
//!   every `forge.play.control` session command on it, forking from the mirror through
//!   [`edit_snapshot`]; `forge --headless` runs the same commands on the same type, forking
//!   from the core's project ([`SimPlay::control_edit`] with
//!   `forge_sim::EditSnapshot::from_project`). The two edit worlds must be equal for the same
//!   project — `test_headless_parity` checks exactly that, by hash.
//! * [`TraceCounters`] — [`crate::profile::CounterSource`] over a `forge_trace::Tracer`: the
//!   tracer's live feed as a `forge_ui::LiveSource` (woken only while the profiler is
//!   visible, at most once per bump until consumed), and its [`TraceSnapshot`] mapped to the
//!   panel's [`ProfileSnapshot`] by [`profile_snapshot`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use forge_cmd::EntityKey;
use forge_frames::Tick;
use forge_sim::{
    EditEntity, EditSnapshot, PlayCommand, PlayLogEntry, PlaySession, PlayState, SimError,
    SimInput, SimTransform,
};
use forge_trace::{TraceSnapshot, Tracer};
use forge_ui::{LiveCell, LiveSource, UiWaker};

use crate::mirror::ProjectMirror;
use crate::play::PlayBackend;
use crate::profile::{Budget, CounterSource, FrameSample, Lane, ProfileSnapshot};

forge_trace::control_switches! {
    /// Test-only fault switches for the bridge (W2 positive control of `test_headless_parity`).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct BridgeFaults {
        /// Fork from what the viewport draws: leave out entities hidden in the hierarchy
        /// (`editor.hidden`). A hidden body still exists and still simulates; a play session
        /// that dropped it would differ from the headless one.
        pub skip_hidden: bool,
    }
}

/// The edit world from the mirror (see the module docs).
#[must_use]
pub fn edit_snapshot(mirror: &ProjectMirror) -> EditSnapshot {
    edit_snapshot_with(mirror, BridgeFaults::default())
}

/// [`edit_snapshot`] with fault switches (tests only).
#[must_use]
pub fn edit_snapshot_with(mirror: &ProjectMirror, faults: BridgeFaults) -> EditSnapshot {
    EditSnapshot::from_entities(
        mirror
            .entities()
            .filter(|(_, e)| {
                !(faults.skip_hidden()
                    && matches!(
                        e.properties.get("editor.hidden"),
                        Some(forge_cmd::Value::Bool(true))
                    ))
            })
            .map(|(k, e)| EditEntity {
                key: *k,
                name: e.name.clone(),
                parent: e.parent,
                properties: e.properties.clone(),
            }),
    )
}

// ---- the play core behind PlayBackend ----------------------------------------------------

/// The editor's play backend: WP-13's play core (see the module docs).
pub struct SimPlay {
    session: PlaySession,
    faults: BridgeFaults,
    /// The play feed (`PlayBackend::feed`): bumped when the session's revision moves, live
    /// while playing.
    feed: Arc<LiveCell>,
    seen_revision: u64,
    error: Option<String>,
}

impl Default for SimPlay {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SimPlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimPlay")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl SimPlay {
    /// A stopped play core reporting to the process-wide tracer.
    #[must_use]
    pub fn new() -> Self {
        Self::with_tracer(forge_trace::global())
    }

    /// A stopped play core reporting to `tracer`.
    #[must_use]
    pub fn with_tracer(tracer: &'static Tracer) -> Self {
        Self {
            session: PlaySession::with_tracer(tracer),
            faults: BridgeFaults::default(),
            feed: LiveCell::new(),
            seen_revision: 0,
            error: None,
        }
    }

    /// Test-only fault switches (`test_headless_parity`'s positive controls).
    #[cfg(any(test, feature = "controls"))]
    #[must_use]
    pub fn with_faults(mut self, sim: forge_sim::SimFaults, bridge: BridgeFaults) -> Self {
        self.session.set_faults(sim);
        self.faults = bridge;
        self
    }

    /// The play session (its recording, state hash, world).
    #[must_use]
    pub fn session(&self) -> &PlaySession {
        &self.session
    }

    /// The play session, for settings that are not controls (checkpoint cadence, taking
    /// the recording).
    pub fn session_mut(&mut self) -> &mut PlaySession {
        &mut self.session
    }

    /// Apply a control against an explicit edit world (headless forks from the core's
    /// project). The shell's route is [`PlayBackend::control`], which forks from the mirror.
    pub fn control_edit(&mut self, cmd: PlayCommand, edit: &EditSnapshot) -> Result<(), SimError> {
        let before = self.session.step();
        let r = self.session.control(cmd, edit);
        if matches!(cmd, PlayCommand::Step(_)) && self.session.step() != before {
            // A Step's steps are one profiler frame, like a batch `advance` ran.
            self.session.tracer().frame_mark();
        }
        self.sync();
        r
    }

    /// Queue a simulation input for the next step boundary (recorded with it). Clients send
    /// inputs as the `forge.play.input` session command (`crate::play::input_command`);
    /// the shell and `forge --headless` hand each one the bus delivers to
    /// [`PlayBackend::input`], which lands here.
    pub fn input(&mut self, i: SimInput) -> Result<(), SimError> {
        self.session.input(i)
    }

    /// [`PlayBackend::advance`] with its error: run a playing simulation up to `now` in fixed
    /// steps, closing a trace frame when any step ran.
    pub fn try_advance(&mut self, now: Duration) -> Result<u32, SimError> {
        let r = self.session.advance(now);
        if matches!(r, Ok(n) if n > 0) {
            self.session.tracer().frame_mark();
        }
        self.sync();
        r
    }

    /// Follow the session: bump the play feed when it changed, live only while playing.
    fn sync(&mut self) {
        let rev = self.session.revision();
        if rev != self.seen_revision {
            self.seen_revision = rev;
            self.feed.bump();
        }
        self.feed
            .set_live(self.session.state() == PlayState::Playing);
    }
}

impl PlayBackend for SimPlay {
    fn backend(&self) -> &str {
        PlaySession::BACKEND
    }

    fn state(&self) -> PlayState {
        self.session.state()
    }

    fn control(&mut self, cmd: PlayCommand, edit: &ProjectMirror) {
        // Only a stopped session forks; a running one is never handed the edit world, so the
        // snapshot (a copy of every entity's properties) is taken only when it is used.
        let snapshot = if self.session.state() == PlayState::Stopped {
            edit_snapshot_with(edit, self.faults)
        } else {
            EditSnapshot::default()
        };
        if let Err(e) = self.control_edit(cmd, &snapshot) {
            self.error = Some(e.to_string());
        }
    }

    fn advance(&mut self, now: Duration) -> u32 {
        match self.try_advance(now) {
            Ok(n) => n,
            Err(e) => {
                self.error = Some(e.to_string());
                0
            }
        }
    }

    fn tick(&self) -> Tick {
        self.session.tick()
    }

    fn transforms(&self) -> &BTreeMap<EntityKey, SimTransform> {
        self.session.transforms()
    }

    fn revision(&self) -> u64 {
        self.session.revision()
    }

    fn log(&self) -> &[PlayLogEntry] {
        self.session.log()
    }

    fn feed(&self) -> Arc<LiveCell> {
        Arc::clone(&self.feed)
    }

    fn last_step_ms(&self) -> f64 {
        self.session.last_step_ms()
    }

    fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    fn input(&mut self, i: SimInput) -> Result<(), String> {
        let r = self.session.input(i).map_err(|e| e.to_string());
        self.sync();
        r
    }
}

// ---- the tracer behind CounterSource -----------------------------------------------------

/// What the profiler says about replication while there is no replication source.
pub const NO_REPLICATION: &str =
    "No replication: forge-net (M4-7) is not built, so there is no session to measure.";

/// Map the tracer's snapshot to the profiler panel's.
///
/// * Frames: the tracer's timeline, oldest first (at most `forge_trace::TIMELINE_FRAMES`);
///   each keeps its time between frame marks (`total_ms`) and its parts, one per zone name
///   that closed during that frame (summed over calls and threads, ms, in name order) — a
///   zone that did not run that frame is not a part of it.
/// * Lanes: per adapter, the spans of its last frame (ms from the frame's start) as the
///   tracer holds them.
/// * A budget keeps its name, latest value, allowance and unit; the tracer's extra
///   statistics (`peak`, `samples`, `over_frames`) are for reports (`forge --headless
///   --trace` prints them) and are not shown by the panel.
/// * The tracer's counters and its dropped-event count go to the snapshot's `counters` and
///   `dropped_events`.
/// * Replication: the tracer has none (forge-net, M4-7, is not built), so `replication` is
///   `None` and `replication_note` says why.
#[must_use]
pub fn profile_snapshot(t: TraceSnapshot) -> ProfileSnapshot {
    ProfileSnapshot {
        frames: t
            .frames
            .into_iter()
            .map(|f| FrameSample {
                total_ms: f.total_ms,
                parts: f.parts,
            })
            .collect(),
        budgets: t
            .budgets
            .into_iter()
            .map(|b| Budget {
                name: b.name,
                value: b.value,
                budget: b.budget,
                unit: b.unit,
            })
            .collect(),
        lanes: t
            .lanes
            .into_iter()
            .map(|l| Lane {
                adapter: l.adapter,
                spans: l.spans,
            })
            .collect(),
        replication: None,
        replication_note: NO_REPLICATION.to_owned(),
        counters: t.counters,
        dropped_events: t.dropped_events,
    }
}

/// The tracer's live feed as the profiler's counter source (see the module docs).
#[derive(Clone, Copy, Debug)]
pub struct TraceCounters {
    tracer: &'static Tracer,
}

impl Default for TraceCounters {
    fn default() -> Self {
        Self::new()
    }
}

impl TraceCounters {
    /// What this source is (the profiler panel shows it).
    pub const BACKEND: &'static str = "forge-trace (live counters, named budgets)";

    /// The process-wide tracer's feed.
    #[must_use]
    pub fn new() -> Self {
        Self::of(forge_trace::global())
    }

    /// A given tracer's feed.
    #[must_use]
    pub fn of(tracer: &'static Tracer) -> Self {
        Self { tracer }
    }

    /// The tracer's own snapshot (with the statistics the panel does not show).
    #[must_use]
    pub fn trace_snapshot(&self) -> TraceSnapshot {
        self.tracer.snapshot()
    }

    #[must_use]
    pub fn tracer(&self) -> &'static Tracer {
        self.tracer
    }
}

impl LiveSource for TraceCounters {
    fn generation(&self) -> u64 {
        self.tracer.feed().generation()
    }
    fn is_live(&self) -> bool {
        self.tracer.feed().is_live()
    }
    fn set_waker(&self, waker: Option<Arc<dyn UiWaker>>) {
        self.tracer.feed().set_waker(waker.map(|w| {
            let f: forge_trace::feed::Waker = Arc::new(move || w.wake());
            f
        }));
    }
    fn consumed(&self) {
        self.tracer.feed().consumed();
    }
}

impl CounterSource for TraceCounters {
    fn backend(&self) -> String {
        Self::BACKEND.to_owned()
    }
    fn snapshot(&self) -> ProfileSnapshot {
        profile_snapshot(self.tracer.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Count(AtomicUsize);
    impl UiWaker for Count {
        fn wake(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn leaked() -> &'static Tracer {
        Box::leak(Box::new(Tracer::new()))
    }

    #[test]
    fn the_trace_feed_wakes_a_visible_panel_once_per_consumed_bump() {
        let t = leaked();
        t.enable(forge_trace::Sinks::COUNTERS)
            .unwrap_or_else(|e| panic!("{e}"));
        let src = TraceCounters::of(t);
        let n = Arc::new(Count(AtomicUsize::new(0)));
        t.frame_mark(); // hidden panel: no waker, nothing woken
        src.set_waker(Some(n.clone()));
        let g = src.generation();
        t.frame_mark();
        t.frame_mark();
        assert_eq!(n.0.load(Ordering::Relaxed), 1);
        assert!(src.generation() >= g + 2);
        src.consumed();
        t.frame_mark();
        assert_eq!(n.0.load(Ordering::Relaxed), 2);
        src.set_waker(None);
        t.frame_mark();
        assert_eq!(n.0.load(Ordering::Relaxed), 2);
        assert_eq!(src.snapshot().frames.len(), 5);
        assert!(CounterSource::backend(&src).contains("forge-trace"));
    }

    #[test]
    fn the_profile_snapshot_maps_every_field_of_the_trace_snapshot() {
        let t = leaked();
        t.enable(forge_trace::Sinks::COUNTERS)
            .unwrap_or_else(|e| panic!("{e}"));
        t.declare_budget("sim.step", 1.0, "ms");
        t.declare_budget("render.draws", 10.0, "");
        {
            let _z = t.zone("sim.step");
        }
        t.counter("render.draws", 12.0);
        t.counter("sim.bodies", 3.0);
        t.set_lane("RTX 3080", vec![(0.0, 1.5, "shadows".into())]);
        t.frame_mark();
        let raw = TraceCounters::of(t).trace_snapshot();
        let p = TraceCounters::of(t).snapshot();
        assert_eq!(p.frames.len(), raw.frames.len());
        assert_eq!(p.frames[0].parts, raw.frames[0].parts);
        assert_eq!(p.frames[0].total_ms, raw.frames[0].total_ms);
        let names: Vec<_> = p.budgets.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["render.draws", "sim.step"]);
        let draws = &p.budgets[0];
        assert_eq!((draws.value, draws.budget, draws.unit), (12.0, 10.0, ""));
        assert!(draws.over(), "12 of 10 draws is over");
        assert_eq!(p.budgets[1].unit, "ms");
        assert_eq!(p.lanes.len(), 1);
        assert_eq!(p.lanes[0].adapter, "RTX 3080");
        assert_eq!(p.lanes[0].spans, raw.lanes[0].spans);
        assert_eq!(p.counters, raw.counters);
        assert!(p.counters.contains(&("sim.bodies".to_owned(), 3.0)));
        assert_eq!(p.dropped_events, raw.dropped_events);
        assert_eq!(p.replication, None);
        assert!(p.replication_note.contains("forge-net"));
    }

    #[test]
    fn sim_play_runs_the_play_core_behind_play_backend() {
        let mut p = SimPlay::with_tracer(leaked());
        let m = ProjectMirror::new();
        let f = p.feed();
        let g = f.generation();
        PlayBackend::control(&mut p, PlayCommand::Step(3), &m);
        assert_eq!(p.state(), PlayState::Paused);
        assert_eq!(p.tick(), forge_sim::tick_of_step(3));
        assert!(f.generation() > g, "a step bumps the play feed");
        assert!(!f.is_live());
        PlayBackend::control(&mut p, PlayCommand::Play, &m);
        assert!(f.is_live());
        let t0 = Duration::from_secs(10);
        assert_eq!(PlayBackend::advance(&mut p, t0), 0);
        assert_eq!(
            PlayBackend::advance(&mut p, t0 + Duration::from_millis(100)),
            6
        );
        PlayBackend::control(&mut p, PlayCommand::Stop, &m);
        assert_eq!(p.state(), PlayState::Stopped);
        assert_eq!(
            p.log().iter().map(|e| e.command).collect::<Vec<_>>(),
            vec![PlayCommand::Step(3), PlayCommand::Play, PlayCommand::Stop]
        );
        assert!(p.session().recording().is_some_and(|r| r.is_complete()));
        assert_eq!(p.take_error(), None);
        assert!(p.backend().contains("forge-sim"));
    }
}
