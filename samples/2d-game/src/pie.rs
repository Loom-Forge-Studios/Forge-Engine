//! Play-in-editor for the 2D sample (feature `pie`; never in an exported build).
//!
//! [`SamplePlay`] is a `forge_editor::play::PlayBackend`: the editor's play controls — the
//! `forge.play.control` session command on the bus, from the play-controls panel, a chord, a
//! script or an automation session — run the sample's level exactly as the shipped game does.
//! Play forks a new run of the level (resumes a paused one), Pause and Step(n) behave as the
//! editor's own play core does (a Step while playing pauses first, a Step while stopped forks
//! first), and Stop throws the run away. It is handed the project's mirror read-only and never a
//! command sink, so playing cannot change the project (I7): the level is the sample's generated
//! level, not project entities, so the mirror is not read at all.
//!
//! The run advances at the same fixed 60 Hz as the shipped game, at most 15 steps per call
//! (`forge_editor::play::PLAY_FRAME` cadence), and sees only the pad state at each step: the
//! same scripted input played in the editor and in the exported build reaches the same bits
//! (`tests/test_pie.rs`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use forge_cmd::EntityKey;
use forge_editor::mirror::ProjectMirror;
use forge_editor::play::{
    PlayBackend, PlayCommand, PlayLogEntry, PlayState, SimTransform, tick_of_step,
};
use forge_editor::services::EditorServices;
use forge_frames::Tick;
use forge_ui::LiveCell;
use forge_ui::game::PadInput;

use crate::{Fingerprint, Game, PadState};

/// What the play controls show as the backend.
pub const BACKEND: &str = "forge-2d-game (the 2D sample's level)";
/// Fixed steps one `advance` may run at most (a longer stall drops the backlog).
pub const MAX_STEPS_PER_ADVANCE: u64 = 15;

/// The sample's play backend (see the module docs).
pub struct SamplePlay {
    game: Option<Game>,
    state: PlayState,
    pad: PadState,
    script: Vec<Vec<PadInput>>,
    /// Where the current play stretch began on the editor clock (`None`: at the next
    /// `advance`), and the game step it began at.
    origin: Option<Duration>,
    origin_step: u64,
    fingerprint: Fingerprint,
    log: Vec<PlayLogEntry>,
    transforms: BTreeMap<EntityKey, SimTransform>,
    feed: Arc<LiveCell>,
    revision: u64,
    last_step_ms: f64,
    error: Option<String>,
}

impl Default for SamplePlay {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SamplePlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SamplePlay")
            .field("state", &self.state)
            .field("steps", &self.game.as_ref().map(|g| g.steps))
            .finish_non_exhaustive()
    }
}

impl SamplePlay {
    /// A stopped backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            game: None,
            state: PlayState::Stopped,
            pad: PadState::default(),
            script: Vec::new(),
            origin: None,
            origin_step: 0,
            fingerprint: Fingerprint::default(),
            log: Vec::new(),
            transforms: BTreeMap::new(),
            feed: LiveCell::new(),
            revision: 0,
            last_step_ms: 0.0,
            error: None,
        }
    }

    /// Make this the editor's play backend; the returned handle is the same object (tests
    /// and the editor's pad route read and drive it).
    pub fn install(services: &mut EditorServices) -> Rc<RefCell<SamplePlay>> {
        let play = Rc::new(RefCell::new(SamplePlay::new()));
        services.play = play.clone();
        services.play_core = None;
        play
    }

    /// The running level (`None` when stopped).
    #[must_use]
    pub fn game(&self) -> Option<&Game> {
        self.game.as_ref()
    }

    /// The run's fingerprint so far (reset by a fork).
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    /// One pad input, for the next step.
    pub fn pad(&mut self, i: PadInput) {
        self.pad.apply(i);
    }

    /// A scripted pad: the inputs for game step `i` are applied just before it.
    pub fn set_script(&mut self, script: Vec<Vec<PadInput>>) {
        self.script = script;
    }

    fn fork(&mut self) -> bool {
        match Game::new() {
            Ok(g) => {
                self.game = Some(g);
                self.pad = PadState::default();
                self.fingerprint = Fingerprint::default();
                self.log.clear();
                self.origin = None;
                self.origin_step = 0;
                true
            }
            Err(e) => {
                self.error = Some(format!("{BACKEND}: the level did not build: {e}"));
                false
            }
        }
    }

    fn step_once(&mut self) -> bool {
        let Some(g) = self.game.as_mut() else {
            return false;
        };
        let i = usize::try_from(g.steps).unwrap_or(usize::MAX);
        if let Some(inputs) = self.script.get(i) {
            for x in inputs {
                self.pad.apply(*x);
            }
        }
        if let Err(e) = g.step(&self.pad) {
            self.error = Some(format!("{BACKEND}: step {i}: {e}"));
            return false;
        }
        self.fingerprint.observe(g.steps - 1, g);
        true
    }

    fn changed(&mut self) {
        self.revision += 1;
        self.feed.bump();
        self.feed.set_live(self.state == PlayState::Playing);
    }
}

impl PlayBackend for SamplePlay {
    fn backend(&self) -> &str {
        BACKEND
    }

    fn state(&self) -> PlayState {
        self.state
    }

    fn control(&mut self, cmd: PlayCommand, _edit: &ProjectMirror) {
        let at = self.tick();
        match cmd {
            PlayCommand::Play => match self.state {
                PlayState::Stopped => {
                    if !self.fork() {
                        return;
                    }
                    self.state = PlayState::Playing;
                }
                PlayState::Paused => {
                    self.state = PlayState::Playing;
                    self.origin = None;
                }
                PlayState::Playing => return,
            },
            PlayCommand::Pause => {
                if self.state != PlayState::Playing {
                    return;
                }
                self.state = PlayState::Paused;
            }
            PlayCommand::Step(n) => {
                if self.state == PlayState::Stopped && !self.fork() {
                    return;
                }
                self.state = PlayState::Paused;
                for _ in 0..n {
                    if !self.step_once() {
                        break;
                    }
                }
            }
            PlayCommand::Stop => {
                if self.state == PlayState::Stopped {
                    return;
                }
                self.game = None;
                self.state = PlayState::Stopped;
            }
        }
        // A fork clears the log; the control that forked is its first entry.
        self.log.push(PlayLogEntry {
            tick: at,
            command: cmd,
        });
        self.changed();
    }

    fn advance(&mut self, now: Duration) -> u32 {
        if self.state != PlayState::Playing {
            return 0;
        }
        let steps = self.game.as_ref().map_or(0, |g| g.steps);
        let Some(origin) = self.origin else {
            // The first advance of a play stretch anchors it on the editor clock.
            self.origin = Some(now);
            self.origin_step = steps;
            return 0;
        };
        let due = (now.saturating_sub(origin).as_micros() * 60 / 1_000_000) as u64;
        let taken = steps - self.origin_step;
        if due.saturating_sub(taken) > MAX_STEPS_PER_ADVANCE {
            // A stall: drop the backlog, keep the cadence.
            self.origin = Some(now);
            self.origin_step = steps;
            return 0;
        }
        let t0 = Instant::now();
        let mut n = 0u32;
        while self.game.as_ref().map_or(0, |g| g.steps) - self.origin_step < due {
            if !self.step_once() {
                break;
            }
            n += 1;
        }
        if n > 0 {
            self.last_step_ms = t0.elapsed().as_secs_f64() * 1000.0;
            self.changed();
        }
        n
    }

    fn tick(&self) -> Tick {
        self.game
            .as_ref()
            .map_or(Tick(0), |g| tick_of_step(g.steps))
    }

    fn transforms(&self) -> &BTreeMap<EntityKey, SimTransform> {
        // The level is the sample's own, not project entities: nothing to show per entity.
        &self.transforms
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn log(&self) -> &[PlayLogEntry] {
        &self.log
    }

    fn feed(&self) -> Arc<LiveCell> {
        Arc::clone(&self.feed)
    }

    fn last_step_ms(&self) -> f64 {
        self.last_step_ms
    }

    fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}
