//! The loader's worker pool: a few threads draining one job queue. Jobs fetch artefact bytes
//! (blob store or generator), decode them, and publish into their slot. A panicking decoder
//! fails its slot (`ASSET-0009`) and the worker lives on.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub(crate) type Job = Box<dyn FnOnce() + Send + 'static>;

pub(crate) struct Pool {
    tx: Option<Sender<Job>>,
    workers: Vec<JoinHandle<()>>,
}

impl Pool {
    pub fn new(threads: usize) -> Self {
        let (tx, rx) = channel::<Job>();
        let rx: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(rx));
        let workers = (0..threads.max(1))
            .filter_map(|i| {
                let rx = Arc::clone(&rx);
                std::thread::Builder::new()
                    .name(format!("forge-asset-{i}"))
                    .spawn(move || {
                        loop {
                            let job = {
                                let guard =
                                    rx.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                guard.recv()
                            };
                            match job {
                                // A job publishes its own failure; a panic that escapes it
                                // must not take the worker down with it.
                                Ok(job) => {
                                    let _ = catch_unwind(AssertUnwindSafe(job));
                                }
                                Err(_) => break, // the pool was dropped
                            }
                        }
                    })
                    .ok()
            })
            .collect();
        Self {
            tx: Some(tx),
            workers,
        }
    }

    /// Queue a job; runs inline if no worker thread could be started.
    pub fn spawn(&self, job: Job) {
        if self.workers.is_empty() {
            job();
            return;
        }
        if let Some(tx) = &self.tx
            && let Err(e) = tx.send(job)
        {
            // Only possible while dropping; run it here rather than lose a slot's result.
            (e.0)();
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.tx = None;
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}
