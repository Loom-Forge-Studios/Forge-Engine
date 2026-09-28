//! Executing a compiled render graph on one device: physical resources from a
//! [`TransientPool`] (reused across frames, so a steady-state frame allocates nothing),
//! imports bound by the caller, one command encoder, one submit, and every wgpu validation
//! error of the frame caught and returned instead of panicking.

use std::collections::BTreeMap;

use crate::error::GpuError;
use crate::graph::{CompiledGraph, Handle, ResourceKind, TextureDesc, Use};
use crate::pool::GpuDevice;

/// A pass body.
pub type PassFn = Box<dyn FnOnce(&mut PassContext<'_>) -> Result<(), GpuError>>;

/// A pass body that can run every time a retained [`CompiledGraph`] executes
/// ([`crate::PassBuilder::run_retained`], [`CompiledGraph::execute_retained`]). It reads its
/// per-frame inputs from state it shares with the caller instead of capturing them.
pub type RetainedPassFn = Box<dyn FnMut(&mut PassContext<'_>) -> Result<(), GpuError>>;

/// How a pass body may be run.
pub(crate) enum PassBody {
    /// Once, then [`PassBody::Spent`].
    Once(PassFn),
    /// Every execution.
    Retained(RetainedPassFn),
    /// A single-use body that has run.
    Spent,
}

enum Phys<'a> {
    Texture(&'a wgpu::Texture, &'a wgpu::TextureView),
    Buffer(&'a wgpu::Buffer),
}

/// What a pass body sees: the encoder, the device, and **only the resources it declared**.
pub struct PassContext<'a> {
    /// The frame's command encoder (open render/compute passes on it).
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The device.
    pub device: &'a wgpu::Device,
    /// The queue (for `write_buffer` staging before the submit).
    pub queue: &'a wgpu::Queue,
    name: &'a str,
    uses: &'a [Use],
    phys: &'a [Option<Phys<'a>>],
    resources: &'a [crate::graph::ResourceInfo],
}

impl PassContext<'_> {
    fn declared(&self, h: Handle) -> Result<&Use, GpuError> {
        self.uses
            .iter()
            .find(|u| u.res == h.resource())
            .ok_or_else(|| {
                GpuError::graph(format!(
                    "pass `{}` touched `{}` without declaring it",
                    self.name,
                    self.resources
                        .get(h.resource().0 as usize)
                        .map_or("?", |r| r.name.as_str())
                ))
            })
    }

    /// The pass's name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name
    }

    /// A declared texture.
    pub fn texture(&self, h: Handle) -> Result<&wgpu::Texture, GpuError> {
        self.declared(h)?;
        match self.phys.get(h.resource().0 as usize) {
            Some(Some(Phys::Texture(t, _))) => Ok(t),
            _ => Err(GpuError::graph(format!(
                "pass `{}`: {h:?} is not a texture",
                self.name
            ))),
        }
    }

    /// A declared texture's default view.
    pub fn view(&self, h: Handle) -> Result<&wgpu::TextureView, GpuError> {
        self.declared(h)?;
        match self.phys.get(h.resource().0 as usize) {
            Some(Some(Phys::Texture(_, v))) => Ok(v),
            _ => Err(GpuError::graph(format!(
                "pass `{}`: {h:?} is not a texture",
                self.name
            ))),
        }
    }

    /// A declared buffer.
    pub fn buffer(&self, h: Handle) -> Result<&wgpu::Buffer, GpuError> {
        self.declared(h)?;
        match self.phys.get(h.resource().0 as usize) {
            Some(Some(Phys::Buffer(b))) => Ok(b),
            _ => Err(GpuError::graph(format!(
                "pass `{}`: {h:?} is not a buffer",
                self.name
            ))),
        }
    }

    /// The load op for a declared colour target: `Load` when the pass `modify`s it, `Clear`
    /// when it `write`s it (contents discarded — required for an aliased allocation, whose
    /// previous resident's bytes are garbage to this one).
    pub fn color_load(
        &self,
        h: Handle,
        clear: wgpu::Color,
    ) -> Result<wgpu::LoadOp<wgpu::Color>, GpuError> {
        let u = self.declared(h)?;
        Ok(if u.reads.is_some() {
            wgpu::LoadOp::Load
        } else {
            wgpu::LoadOp::Clear(clear)
        })
    }
}

/// The caller's resources for a graph's imports.
#[derive(Default)]
pub struct Imports {
    textures: BTreeMap<u32, (wgpu::Texture, wgpu::TextureView)>,
    buffers: BTreeMap<u32, wgpu::Buffer>,
}

impl Imports {
    /// Nothing bound.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind an imported texture.
    pub fn texture(&mut self, h: Handle, t: &wgpu::Texture) -> &mut Self {
        let v = t.create_view(&wgpu::TextureViewDescriptor::default());
        self.textures.insert(h.resource().0, (t.clone(), v));
        self
    }

    /// Bind an imported buffer.
    pub fn buffer(&mut self, h: Handle, b: &wgpu::Buffer) -> &mut Self {
        self.buffers.insert(h.resource().0, b.clone());
        self
    }
}

struct PooledTexture {
    desc: TextureDesc,
    usage: wgpu::TextureUsages,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    last_frame: u64,
}

struct PooledBuffer {
    size: u64,
    usage: wgpu::BufferUsages,
    buffer: wgpu::Buffer,
    last_frame: u64,
}

/// Physical transient resources kept across frames. A frame reuses a free allocation with
/// the same description and usage; one unused for [`TransientPool::KEEP_FRAMES`] frames is
/// released.
#[derive(Default)]
pub struct TransientPool {
    textures: Vec<PooledTexture>,
    buffers: Vec<PooledBuffer>,
    frame: u64,
    /// Allocations ever made.
    pub allocations: u64,
}

impl TransientPool {
    /// Frames an unused allocation is kept before it is released.
    pub const KEEP_FRAMES: u64 = 3;

    /// An empty pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Physical textures and buffers currently held.
    #[must_use]
    pub fn held(&self) -> (usize, usize) {
        (self.textures.len(), self.buffers.len())
    }

    fn texture(
        &mut self,
        dev: &wgpu::Device,
        desc: &TextureDesc,
        usage: wgpu::TextureUsages,
    ) -> usize {
        let frame = self.frame;
        if let Some(i) = self
            .textures
            .iter()
            .position(|t| t.last_frame != frame && t.desc == *desc && t.usage == usage)
        {
            self.textures[i].last_frame = frame;
            return i;
        }
        let texture = dev.create_texture(&wgpu::TextureDescriptor {
            label: Some("forge-gpu transient"),
            size: wgpu::Extent3d {
                width: desc.width,
                height: desc.height,
                depth_or_array_layers: desc.layers,
            },
            mip_level_count: desc.mips,
            sample_count: desc.samples,
            dimension: wgpu::TextureDimension::D2,
            format: desc.format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.allocations += 1;
        self.textures.push(PooledTexture {
            desc: desc.clone(),
            usage,
            texture,
            view,
            last_frame: frame,
        });
        self.textures.len() - 1
    }

    fn buffer(&mut self, dev: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> usize {
        let frame = self.frame;
        let fit = self
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| b.last_frame != frame && b.usage == usage && b.size >= size)
            .min_by_key(|(_, b)| b.size)
            .map(|(i, _)| i);
        if let Some(i) = fit {
            self.buffers[i].last_frame = frame;
            return i;
        }
        let size = size.max(4).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        let buffer = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-gpu transient"),
            size,
            usage,
            mapped_at_creation: false,
        });
        self.allocations += 1;
        self.buffers.push(PooledBuffer {
            size,
            usage,
            buffer,
            last_frame: frame,
        });
        self.buffers.len() - 1
    }

    fn end_frame(&mut self) {
        let frame = self.frame;
        self.textures
            .retain(|t| frame.saturating_sub(t.last_frame) < Self::KEEP_FRAMES);
        self.buffers
            .retain(|b| frame.saturating_sub(b.last_frame) < Self::KEEP_FRAMES);
        self.frame += 1;
    }
}

/// What one execution did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecStats {
    /// Pass bodies run.
    pub passes_run: usize,
    /// Physical allocations made this frame (0 in steady state).
    pub allocations: u64,
    /// Transient resources with a lifetime this frame.
    pub virtual_resources: usize,
    /// Physical allocations they used.
    pub physical_resources: usize,
}

fn check_import(
    kind: &ResourceKind,
    name: &str,
    tex: Option<&(wgpu::Texture, wgpu::TextureView)>,
    buf: Option<&wgpu::Buffer>,
    tu: wgpu::TextureUsages,
    bu: wgpu::BufferUsages,
) -> Result<(), GpuError> {
    match (kind, tex, buf) {
        (ResourceKind::Texture(d), Some((t, _)), _) => {
            if (t.width(), t.height(), t.format()) != (d.width, d.height, d.format) {
                return Err(GpuError::graph(format!(
                    "import `{name}` is bound to a {}x{} {:?} texture, declared {}x{} {:?}",
                    t.width(),
                    t.height(),
                    t.format(),
                    d.width,
                    d.height,
                    d.format
                )));
            }
            if !t.usage().contains(tu) {
                return Err(GpuError::graph(format!(
                    "import `{name}` lacks usage {:?} its passes declare",
                    tu - t.usage()
                )));
            }
            Ok(())
        }
        (ResourceKind::Buffer(d), _, Some(b)) => {
            if b.size() < d.size {
                return Err(GpuError::graph(format!(
                    "import `{name}` is bound to a {}-byte buffer, declared {}",
                    b.size(),
                    d.size
                )));
            }
            if !b.usage().contains(bu) {
                return Err(GpuError::graph(format!(
                    "import `{name}` lacks usage {:?} its passes declare",
                    bu - b.usage()
                )));
            }
            Ok(())
        }
        _ => Err(GpuError::graph(format!("import `{name}` is not bound"))),
    }
}

/// The measured cost of one pass in one execution.
#[derive(Clone, Debug, PartialEq)]
pub struct PassTiming {
    /// The pass's name.
    pub name: String,
    /// Wall time the CPU spent recording the pass body, in nanoseconds.
    pub cpu_record_ns: u64,
    /// GPU time between the timestamps written before and after the pass, in nanoseconds;
    /// `None` when the device has no encoder timestamps (`TIMESTAMP_QUERY_INSIDE_ENCODERS`).
    pub gpu_ns: Option<f64>,
}

/// Per-pass GPU timestamps for [`CompiledGraph::execute_timed`]: a query set with two
/// timestamps per pass, resolved and read back after the frame's submit. Reused across
/// frames (it grows only when a frame has more passes than it has room for).
pub struct GpuTimer {
    queries: Option<wgpu::QuerySet>,
    resolve: Option<wgpu::Buffer>,
    readback: Option<wgpu::Buffer>,
    capacity: u32,
    period_ns: f64,
    supported: bool,
}

impl GpuTimer {
    /// The device features that per-pass GPU timing needs; request them in
    /// [`PoolOptions::wanted_features`](crate::PoolOptions::wanted_features).
    pub const FEATURES: wgpu::Features =
        wgpu::Features::TIMESTAMP_QUERY.union(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);

    /// A timer for `dev`. On a device without encoder timestamps it records CPU time only
    /// and [`GpuTimer::supported`] is false.
    #[must_use]
    pub fn new(dev: &GpuDevice) -> Self {
        Self {
            queries: None,
            resolve: None,
            readback: None,
            capacity: 0,
            period_ns: f64::from(dev.queue.get_timestamp_period()),
            supported: dev.device.features().contains(Self::FEATURES),
        }
    }

    /// Whether GPU timestamps are available on this device.
    #[must_use]
    pub fn supported(&self) -> bool {
        self.supported
    }

    fn ensure(&mut self, dev: &wgpu::Device, passes: u32) {
        let need = passes
            .max(1)
            .saturating_mul(2)
            .min(wgpu::QUERY_SET_MAX_QUERIES);
        if self.capacity >= need && self.queries.is_some() {
            return;
        }
        let cap = need.next_power_of_two().min(wgpu::QUERY_SET_MAX_QUERIES);
        let bytes = u64::from(cap) * 8;
        self.queries = Some(dev.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("forge-gpu pass timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: cap,
        }));
        self.resolve = Some(dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-gpu timestamp resolve"),
            size: bytes,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        self.readback = Some(dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-gpu timestamp readback"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.capacity = cap;
    }
}

impl CompiledGraph {
    /// Run the live passes in order on `dev`, with transients from `pool` and the caller's
    /// `imports`, as one submit. Any wgpu validation error in the frame is returned.
    pub fn execute(
        mut self,
        dev: &GpuDevice,
        pool: &mut TransientPool,
        imports: &Imports,
    ) -> Result<ExecStats, GpuError> {
        self.execute_inner(dev, pool, imports, None).map(|(s, _)| s)
    }

    /// [`CompiledGraph::execute`], measuring every pass: CPU recording time always, and GPU
    /// time from timestamps written on the encoder before and after each pass body when the
    /// device supports them. Waits for the frame to finish (the timestamps are read back).
    pub fn execute_timed(
        mut self,
        dev: &GpuDevice,
        pool: &mut TransientPool,
        imports: &Imports,
        timer: &mut GpuTimer,
    ) -> Result<(ExecStats, Vec<PassTiming>), GpuError> {
        self.execute_inner(dev, pool, imports, Some(timer))
    }

    /// Execute a graph whose passes were all declared with
    /// [`run_retained`](crate::PassBuilder::run_retained), **keeping it** so the next frame
    /// with the same topology executes it again without rebuilding or recompiling. A pass
    /// with a single-use body fails the second execution with a graph error.
    pub fn execute_retained(
        &mut self,
        dev: &GpuDevice,
        pool: &mut TransientPool,
        imports: &Imports,
        timer: Option<&mut GpuTimer>,
    ) -> Result<(ExecStats, Vec<PassTiming>), GpuError> {
        self.execute_inner(dev, pool, imports, timer)
    }

    fn execute_inner(
        &mut self,
        dev: &GpuDevice,
        pool: &mut TransientPool,
        imports: &Imports,
        mut timer: Option<&mut GpuTimer>,
    ) -> Result<(ExecStats, Vec<PassTiming>), GpuError> {
        let CompiledGraph { plan, runs, kinds } = self;
        let (plan, kinds) = (&*plan, &*kinds);
        let live_passes = u32::try_from(plan.order.len()).unwrap_or(u32::MAX);
        if let Some(t) = timer.as_deref_mut()
            && t.supported
        {
            t.ensure(&dev.device, live_passes);
        }
        let (queries, capacity) = match timer.as_deref() {
            Some(t) if t.supported => (t.queries.clone(), t.capacity),
            _ => (None, 0),
        };
        let mut timings: Vec<PassTiming> = Vec::new();
        let allocations_before = pool.allocations;
        // Physical slots first (mutable pool), then borrow them.
        let mut slot_index: Vec<usize> = Vec::with_capacity(plan.slots.len());
        for s in &plan.slots {
            slot_index.push(match &s.kind {
                ResourceKind::Texture(d) => pool.texture(&dev.device, d, s.texture_usage),
                ResourceKind::Buffer(b) => pool.buffer(&dev.device, b.size, s.buffer_usage),
            });
        }
        for (i, r) in plan.resources.iter().enumerate() {
            if r.imported && r.lifetime.is_some() {
                check_import(
                    &kinds[i],
                    &r.name,
                    imports.textures.get(&(i as u32)),
                    imports.buffers.get(&(i as u32)),
                    r.texture_usage,
                    r.buffer_usage,
                )?;
            }
        }
        let phys: Vec<Option<Phys<'_>>> = plan
            .resources
            .iter()
            .enumerate()
            .map(|(i, r)| {
                if r.imported {
                    let k = i as u32;
                    imports
                        .textures
                        .get(&k)
                        .map(|(t, v)| Phys::Texture(t, v))
                        .or_else(|| imports.buffers.get(&k).map(Phys::Buffer))
                } else {
                    r.slot.map(|s| match &plan.slots[s].kind {
                        ResourceKind::Texture(_) => {
                            let t = &pool.textures[slot_index[s]];
                            Phys::Texture(&t.texture, &t.view)
                        }
                        ResourceKind::Buffer(_) => {
                            Phys::Buffer(&pool.buffers[slot_index[s]].buffer)
                        }
                    })
                }
            })
            .collect();

        let scope = dev.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = dev
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("forge-gpu graph"),
            });
        let mut passes_run = 0;
        let mut failure = None;
        for &p in &plan.order {
            let Some(body) = runs.get_mut(p).and_then(Option::as_mut) else {
                continue;
            };
            let mut once = None;
            if matches!(body, PassBody::Once(_))
                && let PassBody::Once(f) = std::mem::replace(body, PassBody::Spent)
            {
                once = Some(f);
            }
            if matches!(body, PassBody::Spent) && once.is_none() {
                failure = Some(GpuError::graph(format!(
                    "pass `{}` has a single-use body and has already run: declare it with \n                     run_retained to execute a compiled graph again",
                    plan.passes[p].name
                )));
                break;
            }
            let slot = u32::try_from(timings.len()).unwrap_or(u32::MAX);
            let q = queries
                .as_ref()
                .filter(|_| slot.saturating_mul(2).saturating_add(1) < capacity);
            if let Some(q) = q {
                encoder.write_timestamp(q, slot * 2);
            }
            let started = std::time::Instant::now();
            let mut ctx = PassContext {
                encoder: &mut encoder,
                device: &dev.device,
                queue: &dev.queue,
                name: &plan.passes[p].name,
                uses: &plan.passes[p].uses,
                phys: &phys,
                resources: &plan.resources,
            };
            let result = match (once, body) {
                (Some(f), _) => f(&mut ctx),
                (None, PassBody::Retained(f)) => f(&mut ctx),
                (None, _) => Ok(()),
            };
            let cpu_record_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            if let Some(q) = q {
                encoder.write_timestamp(q, slot * 2 + 1);
            }
            if let Err(e) = result {
                failure = Some(e);
                break;
            }
            if timer.is_some() {
                timings.push(PassTiming {
                    name: plan.passes[p].name.clone(),
                    cpu_record_ns,
                    gpu_ns: None,
                });
            }
            passes_run += 1;
        }
        // Timestamps exist for the first `stamped` passes (all of them unless the frame has
        // more passes than the largest query set).
        let stamped = u32::try_from(timings.len()).unwrap_or(0).min(capacity / 2);
        let readback = match (timer.as_deref(), &queries) {
            (Some(t), Some(q)) if stamped > 0 && failure.is_none() => {
                match (&t.resolve, &t.readback) {
                    (Some(res), Some(rb)) => {
                        encoder.resolve_query_set(q, 0..stamped * 2, res, 0);
                        encoder.copy_buffer_to_buffer(res, 0, rb, 0, u64::from(stamped) * 16);
                        Some(rb.clone())
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if failure.is_none() {
            dev.queue.submit([encoder.finish()]);
        }
        if let (Some(rb), Some(t)) = (readback, timer.as_deref()) {
            let stamps = read_timestamps(dev, &rb, stamped as usize)?;
            for (i, pt) in timings.iter_mut().take(stamped as usize).enumerate() {
                let (a, b) = (stamps[2 * i], stamps[2 * i + 1]);
                pt.gpu_ns = Some(b.saturating_sub(a) as f64 * t.period_ns);
            }
        }
        let caught = pollster::block_on(scope.pop());
        drop(phys);
        pool.end_frame();
        if let Some(e) = failure {
            return Err(e);
        }
        if let Some(e) = caught {
            return Err(GpuError::Validation {
                what: "render graph frame".into(),
                why: e.to_string(),
            });
        }
        let stats = ExecStats {
            passes_run,
            allocations: pool.allocations - allocations_before,
            virtual_resources: plan
                .resources
                .iter()
                .filter(|r| !r.imported && r.lifetime.is_some())
                .count(),
            physical_resources: plan.slots.len(),
        };
        Ok((stats, timings))
    }
}

/// Map a timestamp readback buffer (`MAP_READ`, holding resolved timestamps) and return its
/// first `2 * passes` values — a begin and an end per pass, in ticks of
/// `queue.get_timestamp_period()` nanoseconds. Waits for the device to go idle.
pub fn read_timestamps(
    dev: &GpuDevice,
    rb: &wgpu::Buffer,
    passes: usize,
) -> Result<Vec<u64>, GpuError> {
    let bytes = (passes * 16) as u64;
    let slice = rb.slice(0..bytes);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    dev.wait_idle()?;
    match rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(GpuError::transfer(e)),
        Err(e) => return Err(GpuError::transfer(e)),
    }
    let out = {
        let data = slice.get_mapped_range().map_err(GpuError::transfer)?;
        data.as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect()
    };
    rb.unmap();
    Ok(out)
}
