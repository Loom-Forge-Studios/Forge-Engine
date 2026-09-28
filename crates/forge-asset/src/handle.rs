//! Typed, asynchronous asset handles.
//!
//! [`crate::AssetServer::load`] returns a [`Handle<T>`] at once; the artefact is fetched from
//! the blob store and decoded on the loader's worker threads. A handle is cheap to clone
//! (one `Arc`), and every handle to one asset shares one slot, so an asset is decoded once
//! however many systems hold it.
//!
//! **Hot reload swaps the value under the handle**: holders call [`Handle::get`] and see the
//! new data on the next call, with [`Handle::generation`] bumped. While a reload decodes,
//! `get` keeps returning the previous value — a texture never flickers to "missing".
//!
//! Waiting: [`Handle::wait`] blocks with a timeout (tools, tests); [`Handle::ready`] is a
//! `Future` for async callers; [`Handle::state`] polls without blocking (the editor's frame
//! loop).

use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::{AssetError, AssetId};

pub(crate) type AnyValue = Arc<dyn Any + Send + Sync>;

#[derive(Default)]
pub(crate) struct SlotState {
    pub value: Option<AnyValue>,
    pub error: Option<AssetError>,
    pub generation: u64,
    pub pending: bool,
    wakers: Vec<Waker>,
}

/// One asset's shared load state.
pub(crate) struct Slot {
    pub id: AssetId,
    state: Mutex<SlotState>,
    cv: Condvar,
}

impl Slot {
    pub fn new(id: AssetId) -> Arc<Self> {
        Arc::new(Self {
            id,
            state: Mutex::new(SlotState {
                pending: true,
                ..SlotState::default()
            }),
            cv: Condvar::new(),
        })
    }

    pub fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Mark a (re)load in flight; the current value stays visible.
    pub fn begin(&self) {
        self.lock().pending = true;
    }

    /// Publish a load's result and wake every waiter.
    pub fn finish(&self, r: Result<AnyValue, AssetError>) {
        let wakers = {
            let mut s = self.lock();
            match r {
                Ok(v) => {
                    s.value = Some(v);
                    s.error = None;
                    s.generation += 1;
                }
                Err(e) => s.error = Some(e),
            }
            s.pending = false;
            std::mem::take(&mut s.wakers)
        };
        self.cv.notify_all();
        for w in wakers {
            w.wake();
        }
    }
}

/// Where a load is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    /// Decoding (a first load, or a reload with the previous value still visible).
    Loading,
    /// A value is available and no reload is in flight.
    Loaded,
    /// The last load failed (a previous value may still be visible through `get`).
    Failed(AssetError),
}

/// A typed handle to an asset. `T` is checked against the asset's kind at load.
pub struct Handle<T> {
    pub(crate) slot: Arc<Slot>,
    _t: PhantomData<fn() -> T>,
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self {
            slot: Arc::clone(&self.slot),
            _t: PhantomData,
        }
    }
}

impl<T> std::fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Handle<{}>({})",
            std::any::type_name::<T>(),
            self.slot.id
        )
    }
}

impl<T: Any + Send + Sync> Handle<T> {
    pub(crate) fn new(slot: Arc<Slot>) -> Self {
        Self {
            slot,
            _t: PhantomData,
        }
    }

    /// The asset's id.
    #[must_use]
    pub fn id(&self) -> AssetId {
        self.slot.id
    }

    /// The current value, if one has loaded.
    #[must_use]
    pub fn get(&self) -> Option<Arc<T>> {
        self.slot
            .lock()
            .value
            .clone()
            .and_then(|v| v.downcast::<T>().ok())
    }

    /// How many values have been published (0 until the first load; +1 per reload).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.slot.lock().generation
    }

    /// Where the load is, without blocking.
    #[must_use]
    pub fn state(&self) -> LoadState {
        let s = self.slot.lock();
        if s.pending {
            LoadState::Loading
        } else if let Some(e) = &s.error {
            LoadState::Failed(e.clone())
        } else {
            LoadState::Loaded
        }
    }

    fn result(s: &SlotState) -> Result<Arc<T>, AssetError> {
        if let Some(e) = &s.error {
            return Err(e.clone());
        }
        s.value
            .clone()
            .and_then(|v| v.downcast::<T>().ok())
            .ok_or_else(|| AssetError::Timeout("(no value)".into()))
    }

    /// Block until no load is in flight (at most `timeout`), then the value or the error.
    pub fn wait(&self, timeout: Duration) -> Result<Arc<T>, AssetError> {
        let deadline = Instant::now() + timeout;
        let mut s = self.slot.lock();
        while s.pending {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(AssetError::Timeout(self.slot.id.to_string()));
            }
            s = self
                .slot
                .cv
                .wait_timeout(s, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        Self::result(&s)
    }

    /// A future resolving when no load is in flight.
    #[must_use]
    pub fn ready(&self) -> Ready<T> {
        Ready {
            handle: self.clone(),
        }
    }
}

/// The future of [`Handle::ready`].
pub struct Ready<T> {
    handle: Handle<T>,
}

impl<T: Any + Send + Sync> Future for Ready<T> {
    type Output = Result<Arc<T>, AssetError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut s = self.handle.slot.lock();
        if s.pending {
            if !s.wakers.iter().any(|w| w.will_wake(cx.waker())) {
                s.wakers.push(cx.waker().clone());
            }
            Poll::Pending
        } else {
            Poll::Ready(Handle::<T>::result(&s))
        }
    }
}
