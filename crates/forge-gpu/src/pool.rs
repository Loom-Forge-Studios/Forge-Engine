//! The adapter pool (Ch.9): every adapter enumerated, the O-8 floor applied, one physical
//! GPU counted once, a device and queue per selected adapter — one adapter in
//! [`GpuMode::Single`] (the default, ADR 0054), every one in [`GpuMode::Multi`].
//!
//! **A pool, not an adapter.** [`AdapterPool::devices`] is a `Vec` from day one even when its
//! length is 1, because retrofitting plurality is the expensive version. Index 0 is the
//! **primary** (the best hardware adapter): UI and the viewport render there (Ch.21 §21.13);
//! Tier-0 compute (Ch.26) may use any member.
//!
//! Selection ([`plan_selection`]) is a pure function of the adapters' [`AdapterFacts`], so it
//! is tested on synthetic machines (two identical cards, a mismatched second card, a
//! software-only box) as well as on the real one.

use std::sync::{Arc, Mutex};

use crate::error::GpuError;
use crate::floor::{AdapterFacts, FloorMiss, below_floor_message, check_floor};
use crate::probe;

/// Whether software rasterisers (WARP, lavapipe/llvmpipe) join the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SoftwarePolicy {
    /// Never.
    Exclude,
    /// Only when no hardware adapter meets the floor (a GPU-less CI runner still renders).
    #[default]
    FallbackOnly,
    /// Always, after the hardware adapters (tests that need a second, mismatched adapter).
    Include,
    /// Only software adapters (reproducible pixel output on any machine).
    Only,
}

/// How many GPUs the pool opens a device on (owner decision 2026-09-25, ADR 0054).
///
/// The setting lives in the editor's preferences (Graphics) and in the project's Graphics
/// settings for an exported game; the pool is built at startup, so a change applies at the
/// next start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum GpuMode {
    /// One device, on the best adapter by [`plan_selection_in`]'s order (the primary). The
    /// default: every other qualifying adapter is reported as left out by the GPU mode.
    #[default]
    Single,
    /// A device on every selected adapter ([`PoolOptions::max_devices`] still caps it).
    /// **Experimental:** moving work between GPUs is unmeasured on two physical adapters
    /// (DoD M1-2, ADR 0020; spike S1 is AWAITING a second physical adapter).
    Multi,
}

impl GpuMode {
    /// Both modes, default first.
    pub const ALL: [GpuMode; 2] = [GpuMode::Single, GpuMode::Multi];

    /// The stable id settings files and `forge.json` store: `"single"` / `"multi"`.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            GpuMode::Single => "single",
            GpuMode::Multi => "multi",
        }
    }

    /// Parse an id (case-insensitive: `"Single"` and `"single"` both work).
    #[must_use]
    pub fn from_id(id: &str) -> Option<GpuMode> {
        Self::ALL
            .into_iter()
            .find(|m| m.id().eq_ignore_ascii_case(id.trim()))
    }
}

/// How to build a pool.
#[derive(Clone, Debug)]
pub struct PoolOptions {
    /// Backends to enumerate. `None`: `WGPU_BACKEND` from the environment, else Vulkan,
    /// Direct3D 12 and Metal. OpenGL is not enumerated by default: it is below the floor
    /// anyway, and creating a GL context on Windows opens a hidden window.
    pub backends: Option<wgpu::Backends>,
    /// Software rasterisers.
    pub software: SoftwarePolicy,
    /// Optional device features to enable where an adapter has them (never required).
    pub wanted_features: wgpu::Features,
    /// At most this many devices (`None`: every selected adapter).
    pub max_devices: Option<usize>,
    /// One GPU (the default) or every GPU: [`GpuMode::Single`] opens exactly one device.
    pub mode: GpuMode,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            backends: None,
            software: SoftwarePolicy::FallbackOnly,
            wanted_features: wgpu::Features::empty(),
            max_devices: None,
            mode: GpuMode::Single,
        }
    }
}

/// What the pool decided for one enumerated adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterVerdict {
    /// In the pool at this device index.
    Selected(usize),
    /// The same physical GPU as a selected adapter, on another backend.
    Duplicate {
        /// The selected adapter's label.
        of: String,
    },
    /// Below the O-8 floor.
    BelowFloor(FloorMiss),
    /// A software rasteriser left out by [`SoftwarePolicy`] (or hardware left out by `Only`).
    NotRequested,
    /// Left out by [`PoolOptions::max_devices`].
    Capped,
    /// Qualifies, but [`GpuMode::Single`] opens only one device.
    LeftOutByGpuMode,
    /// Selected, but creating its device failed.
    DeviceFailed(String),
}

impl AdapterVerdict {
    /// Why the adapter is or is not in the pool, in words (the adapter report a person
    /// reads; an adapter the mode left out says `GPU mode: Single`).
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            AdapterVerdict::Selected(i) => format!("in the pool (device {i})"),
            AdapterVerdict::Duplicate { of } => format!("the same GPU as {of}"),
            AdapterVerdict::BelowFloor(m) => format!("below the minimum: {m:?}"),
            AdapterVerdict::NotRequested => "left out by the software-adapter setting".into(),
            AdapterVerdict::Capped => "left out by the device cap".into(),
            AdapterVerdict::LeftOutByGpuMode => {
                "GPU mode: Single (one GPU is used; GPU mode Multi uses every GPU)".into()
            }
            AdapterVerdict::DeviceFailed(why) => format!("its device failed to open: {why}"),
        }
    }
}

/// One row of the pool's report: every adapter the driver listed and what happened to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterReport {
    /// The adapter.
    pub facts: AdapterFacts,
    /// The decision.
    pub verdict: AdapterVerdict,
}

/// A selected adapter with its device and queue.
pub struct GpuDevice {
    /// Position in the pool (0 = primary).
    pub index: usize,
    /// What the probe read.
    pub facts: AdapterFacts,
    /// The wgpu adapter.
    pub adapter: wgpu::Adapter,
    /// The device.
    pub device: wgpu::Device,
    /// Its queue.
    pub queue: wgpu::Queue,
    errors: ErrorSink,
}

impl GpuDevice {
    /// wgpu errors no error scope caught, oldest first, drained. wgpu's default would panic
    /// the process; the pool records them instead so a bad frame is a report, not a crash.
    pub fn take_uncaptured_errors(&self) -> Vec<String> {
        match self.errors.lock() {
            Ok(mut g) => std::mem::take(&mut *g),
            Err(p) => std::mem::take(&mut *p.into_inner()),
        }
    }

    /// `"NVIDIA GeForce RTX 3080 (Vulkan 1.4)"`.
    #[must_use]
    pub fn label(&self) -> String {
        self.facts.label()
    }

    /// Wait until all submitted work on this device has finished.
    pub fn wait_idle(&self) -> Result<(), GpuError> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map(|_| ())
            .map_err(GpuError::transfer)
    }
}

impl std::fmt::Debug for GpuDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuDevice")
            .field("index", &self.index)
            .field("facts", &self.facts)
            .finish_non_exhaustive()
    }
}

/// Every usable adapter, each with a device.
pub struct AdapterPool {
    instance: wgpu::Instance,
    devices: Vec<GpuDevice>,
    report: Vec<AdapterReport>,
    mode: GpuMode,
    /// Test builds only (feature `test-gpu-turn`, enabled by the dev-dependencies of the
    /// crates with GPU tests): this process's shared GPU turn, so no timed body measures
    /// while this pool's devices exist (`forge_trace::timed`). Declared last, so it is
    /// released after the devices.
    #[cfg(feature = "test-gpu-turn")]
    _gpu_turn: forge_trace::timed::GpuTurn,
}

impl std::fmt::Debug for AdapterPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterPool")
            .field("devices", &self.devices)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

/// The pure part of selection: one entry per input adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Use it; `rank` orders the pool (0 = primary).
    Use {
        /// Pool position before any device fails.
        rank: usize,
    },
    /// Same physical GPU as input adapter `of`, which is used.
    Duplicate {
        /// Index into the input of the adapter that is used instead.
        of: usize,
    },
    /// Below the floor.
    BelowFloor(FloorMiss),
    /// Excluded by the software policy.
    NotRequested,
    /// Beyond `max_devices`.
    Capped,
    /// Qualifies, but [`GpuMode::Single`] uses one device. Next in line, by `rank`, if the
    /// adapters before it fail to open a device.
    SingleMode {
        /// Pool position it would have in [`GpuMode::Multi`].
        rank: usize,
    },
}

/// Physical identity: (name when ids are unknown, vendor, device, type rank).
type PhysKey = (String, u32, u32, u8);

/// Where a device's uncaptured wgpu errors are recorded.
type ErrorSink = Arc<Mutex<Vec<String>>>;

fn type_rank(t: wgpu::DeviceType) -> u8 {
    match t {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

/// Backend preference when one physical GPU is exposed on several: Vulkan first on every
/// platform (one backend's behaviour on both CI legs, I18, and the only backend with an
/// external-memory path for spike S1), then Direct3D 12, then Metal. ADR 0019.
fn backend_rank(b: wgpu::Backend) -> u8 {
    match b {
        wgpu::Backend::Vulkan => 0,
        wgpu::Backend::Dx12 => 1,
        wgpu::Backend::Metal => 2,
        _ => 3,
    }
}

/// [`plan_selection_in`] for [`GpuMode::Multi`]: every qualifying adapter is used.
#[must_use]
pub fn plan_selection(
    facts: &[AdapterFacts],
    policy: SoftwarePolicy,
    max_devices: Option<usize>,
) -> Vec<Choice> {
    plan_selection_in(facts, policy, max_devices, GpuMode::Multi)
}

/// Decide, for each adapter, whether it joins the pool. Pure: no driver calls.
///
/// * The floor (O-8) is applied first.
/// * Adapters of the same physical GPU (same vendor id, device id and type) enumerated on
///   several backends are counted once: the backend with the **most** qualifying adapters
///   in that group wins (two identical cards stay two), ties go to [`backend_rank`].
/// * Software adapters follow `policy`.
/// * Order: discrete, integrated, virtual, other, software; enumeration order within a type.
/// * `max_devices` caps the pool; [`GpuMode::Single`] then uses only rank 0 and marks the
///   rest [`Choice::SingleMode`] (next in line if rank 0's device fails to open).
#[must_use]
pub fn plan_selection_in(
    facts: &[AdapterFacts],
    policy: SoftwarePolicy,
    max_devices: Option<usize>,
    mode: GpuMode,
) -> Vec<Choice> {
    let mut out: Vec<Option<Choice>> = vec![None; facts.len()];
    let mut eligible = Vec::new();
    for (i, f) in facts.iter().enumerate() {
        match check_floor(f) {
            Ok(()) => eligible.push(i),
            Err(m) => out[i] = Some(Choice::BelowFloor(m)),
        }
    }
    // Group by physical identity.
    // PCI ids and type identify the part; backends can spell the name differently, so the
    // name only counts when the ids are unknown (0).
    let key = |f: &AdapterFacts| {
        let name = if f.vendor == 0 && f.device == 0 {
            f.name.clone()
        } else {
            String::new()
        };
        (name, f.vendor, f.device, type_rank(f.device_type))
    };
    let mut groups: Vec<(PhysKey, Vec<usize>)> = Vec::new();
    for &i in &eligible {
        let k = key(&facts[i]);
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, v)) => v.push(i),
            None => groups.push((k, vec![i])),
        }
    }
    let mut kept = Vec::new();
    for (_, members) in &groups {
        let mut backends: Vec<wgpu::Backend> = Vec::new();
        for &i in members {
            if !backends.contains(&facts[i].backend) {
                backends.push(facts[i].backend);
            }
        }
        let count = |b: wgpu::Backend| members.iter().filter(|&&i| facts[i].backend == b).count();
        let best = backends
            .iter()
            .copied()
            .min_by_key(|&b| (std::cmp::Reverse(count(b)), backend_rank(b)));
        let Some(best) = best else { continue };
        let chosen: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&i| facts[i].backend == best)
            .collect();
        for &i in members {
            if facts[i].backend != best {
                // Pair the n-th duplicate with the n-th chosen adapter of the same group.
                let nth = members
                    .iter()
                    .filter(|&&j| facts[j].backend == facts[i].backend)
                    .position(|&j| j == i)
                    .unwrap_or(0);
                let of = chosen.get(nth).or(chosen.first()).copied().unwrap_or(i);
                out[i] = Some(Choice::Duplicate { of });
            }
        }
        kept.extend(chosen);
    }
    let any_hw = kept.iter().any(|&i| !facts[i].is_software());
    kept.retain(|&i| {
        let sw = facts[i].is_software();
        let keep = match policy {
            SoftwarePolicy::Exclude => !sw,
            SoftwarePolicy::FallbackOnly => !sw || !any_hw,
            SoftwarePolicy::Include => true,
            SoftwarePolicy::Only => sw,
        };
        if !keep {
            out[i] = Some(Choice::NotRequested);
        }
        keep
    });
    kept.sort_by_key(|&i| (type_rank(facts[i].device_type), i));
    for (rank, &i) in kept.iter().enumerate() {
        out[i] = Some(if max_devices.is_some_and(|m| rank >= m) {
            Choice::Capped
        } else if mode == GpuMode::Single && rank > 0 {
            Choice::SingleMode { rank }
        } else {
            Choice::Use { rank }
        });
    }
    out.into_iter()
        .map(|c| c.unwrap_or(Choice::NotRequested))
        .collect()
}

impl AdapterPool {
    /// Enumerate every adapter, apply the floor and `opts`, and create a device per selected
    /// adapter. Fails with a user-facing `GPU-0002` message when nothing qualifies.
    pub fn new(opts: &PoolOptions) -> Result<Self, GpuError> {
        #[cfg(feature = "test-gpu-turn")]
        let gpu_turn = forge_trace::timed::gpu_turn().map_err(|why| GpuError::Validation {
            what: "taking the test GPU turn (forge_trace::timed::gpu_turn)".into(),
            why,
        })?;
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = opts
            .backends
            .unwrap_or(wgpu::Backends::VULKAN | wgpu::Backends::DX12 | wgpu::Backends::METAL);
        // `WGPU_BACKEND` / `WGPU_DEBUG` etc. override, as everywhere else wgpu is used.
        let desc = if opts.backends.is_some() {
            desc
        } else {
            desc.with_env()
        };
        let backends = format!("{:?}", desc.backends);
        let instance = wgpu::Instance::new(desc);
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        if adapters.is_empty() {
            return Err(GpuError::NoAdapter { backends });
        }
        let facts: Vec<AdapterFacts> = adapters.iter().map(probe::facts).collect();
        let plan = plan_selection_in(&facts, opts.software, opts.max_devices, opts.mode);
        let (opened, mut verdicts) = open_in_order(&plan, opts.mode, |i| {
            open_device(&adapters[i], opts.wanted_features)
        });
        let first_failure = verdicts.iter().enumerate().find_map(|(i, v)| match v {
            Some(AdapterVerdict::DeviceFailed(why)) => Some(GpuError::RequestDevice {
                adapter: facts[i].label(),
                why: why.clone(),
            }),
            _ => None,
        });
        let devices: Vec<GpuDevice> = opened
            .into_iter()
            .enumerate()
            .map(|(index, (i, (device, queue, errors)))| GpuDevice {
                index,
                facts: facts[i].clone(),
                adapter: adapters[i].clone(),
                device,
                queue,
                errors,
            })
            .collect();
        let report: Vec<AdapterReport> = facts
            .iter()
            .zip(plan)
            .enumerate()
            .map(|(i, (f, c))| {
                let verdict = verdicts[i].take().unwrap_or_else(|| match c {
                    Choice::Duplicate { of } => AdapterVerdict::Duplicate {
                        of: facts[of].label(),
                    },
                    Choice::BelowFloor(m) => AdapterVerdict::BelowFloor(m),
                    Choice::Capped => AdapterVerdict::Capped,
                    Choice::SingleMode { .. } => AdapterVerdict::LeftOutByGpuMode,
                    Choice::NotRequested | Choice::Use { .. } => AdapterVerdict::NotRequested,
                });
                AdapterReport {
                    facts: f.clone(),
                    verdict,
                }
            })
            .collect();
        if devices.is_empty() {
            if let Some(e) = first_failure {
                return Err(e);
            }
            return Err(GpuError::BelowFloor {
                message: nothing_qualifies_message(&report, opts.software),
            });
        }
        Ok(Self {
            instance,
            devices,
            report,
            mode: opts.mode,
            #[cfg(feature = "test-gpu-turn")]
            _gpu_turn: gpu_turn,
        })
    }

    /// The instance every device came from (create window surfaces with it).
    #[must_use]
    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// The primary device: the best hardware adapter (UI, viewport).
    #[must_use]
    pub fn primary(&self) -> &GpuDevice {
        // A pool is never empty: `new` fails instead of building one.
        &self.devices[0]
    }

    /// Every device, primary first.
    #[must_use]
    pub fn devices(&self) -> &[GpuDevice] {
        &self.devices
    }

    /// Number of devices (at least 1).
    #[must_use]
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// Always false: a pool holds at least one device.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// The GPU mode the pool was built with (it applies until the next start).
    #[must_use]
    pub fn mode(&self) -> GpuMode {
        self.mode
    }

    /// Every adapter the driver listed, with the decision taken for it.
    #[must_use]
    pub fn report(&self) -> &[AdapterReport] {
        &self.report
    }

    /// The first device (primary preferred) whose adapter can present to `surface`.
    pub fn device_for_surface(&self, surface: &wgpu::Surface<'_>) -> Result<&GpuDevice, GpuError> {
        self.devices
            .iter()
            .find(|d| d.adapter.is_surface_supported(surface))
            .ok_or(GpuError::NoPresentableAdapter)
    }
}

/// Open devices in pool order: every [`Choice::Use`] in [`GpuMode::Multi`]; in
/// [`GpuMode::Single`] the first candidate (`Use`, then `SingleMode` by rank) whose device
/// opens, and no other. Returns `(input index, opened)` in pool order and a verdict for every
/// adapter that was tried (`Selected(pool index)` or `DeviceFailed`). Pure apart from `open`,
/// so the Single/Multi rule is tested without a driver.
pub(crate) fn open_in_order<T>(
    plan: &[Choice],
    mode: GpuMode,
    mut open: impl FnMut(usize) -> Result<T, String>,
) -> (Vec<(usize, T)>, Vec<Option<AdapterVerdict>>) {
    let mut order: Vec<(usize, usize)> = plan
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match (c, mode) {
            (Choice::Use { rank }, _) | (Choice::SingleMode { rank }, GpuMode::Single) => {
                Some((*rank, i))
            }
            _ => None,
        })
        .collect();
    order.sort_unstable();
    let mut verdicts: Vec<Option<AdapterVerdict>> = vec![None; plan.len()];
    let mut opened = Vec::new();
    for (_, i) in order {
        if mode == GpuMode::Single && !opened.is_empty() {
            break;
        }
        match open(i) {
            Ok(t) => {
                verdicts[i] = Some(AdapterVerdict::Selected(opened.len()));
                opened.push((i, t));
            }
            Err(why) => verdicts[i] = Some(AdapterVerdict::DeviceFailed(why)),
        }
    }
    (opened, verdicts)
}

fn open_device(
    adapter: &wgpu::Adapter,
    wanted: wgpu::Features,
) -> Result<(wgpu::Device, wgpu::Queue, ErrorSink), String> {
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("forge-gpu"),
        required_features: adapter.features() & wanted,
        required_limits: adapter.limits(),
        ..wgpu::DeviceDescriptor::default()
    }))
    .map_err(|e| e.to_string())?;
    let errors: ErrorSink = Arc::default();
    let sink = errors.clone();
    device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
        let mut g = match sink.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // Bounded: a runaway error per frame must not grow memory without limit.
        if g.len() < 1024 {
            g.push(e.to_string());
        }
    }));
    Ok((device, queue, errors))
}

fn nothing_qualifies_message(report: &[AdapterReport], policy: SoftwarePolicy) -> String {
    let misses: Vec<FloorMiss> = report
        .iter()
        .filter_map(|r| match &r.verdict {
            AdapterVerdict::BelowFloor(m) => Some(m.clone()),
            _ => None,
        })
        .collect();
    let excluded: Vec<String> = report
        .iter()
        .filter(|r| r.verdict == AdapterVerdict::NotRequested)
        .map(|r| r.facts.label())
        .collect();
    let mut s = below_floor_message(&misses);
    if !excluded.is_empty() {
        s.push_str(&format!(
            "\nAlso found, but excluded by the software-adapter setting ({policy:?}): {}",
            excluded.join(", ")
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two adapters that both qualify: the plan every Single/Multi test below starts from.
    const TWO: [Choice; 2] = [Choice::Use { rank: 0 }, Choice::SingleMode { rank: 1 }];
    const TWO_MULTI: [Choice; 2] = [Choice::Use { rank: 0 }, Choice::Use { rank: 1 }];

    /// The Single guard: exactly one device opened and every other candidate reported as
    /// left out (by the mode, or because its device failed first).
    fn single_guard(opened: usize, verdicts: &[Option<AdapterVerdict>]) -> Result<(), String> {
        if opened != 1 {
            return Err(format!("GPU mode Single opened {opened} devices"));
        }
        let selected = verdicts
            .iter()
            .filter(|v| matches!(v, Some(AdapterVerdict::Selected(_))))
            .count();
        if selected != 1 {
            return Err(format!("{selected} adapters were selected"));
        }
        Ok(())
    }

    #[test]
    fn the_default_gpu_mode_is_single() {
        assert_eq!(GpuMode::default(), GpuMode::Single);
        assert_eq!(PoolOptions::default().mode, GpuMode::Single);
        assert_eq!(GpuMode::from_id("Multi"), Some(GpuMode::Multi));
        assert_eq!(GpuMode::from_id(" single "), Some(GpuMode::Single));
        assert_eq!(GpuMode::from_id("both"), None);
        for m in GpuMode::ALL {
            assert_eq!(GpuMode::from_id(m.id()), Some(m));
        }
    }

    #[test]
    fn single_opens_one_device_of_two_and_multi_opens_both() {
        let (one, v) = open_in_order(&TWO, GpuMode::Single, Ok::<_, String>);
        assert_eq!(one, vec![(0, 0)]);
        assert_eq!(v, vec![Some(AdapterVerdict::Selected(0)), None]);
        assert!(single_guard(one.len(), &v).is_ok());

        let (both, v) = open_in_order(&TWO_MULTI, GpuMode::Multi, Ok::<_, String>);
        assert_eq!(both, vec![(0, 0), (1, 1)]);
        // Positive control: Multi forced, the Single guard fails.
        assert!(single_guard(both.len(), &v).is_err());
    }

    #[test]
    fn single_falls_to_the_next_adapter_when_the_first_device_fails() {
        let (one, v) = open_in_order(&TWO, GpuMode::Single, |i| {
            if i == 0 {
                Err("lost".to_string())
            } else {
                Ok(i)
            }
        });
        assert_eq!(one, vec![(1, 1)]);
        assert_eq!(
            v,
            vec![
                Some(AdapterVerdict::DeviceFailed("lost".into())),
                Some(AdapterVerdict::Selected(0))
            ]
        );
        assert!(single_guard(one.len(), &v).is_ok());
        // A SingleMode choice is never opened in Multi (the plan would say Use there).
        let (none_extra, _) = open_in_order(&TWO, GpuMode::Multi, Ok::<_, String>);
        assert_eq!(none_extra, vec![(0, 0)]);
    }

    #[test]
    fn the_report_says_gpu_mode_single() {
        assert!(
            AdapterVerdict::LeftOutByGpuMode
                .reason()
                .starts_with("GPU mode: Single")
        );
    }
}
