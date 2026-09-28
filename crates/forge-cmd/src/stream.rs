//! The [`Applied`] stream's subscriber side: a **bounded, coalescing** queue per subscriber.
//!
//! A subscriber that stops pumping (a panel frozen behind a modal, a remote mirror whose
//! socket stalled, a debugger breakpoint) must not grow the bus's memory without limit, and
//! it must not silently miss state either. So each subscription holds at most `capacity`
//! events; when it overflows, the queued events are discarded and replaced by one [`Gap`]
//! marker that keeps absorbing further events in `O(1)` memory until the subscriber drains.
//! The gap tells the subscriber exactly which `seq` range it missed, and the subscriber
//! resyncs from the latest state ([`crate::Bus::project`] plus [`crate::Bus::next_seq`]) —
//! coalescing to the latest state is cheaper than replaying thousands of stale events.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use serde::{Deserialize, Serialize};

use crate::Applied;

/// Default bound on a subscription's undrained events. A UI pumping once a frame never
/// comes near it; a stalled subscriber hits it and gets a [`Gap`] instead of unbounded memory.
pub const DEFAULT_SUBSCRIPTION_CAPACITY: usize = 4096;

/// Events a stalled subscriber missed. Every event with `first_missed <= seq <= last_missed`
/// was dropped (none of them is delivered later). Resync: rebuild from
/// [`crate::Bus::project`], note [`crate::Bus::next_seq`] at that moment, and skip delivered
/// events whose `seq` is below it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Gap {
    /// The oldest dropped event's `seq`.
    pub first_missed: u64,
    /// The newest dropped event's `seq`.
    pub last_missed: u64,
    /// How many events were dropped.
    pub missed: u64,
}

/// One item of the stream, as [`Subscription::try_next`] delivers it.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamItem {
    /// The subscriber fell behind; resync before using later events (see [`Gap`]).
    Gap(Gap),
    /// An event.
    Applied(Arc<Applied>),
}

/// What [`Subscription::drain`] returns: an optional gap (always *before* the events: every
/// event in `events` is newer than the gap) and the events, in `seq` order.
#[derive(Clone, Debug, Default, PartialEq)]
#[must_use = "a gap means the subscriber missed state and must resync"]
pub struct Drained {
    /// Set if events were dropped since the last drain.
    pub gap: Option<Gap>,
    /// The waiting events, oldest first.
    pub events: Vec<Arc<Applied>>,
}

impl Drained {
    /// Number of events (the gap is not counted).
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// No events and no gap.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.gap.is_none()
    }

    /// The events, oldest first.
    pub fn iter(&self) -> std::slice::Iter<'_, Arc<Applied>> {
        self.events.iter()
    }
}

impl std::ops::Index<usize> for Drained {
    type Output = Arc<Applied>;
    fn index(&self, i: usize) -> &Arc<Applied> {
        &self.events[i]
    }
}

#[derive(Debug)]
pub(crate) struct Queue {
    events: VecDeque<Arc<Applied>>,
    capacity: usize,
    gap: Option<Gap>,
}

impl Queue {
    fn push(&mut self, a: &Arc<Applied>) {
        if let Some(g) = &mut self.gap {
            g.last_missed = a.seq;
            g.missed += 1;
            return;
        }
        if self.events.len() < self.capacity {
            self.events.push_back(Arc::clone(a));
            return;
        }
        // Overflow: coalesce everything queued plus this event into one gap marker.
        let first = self.events.front().map_or(a.seq, |e| e.seq);
        let missed = self.events.len() as u64 + 1;
        self.events.clear();
        self.gap = Some(Gap {
            first_missed: first,
            last_missed: a.seq,
            missed,
        });
    }
}

fn lock(q: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    // A panic while holding this lock cannot leave the queue half-updated in a way that
    // matters (each push is a single VecDeque op or a field write), so recover the guard.
    q.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The bus's side of a subscription: a weak handle, so a dropped [`Subscription`]
/// unsubscribes itself.
pub(crate) struct Publisher {
    subs: Vec<Weak<Mutex<Queue>>>,
}

impl Publisher {
    pub(crate) fn new() -> Self {
        Self { subs: Vec::new() }
    }

    pub(crate) fn subscribe(&mut self, capacity: usize) -> Subscription {
        let capacity = capacity.max(1);
        let q = Arc::new(Mutex::new(Queue {
            events: VecDeque::with_capacity(capacity.min(64)),
            capacity,
            gap: None,
        }));
        self.subs.push(Arc::downgrade(&q));
        Subscription { q }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }

    pub(crate) fn publish(&mut self, a: &Arc<Applied>) {
        self.subs.retain(|w| match w.upgrade() {
            Some(q) => {
                lock(&q).push(a);
                true
            }
            None => false,
        });
    }

    pub(crate) fn len(&self) -> usize {
        self.subs.len()
    }
}

/// A subscription to the [`Applied`] stream. Events are shared (`Arc`), never deep-copied
/// per subscriber; memory is bounded by the capacity (see the module docs). Dropping it
/// unsubscribes.
pub struct Subscription {
    q: Arc<Mutex<Queue>>,
}

impl Subscription {
    /// The next item, if one is waiting: a pending [`Gap`] comes first.
    #[must_use]
    pub fn try_next(&self) -> Option<StreamItem> {
        let mut q = lock(&self.q);
        if let Some(g) = q.gap.take() {
            return Some(StreamItem::Gap(g));
        }
        q.events.pop_front().map(StreamItem::Applied)
    }

    /// Everything waiting (the shell's stream pump calls this once a frame).
    pub fn drain(&self) -> Drained {
        let mut q = lock(&self.q);
        Drained {
            gap: q.gap.take(),
            events: q.events.drain(..).collect(),
        }
    }

    /// Events waiting right now (never more than [`Subscription::capacity`]).
    #[must_use]
    pub fn pending(&self) -> usize {
        lock(&self.q).events.len()
    }

    /// The bound on waiting events.
    #[must_use]
    pub fn capacity(&self) -> usize {
        lock(&self.q).capacity
    }
}
