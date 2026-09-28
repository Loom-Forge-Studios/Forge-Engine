//! **Compute and farm** (Ch.21 §21.21, Ch.26; DoD M2-56): the adapters this machine
//! dispatches batch work to, with their **measured** throughput (Ch.26 Tier 0 schedules by
//! it, not by device name), and the LAN farm's nodes — discovery, status and failures
//! (Tier 2) — over the [`ComputePool`] trait.
//!
//! Throughput and node status change by themselves, so the pool is a live source the panel
//! refreshes at most twice a second, only while visible (§21.11).
//!
//! The heterogeneous job scheduler (`forge-jobs`, M6-1) and the farm daemon (`forge-farm`,
//! M6-3) are not built: [`MemoryComputePool`] is the labelled in-memory implementation
//! (D-4). Its adapters can be filled from the real adapter pool's report
//! ([`MemoryComputePool::set_adapters`]); throughput and nodes are what a producer reports.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use forge_ui::{LiveCell, LiveSource, UiWaker};

/// One compute adapter.
#[derive(Clone, Debug, PartialEq)]
pub struct AdapterInfo {
    pub name: String,
    /// `discrete`, `integrated`, `software`, `cpu`.
    pub kind: String,
    /// The backend (`Vulkan`, `D3D12`, `cpu`).
    pub backend: String,
    /// Measured throughput, work items per second over the last window (`None`: not
    /// measured yet — never a guess from the device name).
    pub items_per_s: Option<f64>,
    /// Jobs finished on it.
    pub jobs_done: u64,
    /// Whether it takes batch work (a user may exclude one).
    pub enabled: bool,
}

/// A farm node's status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeStatus {
    /// Seen on the LAN, not yet paired for work.
    Discovered,
    /// Ready for jobs.
    Idle,
    /// Running jobs.
    Busy { jobs: u32 },
    /// Unreachable since the time given (ms since the Unix epoch).
    Lost { since_ms: u64 },
}

impl NodeStatus {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Discovered => forge_ui::tr!("discovered").into(),
            Self::Idle => forge_ui::tr!("idle").into(),
            Self::Busy { jobs } => forge_ui::trf!("busy ({jobs} job(s))", jobs),
            Self::Lost { .. } => forge_ui::tr!("lost").into(),
        }
    }
}

/// A LAN farm node.
#[derive(Clone, Debug, PartialEq)]
pub struct FarmNode {
    pub id: String,
    pub host: String,
    pub status: NodeStatus,
    /// Its adapters' combined measured throughput (items/s).
    pub items_per_s: Option<f64>,
    /// Failures it reported (newest last, at most [`MAX_FAILURES`]).
    pub failures: Vec<String>,
}

/// Failures kept per node.
pub const MAX_FAILURES: usize = 16;

/// A snapshot of the pool.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComputeSnapshot {
    pub adapters: Vec<AdapterInfo>,
    pub nodes: Vec<FarmNode>,
    /// Whether LAN discovery is running.
    pub discovering: bool,
}

/// The compute pool (see the module docs).
pub trait ComputePool: LiveSource {
    /// What it is (a labelled in-memory pool says so).
    fn backend(&self) -> String;
    /// The adapters and farm nodes now.
    fn snapshot(&self) -> ComputeSnapshot;
}

/// The labelled in-memory pool (D-4 until `forge-jobs` / `forge-farm`, M6-1 / M6-3).
#[derive(Default)]
pub struct MemoryComputePool {
    cell: LiveCell,
    state: Mutex<ComputeSnapshot>,
}

impl std::fmt::Debug for MemoryComputePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryComputePool").finish_non_exhaustive()
    }
}

impl MemoryComputePool {
    /// An empty pool with this machine's CPU as its one adapter.
    #[must_use]
    pub fn new() -> Self {
        let p = Self::default();
        let threads = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        p.set_adapters(vec![AdapterInfo {
            name: format!("CPU ({threads} threads)"),
            kind: "cpu".into(),
            backend: "cpu".into(),
            items_per_s: None,
            jobs_done: 0,
            enabled: true,
        }]);
        p
    }

    fn lock(&self) -> MutexGuard<'_, ComputeSnapshot> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replace the adapter list (the binary fills it from `forge_gpu::AdapterPool::report`).
    pub fn set_adapters(&self, adapters: Vec<AdapterInfo>) {
        self.lock().adapters = adapters;
        self.cell.bump();
    }

    /// Producer side: `items` finished on adapter `name` in `secs` (one job).
    pub fn report_job(&self, name: &str, items: u64, secs: f64) {
        {
            let mut s = self.lock();
            if let Some(a) = s.adapters.iter_mut().find(|a| a.name == name)
                && secs > 0.0
            {
                a.items_per_s = Some(items as f64 / secs);
                a.jobs_done += 1;
            }
        }
        self.cell.set_live(true);
        self.cell.bump();
    }

    /// Producer side: discovery found or updated a node.
    pub fn node(&self, id: &str, host: &str, status: NodeStatus, items_per_s: Option<f64>) {
        {
            let mut s = self.lock();
            match s.nodes.iter_mut().find(|n| n.id == id) {
                Some(n) => {
                    n.host = host.to_string();
                    n.status = status;
                    n.items_per_s = items_per_s;
                }
                None => s.nodes.push(FarmNode {
                    id: id.to_string(),
                    host: host.to_string(),
                    status,
                    items_per_s,
                    failures: Vec::new(),
                }),
            }
        }
        self.cell.bump();
    }

    /// Producer side: a node reported a failure.
    pub fn failure(&self, id: &str, what: &str) {
        {
            let mut s = self.lock();
            if let Some(n) = s.nodes.iter_mut().find(|n| n.id == id) {
                n.failures.push(what.to_string());
                if n.failures.len() > MAX_FAILURES {
                    n.failures.remove(0);
                }
            }
        }
        self.cell.bump();
    }

    /// Producer side: discovery on or off.
    pub fn set_discovering(&self, on: bool) {
        self.lock().discovering = on;
        self.cell.set_live(on);
        self.cell.bump();
    }

    /// Producer side: the pool stopped producing (nothing running).
    pub fn set_idle(&self) {
        self.cell.set_live(false);
    }
}

impl LiveSource for MemoryComputePool {
    fn generation(&self) -> u64 {
        self.cell.generation()
    }
    fn is_live(&self) -> bool {
        self.cell.is_live()
    }
    fn set_waker(&self, waker: Option<Arc<dyn UiWaker>>) {
        self.cell.set_waker(waker);
    }
    fn consumed(&self) {
        self.cell.consumed();
    }
}

impl ComputePool for MemoryComputePool {
    fn backend(&self) -> String {
        "in-memory pool (D-4: the job scheduler is forge-jobs, M6-1, and the LAN farm forge-farm, M6-3; both UNBUILT)".into()
    }
    fn snapshot(&self) -> ComputeSnapshot {
        self.lock().clone()
    }
}
