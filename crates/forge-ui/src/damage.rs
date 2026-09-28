//! Dirty flags, the timer heap, frame scheduling and live-data feeds (Ch.21 §21.3, §21.11).
//!
//! **The zero-idle rule (D-5).** The loop blocks in `Wait` whenever there is no damage,
//! no running animation and no due timer. [`Scheduler::next_wake`] is the single source of
//! that decision; the winit runner and the headless harness both obey it.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::ops::{BitOr, BitOrAssign};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::id::WidgetId;

/// Per-widget dirty flags. `LAYOUT` implies `PAINT` and `A11Y` for that widget.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Dirty(pub u8);

impl Dirty {
    pub const NONE: Dirty = Dirty(0);
    pub const LAYOUT: Dirty = Dirty(1);
    pub const PAINT: Dirty = Dirty(2);
    pub const A11Y: Dirty = Dirty(4);
    pub const ALL: Dirty = Dirty(7);

    pub fn contains(self, o: Dirty) -> bool {
        self.0 & o.0 == o.0
    }
    pub fn intersects(self, o: Dirty) -> bool {
        self.0 & o.0 != 0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    /// Normalise: `LAYOUT` implies `PAINT | A11Y`.
    pub fn implied(self) -> Dirty {
        if self.contains(Dirty::LAYOUT) {
            Dirty::ALL
        } else {
            self
        }
    }
}

impl BitOr for Dirty {
    type Output = Dirty;
    fn bitor(self, o: Dirty) -> Dirty {
        Dirty(self.0 | o.0)
    }
}
impl BitOrAssign for Dirty {
    fn bitor_assign(&mut self, o: Dirty) {
        self.0 |= o.0;
    }
}

/// Presentation time since the `Ui` was created. Not the canonical `Tick` (§21.15): no
/// simulation reads it. Tests inject it, so a 10 s idle takes no wall-clock time.
pub type UiTime = Duration;

/// What the loop should do next (§21.3). There is no `Poll`: nothing polls.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Wake {
    /// Nothing to do until an OS event or a waker arrives.
    Wait,
    /// Sleep until this deadline (the earliest timer or the next animation frame).
    WaitUntil(UiTime),
    /// Damage is pending: draw now (the platform requests a redraw).
    Now,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TimerEntry {
    at: UiTime,
    seq: u64,
    widget: WidgetId,
    tag: u64,
}

/// One timer heap (tooltips, key repeat, caret blink, toast expiry) and the animation
/// registry. Fires nothing early.
#[derive(Default)]
pub struct Scheduler {
    heap: BinaryHeap<Reverse<TimerEntry>>,
    seq: u64,
    /// Widgets with a running animation → the time of their next frame.
    anims: BTreeMap<WidgetId, UiTime>,
    /// The animation frame interval (the display's refresh; 60 Hz by default).
    pub frame_interval: Duration,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            frame_interval: Duration::from_micros(16_667),
            ..Self::default()
        }
    }

    /// Arm a timer; returns its sequence number (for cancellation by tag).
    pub fn set_timer(&mut self, widget: WidgetId, at: UiTime, tag: u64) {
        self.seq += 1;
        self.heap.push(Reverse(TimerEntry {
            at,
            seq: self.seq,
            widget,
            tag,
        }));
    }

    /// Cancel every timer of `widget` with `tag`.
    pub fn cancel_timer(&mut self, widget: WidgetId, tag: u64) {
        let kept: Vec<_> = self
            .heap
            .drain()
            .filter(|Reverse(t)| !(t.widget == widget && t.tag == tag))
            .collect();
        self.heap.extend(kept);
    }

    /// Cancel everything a removed widget scheduled.
    pub fn forget_widget(&mut self, widget: WidgetId) {
        let kept: Vec<_> = self
            .heap
            .drain()
            .filter(|Reverse(t)| t.widget != widget)
            .collect();
        self.heap.extend(kept);
        self.anims.remove(&widget);
    }

    /// Pop every timer due at `now`, in deadline order.
    pub fn due_timers(&mut self, now: UiTime) -> Vec<(WidgetId, u64)> {
        let mut out = Vec::new();
        while let Some(Reverse(t)) = self.heap.peek() {
            if t.at > now {
                break;
            }
            out.push((t.widget, t.tag));
            self.heap.pop();
        }
        out
    }

    /// A widget's animation wants a frame at `next`. Called every frame while it runs.
    pub fn request_anim_frame(&mut self, widget: WidgetId, next: UiTime) {
        self.anims.insert(widget, next);
    }
    pub fn stop_anim(&mut self, widget: WidgetId) {
        self.anims.remove(&widget);
    }
    /// Animations whose frame is due at `now` (removed; they re-request if still running).
    pub fn due_anims(&mut self, now: UiTime) -> Vec<WidgetId> {
        let due: Vec<WidgetId> = self
            .anims
            .iter()
            .filter(|(_, t)| **t <= now)
            .map(|(w, _)| *w)
            .collect();
        for w in &due {
            self.anims.remove(w);
        }
        due
    }
    pub fn running_anims(&self) -> usize {
        self.anims.len()
    }
    pub fn pending_timers(&self) -> usize {
        self.heap.len()
    }

    /// The zero-idle decision: `Now` if there is damage, else the earliest timer or
    /// animation deadline, else `Wait`.
    pub fn next_wake(&self, has_damage: bool) -> Wake {
        if has_damage {
            return Wake::Now;
        }
        let t = self.heap.peek().map(|Reverse(t)| t.at);
        let a = self.anims.values().min().copied();
        match (t, a) {
            (None, None) => Wake::Wait,
            (Some(x), None) | (None, Some(x)) => Wake::WaitUntil(x),
            (Some(x), Some(y)) => Wake::WaitUntil(x.min(y)),
        }
    }
}

// ---- live-data feeds (§21.11 "Live-data panels") --------------------------------------

/// Wakes the UI loop from another thread (`EventLoopProxy` in the winit runner; a
/// counter in the headless harness).
pub trait UiWaker: Send + Sync {
    fn wake(&self);
}

/// A producer of data that changes by itself (profiler counters, latency, presence).
pub trait LiveSource: Send + Sync {
    /// Bumped by the producer whenever the data changes. Unchanged ⇒ nothing to draw.
    fn generation(&self) -> u64;
    /// True while the producer is running (engine playing, session connected, job live).
    fn is_live(&self) -> bool;
    /// Rule 1 (push, not poll): the shell registers a waker only while the feed's panel
    /// is visible; the producer calls it on a bump, at most once until consumed.
    fn set_waker(&self, waker: Option<Arc<dyn UiWaker>>);
    /// The UI consumed the latest bump; the next bump may wake again.
    fn consumed(&self);
}

/// The standard [`LiveSource`]: an atomic generation plus an optional waker.
#[derive(Default)]
pub struct LiveCell {
    epoch: AtomicU64,
    live: AtomicBool,
    /// Set when a wake was sent and the UI has not consumed the bump yet.
    armed: AtomicBool,
    waker: Mutex<Option<Arc<dyn UiWaker>>>,
}

impl LiveCell {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// Producer side: the data changed.
    pub fn bump(&self) {
        self.epoch.fetch_add(1, Ordering::Release);
        if let Ok(w) = self.waker.lock()
            && let Some(w) = w.as_ref()
            && !self.armed.swap(true, Ordering::AcqRel)
        {
            w.wake();
        }
    }
    pub fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Release);
    }
}

impl LiveSource for LiveCell {
    fn generation(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }
    fn set_waker(&self, waker: Option<Arc<dyn UiWaker>>) {
        if let Ok(mut w) = self.waker.lock() {
            *w = waker;
        }
        self.armed.store(false, Ordering::Release);
    }
    fn consumed(&self) {
        self.armed.store(false, Ordering::Release);
    }
}

/// A panel's subscription to a live source.
pub struct LiveFeed {
    pub source: Arc<dyn LiveSource>,
    /// Refresh at most this often while visible (profiler 10, latency 2, compute 2,
    /// presence 10).
    pub max_hz: u8,
    /// A `ui.self` feed (the UI's own counters): never wakes, never marks dirty.
    pub self_ui: bool,
}

/// Opaque id of a registered feed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeedId(pub u32);

pub(crate) struct FeedState {
    pub(crate) feed: LiveFeed,
    pub(crate) owner: WidgetId,
    pub(crate) seen_gen: u64,
    pub(crate) last_refresh: Option<UiTime>,
    pub(crate) visible: bool,
    pub(crate) waker_registered: bool,
}

/// Rules 1–4 of §21.11, applied to every feed.
#[derive(Default)]
pub struct LiveFeeds {
    pub(crate) feeds: BTreeMap<FeedId, FeedState>,
    next: u32,
}

forge_trace::control_switches! {
    /// Test-only fault switches for the live-feed rules (W2 positive controls).
    #[derive(Clone, Copy, Debug, Default)]
    pub struct FeedFaults {
        /// The `ui.self` feed is allowed to wake the loop (it then redraws forever).
        pub self_ui_can_wake: bool,
        /// A hidden feed keeps its waker.
        pub keep_waker_when_hidden: bool,
        /// Feeds notice a visibility change only at the next frame's start (not when it
        /// happens): a panel shown mid-frame by the dock does not catch up until something else
        /// wakes the loop (`test_remote_connect`'s hidden-panel control).
        pub visibility_on_next_frame: bool,
    }
}

/// Timer tag the scheduler uses for a feed's deferred (rate-capped) refresh.
pub(crate) const FEED_TIMER_BASE: u64 = 0xFEED_0000_0000;

impl LiveFeeds {
    pub(crate) fn add(&mut self, owner: WidgetId, feed: LiveFeed) -> FeedId {
        self.next += 1;
        let id = FeedId(self.next);
        let seen_gen = feed.source.generation();
        self.feeds.insert(
            id,
            FeedState {
                feed,
                owner,
                seen_gen,
                last_refresh: None,
                visible: false,
                waker_registered: false,
            },
        );
        id
    }

    pub(crate) fn remove_owner(&mut self, owner: WidgetId) {
        self.feeds.retain(|_, f| {
            if f.owner == owner {
                f.feed.source.set_waker(None);
                false
            } else {
                true
            }
        });
    }

    /// Re-evaluate waker registration from visibility (rule 2) and self-ui (rule 4).
    pub(crate) fn sync_wakers(
        &mut self,
        waker: &Option<Arc<dyn UiWaker>>,
        is_visible: impl Fn(WidgetId) -> bool,
        faults: FeedFaults,
    ) -> Vec<FeedId> {
        let mut became_visible = Vec::new();
        for (id, f) in self.feeds.iter_mut() {
            let vis = is_visible(f.owner);
            if vis && !f.visible {
                became_visible.push(*id);
            }
            f.visible = vis;
            let want = waker.is_some()
                && (vis || faults.keep_waker_when_hidden())
                && (!f.feed.self_ui || faults.self_ui_can_wake());
            if want != f.waker_registered {
                f.feed
                    .source
                    .set_waker(if want { waker.clone() } else { None });
                f.waker_registered = want;
            }
        }
        became_visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scheduler_waits() {
        let s = Scheduler::new();
        assert_eq!(s.next_wake(false), Wake::Wait);
        assert_eq!(s.next_wake(true), Wake::Now);
    }

    #[test]
    fn timers_fire_in_order_and_never_early() {
        let mut s = Scheduler::new();
        let w = WidgetId(1);
        s.set_timer(w, Duration::from_millis(500), 2);
        s.set_timer(w, Duration::from_millis(100), 1);
        assert_eq!(
            s.next_wake(false),
            Wake::WaitUntil(Duration::from_millis(100))
        );
        assert!(s.due_timers(Duration::from_millis(99)).is_empty());
        assert_eq!(
            s.due_timers(Duration::from_millis(600)),
            vec![(w, 1), (w, 2)]
        );
    }

    #[test]
    fn live_cell_wakes_once_until_consumed() {
        struct Count(AtomicU64);
        impl UiWaker for Count {
            fn wake(&self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let c = Arc::new(Count(AtomicU64::new(0)));
        let cell = LiveCell::new();
        cell.bump(); // no waker: silent
        cell.set_waker(Some(c.clone()));
        cell.bump();
        cell.bump();
        assert_eq!(c.0.load(Ordering::Relaxed), 1);
        cell.consumed();
        cell.bump();
        assert_eq!(c.0.load(Ordering::Relaxed), 2);
        assert_eq!(cell.generation(), 4);
    }
}
