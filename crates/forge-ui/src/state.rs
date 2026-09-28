//! Fine-grained reactive state: `Signal`, `Memo`, `Effect`, `Bind` (Ch.21 §21.5).
//!
//! A panel's build function runs **once**. Each dynamic property is bound to a signal;
//! when it changes, only its subscribers are invalidated. Nothing is diffed and no build
//! function re-runs, which is what makes "work proportional to change" true.
//!
//! * [`Signal::set`] with an equal value is a no-op: no invalidation, no damage.
//! * Writes during an iteration are batched; [`Runtime::flush`] (step 4 of §21.3) runs
//!   subscribers once, in dependency order (by memo height), and returns the widget
//!   dirty marks.
//! * A [`Memo`] with subscribers is recomputed during the flush and stops propagation
//!   when its value did not change, so a change the user cannot see (latency equal after
//!   rounding) causes no damage (§21.11 rule 3). A memo nobody watches is recomputed
//!   lazily on the next read.
//!
//! All of this is serialised against the UI thread (W7): the runtime is `!Send`. Other
//! threads post closures into the UI queue instead (`Ui::poster`).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;

use crate::damage::Dirty;
use crate::id::WidgetId;

/// A writable reactive value. `Copy` handle into the [`Runtime`]'s arena.
pub struct Signal<T> {
    idx: u32,
    epoch: u32,
    _t: PhantomData<fn() -> T>,
}

/// A derived value, recomputed only when a dependency changed.
pub struct Memo<T> {
    idx: u32,
    epoch: u32,
    _t: PhantomData<fn() -> T>,
}

/// A side effect re-run (after the flush) whenever a signal it read changed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    idx: u32,
    epoch: u32,
}

macro_rules! handle_impls {
    ($t:ident) => {
        impl<T> Clone for $t<T> {
            fn clone(&self) -> Self {
                *self
            }
        }
        impl<T> Copy for $t<T> {}
        impl<T> PartialEq for $t<T> {
            fn eq(&self, o: &Self) -> bool {
                self.idx == o.idx && self.epoch == o.epoch
            }
        }
        impl<T> Eq for $t<T> {}
        impl<T> std::fmt::Debug for $t<T> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({}#{})", stringify!($t), self.idx, self.epoch)
            }
        }
        impl<T> $t<T> {
            /// The untyped handle, for subscriptions.
            pub fn any(&self) -> AnySignal {
                AnySignal {
                    idx: self.idx,
                    epoch: self.epoch,
                }
            }
        }
    };
}
handle_impls!(Signal);
handle_impls!(Memo);

/// An untyped signal or memo handle (what a widget subscribes to).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnySignal {
    idx: u32,
    epoch: u32,
}

/// A property that is either constant or bound to reactive state.
#[derive(Clone, Debug)]
pub enum Bind<T> {
    Const(T),
    Signal(Signal<T>),
    Memo(Memo<T>),
}

impl<T: Clone + PartialEq + Default + 'static> Bind<T> {
    pub fn get(&self, rt: &Runtime) -> T {
        match self {
            Bind::Const(v) => v.clone(),
            Bind::Signal(s) => s.get(rt),
            Bind::Memo(m) => m.get(rt),
        }
    }
    /// The handle to subscribe to, if the property is reactive.
    pub fn source(&self) -> Option<AnySignal> {
        match self {
            Bind::Const(_) => None,
            Bind::Signal(s) => Some(s.any()),
            Bind::Memo(m) => Some(m.any()),
        }
    }
}

impl<T> From<Signal<T>> for Bind<T> {
    fn from(s: Signal<T>) -> Self {
        Bind::Signal(s)
    }
}
impl<T> From<Memo<T>> for Bind<T> {
    fn from(m: Memo<T>) -> Self {
        Bind::Memo(m)
    }
}
impl From<&str> for Bind<String> {
    fn from(s: &str) -> Self {
        Bind::Const(s.to_string())
    }
}
impl From<&String> for Bind<String> {
    fn from(s: &String) -> Self {
        Bind::Const(s.clone())
    }
}

impl From<String> for Bind<String> {
    fn from(s: String) -> Self {
        Bind::Const(s)
    }
}
impl From<bool> for Bind<bool> {
    fn from(b: bool) -> Self {
        Bind::Const(b)
    }
}
impl From<f32> for Bind<f32> {
    fn from(v: f32) -> Self {
        Bind::Const(v)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sub {
    Widget(WidgetId, Dirty),
    Node(u32),
}

type Compute = Box<dyn Fn(&Runtime) -> Box<dyn Any>>;
type EqFn = fn(&dyn Any, &dyn Any) -> bool;
type EffectFn = RefCell<Box<dyn FnMut(&Runtime)>>;

enum Kind {
    Signal,
    Memo {
        compute: Compute,
        eq: EqFn,
        stale: Cell<bool>,
    },
    Effect {
        run: EffectFn,
    },
}

struct Slot {
    epoch: u32,
    live: bool,
    value: RefCell<Option<Box<dyn Any>>>,
    kind: Kind,
    subs: RefCell<Vec<Sub>>,
    deps: RefCell<Vec<u32>>,
    /// Memo/effect height: 1 + the max height of its dependencies. Signals are 0.
    height: Cell<u32>,
}

/// Counters the budget tests read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStats {
    pub memo_recomputes: u64,
    pub effect_runs: u64,
    pub widget_invalidations: u64,
}

/// The UI thread's signal arena.
#[derive(Default)]
pub struct Runtime {
    slots: Vec<Slot>,
    free: Vec<u32>,
    pending: Vec<u32>,
    tracking: RefCell<Vec<Vec<u32>>>,
    widget_subs: HashMap<WidgetId, Vec<u32>>,
    stats: Cell<RuntimeStats>,
}

fn eq_of<T: PartialEq + 'static>(a: &dyn Any, b: &dyn Any) -> bool {
    match (a.downcast_ref::<T>(), b.downcast_ref::<T>()) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

impl Runtime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> RuntimeStats {
        self.stats.get()
    }

    fn bump(&self, f: impl FnOnce(&mut RuntimeStats)) {
        let mut s = self.stats.get();
        f(&mut s);
        self.stats.set(s);
    }

    fn alloc(&mut self, value: Option<Box<dyn Any>>, kind: Kind) -> (u32, u32) {
        let slot_of = |epoch| Slot {
            epoch,
            live: true,
            value: RefCell::new(value),
            kind,
            subs: RefCell::new(Vec::new()),
            deps: RefCell::new(Vec::new()),
            height: Cell::new(0),
        };
        if let Some(idx) = self.free.pop() {
            let epoch = self.slots[idx as usize].epoch.wrapping_add(1);
            self.slots[idx as usize] = slot_of(epoch);
            (idx, epoch)
        } else {
            let idx = u32::try_from(self.slots.len()).unwrap_or(u32::MAX);
            self.slots.push(slot_of(0));
            (idx, 0)
        }
    }

    /// Create a signal.
    pub fn signal<T: PartialEq + 'static>(&mut self, v: T) -> Signal<T> {
        let (idx, epoch) = self.alloc(Some(Box::new(v)), Kind::Signal);
        Signal {
            idx,
            epoch,
            _t: PhantomData,
        }
    }

    /// Create a memo. `f` is run once now (to learn its dependencies) and again whenever
    /// one of the signals it read changes.
    pub fn memo<T: PartialEq + 'static>(&mut self, f: impl Fn(&Runtime) -> T + 'static) -> Memo<T> {
        let compute: Compute = Box::new(move |rt| Box::new(f(rt)));
        let (idx, epoch) = self.alloc(
            None,
            Kind::Memo {
                compute,
                eq: eq_of::<T>,
                stale: Cell::new(true),
            },
        );
        self.recompute(idx);
        Memo {
            idx,
            epoch,
            _t: PhantomData,
        }
    }

    /// Create an effect. `f` runs now and after every flush in which a signal it read
    /// changed.
    pub fn effect(&mut self, f: impl FnMut(&Runtime) + 'static) -> Effect {
        let (idx, epoch) = self.alloc(
            None,
            Kind::Effect {
                run: RefCell::new(Box::new(f)),
            },
        );
        self.run_effect(idx);
        Effect { idx, epoch }
    }

    fn slot(&self, idx: u32, epoch: u32) -> Option<&Slot> {
        self.slots
            .get(idx as usize)
            .filter(|s| s.live && s.epoch == epoch)
    }

    fn track(&self, idx: u32) {
        if let Some(top) = self.tracking.borrow_mut().last_mut()
            && !top.contains(&idx)
        {
            top.push(idx);
        }
    }

    /// Run `idx`'s compute with dependency tracking and re-register its dependencies.
    fn tracked<R>(&self, idx: u32, run: impl FnOnce() -> R) -> R {
        self.tracking.borrow_mut().push(Vec::new());
        let out = run();
        let deps = self.tracking.borrow_mut().pop().unwrap_or_default();
        let slot = &self.slots[idx as usize];
        // Unsubscribe from the old dependency set, subscribe to the new.
        for d in slot.deps.borrow().iter() {
            if let Some(ds) = self.slots.get(*d as usize) {
                ds.subs.borrow_mut().retain(|s| *s != Sub::Node(idx));
            }
        }
        let mut height = 0;
        for d in &deps {
            if let Some(ds) = self.slots.get(*d as usize) {
                let mut subs = ds.subs.borrow_mut();
                if !subs.contains(&Sub::Node(idx)) {
                    subs.push(Sub::Node(idx));
                }
                height = height.max(ds.height.get() + 1);
            }
        }
        slot.height.set(height);
        *slot.deps.borrow_mut() = deps;
        out
    }

    /// Recompute a memo; returns whether its value changed.
    fn recompute(&self, idx: u32) -> bool {
        let slot = &self.slots[idx as usize];
        let Kind::Memo { compute, eq, stale } = &slot.kind else {
            return false;
        };
        self.bump(|s| s.memo_recomputes += 1);
        let new = self.tracked(idx, || compute(self));
        stale.set(false);
        let mut v = slot.value.borrow_mut();
        let changed = match v.as_deref() {
            Some(old) => !eq(old, new.as_ref()),
            None => true,
        };
        if changed {
            *v = Some(new);
        }
        changed
    }

    fn run_effect(&self, idx: u32) {
        let slot = &self.slots[idx as usize];
        let Kind::Effect { run } = &slot.kind else {
            return;
        };
        self.bump(|s| s.effect_runs += 1);
        self.tracked(idx, || {
            if let Ok(mut f) = run.try_borrow_mut() {
                f(self);
            }
        });
    }

    fn read<T: 'static, R>(&self, idx: u32, epoch: u32, f: impl FnOnce(&T) -> R) -> Option<R> {
        let slot = self.slot(idx, epoch)?;
        self.track(idx);
        if let Kind::Memo { stale, .. } = &slot.kind
            && stale.get()
        {
            self.recompute(idx);
        }
        let v = slot.value.borrow();
        v.as_deref().and_then(|b| b.downcast_ref::<T>()).map(f)
    }

    /// Subscribe widget `w` to `s`: a change marks `w` with `dirty` at the next flush.
    pub fn subscribe(&mut self, s: AnySignal, w: WidgetId, dirty: Dirty) {
        let Some(slot) = self.slot(s.idx, s.epoch) else {
            return;
        };
        {
            let mut subs = slot.subs.borrow_mut();
            let sub = Sub::Widget(w, dirty);
            if !subs.contains(&sub) {
                subs.push(sub);
            }
        }
        self.widget_subs.entry(w).or_default().push(s.idx);
    }

    /// Drop every subscription of a removed widget.
    pub fn unsubscribe_widget(&mut self, w: WidgetId) {
        if let Some(idxs) = self.widget_subs.remove(&w) {
            for i in idxs {
                if let Some(slot) = self.slots.get(i as usize) {
                    slot.subs
                        .borrow_mut()
                        .retain(|s| !matches!(s, Sub::Widget(id, _) if *id == w));
                }
            }
        }
    }

    /// Free a signal or memo. Later reads through stale handles return `None`/panic-free
    /// defaults instead of aliasing a new value.
    pub fn dispose(&mut self, s: AnySignal) {
        if self.slot(s.idx, s.epoch).is_some() {
            let slot = &mut self.slots[s.idx as usize];
            slot.live = false;
            *slot.value.borrow_mut() = None;
            slot.subs.borrow_mut().clear();
            self.free.push(s.idx);
        }
    }

    /// Whether any signal changed since the last flush.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Step 4 of §21.3: propagate batched writes. Returns widget dirty marks, merged per
    /// widget, in a deterministic order.
    pub fn flush(&mut self) -> Vec<(WidgetId, Dirty)> {
        let mut out: BTreeMap<WidgetId, Dirty> = BTreeMap::new();
        // Work queue ordered by (height, idx): a memo runs after every dependency.
        let mut queue: BTreeMap<(u32, u32), ()> = BTreeMap::new();
        let mut effects: Vec<u32> = Vec::new();
        for idx in std::mem::take(&mut self.pending) {
            let h = self.slots[idx as usize].height.get();
            queue.insert((h, idx), ());
        }
        while let Some(((_, idx), ())) = queue.pop_first() {
            // A memo is recomputed only now, after every dependency (lower height) has
            // settled; if its value did not change, nothing downstream is touched.
            if matches!(self.slots[idx as usize].kind, Kind::Memo { .. }) && !self.recompute(idx) {
                continue;
            }
            let subs = self.slots[idx as usize].subs.borrow().clone();
            for sub in subs {
                match sub {
                    Sub::Widget(w, d) => {
                        *out.entry(w).or_insert(Dirty::NONE) |= d;
                    }
                    Sub::Node(n) => {
                        let ns = &self.slots[n as usize];
                        if !ns.live {
                            continue;
                        }
                        match &ns.kind {
                            Kind::Memo { stale, .. } => {
                                if ns.subs.borrow().is_empty() {
                                    // Nobody watches: recompute lazily on next read.
                                    stale.set(true);
                                } else {
                                    queue.insert((ns.height.get(), n), ());
                                }
                            }
                            Kind::Effect { .. } => {
                                if !effects.contains(&n) {
                                    effects.push(n);
                                }
                            }
                            Kind::Signal => {}
                        }
                    }
                }
            }
        }
        for e in effects {
            self.run_effect(e);
        }
        self.bump(|s| s.widget_invalidations += out.len() as u64);
        out.into_iter().collect()
    }
}

impl<T: PartialEq + 'static> Signal<T> {
    /// A clone of the current value. A stale handle yields `T::default()`.
    pub fn get(&self, rt: &Runtime) -> T
    where
        T: Clone + Default,
    {
        rt.read::<T, T>(self.idx, self.epoch, T::clone)
            .unwrap_or_default()
    }

    /// Borrow the current value. `None` for a disposed signal.
    pub fn with<R>(&self, rt: &Runtime, f: impl FnOnce(&T) -> R) -> Option<R> {
        rt.read::<T, R>(self.idx, self.epoch, f)
    }

    /// Set the value. Setting an equal value is a no-op: no invalidation, no damage.
    pub fn set(&self, rt: &mut Runtime, v: T) {
        let Some(slot) = rt.slot(self.idx, self.epoch) else {
            return;
        };
        {
            let mut cur = slot.value.borrow_mut();
            if let Some(old) = cur.as_deref().and_then(|b| b.downcast_ref::<T>())
                && *old == v
            {
                return;
            }
            *cur = Some(Box::new(v));
        }
        if !rt.pending.contains(&self.idx) {
            rt.pending.push(self.idx);
        }
    }

    /// Mutate in place; always counts as a change.
    pub fn update(&self, rt: &mut Runtime, f: impl FnOnce(&mut T)) {
        let Some(slot) = rt.slot(self.idx, self.epoch) else {
            return;
        };
        if let Some(v) = slot
            .value
            .borrow_mut()
            .as_deref_mut()
            .and_then(|b| b.downcast_mut::<T>())
        {
            f(v);
        }
        if !rt.pending.contains(&self.idx) {
            rt.pending.push(self.idx);
        }
    }
}

impl<T: PartialEq + 'static> Memo<T> {
    pub fn get(&self, rt: &Runtime) -> T
    where
        T: Clone + Default,
    {
        rt.read::<T, T>(self.idx, self.epoch, T::clone)
            .unwrap_or_default()
    }
    pub fn with<R>(&self, rt: &Runtime, f: impl FnOnce(&T) -> R) -> Option<R> {
        rt.read::<T, R>(self.idx, self.epoch, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn equal_set_is_a_no_op() {
        let mut rt = Runtime::new();
        let s = rt.signal(3);
        let w = WidgetId(1);
        rt.subscribe(s.any(), w, Dirty::PAINT);
        s.set(&mut rt, 3);
        assert!(rt.flush().is_empty());
        s.set(&mut rt, 4);
        assert_eq!(rt.flush(), vec![(w, Dirty::PAINT)]);
    }

    #[test]
    fn writes_are_batched_and_merged() {
        let mut rt = Runtime::new();
        let a = rt.signal(1);
        let b = rt.signal(1);
        let w = WidgetId(9);
        rt.subscribe(a.any(), w, Dirty::PAINT);
        rt.subscribe(b.any(), w, Dirty::LAYOUT);
        a.set(&mut rt, 2);
        a.set(&mut rt, 3);
        b.set(&mut rt, 2);
        assert_eq!(rt.flush(), vec![(w, Dirty::PAINT | Dirty::LAYOUT)]);
    }

    #[test]
    fn memo_stops_propagation_when_its_value_is_unchanged() {
        let mut rt = Runtime::new();
        let latency_us = rt.signal(12_300u32);
        let shown_ms = rt.memo(move |rt| latency_us.get(rt) / 1000);
        let w = WidgetId(5);
        rt.subscribe(shown_ms.any(), w, Dirty::PAINT);
        latency_us.set(&mut rt, 12_400); // still 12 ms
        assert!(rt.flush().is_empty(), "an invisible change caused damage");
        latency_us.set(&mut rt, 13_050);
        assert_eq!(rt.flush(), vec![(w, Dirty::PAINT)]);
        assert_eq!(shown_ms.get(&rt), 13);
    }

    #[test]
    fn diamond_memo_runs_once_in_dependency_order() {
        let mut rt = Runtime::new();
        let s = rt.signal(1i64);
        let a = rt.memo(move |rt| s.get(rt) + 1);
        let b = rt.memo(move |rt| s.get(rt) * 10);
        let runs = Rc::new(Cell::new(0));
        let r2 = runs.clone();
        let c = rt.memo(move |rt| {
            r2.set(r2.get() + 1);
            a.get(rt) + b.get(rt)
        });
        rt.subscribe(c.any(), WidgetId(1), Dirty::PAINT);
        runs.set(0);
        s.set(&mut rt, 2);
        rt.flush();
        assert_eq!(runs.get(), 1);
        assert_eq!(c.get(&rt), 3 + 20);
    }

    #[test]
    fn effects_run_after_flush() {
        let mut rt = Runtime::new();
        let s = rt.signal(String::from("a"));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s2 = seen.clone();
        rt.effect(move |rt| s2.borrow_mut().push(s.get(rt)));
        s.set(&mut rt, "b".into());
        rt.flush();
        assert_eq!(*seen.borrow(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn disposed_handles_do_not_alias() {
        let mut rt = Runtime::new();
        let s = rt.signal(1u8);
        rt.dispose(s.any());
        let t = rt.signal(7u8);
        assert_eq!(s.with(&rt, |v| *v), None);
        assert_eq!(t.get(&rt), 7);
    }
}
