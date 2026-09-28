//! The declarative render graph (Ch.9): passes declare what they read and write, the graph
//! computes the order, culls dead passes, aliases transient resources whose lifetimes do
//! not overlap, and derives the barrier plan and every resource's usage flags.
//!
//! Same discipline as `RegionSystem` (Ch.5): **a pass touches only what it declared** — the
//! [`PassContext`](crate::PassContext) refuses any other resource at run time.
//!
//! **Versioned handles.** Creating a resource gives version 0. [`PassBuilder::write`]
//! (contents discarded) and [`PassBuilder::modify`] (read, then write) return the next
//! version; only the latest version may be written, so a resource never forks. A reader
//! names the exact version it wants, which is how order is *computed* rather than taken
//! from declaration order: a pass declared late that reads an old version runs before the
//! pass that overwrote it (write-after-read), and a pair of passes that each need the
//! other's input intact is reported as a cycle.
//!
//! **Barriers.** wgpu inserts the actual API barriers from each resource's usage; the graph
//! (a) creates every resource with exactly the union of the usages its passes declared, so
//! wgpu never sees an undeclared transition, and (b) emits the transition plan
//! ([`Barrier`]) — including the aliasing hand-over between two residents of one physical
//! allocation — which drives load ops (an aliased or freshly written target is cleared,
//! never loaded) and is checked by an independent verifier ([`verify_plan`]).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use crate::error::GpuError;
use crate::exec::{PassBody, RetainedPassFn};

/// A resource of the graph (transient or imported).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(pub u32);

/// One version of a resource. Copyable; a stale version may be read (which orders the reader
/// before the next writer) but never written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle {
    res: u32,
    version: u32,
}

impl Handle {
    /// The resource.
    #[must_use]
    pub fn resource(self) -> ResourceId {
        ResourceId(self.res)
    }

    /// The version (0 = as created or imported).
    #[must_use]
    pub fn version(self) -> u32 {
        self.version
    }
}

/// A 2D texture (array) description.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureDesc {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// Array layers.
    pub layers: u32,
    /// Mip levels.
    pub mips: u32,
    /// MSAA samples.
    pub samples: u32,
    /// Format.
    pub format: wgpu::TextureFormat,
}

impl TextureDesc {
    /// A single-layer, single-mip, single-sample 2D texture.
    #[must_use]
    pub fn d2(width: u32, height: u32, format: wgpu::TextureFormat) -> Self {
        Self {
            width,
            height,
            layers: 1,
            mips: 1,
            samples: 1,
            format,
        }
    }

    /// Approximate bytes (mip 0 only is exact; mips add a third).
    #[must_use]
    pub fn approx_bytes(&self) -> u64 {
        let texel = u64::from(self.format.block_copy_size(None).unwrap_or(4));
        let base = u64::from(self.width)
            * u64::from(self.height)
            * u64::from(self.layers)
            * texel
            * u64::from(self.samples);
        if self.mips > 1 { base + base / 3 } else { base }
    }
}

/// A buffer description.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BufferDesc {
    /// Size in bytes.
    pub size: u64,
}

/// What a resource is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// A texture.
    Texture(TextureDesc),
    /// A buffer.
    Buffer(BufferDesc),
}

impl ResourceKind {
    fn is_texture(&self) -> bool {
        matches!(self, Self::Texture(_))
    }

    fn approx_bytes(&self) -> u64 {
        match self {
            Self::Texture(t) => t.approx_bytes(),
            Self::Buffer(b) => b.size,
        }
    }
}

/// How a pass uses a resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Access {
    /// Texture sampled in a shader.
    Sampled,
    /// Storage texture or storage buffer, read only.
    StorageRead,
    /// Storage texture or storage buffer, written (read-write storage is `modify`).
    StorageWrite,
    /// Colour render target.
    ColorAttachment,
    /// Depth/stencil target, read only.
    DepthRead,
    /// Depth/stencil target, written.
    DepthWrite,
    /// Copy source.
    CopySrc,
    /// Copy destination.
    CopyDst,
    /// Uniform buffer.
    Uniform,
    /// Vertex buffer.
    Vertex,
    /// Index buffer.
    Index,
    /// Indirect-arguments buffer.
    Indirect,
}

impl Access {
    /// Whether this access writes.
    #[must_use]
    pub fn is_write(self) -> bool {
        matches!(
            self,
            Self::StorageWrite | Self::ColorAttachment | Self::DepthWrite | Self::CopyDst
        )
    }

    fn for_texture(self) -> bool {
        matches!(
            self,
            Self::Sampled
                | Self::StorageRead
                | Self::StorageWrite
                | Self::ColorAttachment
                | Self::DepthRead
                | Self::DepthWrite
                | Self::CopySrc
                | Self::CopyDst
        )
    }

    fn for_buffer(self) -> bool {
        matches!(
            self,
            Self::StorageRead
                | Self::StorageWrite
                | Self::CopySrc
                | Self::CopyDst
                | Self::Uniform
                | Self::Vertex
                | Self::Index
                | Self::Indirect
        )
    }

    /// The wgpu texture usage this access needs.
    #[must_use]
    pub fn texture_usage(self) -> wgpu::TextureUsages {
        use wgpu::TextureUsages as U;
        match self {
            Self::Sampled => U::TEXTURE_BINDING,
            Self::StorageRead | Self::StorageWrite => U::STORAGE_BINDING,
            Self::ColorAttachment | Self::DepthRead | Self::DepthWrite => U::RENDER_ATTACHMENT,
            Self::CopySrc => U::COPY_SRC,
            Self::CopyDst => U::COPY_DST,
            _ => U::empty(),
        }
    }

    /// The wgpu buffer usage this access needs.
    #[must_use]
    pub fn buffer_usage(self) -> wgpu::BufferUsages {
        use wgpu::BufferUsages as U;
        match self {
            Self::StorageRead | Self::StorageWrite => U::STORAGE,
            Self::CopySrc => U::COPY_SRC,
            Self::CopyDst => U::COPY_DST,
            Self::Uniform => U::UNIFORM,
            Self::Vertex => U::VERTEX,
            Self::Index => U::INDEX,
            Self::Indirect => U::INDIRECT,
            _ => U::empty(),
        }
    }
}

/// One declared use of a resource by a pass (input data, copied verbatim into the plan).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Use {
    /// The resource.
    pub res: ResourceId,
    /// The version read (`read` / `modify`).
    pub reads: Option<u32>,
    /// The version produced (`write` / `modify`).
    pub writes: Option<u32>,
    /// How.
    pub access: Access,
}

pub(crate) struct ResourceDecl {
    pub(crate) name: String,
    pub(crate) kind: ResourceKind,
    pub(crate) imported: bool,
    latest: u32,
}

pub(crate) struct PassDecl {
    pub(crate) name: String,
    pub(crate) uses: Vec<Use>,
    side_effect: bool,
    pub(crate) run: Option<PassBody>,
}

/// A frame's graph under construction. Build it, [`compile`](RenderGraph::compile) it,
/// execute the result. Rebuilding every frame is the intended use (compiling is cheap:
/// see `test_render_graph`'s measured figure).
#[derive(Default)]
pub struct RenderGraph {
    pub(crate) resources: Vec<ResourceDecl>,
    pub(crate) passes: Vec<PassDecl>,
    errors: Vec<String>,
}

/// Compile switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompileOptions {
    /// Share physical allocations between transients whose lifetimes do not overlap.
    pub aliasing: bool,
    /// Drop passes whose results nothing live consumes.
    pub cull: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            aliasing: true,
            cull: true,
        }
    }
}

impl RenderGraph {
    /// An empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn add_resource(&mut self, name: &str, kind: ResourceKind, imported: bool) -> Handle {
        let res = self.resources.len() as u32;
        self.resources.push(ResourceDecl {
            name: name.to_string(),
            kind,
            imported,
            latest: 0,
        });
        Handle { res, version: 0 }
    }

    /// A transient texture, owned by the graph for this frame. Version 0 has no contents:
    /// reading it before a pass writes it is an error.
    pub fn create_texture(&mut self, name: &str, desc: TextureDesc) -> Handle {
        self.add_resource(name, ResourceKind::Texture(desc), false)
    }

    /// A transient buffer.
    pub fn create_buffer(&mut self, name: &str, desc: BufferDesc) -> Handle {
        self.add_resource(name, ResourceKind::Buffer(desc), false)
    }

    /// A texture owned outside the graph (a swapchain image, a persistent history buffer).
    /// Version 0 is its current contents. Writing it makes the writer a root: never culled.
    pub fn import_texture(&mut self, name: &str, desc: TextureDesc) -> Handle {
        self.add_resource(name, ResourceKind::Texture(desc), true)
    }

    /// A buffer owned outside the graph.
    pub fn import_buffer(&mut self, name: &str, desc: BufferDesc) -> Handle {
        self.add_resource(name, ResourceKind::Buffer(desc), true)
    }

    /// Start declaring a pass. It joins the graph when the builder is dropped or
    /// [`PassBuilder::run`] is called.
    pub fn add_pass(&mut self, name: &str) -> PassBuilder<'_> {
        PassBuilder {
            decl: Some(PassDecl {
                name: name.to_string(),
                uses: Vec::new(),
                side_effect: false,
                run: None,
            }),
            g: self,
        }
    }

    /// A resource's name.
    #[must_use]
    pub fn name(&self, r: ResourceId) -> &str {
        self.resources
            .get(r.0 as usize)
            .map_or("?", |d| d.name.as_str())
    }

    /// Compile with default options (aliasing and culling on).
    pub fn compile(self) -> Result<CompiledGraph, GpuError> {
        self.compile_with(CompileOptions::default())
    }

    /// Order, cull, alias and plan barriers.
    pub fn compile_with(self, opts: CompileOptions) -> Result<CompiledGraph, GpuError> {
        if let Some(e) = self.errors.first() {
            let more = self.errors.len() - 1;
            return Err(GpuError::graph(if more > 0 {
                format!("{e} (and {more} more)")
            } else {
                e.clone()
            }));
        }
        let plan = plan(&self, opts)?;
        let runs = self.passes.into_iter().map(|p| p.run).collect();
        Ok(CompiledGraph {
            plan,
            runs,
            kinds: self.resources.into_iter().map(|r| r.kind).collect(),
        })
    }
}

/// Declares one pass's uses. Dropping it (or calling [`run`](PassBuilder::run)) adds the
/// pass to the graph; misuse is recorded and reported by `compile`.
pub struct PassBuilder<'g> {
    g: &'g mut RenderGraph,
    decl: Option<PassDecl>,
}

impl PassBuilder<'_> {
    fn check(&mut self, h: Handle, access: Access, verb: &str) -> Option<&mut ResourceDecl> {
        let pass = self.decl.as_ref().map_or(String::new(), |d| d.name.clone());
        let Some(r) = self.g.resources.get(h.res as usize) else {
            self.g
                .errors
                .push(format!("pass `{pass}`: handle {h:?} is from another graph"));
            return None;
        };
        let ok = if r.kind.is_texture() {
            access.for_texture()
        } else {
            access.for_buffer()
        };
        let wants_write = verb != "read";
        let mut err = None;
        if !ok {
            err = Some(format!(
                "pass `{pass}`: {access:?} is not a valid access for {} `{}`",
                if r.kind.is_texture() {
                    "texture"
                } else {
                    "buffer"
                },
                r.name
            ));
        } else if wants_write != access.is_write() {
            err = Some(format!(
                "pass `{pass}`: `{verb}` of `{}` with {access:?}, which {} (use `{}`)",
                r.name,
                if access.is_write() {
                    "writes"
                } else {
                    "only reads"
                },
                if access.is_write() {
                    "write` or `modify"
                } else {
                    "read"
                }
            ));
        } else if self
            .decl
            .as_ref()
            .is_some_and(|d| d.uses.iter().any(|u| u.res.0 == h.res))
        {
            err = Some(format!(
                "pass `{pass}` uses `{}` twice; declare one use (`modify` reads and writes)",
                r.name
            ));
        } else if wants_write && h.version != r.latest {
            err = Some(format!(
                "pass `{pass}` writes `{}` version {} but version {} exists: writing an old \
                 version would fork the resource",
                r.name, h.version, r.latest
            ));
        }
        if let Some(e) = err {
            self.g.errors.push(e);
            return None;
        }
        self.g.resources.get_mut(h.res as usize)
    }

    fn push(&mut self, u: Use) {
        if let Some(d) = self.decl.as_mut() {
            d.uses.push(u);
        }
    }

    /// Read version `h` (orders this pass after `h`'s writer and before the next writer).
    pub fn read(&mut self, h: Handle, access: Access) -> &mut Self {
        if self.check(h, access, "read").is_some() {
            self.push(Use {
                res: h.resource(),
                reads: Some(h.version),
                writes: None,
                access,
            });
        }
        self
    }

    /// Write the resource, discarding its contents; returns the new version.
    pub fn write(&mut self, h: Handle, access: Access) -> Handle {
        self.produce(h, access, "write", None)
    }

    /// Read and write (load-then-store, read-write storage); returns the new version.
    pub fn modify(&mut self, h: Handle, access: Access) -> Handle {
        self.produce(h, access, "modify", Some(h.version))
    }

    fn produce(&mut self, h: Handle, access: Access, verb: &str, reads: Option<u32>) -> Handle {
        match self.check(h, access, verb) {
            Some(r) => {
                r.latest += 1;
                let v = r.latest;
                self.push(Use {
                    res: h.resource(),
                    reads,
                    writes: Some(v),
                    access,
                });
                Handle {
                    res: h.res,
                    version: v,
                }
            }
            None => h,
        }
    }

    /// Never cull this pass (it has an effect outside the graph: a readback, a present).
    pub fn side_effect(&mut self) -> &mut Self {
        if let Some(d) = self.decl.as_mut() {
            d.side_effect = true;
        }
        self
    }

    /// Set the pass body and add the pass.
    pub fn run(
        mut self,
        f: impl FnOnce(&mut crate::exec::PassContext<'_>) -> Result<(), GpuError> + 'static,
    ) {
        if let Some(d) = self.decl.as_mut() {
            d.run = Some(PassBody::Once(Box::new(f)));
        }
    }

    /// Set a body that runs on **every** execution of the compiled graph, and add the pass.
    /// Used with [`CompiledGraph::execute_retained`]: a renderer whose frame topology has
    /// not changed executes last frame's compiled graph again, and the bodies read that
    /// frame's data from state shared with the renderer.
    pub fn run_retained(
        mut self,
        f: impl FnMut(&mut crate::exec::PassContext<'_>) -> Result<(), GpuError> + 'static,
    ) {
        if let Some(d) = self.decl.as_mut() {
            let body: RetainedPassFn = Box::new(f);
            d.run = Some(PassBody::Retained(body));
        }
    }
}

impl Drop for PassBuilder<'_> {
    fn drop(&mut self) {
        if let Some(d) = self.decl.take() {
            self.g.passes.push(d);
        }
    }
}

/// A declared pass, as the plan reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassInfo {
    /// Its name.
    pub name: String,
    /// Its declared uses (verbatim).
    pub uses: Vec<Use>,
    /// Declared `side_effect`.
    pub side_effect: bool,
    /// Culled: nothing live consumes its results.
    pub culled: bool,
}

/// A resource, as the plan reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceInfo {
    /// Its name.
    pub name: String,
    /// Texture or buffer, with its description.
    pub kind: ResourceKind,
    /// Owned outside the graph.
    pub imported: bool,
    /// Positions in `order` of its first and last live use (`None`: unused this frame).
    pub lifetime: Option<(usize, usize)>,
    /// Union of its live uses' texture usages (textures).
    pub texture_usage: wgpu::TextureUsages,
    /// Union of its live uses' buffer usages (buffers).
    pub buffer_usage: wgpu::BufferUsages,
    /// The physical slot (transients with a lifetime).
    pub slot: Option<usize>,
}

/// One physical allocation shared by transients in turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotInfo {
    /// What is allocated (for buffers, the largest resident's size).
    pub kind: ResourceKind,
    /// Union of the residents' usages.
    pub texture_usage: wgpu::TextureUsages,
    /// Union of the residents' usages.
    pub buffer_usage: wgpu::BufferUsages,
    /// Residents in lifetime order.
    pub residents: Vec<ResourceId>,
}

/// One state transition of a resource, before a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Barrier {
    /// Declared index of the pass it precedes.
    pub before: usize,
    /// The resource.
    pub res: ResourceId,
    /// Previous access this frame (`None`: first use — undefined contents for a transient,
    /// the owner's state for an import).
    pub from: Option<Access>,
    /// The access the pass declared.
    pub to: Access,
    /// The previous resident of the same physical allocation (an aliasing hand-over).
    pub aliased_from: Option<ResourceId>,
}

/// Everything compile decided, as plain data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphPlan {
    /// Every declared pass, in declaration order.
    pub passes: Vec<PassInfo>,
    /// Declared indices of the live passes, in execution order.
    pub order: Vec<usize>,
    /// Every resource, by id.
    pub resources: Vec<ResourceInfo>,
    /// Physical transient allocations.
    pub slots: Vec<SlotInfo>,
    /// Transitions in execution order.
    pub barriers: Vec<Barrier>,
}

impl GraphPlan {
    /// Names of the live passes in execution order.
    #[must_use]
    pub fn order_names(&self) -> Vec<&str> {
        self.order
            .iter()
            .map(|&p| self.passes[p].name.as_str())
            .collect()
    }

    /// Declared index of the pass called `name`.
    #[must_use]
    pub fn pass_index(&self, name: &str) -> Option<usize> {
        self.passes.iter().position(|p| p.name == name)
    }

    /// Bytes the transients would need without aliasing, and with it.
    #[must_use]
    pub fn transient_bytes(&self) -> (u64, u64) {
        let virt = self
            .resources
            .iter()
            .filter(|r| !r.imported && r.lifetime.is_some())
            .map(|r| r.kind.approx_bytes())
            .sum();
        let phys = self.slots.iter().map(|s| s.kind.approx_bytes()).sum();
        (virt, phys)
    }
}

/// A compiled graph: the plan plus the pass bodies, ready to execute once.
pub struct CompiledGraph {
    pub(crate) plan: GraphPlan,
    pub(crate) runs: Vec<Option<PassBody>>,
    pub(crate) kinds: Vec<ResourceKind>,
}

impl CompiledGraph {
    /// What compile decided.
    #[must_use]
    pub fn plan(&self) -> &GraphPlan {
        &self.plan
    }
}

type Key = (u32, u32);

/// The writer of the lowest version of `res` above `v` whose writer is live.
fn next_live_writer(
    producer: &BTreeMap<Key, usize>,
    live: &[bool],
    res: u32,
    v: u32,
) -> Option<usize> {
    let lo = v.checked_add(1)?;
    producer
        .range((res, lo)..=(res, u32::MAX))
        .map(|(_, &w)| w)
        .find(|&w| live[w])
}

/// The writer of the highest version of `res` below `v` whose writer is live.
fn prev_live_writer(
    producer: &BTreeMap<Key, usize>,
    live: &[bool],
    res: u32,
    v: u32,
) -> Option<usize> {
    producer
        .range((res, 0)..(res, v))
        .rev()
        .map(|(_, &w)| w)
        .find(|&w| live[w])
}

/// The strongly connected components of size > 1 among the nodes with `on[n]`, each sorted
/// by declaration index, in order of their smallest member (iterative Tarjan: no recursion
/// depth limit on a long chain).
pub(crate) fn cyclic_components(succ: &[Vec<usize>], on: &[bool]) -> Vec<Vec<usize>> {
    let n = succ.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut out: Vec<Vec<usize>> = Vec::new();
    for root in 0..n {
        if !on[root] || index[root] != usize::MAX {
            continue;
        }
        // (node, next successor position)
        let mut call: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(top) = call.last_mut() {
            let v = top.0;
            if let Some(&w) = succ[v].get(top.1) {
                top.1 += 1;
                if !on[w] {
                    continue;
                }
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            call.pop();
            if let Some(&(parent, _)) = call.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut comp = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                if comp.len() > 1 {
                    comp.sort_unstable();
                    out.push(comp);
                }
            }
        }
    }
    out.sort_by_key(|c| c[0]);
    out
}

fn plan(g: &RenderGraph, opts: CompileOptions) -> Result<GraphPlan, GpuError> {
    let np = g.passes.len();
    let mut producer: BTreeMap<Key, usize> = BTreeMap::new();
    let mut readers: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    for (p, d) in g.passes.iter().enumerate() {
        for u in &d.uses {
            if let Some(v) = u.writes {
                producer.insert((u.res.0, v), p);
            }
            if let Some(v) = u.reads {
                readers.entry((u.res.0, v)).or_default().push(p);
            }
        }
    }
    // Reads of a transient's version 0 read garbage.
    for d in &g.passes {
        for u in &d.uses {
            let r = &g.resources[u.res.0 as usize];
            if u.reads == Some(0) && !r.imported {
                return Err(GpuError::graph(format!(
                    "pass `{}` reads `{}` before any pass writes it",
                    d.name, r.name
                )));
            }
        }
    }

    // Liveness: roots are side-effect passes and writers of imports; a live pass keeps the
    // producers of everything it reads.
    let mut live = vec![!opts.cull; np];
    let mut work: Vec<usize> = Vec::new();
    for (p, d) in g.passes.iter().enumerate() {
        let root = d.side_effect
            || d.uses
                .iter()
                .any(|u| u.writes.is_some() && g.resources[u.res.0 as usize].imported);
        if root || !opts.cull {
            live[p] = true;
            work.push(p);
        }
    }
    while let Some(p) = work.pop() {
        for u in &g.passes[p].uses {
            if let Some(v) = u.reads
                && let Some(&q) = producer.get(&(u.res.0, v))
                && !live[q]
            {
                live[q] = true;
                work.push(q);
            }
        }
    }

    // Edges among live passes: RAW, WAR, WAW.
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); np];
    let mut indeg = vec![0usize; np];
    let mut edge = |a: usize, b: usize, succ: &mut Vec<Vec<usize>>| {
        if a != b && live[a] && live[b] && !succ[a].contains(&b) {
            succ[a].push(b);
            indeg[b] += 1;
        }
    };
    // WAR and WAW are ordered against the next *live* writer, not the next version's writer:
    // a culled writer never runs, so an edge through it would order nothing, and a reader of
    // v could then run after the live writer of v+2 and sample the wrong contents.
    for (&(res, v), &w) in &producer {
        // RAW: this version's readers run after its writer.
        if let Some(rs) = readers.get(&(res, v)) {
            for &r in rs {
                edge(w, r, &mut succ);
            }
        }
        // WAW: the previous live writer of this resource runs first.
        if live[w]
            && let Some(prev) = prev_live_writer(&producer, &live, res, v)
        {
            edge(prev, w, &mut succ);
        }
    }
    // WAR: each version's readers run before the resource's next live writer.
    for (&(res, v), rs) in &readers {
        if let Some(w) = next_live_writer(&producer, &live, res, v) {
            for &r in rs {
                edge(r, w, &mut succ);
            }
        }
    }
    let mut heap: BinaryHeap<Reverse<usize>> = (0..np)
        .filter(|&p| live[p] && indeg[p] == 0)
        .map(Reverse)
        .collect();
    let mut order = Vec::new();
    while let Some(Reverse(p)) = heap.pop() {
        order.push(p);
        for &s in &succ[p] {
            indeg[s] -= 1;
            if indeg[s] == 0 {
                heap.push(Reverse(s));
            }
        }
    }
    let live_count = live.iter().filter(|&&l| l).count();
    if order.len() != live_count {
        // Name only the passes that are *on* a cycle (the non-trivial strongly connected
        // components), not every pass stuck downstream of one.
        let mut placed = vec![false; np];
        for &p in &order {
            placed[p] = true;
        }
        let stuck: Vec<bool> = (0..np).map(|p| live[p] && !placed[p]).collect();
        let cycles: Vec<Vec<&str>> = cyclic_components(&succ, &stuck)
            .into_iter()
            .map(|c| c.into_iter().map(|p| g.passes[p].name.as_str()).collect())
            .collect();
        let named = cycles
            .iter()
            .map(|c| format!("{c:?}"))
            .collect::<Vec<_>>()
            .join(" and ");
        return Err(GpuError::graph(format!(
            "cycle between passes {named}: each needs a resource version another overwrites; \
             copy the resource so both versions exist"
        )));
    }
    let mut pos = vec![usize::MAX; np];
    for (i, &p) in order.iter().enumerate() {
        pos[p] = i;
    }

    // Lifetimes and usages.
    let mut resources: Vec<ResourceInfo> = g
        .resources
        .iter()
        .map(|r| ResourceInfo {
            name: r.name.clone(),
            kind: r.kind.clone(),
            imported: r.imported,
            lifetime: None,
            texture_usage: wgpu::TextureUsages::empty(),
            buffer_usage: wgpu::BufferUsages::empty(),
            slot: None,
        })
        .collect();
    for &p in &order {
        for u in &g.passes[p].uses {
            let r = &mut resources[u.res.0 as usize];
            let i = pos[p];
            r.lifetime = Some(match r.lifetime {
                None => (i, i),
                Some((a, b)) => (a.min(i), b.max(i)),
            });
            r.texture_usage |= u.access.texture_usage();
            r.buffer_usage |= u.access.buffer_usage();
        }
    }

    // Aliasing: greedy interval assignment in first-use order.
    let mut slots: Vec<SlotInfo> = Vec::new();
    let mut busy_until: Vec<usize> = Vec::new();
    let mut transients: Vec<usize> = (0..resources.len())
        .filter(|&i| !resources[i].imported && resources[i].lifetime.is_some())
        .collect();
    transients.sort_by_key(|&i| (resources[i].lifetime.map_or(0, |l| l.0), i));
    for i in transients {
        let (first, last) = resources[i].lifetime.unwrap_or((0, 0));
        let pick = if opts.aliasing {
            match &resources[i].kind {
                ResourceKind::Texture(d) => (0..slots.len()).find(|&s| {
                    busy_until[s] < first
                        && matches!(&slots[s].kind, ResourceKind::Texture(sd) if sd == d)
                }),
                ResourceKind::Buffer(b) => {
                    let free: Vec<usize> = (0..slots.len())
                        .filter(|&s| busy_until[s] < first && !slots[s].kind.is_texture())
                        .collect();
                    let size = |s: usize| match &slots[s].kind {
                        ResourceKind::Buffer(sb) => sb.size,
                        ResourceKind::Texture(_) => 0,
                    };
                    free.iter()
                        .copied()
                        .filter(|&s| size(s) >= b.size)
                        .min_by_key(|&s| size(s))
                        .or_else(|| free.iter().copied().max_by_key(|&s| size(s)))
                }
            }
        } else {
            None
        };
        let s = match pick {
            Some(s) => s,
            None => {
                slots.push(SlotInfo {
                    kind: resources[i].kind.clone(),
                    texture_usage: wgpu::TextureUsages::empty(),
                    buffer_usage: wgpu::BufferUsages::empty(),
                    residents: Vec::new(),
                });
                busy_until.push(0);
                slots.len() - 1
            }
        };
        if let (ResourceKind::Buffer(sb), ResourceKind::Buffer(b)) =
            (&mut slots[s].kind, &resources[i].kind)
        {
            sb.size = sb.size.max(b.size);
        }
        slots[s].texture_usage |= resources[i].texture_usage;
        slots[s].buffer_usage |= resources[i].buffer_usage;
        slots[s].residents.push(ResourceId(i as u32));
        busy_until[s] = last;
        resources[i].slot = Some(s);
    }

    // Barriers.
    let mut state: Vec<Option<Access>> = vec![None; resources.len()];
    let mut barriers = Vec::new();
    for &p in &order {
        for u in &g.passes[p].uses {
            let ri = u.res.0 as usize;
            let prev = state[ri];
            let needed = prev != Some(u.access) || u.access == Access::StorageWrite;
            if needed {
                let aliased_from = if prev.is_none() {
                    resources[ri].slot.and_then(|s| {
                        let res = &slots[s].residents;
                        res.iter()
                            .position(|r| r.0 as usize == ri)
                            .and_then(|k| k.checked_sub(1))
                            .map(|k| res[k])
                    })
                } else {
                    None
                };
                barriers.push(Barrier {
                    before: p,
                    res: u.res,
                    from: prev,
                    to: u.access,
                    aliased_from,
                });
            }
            state[ri] = Some(u.access);
        }
    }

    Ok(GraphPlan {
        passes: g
            .passes
            .iter()
            .enumerate()
            .map(|(p, d)| PassInfo {
                name: d.name.clone(),
                uses: d.uses.clone(),
                side_effect: d.side_effect,
                culled: !live[p],
            })
            .collect(),
        order,
        resources,
        slots,
        barriers,
    })
}

/// Check a plan against its own declared uses, independently of the compiler: every
/// hazard ordered, roots live, aliased residents disjoint and compatible, slot usages
/// sufficient, every transition covered by a barrier. Returns every violation found.
#[must_use]
pub fn verify_plan(plan: &GraphPlan) -> Vec<String> {
    let mut bad = Vec::new();
    let n = plan.passes.len();
    let mut pos = vec![None; n];
    for (i, &p) in plan.order.iter().enumerate() {
        if p >= n {
            bad.push(format!("order names pass {p}, which does not exist"));
            continue;
        }
        if pos[p].is_some() {
            bad.push(format!(
                "pass `{}` is in the order twice",
                plan.passes[p].name
            ));
        }
        pos[p] = Some(i);
    }
    for (p, info) in plan.passes.iter().enumerate() {
        if info.culled == pos[p].is_some() {
            bad.push(format!(
                "pass `{}`: culled={} but {} the order",
                info.name,
                info.culled,
                if pos[p].is_some() { "in" } else { "not in" }
            ));
        }
        let root = info.side_effect
            || info
                .uses
                .iter()
                .any(|u| u.writes.is_some() && plan.resources[u.res.0 as usize].imported);
        if root && pos[p].is_none() {
            bad.push(format!("root pass `{}` was culled", info.name));
        }
    }
    // Hazards.
    let mut producer: BTreeMap<Key, usize> = BTreeMap::new();
    for (p, info) in plan.passes.iter().enumerate() {
        for u in &info.uses {
            if let Some(v) = u.writes {
                producer.insert((u.res.0, v), p);
            }
        }
    }
    let live: Vec<bool> = pos.iter().map(Option::is_some).collect();
    let name = |p: usize| plan.passes[p].name.as_str();
    for &p in &plan.order {
        for u in &plan.passes[p].uses {
            let Some(v) = u.reads else { continue };
            if let Some(&w) = producer.get(&(u.res.0, v)) {
                match (pos[w], pos[p]) {
                    (Some(a), Some(b)) if a < b => {}
                    _ => bad.push(format!(
                        "RAW: `{}` reads {}@{v} but its writer `{}` does not run before it",
                        name(p),
                        plan.resources[u.res.0 as usize].name,
                        name(w)
                    )),
                }
            }
            // The next writer that actually runs, not the next version's writer: a culled
            // writer overwrites nothing.
            if let Some(w) = next_live_writer(&producer, &live, u.res.0, v)
                && w != p
                && let (Some(a), Some(b)) = (pos[p], pos[w])
                && a > b
            {
                bad.push(format!(
                    "WAR: `{}` reads {}@{v} after `{}` overwrote it",
                    name(p),
                    plan.resources[u.res.0 as usize].name,
                    name(w)
                ));
            }
        }
    }
    for (&(res, v), &w) in &producer {
        if live[w]
            && let Some(prev) = prev_live_writer(&producer, &live, res, v)
            && let (Some(a), Some(b)) = (pos[prev], pos[w])
            && a > b
        {
            bad.push(format!(
                "WAW: `{}` and `{}` write {} out of order",
                name(prev),
                name(w),
                plan.resources[res as usize].name
            ));
        }
    }
    // Lifetimes and usages recomputed from uses and order.
    let mut life: Vec<Option<(usize, usize)>> = vec![None; plan.resources.len()];
    let mut tu = vec![wgpu::TextureUsages::empty(); plan.resources.len()];
    let mut bu = vec![wgpu::BufferUsages::empty(); plan.resources.len()];
    for (i, &p) in plan.order.iter().enumerate() {
        for u in &plan.passes[p].uses {
            let r = u.res.0 as usize;
            life[r] = Some(life[r].map_or((i, i), |(a, b)| (a.min(i), b.max(i))));
            tu[r] |= u.access.texture_usage();
            bu[r] |= u.access.buffer_usage();
        }
    }
    for (s, slot) in plan.slots.iter().enumerate() {
        for (k, &a) in slot.residents.iter().enumerate() {
            let ra = &plan.resources[a.0 as usize];
            if ra.imported {
                bad.push(format!(
                    "slot {s}: import `{}` in a transient slot",
                    ra.name
                ));
            }
            match (&slot.kind, &ra.kind) {
                (ResourceKind::Texture(sd), ResourceKind::Texture(d)) if sd == d => {}
                (ResourceKind::Buffer(sb), ResourceKind::Buffer(b)) if sb.size >= b.size => {}
                _ => bad.push(format!(
                    "slot {s}: `{}` does not fit the allocation",
                    ra.name
                )),
            }
            if !slot.texture_usage.contains(tu[a.0 as usize])
                || !slot.buffer_usage.contains(bu[a.0 as usize])
            {
                bad.push(format!("slot {s}: usage lacks what `{}` needs", ra.name));
            }
            for &b in &slot.residents[k + 1..] {
                if let (Some((a0, a1)), Some((b0, b1))) = (life[a.0 as usize], life[b.0 as usize])
                    && a0 <= b1
                    && b0 <= a1
                {
                    bad.push(format!(
                        "slot {s}: `{}` [{a0},{a1}] and `{}` [{b0},{b1}] overlap",
                        ra.name, plan.resources[b.0 as usize].name
                    ));
                }
            }
        }
    }
    for (r, info) in plan.resources.iter().enumerate() {
        if !info.imported && life[r].is_some() && info.slot.is_none() {
            bad.push(format!("transient `{}` has no allocation", info.name));
        }
        if let Some(s) = info.slot
            && !plan
                .slots
                .get(s)
                .is_some_and(|sl| sl.residents.contains(&ResourceId(r as u32)))
        {
            bad.push(format!(
                "`{}` names slot {s}, which does not list it",
                info.name
            ));
        }
    }
    // Barriers: replay each resource's access sequence.
    let mut state: Vec<Option<Access>> = vec![None; plan.resources.len()];
    let mut want = Vec::new();
    for &p in &plan.order {
        for u in &plan.passes[p].uses {
            let r = u.res.0 as usize;
            if state[r] != Some(u.access) || u.access == Access::StorageWrite {
                want.push((p, u.res, state[r], u.access));
            }
            state[r] = Some(u.access);
        }
    }
    let got: Vec<(usize, ResourceId, Option<Access>, Access)> = plan
        .barriers
        .iter()
        .map(|b| (b.before, b.res, b.from, b.to))
        .collect();
    for w in &want {
        if !got.contains(w) {
            bad.push(format!(
                "missing barrier before `{}`: {} {:?} -> {:?}",
                name(w.0),
                plan.resources[w.1.0 as usize].name,
                w.2,
                w.3
            ));
        }
    }
    for g in &got {
        if !want.contains(g) {
            bad.push(format!(
                "spurious barrier before `{}`: {} {:?} -> {:?}",
                name(g.0),
                plan.resources[g.1.0 as usize].name,
                g.2,
                g.3
            ));
        }
    }
    bad
}
