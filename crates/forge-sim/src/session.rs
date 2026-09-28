//! The play session: play controls, the fixed-step clock, inputs and the recorder — the play
//! core the editor runs behind `forge_editor::play::PlayBackend` (`sim_bridge::SimPlay`) and
//! that `forge --headless` runs with no UI. [`PlayCommand`], [`PlayState`] and
//! [`PlayLogEntry`] are the editor's play types too (`forge_editor::play` re-exports them).
//!
//! **Play runs against a fork.** Play (or Step from stopped) forks a [`SimWorld`] from the
//! edit world it is handed — read-only: the session has no way to change the project — and
//! Stop throws it away. Controls follow the editor's toolbar: Play, Pause, Step(n) (from
//! paused; from playing: pause, then step; from stopped: fork paused, then step), Stop;
//! anything else is a no-op and is not recorded.
//!
//! **Time.** [`PlaySession::advance`] turns the editor's clock into a number of fixed steps
//! (at most [`MAX_CATCH_UP`] per call: a stalled editor drops time rather than spiral). The
//! clock decides *how many* steps run, never *what a step computes* (I-20); the recording
//! stamps every event with its step, so a replay reproduces the session exactly whatever the
//! frame rate was.

use std::collections::BTreeMap;
use std::time::Duration;

use forge_cmd::EntityKey;
use forge_frames::{TICKS_PER_SECOND, Tick};
use forge_trace::Tracer;
use serde::{Deserialize, Serialize};

use crate::SimError;
use crate::edit::EditSnapshot;
use crate::record::{RecEvent, Recording};
use crate::world::{SIM_DT, SIM_RATE_HZ, SimFaults, SimInput, SimTransform, SimWorld};

/// Most steps one [`PlaySession::advance`] runs (a quarter second at 60 Hz).
pub const MAX_CATCH_UP: u32 = 15;
/// Default checkpoint cadence: one state hash per simulated second.
pub const DEFAULT_CHECK_EVERY: u64 = SIM_RATE_HZ as u64;
/// The `sim.step` budget: simulation time per frame (ms), a quarter of a 60 Hz frame.
pub const SIM_STEP_BUDGET_MS: f64 = 4.0;

/// Where a play session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PlayState {
    /// Editing: no simulation world exists.
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// A play control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayCommand {
    /// Fork the simulation from the edit world and run it (or resume a paused one).
    Play,
    Pause,
    /// Advance the simulation by `n` fixed steps and leave it paused: a playing one is paused
    /// first, a stopped one is forked first.
    Step(u32),
    /// Discard the simulation world.
    Stop,
}

/// One entry of a session's control log: a control and the simulation tick it took effect at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayLogEntry {
    pub tick: Tick,
    pub command: PlayCommand,
}

/// The canonical tick of step `n` at [`SIM_RATE_HZ`]: `floor(n * 10^6 / rate)` (Ch.2.5).
#[must_use]
pub fn tick_of_step(n: u64) -> Tick {
    let t = u128::from(n) * TICKS_PER_SECOND as u128 / u128::from(SIM_RATE_HZ);
    Tick(i64::try_from(t).unwrap_or(i64::MAX))
}

/// The step whose tick is `t` (the inverse of [`tick_of_step`]: `ceil(t * rate / 10^6)`);
/// a negative tick is step 0.
#[must_use]
pub fn step_of_tick(t: Tick) -> u64 {
    let t = u128::try_from(t.0).unwrap_or(0);
    let r = u128::from(SIM_RATE_HZ);
    let s = (t * r).div_ceil(TICKS_PER_SECOND as u128);
    u64::try_from(s).unwrap_or(u64::MAX)
}

/// The play core (see the module docs).
pub struct PlaySession {
    state: PlayState,
    sim: Option<SimWorld>,
    pending: Vec<SimInput>,
    log: Vec<PlayLogEntry>,
    recording: Option<Recording>,
    embed_edit: bool,
    check_every: u64,
    /// Editor clock at which the current run of steps started, and steps run since.
    clock_origin: Option<Duration>,
    steps_since_origin: u64,
    last_now: Option<Duration>,
    revision: u64,
    moving: BTreeMap<EntityKey, SimTransform>,
    tracer: &'static Tracer,
    last_step_ms: f64,
    faults: SimFaults,
}

impl Default for PlaySession {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for PlaySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaySession")
            .field("state", &self.state)
            .field("step", &self.step())
            .finish_non_exhaustive()
    }
}

impl PlaySession {
    /// What this backend is (the play controls show it).
    pub const BACKEND: &'static str =
        "forge-sim play core: forked simulation world, fixed 60 Hz steps, recorded for replay";

    /// A stopped session reporting to the process-wide tracer.
    #[must_use]
    pub fn new() -> Self {
        Self::with_tracer(forge_trace::global())
    }

    /// A stopped session reporting to `tracer` (declares the `sim.step` budget on it).
    #[must_use]
    pub fn with_tracer(tracer: &'static Tracer) -> Self {
        tracer.declare_budget("sim.step", SIM_STEP_BUDGET_MS, "ms");
        Self {
            state: PlayState::Stopped,
            sim: None,
            pending: Vec::new(),
            log: Vec::new(),
            recording: None,
            embed_edit: true,
            check_every: DEFAULT_CHECK_EVERY,
            clock_origin: None,
            steps_since_origin: 0,
            last_now: None,
            revision: 0,
            moving: BTreeMap::new(),
            tracer,
            last_step_ms: 0.0,
            faults: SimFaults::default(),
        }
    }

    /// Test-only fault switches (see [`SimFaults`]).
    #[cfg(any(test, feature = "controls"))]
    #[must_use]
    pub fn with_faults(mut self, faults: SimFaults) -> Self {
        self.faults = faults;
        self
    }

    /// Test-only fault switches, on a session already built.
    #[cfg(any(test, feature = "controls"))]
    pub fn set_faults(&mut self, faults: SimFaults) {
        self.faults = faults;
    }

    /// Record a checkpoint hash every `n` steps (from the next Play on; `0` is taken as 1).
    pub fn set_check_every(&mut self, n: u64) {
        self.check_every = n.max(1);
    }

    /// Embed the edit world in recordings (default) or only its hash.
    pub fn set_embed_edit(&mut self, embed: bool) {
        self.embed_edit = embed;
    }

    #[must_use]
    pub fn state(&self) -> PlayState {
        self.state
    }

    /// A simulation world exists (playing or paused).
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.sim.is_some()
    }

    /// Steps since the fork (0 when stopped).
    #[must_use]
    pub fn step(&self) -> u64 {
        self.sim.as_ref().map_or(0, SimWorld::step_count)
    }

    /// The simulation's canonical tick (0 when stopped).
    #[must_use]
    pub fn tick(&self) -> Tick {
        tick_of_step(self.step())
    }

    /// The simulation world, while one exists.
    #[must_use]
    pub fn world(&self) -> Option<&SimWorld> {
        self.sim.as_ref()
    }

    /// The state hash, while a simulation exists.
    #[must_use]
    pub fn state_hash(&self) -> Option<String> {
        self.sim.as_ref().map(SimWorld::state_hash)
    }

    /// Simulated transforms of the bodies that can move (updated after every batch of steps).
    #[must_use]
    pub fn transforms(&self) -> &BTreeMap<EntityKey, SimTransform> {
        &self.moving
    }

    /// Bumped whenever the simulation changed (state, transforms).
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The session's controls so far (cleared by the next Play from stopped).
    #[must_use]
    pub fn log(&self) -> &[PlayLogEntry] {
        &self.log
    }

    /// The current session's recording (or the last one's, after Stop).
    #[must_use]
    pub fn recording(&self) -> Option<&Recording> {
        self.recording.as_ref()
    }

    /// Take the recording out of the session.
    pub fn take_recording(&mut self) -> Option<Recording> {
        self.recording.take()
    }

    /// The tracer the session reports to (the profiler's live feed is its `feed()`).
    #[must_use]
    pub fn tracer(&self) -> &'static Tracer {
        self.tracer
    }

    /// Wall-clock milliseconds the last batch of steps took (the profiler's `sim.step` row).
    #[must_use]
    pub fn last_step_ms(&self) -> f64 {
        self.last_step_ms
    }

    /// After a control (a fork, a stop): the moving bodies' map is rebuilt, once per control.
    fn touch(&mut self) {
        self.revision += 1;
        self.moving = self
            .sim
            .as_ref()
            .map(SimWorld::moving_transforms)
            .unwrap_or_default();
    }

    /// After a batch of steps: the moving bodies' transforms are updated in place — no
    /// allocation per body per frame (WP-19; `test_play_alloc`). The set of moving bodies
    /// changes only through an input (which [`Self::run_steps_dt`] adds as it applies it) or
    /// a control ([`Self::touch`]).
    fn touch_steps(&mut self) {
        self.revision += 1;
        if self.faults.rebuild_transforms() {
            // W2 positive control: the WP-13 behaviour, a fresh map every batch.
            self.moving = self
                .sim
                .as_ref()
                .map(SimWorld::moving_transforms)
                .unwrap_or_default();
            return;
        }
        if let Some(sim) = self.sim.as_ref() {
            sim.refresh_transforms(&mut self.moving);
        }
    }

    fn record(&mut self, ev: RecEvent) {
        if let Some(r) = self.recording.as_mut() {
            r.events.push(ev);
        }
    }

    fn control_event(&mut self, cmd: PlayCommand) {
        let at = self.step();
        self.log.push(PlayLogEntry {
            tick: tick_of_step(at),
            command: cmd,
        });
        self.record(RecEvent::Control { at, cmd });
        self.tracer.instant(match cmd {
            PlayCommand::Play => "sim.play",
            PlayCommand::Pause => "sim.pause",
            PlayCommand::Step(_) => "sim.step_control",
            PlayCommand::Stop => "sim.stop",
        });
    }

    fn fork(&mut self, edit: &EditSnapshot) -> Result<(), SimError> {
        let sim = SimWorld::fork(edit)?;
        self.sim = Some(sim);
        self.pending.clear();
        self.log.clear();
        self.recording = Some(Recording::new(edit, self.check_every, self.embed_edit));
        self.clock_origin = None;
        self.steps_since_origin = 0;
        self.last_now = None;
        Ok(())
    }

    /// Apply a play control. `edit` is the edit world to fork from (read only).
    pub fn control(&mut self, cmd: PlayCommand, edit: &EditSnapshot) -> Result<(), SimError> {
        match (cmd, self.state) {
            (PlayCommand::Play, PlayState::Stopped) => {
                self.fork(edit)?;
                self.state = PlayState::Playing;
                self.tracer.set_live(true);
                self.control_event(cmd);
            }
            (PlayCommand::Play, PlayState::Paused) => {
                self.state = PlayState::Playing;
                self.clock_origin = None;
                self.steps_since_origin = 0;
                self.tracer.set_live(true);
                self.control_event(cmd);
            }
            (PlayCommand::Pause, PlayState::Playing) => {
                self.state = PlayState::Paused;
                self.tracer.set_live(false);
                self.control_event(cmd);
            }
            (PlayCommand::Step(n), PlayState::Stopped) => {
                self.fork(edit)?;
                self.state = PlayState::Paused;
                self.control_event(cmd);
                self.run_steps(n)?;
            }
            (PlayCommand::Step(n), PlayState::Paused) => {
                self.control_event(cmd);
                self.run_steps(n)?;
            }
            (PlayCommand::Step(n), PlayState::Playing) => {
                // Step pauses a running simulation first; both are logged and recorded, so a
                // replay pauses and steps exactly as the session did.
                self.state = PlayState::Paused;
                self.tracer.set_live(false);
                self.control_event(PlayCommand::Pause);
                self.control_event(cmd);
                self.run_steps(n)?;
            }
            (PlayCommand::Stop, PlayState::Playing | PlayState::Paused) => {
                self.control_event(cmd);
                let at = self.step();
                if let Some(hash) = self.state_hash() {
                    self.record(RecEvent::End { at, hash });
                }
                self.state = PlayState::Stopped;
                self.sim = None;
                self.pending.clear();
                self.tracer.set_live(false);
            }
            // Pause while paused or stopped, Play while playing, Stop while stopped: nothing to do.
            _ => return Ok(()),
        }
        self.touch();
        Ok(())
    }

    /// Queue an input for the next step boundary (checked now: `SIM-0002`/`SIM-0003`, or
    /// `SIM-0004` with no simulation). It is recorded now, in issue order among the controls,
    /// stamped with the step at whose start it applies.
    pub fn input(&mut self, i: SimInput) -> Result<(), SimError> {
        let sim = self.sim.as_ref().ok_or(SimError::NotPlaying)?;
        sim.check_input(&i)?;
        let at = sim.step_count();
        self.pending.push(i);
        self.record(RecEvent::Input { at, input: i });
        Ok(())
    }

    /// Run `n` fixed steps now (Step, a headless `run`, a replay's fast-forward).
    pub fn run_steps(&mut self, n: u32) -> Result<(), SimError> {
        self.run_steps_dt(n, SIM_DT)
    }

    fn run_steps_dt(&mut self, n: u32, dt: f64) -> Result<(), SimError> {
        if n == 0 {
            return Ok(());
        }
        let t0 = std::time::Instant::now(); // measures the batch; never feeds the simulation
        let check_every = self
            .recording
            .as_ref()
            .map_or(DEFAULT_CHECK_EVERY, |r| r.header.check_every);
        let Self {
            sim,
            pending,
            recording,
            tracer,
            moving,
            ..
        } = self;
        let sim = sim.as_mut().ok_or(SimError::NotPlaying)?;
        for _ in 0..n {
            for i in pending.drain(..) {
                sim.apply_input(&i)?;
                // An input can set a resting body moving: it joins the moving map (its
                // transform is filled in after the batch).
                if !moving.contains_key(&i.entity)
                    && sim.is_moving(i.entity)
                    && let Some(t) = sim.transform_of(i.entity)
                {
                    moving.insert(i.entity, t);
                }
            }
            sim.step_dt(dt, tracer)?;
            let now = sim.step_count();
            if now % check_every == 0
                && let Some(r) = recording.as_mut()
            {
                r.events.push(RecEvent::Check {
                    at: now,
                    hash: sim.state_hash(),
                });
            }
        }
        self.tracer.counter("sim.bodies", sim.len() as f64);
        self.last_step_ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.touch_steps();
        Ok(())
    }

    /// Run a playing simulation up to `now` (the editor's clock) in fixed steps. Returns the
    /// steps run.
    pub fn advance(&mut self, now: Duration) -> Result<u32, SimError> {
        if self.state != PlayState::Playing {
            self.last_now = Some(now);
            return Ok(0);
        }
        let origin = *self.clock_origin.get_or_insert(now);
        let micros = u64::try_from(now.saturating_sub(origin).as_micros()).unwrap_or(u64::MAX);
        let due = micros.saturating_mul(u64::from(SIM_RATE_HZ)) / 1_000_000;
        let n = u32::try_from(due.saturating_sub(self.steps_since_origin))
            .unwrap_or(u32::MAX)
            .min(MAX_CATCH_UP);
        let elapsed = self
            .last_now
            .map_or(Duration::ZERO, |l| now.saturating_sub(l));
        self.last_now = Some(now);
        if due > self.steps_since_origin + u64::from(n) {
            // Behind by more than the catch-up: drop the rest rather than spiral.
            self.steps_since_origin = due;
        } else {
            self.steps_since_origin += u64::from(n);
        }
        if n > 0 {
            let dt = if self.faults.frame_dt() {
                elapsed.as_secs_f64() / f64::from(n)
            } else {
                SIM_DT
            };
            self.run_steps_dt(n, dt)?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaked() -> &'static Tracer {
        Box::leak(Box::new(Tracer::new()))
    }

    #[test]
    fn controls_follow_the_toolbar_and_are_logged() {
        let mut p = PlaySession::with_tracer(leaked());
        let e = EditSnapshot::default();
        p.control(PlayCommand::Step(3), &e).unwrap();
        assert_eq!(p.state(), PlayState::Paused);
        assert_eq!(p.step(), 3);
        assert_eq!(p.tick(), Tick(50_000));
        p.control(PlayCommand::Pause, &e).unwrap(); // no-op
        p.control(PlayCommand::Stop, &e).unwrap();
        assert_eq!(p.state(), PlayState::Stopped);
        assert_eq!(p.tick(), Tick(0));
        assert_eq!(
            p.log().iter().map(|l| l.command).collect::<Vec<_>>(),
            vec![PlayCommand::Step(3), PlayCommand::Stop]
        );
        assert!(p.recording().unwrap().is_complete());
        assert_eq!(
            p.input(SimInput {
                entity: EntityKey(1),
                action: crate::InputAction::SetSpin(1.0)
            }),
            Err(SimError::NotPlaying)
        );
    }

    #[test]
    fn advance_runs_fixed_steps_only_while_playing_and_bounds_catch_up() {
        let mut p = PlaySession::with_tracer(leaked());
        let e = EditSnapshot::default();
        assert_eq!(p.advance(Duration::from_secs(1)).unwrap(), 0);
        p.control(PlayCommand::Play, &e).unwrap();
        let t0 = Duration::from_secs(10);
        assert_eq!(p.advance(t0).unwrap(), 0);
        assert_eq!(p.advance(t0 + Duration::from_millis(100)).unwrap(), 6);
        // A 2 s stall runs at most MAX_CATCH_UP steps and drops the rest.
        assert_eq!(
            p.advance(t0 + Duration::from_millis(2100)).unwrap(),
            MAX_CATCH_UP
        );
        assert_eq!(p.advance(t0 + Duration::from_millis(2116)).unwrap(), 0);
        assert_eq!(p.advance(t0 + Duration::from_millis(2117)).unwrap(), 1);
        p.control(PlayCommand::Pause, &e).unwrap();
        assert_eq!(p.advance(t0 + Duration::from_secs(5)).unwrap(), 0);
        assert_eq!(p.step(), 6 + 15 + 1);
    }

    #[test]
    fn step_ticks_follow_the_integer_schedule() {
        assert_eq!(tick_of_step(0), Tick(0));
        assert_eq!(tick_of_step(1), Tick(16_666));
        assert_eq!(tick_of_step(60), Tick(1_000_000));
        assert_eq!(tick_of_step(61), Tick(1_016_666));
        for n in [0, 1, 2, 3, 59, 60, 61, 1_000_003, 36_000_000] {
            assert_eq!(step_of_tick(tick_of_step(n)), n);
        }
    }

    #[test]
    fn step_while_playing_pauses_first_and_both_are_recorded() {
        let mut p = PlaySession::with_tracer(leaked());
        let e = EditSnapshot::default();
        p.control(PlayCommand::Play, &e).unwrap();
        p.run_steps(5).unwrap();
        p.control(PlayCommand::Step(2), &e).unwrap();
        assert_eq!(p.state(), PlayState::Paused);
        assert_eq!(p.step(), 7);
        assert_eq!(
            p.log().iter().map(|l| l.command).collect::<Vec<_>>(),
            vec![PlayCommand::Play, PlayCommand::Pause, PlayCommand::Step(2)]
        );
        let controls = p
            .recording()
            .unwrap()
            .events
            .iter()
            .filter(|ev| matches!(ev, RecEvent::Control { .. }))
            .count();
        assert_eq!(controls, 3);
    }
}
