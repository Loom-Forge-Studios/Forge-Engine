//! The live feed the editor's profiler panel reads (Ch.21 §21.11, §21.21 "Profiler").
//!
//! [`FeedCell`] has exactly the semantics of `forge_ui::LiveCell` — an atomic generation, a
//! live flag, and a waker called **at most once until the UI consumes the bump** — without
//! depending on the UI crate (forge-trace sits under the whole engine). The editor's adapter
//! (`forge_editor::sim_bridge::TraceCounters`) implements `forge_ui::LiveSource` by
//! forwarding to it, and maps a [`TraceSnapshot`] to the panel's `ProfileSnapshot`
//! (`sim_bridge::profile_snapshot`): the frames (time and per-zone parts) and lanes are
//! copied, a budget keeps its name, value, allowance and unit, counters and dropped events
//! have their own fields.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::Budget;

/// One frame on the timeline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameSample {
    /// Whole-frame time (between frame marks), ms.
    pub total_ms: f64,
    /// Named parts of the frame (zone totals: `sim.step`, `render.frame.prepare_cpu`), ms.
    pub parts: Vec<(String, f64)>,
}

/// Work on one adapter of the pool (Ch.26): spans within the last frame, ms from its start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lane {
    pub adapter: String,
    pub spans: Vec<(f64, f64, String)>,
}

/// Everything the profiler shows, at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TraceSnapshot {
    pub frames: Vec<FrameSample>,
    pub budgets: Vec<Budget>,
    /// Latest value of every counter.
    pub counters: Vec<(String, f64)>,
    pub lanes: Vec<Lane>,
    /// Perfetto events dropped at the recording's cap.
    pub dropped_events: u64,
}

/// A waker: called when the feed changes while a waker is registered (the panel visible).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The feed's change signal (see the module docs).
pub struct FeedCell {
    generation: AtomicU64,
    live: AtomicBool,
    /// A wake was sent and the UI has not consumed the bump yet.
    armed: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Default for FeedCell {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FeedCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedCell")
            .field("generation", &self.generation())
            .field("live", &self.is_live())
            .finish_non_exhaustive()
    }
}

impl FeedCell {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            live: AtomicBool::new(false),
            armed: AtomicBool::new(false),
            waker: Mutex::new(None),
        }
    }

    /// Producer side: the data changed. Wakes the registered waker once until consumed.
    pub fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        let w = self
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(w) = w
            && !self.armed.swap(true, Ordering::AcqRel)
        {
            w();
        }
    }

    /// Bumped whenever the data changes. Unchanged: nothing to redraw.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// True while the producer runs (a simulation playing).
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    pub fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Release);
    }

    /// Register (panel visible) or drop (hidden) the waker.
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = waker;
        self.armed.store(false, Ordering::Release);
    }

    /// The UI consumed the latest bump; the next bump may wake again.
    pub fn consumed(&self) {
        self.armed.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn wakes_once_until_consumed_and_never_without_a_waker() {
        let c = FeedCell::new();
        c.bump();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        c.set_waker(Some(Arc::new(move || {
            n2.fetch_add(1, Ordering::Relaxed);
        })));
        c.bump();
        c.bump();
        assert_eq!(n.load(Ordering::Relaxed), 1);
        c.consumed();
        c.bump();
        assert_eq!(n.load(Ordering::Relaxed), 2);
        c.set_waker(None);
        c.bump();
        assert_eq!(n.load(Ordering::Relaxed), 2);
        assert_eq!(c.generation(), 5);
    }
}
