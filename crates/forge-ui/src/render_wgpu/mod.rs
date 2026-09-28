//! `WgpuRenderer` (Ch.21 §21.13) — feature `wgpu`. **The only module that names `wgpu`**
//! (`test_ui_wgpu_confined`).
//!
//! * One uber-pipeline instances quads, shadows, glyphs and images; one bind group holds
//!   the glyph atlas's two planes and the image atlas, so batches break only on clips and
//!   viewport composites.
//! * Each target has a **persistent** `Rgba8UnormSrgb` UI texture. A frame re-rasterises
//!   only the damaged rects (scissored), then — for a window — composites the whole target
//!   onto the swapchain image with one full-screen triangle (`wgpu` has no portable
//!   partial present). No damage ⇒ `Ui::render` never calls this at all.
//! * Offscreen targets read back to RGBA8 for pixel goldens.
//! * The device and queue come from `forge-gpu`'s adapter pool (D-3, M1-1): the pool's
//!   primary adapter, or the first one that can present to the window, because UI is
//!   latency-critical and not Tier-0 work. Shaders compile through `forge_gpu::shader`
//!   (WGSL validated by naga; an error is returned, never a panic).

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::sync::Arc;

/// The wgpu the renderer is built on (forge-gpu's copy), for callers that register textures.
pub use forge_gpu::wgpu;
pub use forge_gpu::{AdapterPool, GpuMode, PoolOptions, SoftwarePolicy};

use crate::UiError;
use crate::geom::{PhysicalSize, PxRect};
use crate::render::{
    BatchKind, BatchList, ExternalTexture, FrameStats, GpuMeshVertex, Instance, TargetId,
    TextureId, TextureUpdate, TextureUpdateKind, UiRenderer,
};

const UI_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn gpu<E: std::fmt::Display>(e: E) -> UiError {
    UiError::Gpu(e.to_string())
}

struct Tex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    w: u32,
    h: u32,
}

struct Surface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    blit: wgpu::RenderPipeline,
}

/// A render target: its persistent UI texture and its own GPU copies of the frame's
/// instance image and mesh buffers (windows never overwrite each other's, and each
/// tracks which layout generation it holds so a frame uploads only what changed).
struct Target {
    ui: Tex,
    fresh: bool,
    surface: Option<Surface>,
    instances: wgpu::Buffer,
    instance_capacity: u64,
    uploaded_generation: Option<u64>,
    mesh_vb: wgpu::Buffer,
    mesh_vb_capacity: u64,
    mesh_ib: wgpu::Buffer,
    mesh_ib_capacity: u64,
    mesh_generation: Option<u64>,
}

/// The UI's device: one member of a `forge-gpu` [`AdapterPool`] (D-3).
pub struct GpuContext {
    /// The pool the device belongs to (other members do Tier-0 work, never UI).
    pub pool: Arc<AdapterPool>,
    /// Which pool member the UI renders on.
    pub index: usize,
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    /// The pool's `index`-th device (0 = primary, the UI's default).
    pub fn from_pool(pool: Arc<AdapterPool>, index: usize) -> Result<GpuContext, UiError> {
        let d = pool
            .devices()
            .get(index)
            .ok_or_else(|| UiError::Gpu(format!("the adapter pool has no device {index}")))?;
        Ok(GpuContext {
            index,
            instance: pool.instance().clone(),
            adapter: d.adapter.clone(),
            device: d.device.clone(),
            queue: d.queue.clone(),
            pool: pool.clone(),
        })
    }

    /// The first pool member (primary preferred) that can present to `surface`.
    pub fn for_surface(
        pool: Arc<AdapterPool>,
        surface: &wgpu::Surface<'_>,
    ) -> Result<GpuContext, UiError> {
        let index = pool.device_for_surface(surface).map_err(gpu)?.index;
        Self::from_pool(pool, index)
    }

    /// A headless context for offscreen rendering (pixel goldens): a pool with the default
    /// options (hardware, software only as a fallback), or — with `fallback` — the software
    /// rasteriser only (WARP on Windows, lavapipe on Linux).
    pub fn headless(fallback: bool) -> Result<GpuContext, UiError> {
        let software = if fallback {
            SoftwarePolicy::Only
        } else {
            SoftwarePolicy::FallbackOnly
        };
        let pool = AdapterPool::new(&PoolOptions {
            software,
            ..PoolOptions::default()
        })
        .map_err(gpu)?;
        Self::from_pool(Arc::new(pool), 0)
    }

    /// `"NVIDIA GeForce RTX 3080 (Vulkan 1.4)"`.
    pub fn adapter_name(&self) -> String {
        self.pool
            .devices()
            .get(self.index)
            .map_or_else(|| "?".to_string(), forge_gpu::GpuDevice::label)
    }
}

/// The GPU implementation of [`UiRenderer`].
pub struct WgpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    blit_module: wgpu::ShaderModule,
    blit_bgl: wgpu::BindGroupLayout,
    ui_blit: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    globals: wgpu::Buffer,
    /// The second pipeline: lyon meshes.
    mesh_pipeline: wgpu::RenderPipeline,
    /// Reused byte staging for buffer writes.
    scratch: Vec<u8>,
    textures: BTreeMap<TextureId, Tex>,
    bind_group: Option<wgpu::BindGroup>,
    targets: BTreeMap<TargetId, Target>,
    external: BTreeMap<ExternalTexture, wgpu::TextureView>,
    /// The instance and adapter windows' surfaces are created from.
    window_ctx: Option<(wgpu::Instance, wgpu::Adapter)>,
    /// The pool member the UI renders on, when it came from a pool: an application renders
    /// textures it composites (viewports) on the same device (`ExternalCx::device`).
    pool_member: Option<(Arc<AdapterPool>, usize)>,
}

/// What an application gets, once per window frame just before the UI is drawn, to render
/// the textures it composites (`Primitive::Viewport`): the UI's own device, and a way to
/// register a rendered texture under its `ExternalTexture` id (Ch.21 §21.13).
pub struct ExternalCx<'a> {
    renderer: &'a mut WgpuRenderer,
}

impl<'a> ExternalCx<'a> {
    pub fn new(renderer: &'a mut WgpuRenderer) -> Self {
        Self { renderer }
    }
    /// The device the UI draws with (`None`: a renderer built outside a pool).
    pub fn device(&self) -> Option<&forge_gpu::GpuDevice> {
        let (pool, i) = self.renderer.pool_member.as_ref()?;
        pool.devices().get(*i)
    }
    /// The whole pool (Tier-0 work goes to its other members).
    pub fn pool(&self) -> Option<Arc<AdapterPool>> {
        self.renderer
            .pool_member
            .as_ref()
            .map(|(p, _)| Arc::clone(p))
    }
    /// Register (or replace) the texture composited for `id`.
    pub fn register(&mut self, id: ExternalTexture, view: wgpu::TextureView) {
        self.renderer.register_external(id, view);
    }
    /// Forget `id`'s texture (its viewport went away).
    pub fn unregister(&mut self, id: ExternalTexture) {
        self.renderer.external.remove(&id);
    }
}

fn blit_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    bgl: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("forge-ui blit"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forge-ui blit"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_blit"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_blit"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

impl WgpuRenderer {
    /// A renderer on `device` / `queue` (a `forge-gpu` pool member, D-3).
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Result<Self, UiError> {
        let shader = |name: &str, src: &str| {
            forge_gpu::shader::compile_wgsl(&device, name, src).map_err(gpu)
        };
        let module = shader("forge-ui/ui.wgsl", include_str!("ui.wgsl"))?;
        let blit_module = shader("forge-ui/blit.wgsl", include_str!("blit.wgsl"))?;
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forge-ui textures"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(16),
                    },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forge-ui blit"),
            entries: &[
                tex_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forge-ui uber"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4,
            3 => Float32x4, 4 => Float32x4, 5 => Float32x4
        ];
        let premul = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("forge-ui uber"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: Instance::SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: UI_FORMAT,
                    blend: Some(premul),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let ui_blit = blit_pipeline(&device, &blit_module, &blit_bgl, UI_FORMAT, None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("forge-ui linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-ui globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mesh_module = shader("forge-ui/mesh.wgsl", include_str!("mesh.wgsl"))?;
        let mesh_attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x2];
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("forge-ui mesh"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &mesh_module,
                entry_point: Some("vs_mesh"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: GpuMeshVertex::SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &mesh_attrs,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &mesh_module,
                entry_point: Some("fs_mesh"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: UI_FORMAT,
                    blend: Some(premul),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let mut r = Self {
            device,
            queue,
            pipeline,
            blit_module,
            blit_bgl,
            ui_blit,
            bgl,
            sampler,
            globals,
            mesh_pipeline,
            scratch: Vec::new(),
            textures: BTreeMap::new(),
            bind_group: None,
            targets: BTreeMap::new(),
            external: BTreeMap::new(),
            window_ctx: None,
            pool_member: None,
        };
        // Placeholders so the bind group is always complete before the atlases arrive.
        for id in [
            TextureId::GlyphMask,
            TextureId::GlyphColor,
            TextureId::Images,
        ] {
            r.create_texture(id, 1, 1);
        }
        Ok(r)
    }

    /// A renderer on a context's device.
    pub fn from_context(ctx: &GpuContext) -> Result<Self, UiError> {
        Self::new(ctx.device.clone(), ctx.queue.clone())
    }

    /// Build an adapter pool in `mode` (D-3; [`GpuMode::Single`] unless the app opted in to
    /// Multi) and a renderer with `window`'s surface as `target`, on the first pool member
    /// that can present to it (the primary, normally).
    /// The platform layer passes its window without naming `wgpu`. Returns the renderer
    /// and the adapter's name.
    pub fn for_window(
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        target: TargetId,
        size: PhysicalSize,
        mode: GpuMode,
    ) -> Result<(WgpuRenderer, String), UiError> {
        let pool = AdapterPool::new(&PoolOptions {
            mode,
            ..PoolOptions::default()
        })
        .map_err(gpu)?;
        Self::for_window_in(Arc::new(pool), window, target, size)
    }

    /// [`WgpuRenderer::for_window`] on an existing pool (the editor's, shared with the
    /// renderer and Tier-0 compute).
    pub fn for_window_in(
        pool: Arc<AdapterPool>,
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        target: TargetId,
        size: PhysicalSize,
    ) -> Result<(WgpuRenderer, String), UiError> {
        let surface = pool.instance().create_surface(window).map_err(gpu)?;
        let ctx = GpuContext::for_surface(pool, &surface)?;
        let mut r = Self::from_context(&ctx)?;
        r.add_surface_target(target, surface, &ctx.adapter, size)?;
        let name = ctx.adapter_name();
        r.pool_member = Some((Arc::clone(&ctx.pool), ctx.index));
        r.window_ctx = Some((ctx.instance, ctx.adapter));
        Ok((r, name))
    }

    /// Another window on the same device (multi-window, §21.14): its own surface and
    /// persistent UI target; the atlases and pipelines are shared.
    pub fn add_window(
        &mut self,
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        target: TargetId,
        size: PhysicalSize,
    ) -> Result<(), UiError> {
        let (instance, adapter) = self
            .window_ctx
            .clone()
            .ok_or_else(|| UiError::Gpu("add_window before for_window".into()))?;
        let surface = instance.create_surface(window).map_err(gpu)?;
        self.add_surface_target(target, surface, &adapter, size)
    }

    /// Drop a target (a closed window).
    pub fn remove_target(&mut self, target: TargetId) {
        self.targets.remove(&target);
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    fn format_of(id: TextureId) -> wgpu::TextureFormat {
        match id {
            TextureId::GlyphMask => wgpu::TextureFormat::R8Unorm,
            TextureId::GlyphColor | TextureId::Images => wgpu::TextureFormat::Rgba8UnormSrgb,
        }
    }

    fn make_tex(
        &self,
        label: &str,
        format: wgpu::TextureFormat,
        w: u32,
        h: u32,
        usage: wgpu::TextureUsages,
    ) -> Tex {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Tex {
            texture,
            view,
            w: w.max(1),
            h: h.max(1),
        }
    }

    fn create_texture(&mut self, id: TextureId, w: u32, h: u32) {
        let t = self.make_tex(
            "forge-ui atlas",
            Self::format_of(id),
            w,
            h,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
        );
        self.textures.insert(id, t);
        self.bind_group = None;
    }

    /// The number of sampled textures in the one bind group (the budget test reads it).
    pub fn bound_textures(&self) -> Vec<(TextureId, wgpu::TextureFormat, u32, u32)> {
        self.textures
            .iter()
            .map(|(id, t)| (*id, Self::format_of(*id), t.w, t.h))
            .collect()
    }

    fn apply_update(&mut self, u: &TextureUpdate, stats: &mut FrameStats) -> Result<(), UiError> {
        stats.texture_uploads += 1;
        match &u.kind {
            TextureUpdateKind::Create { w, h } => self.create_texture(u.tex, *w, *h),
            TextureUpdateKind::Grow { w, h } => {
                let old = self.textures.remove(&u.tex);
                self.create_texture(u.tex, *w, *h);
                if let (Some(old), Some(new)) = (old, self.textures.get(&u.tex)) {
                    let mut enc =
                        self.device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("forge-ui atlas grow"),
                            });
                    enc.copy_texture_to_texture(
                        old.texture.as_image_copy(),
                        new.texture.as_image_copy(),
                        wgpu::Extent3d {
                            width: old.w.min(new.w),
                            height: old.h.min(new.h),
                            depth_or_array_layers: 1,
                        },
                    );
                    self.queue.submit([enc.finish()]);
                }
            }
            TextureUpdateKind::Write { x, y, w, h, data } => {
                let bpp = match u.tex {
                    TextureId::GlyphMask => 1,
                    _ => 4,
                };
                stats.texture_bytes += data.len() as u64;
                let t = self
                    .textures
                    .get(&u.tex)
                    .ok_or_else(|| UiError::Gpu(format!("write to missing texture {:?}", u.tex)))?;
                if x + w > t.w
                    || y + h > t.h
                    || data.len() as u64 != u64::from(*w) * u64::from(*h) * bpp
                {
                    return Err(UiError::Gpu(format!(
                        "texture write {w}x{h}@{x},{y} outside {:?} ({}x{}) or wrong size",
                        u.tex, t.w, t.h
                    )));
                }
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &t.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: *x, y: *y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * bpp as u32),
                        rows_per_image: Some(*h),
                    },
                    wgpu::Extent3d {
                        width: *w,
                        height: *h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        Ok(())
    }

    fn bind_group(&mut self) -> Result<wgpu::BindGroup, UiError> {
        if let Some(b) = &self.bind_group {
            return Ok(b.clone());
        }
        let view = |id| {
            self.textures
                .get(&id)
                .map(|t| &t.view)
                .ok_or_else(|| UiError::Gpu(format!("missing texture {id:?}")))
        };
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forge-ui textures"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view(TextureId::GlyphMask)?),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(view(TextureId::GlyphColor)?),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(view(TextureId::Images)?),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.bind_group = Some(bg.clone());
        Ok(bg)
    }

    fn blit_group(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forge-ui blit"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn new_ui_target(&self, size: PhysicalSize) -> Tex {
        self.make_tex(
            "forge-ui target",
            UI_FORMAT,
            size.w,
            size.h,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        )
    }

    /// An offscreen target (pixel goldens, thumbnails of UI).
    pub fn add_offscreen_target(&mut self, id: TargetId, size: PhysicalSize) {
        let ui = self.new_ui_target(size);
        let t = self.new_target(ui, None);
        self.targets.insert(id, t);
    }

    /// A window target: the persistent UI texture plus the window's surface (vsync, Fifo).
    pub fn add_surface_target(
        &mut self,
        id: TargetId,
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        size: PhysicalSize,
    ) -> Result<(), UiError> {
        let caps = surface.get_capabilities(adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| UiError::Gpu("surface reports no formats".into()))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.w.max(1),
            height: size.h.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&self.device, &config);
        let blit = blit_pipeline(
            &self.device,
            &self.blit_module,
            &self.blit_bgl,
            format,
            None,
        );
        let ui = self.new_ui_target(size);
        let t = self.new_target(
            ui,
            Some(Surface {
                surface,
                config,
                blit,
            }),
        );
        self.targets.insert(id, t);
        Ok(())
    }

    /// Register a texture rendered elsewhere, for `Primitive::Viewport`.
    pub fn register_external(&mut self, id: ExternalTexture, view: wgpu::TextureView) {
        self.external.insert(id, view);
    }

    /// Read an offscreen target back as tightly packed RGBA8 (sRGB-encoded) rows.
    pub fn read_pixels(&self, id: TargetId) -> Result<(u32, u32, Vec<u8>), UiError> {
        let t = self
            .targets
            .get(&id)
            .ok_or_else(|| UiError::Gpu(format!("no target {id:?}")))?;
        let (w, h) = (t.ui.w, t.ui.h);
        let row = w * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-ui readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("forge-ui readback"),
            });
        enc.copy_texture_to_buffer(
            t.ui.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let done = Arc::new(std::sync::Mutex::new(None));
        let d2 = done.clone();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            if let Ok(mut g) = d2.lock() {
                *g = Some(r.is_ok());
            }
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(gpu)?;
        let ok = done.lock().ok().and_then(|g| *g).unwrap_or(false);
        if !ok {
            return Err(UiError::Gpu("readback map failed".into()));
        }
        let data = slice.get_mapped_range().map_err(gpu)?;
        let mut out = Vec::with_capacity((row * h) as usize);
        for y in 0..h as usize {
            let s = y * padded as usize;
            out.extend_from_slice(&data[s..s + row as usize]);
        }
        drop(data);
        buf.unmap();
        Ok((w, h, out))
    }

    fn make_buffer(&self, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.max(16),
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn new_target(&self, ui: Tex, surface: Option<Surface>) -> Target {
        let cap = 4096u64;
        Target {
            ui,
            fresh: true,
            surface,
            instances: self.make_buffer(
                "forge-ui instances",
                cap * Instance::SIZE as u64,
                wgpu::BufferUsages::VERTEX,
            ),
            instance_capacity: cap,
            uploaded_generation: None,
            mesh_vb: self.make_buffer("forge-ui mesh vertices", 1024, wgpu::BufferUsages::VERTEX),
            mesh_vb_capacity: 1024,
            mesh_ib: self.make_buffer("forge-ui mesh indices", 1024, wgpu::BufferUsages::INDEX),
            mesh_ib_capacity: 1024,
            mesh_generation: None,
        }
    }

    /// Upload the instance image: everything on a new layout generation (or a grown
    /// buffer), otherwise only the ranges the frame changed. Returns instances written.
    fn upload_instances(&mut self, target: TargetId, list: &BatchList) -> Result<u32, UiError> {
        let need = list.instances.len() as u64;
        let (mut full, grow) = {
            let t = self
                .targets
                .get(&target)
                .ok_or_else(|| UiError::Gpu(format!("no target {target:?}")))?;
            (
                t.uploaded_generation != Some(list.generation),
                need > t.instance_capacity,
            )
        };
        if grow {
            let cap = need.next_power_of_two();
            let buf = self.make_buffer(
                "forge-ui instances",
                cap * Instance::SIZE as u64,
                wgpu::BufferUsages::VERTEX,
            );
            if let Some(t) = self.targets.get_mut(&target) {
                t.instances = buf;
                t.instance_capacity = cap;
            }
            full = true;
        }
        let Some(t) = self.targets.get_mut(&target) else {
            return Ok(0);
        };
        t.uploaded_generation = Some(list.generation);
        let mut written = 0u32;
        let mut write = |range: std::ops::Range<usize>, scratch: &mut Vec<u8>| {
            if range.is_empty() || range.end > list.instances.len() {
                return;
            }
            scratch.clear();
            for i in &list.instances[range.clone()] {
                i.write_bytes(scratch);
            }
            self.queue
                .write_buffer(&t.instances, (range.start * Instance::SIZE) as u64, scratch);
            written += (range.end - range.start) as u32;
        };
        if full {
            write(0..list.instances.len(), &mut self.scratch);
        } else {
            for r in &list.instance_uploads {
                write(r.start as usize..r.end as usize, &mut self.scratch);
            }
        }
        Ok(written)
    }

    /// Upload the mesh buffers: all of them when this target has not seen this generation
    /// (a reassembly) or a buffer had to grow; otherwise only the dirty meshes' ranges.
    fn upload_meshes(&mut self, target: TargetId, list: &BatchList) -> Result<(), UiError> {
        let current = self
            .targets
            .get(&target)
            .ok_or_else(|| UiError::Gpu(format!("no target {target:?}")))?
            .mesh_generation
            == Some(list.generation);
        if current && !list.meshes_changed {
            let Some(t) = self.targets.get(&target) else {
                return Ok(());
            };
            for r in &list.mesh_vertex_uploads {
                let (s, e) = (r.start as usize, r.end as usize);
                if s >= e || e > list.mesh_vertices.len() {
                    continue;
                }
                self.scratch.clear();
                for v in &list.mesh_vertices[s..e] {
                    v.write_bytes(&mut self.scratch);
                }
                self.queue.write_buffer(
                    &t.mesh_vb,
                    (s * GpuMeshVertex::SIZE) as u64,
                    &self.scratch,
                );
            }
            for r in &list.mesh_index_uploads {
                let (s, e) = (r.start as usize, r.end as usize);
                if s >= e || e > list.mesh_indices.len() {
                    continue;
                }
                self.scratch.clear();
                for i in &list.mesh_indices[s..e] {
                    self.scratch.extend_from_slice(&i.to_le_bytes());
                }
                self.queue
                    .write_buffer(&t.mesh_ib, (s * 4) as u64, &self.scratch);
            }
            return Ok(());
        }
        self.upload_all_meshes(target, list)
    }

    fn upload_all_meshes(&mut self, target: TargetId, list: &BatchList) -> Result<(), UiError> {
        let vb_need = (list.mesh_vertices.len() * GpuMeshVertex::SIZE) as u64;
        let ib_need = (list.mesh_indices.len() * 4) as u64;
        let (stale, vb_grow, ib_grow) = {
            let t = self
                .targets
                .get(&target)
                .ok_or_else(|| UiError::Gpu(format!("no target {target:?}")))?;
            (
                list.meshes_changed || t.mesh_generation != Some(list.generation),
                vb_need > t.mesh_vb_capacity,
                ib_need > t.mesh_ib_capacity,
            )
        };
        if !stale && !vb_grow && !ib_grow {
            return Ok(());
        }
        if vb_grow {
            let cap = vb_need.next_power_of_two();
            let b = self.make_buffer("forge-ui mesh vertices", cap, wgpu::BufferUsages::VERTEX);
            if let Some(t) = self.targets.get_mut(&target) {
                t.mesh_vb = b;
                t.mesh_vb_capacity = cap;
            }
        }
        if ib_grow {
            let cap = ib_need.next_power_of_two();
            let b = self.make_buffer("forge-ui mesh indices", cap, wgpu::BufferUsages::INDEX);
            if let Some(t) = self.targets.get_mut(&target) {
                t.mesh_ib = b;
                t.mesh_ib_capacity = cap;
            }
        }
        let Some(t) = self.targets.get_mut(&target) else {
            return Ok(());
        };
        t.mesh_generation = Some(list.generation);
        if !list.mesh_vertices.is_empty() {
            self.scratch.clear();
            for v in &list.mesh_vertices {
                v.write_bytes(&mut self.scratch);
            }
            self.queue.write_buffer(&t.mesh_vb, 0, &self.scratch);
        }
        if !list.mesh_indices.is_empty() {
            self.scratch.clear();
            for i in &list.mesh_indices {
                self.scratch.extend_from_slice(&i.to_le_bytes());
            }
            // Index writes must be 4-byte multiples: always true for u32 indices.
            self.queue.write_buffer(&t.mesh_ib, 0, &self.scratch);
        }
        Ok(())
    }
}

fn scissor_of(clip: Option<PxRect>, damage: &PxRect, w: u32, h: u32) -> Option<PxRect> {
    let full = PxRect { x: 0, y: 0, w, h };
    let s = damage.intersect(&full);
    let s = match clip {
        Some(c) => s.intersect(&c),
        None => s,
    };
    (!s.is_empty()).then_some(s)
}

impl UiRenderer for WgpuRenderer {
    fn upload(&mut self, update: &TextureUpdate) -> Result<(), UiError> {
        let mut s = FrameStats::default();
        self.apply_update(update, &mut s)
    }

    fn resize(&mut self, target: TargetId, size: PhysicalSize) -> Result<(), UiError> {
        let ui = self.new_ui_target(size);
        let device = self.device.clone();
        let t = self
            .targets
            .get_mut(&target)
            .ok_or_else(|| UiError::Gpu(format!("no target {target:?}")))?;
        t.ui = ui;
        t.fresh = true;
        if let Some(s) = t.surface.as_mut() {
            s.config.width = size.w.max(1);
            s.config.height = size.h.max(1);
            s.surface.configure(&device, &s.config);
        }
        Ok(())
    }

    fn render(
        &mut self,
        target: TargetId,
        batches: &BatchList,
        damage: &[PxRect],
    ) -> Result<FrameStats, UiError> {
        let mut stats = FrameStats {
            batches: batches.batches.len() as u32,
            instances: batches.instances.len() as u32,
            damage_rects: damage.len() as u32,
            damage_px: damage.iter().map(PxRect::area).sum(),
            extra_epochs: batches.extra_epochs,
            ..FrameStats::default()
        };
        let (tw, th) = {
            let t = self
                .targets
                .get(&target)
                .ok_or_else(|| UiError::Gpu(format!("no target {target:?}")))?;
            (t.ui.w, t.ui.h)
        };
        for u in &batches.pre_uploads {
            self.apply_update(u, &mut stats)?;
        }
        let mut g = Vec::with_capacity(16);
        for v in [tw as f32, th as f32, 0.0, 0.0] {
            g.extend_from_slice(&v.to_le_bytes());
        }
        self.queue.write_buffer(&self.globals, 0, &g);
        stats.instances_uploaded = self.upload_instances(target, batches)?;
        self.upload_meshes(target, batches)?;
        // Segments split at batches carrying uploads (atlas epochs): uploads are queue
        // writes, which execute between submits, so each segment is its own submit.
        let mut seg_start = 0;
        let n = batches.batches.len();
        let mut first_pass = true;
        while seg_start <= n {
            if seg_start < n {
                for u in batches.batches[seg_start].uploads.clone() {
                    self.apply_update(&u, &mut stats)?;
                }
            }
            let mut seg_end = seg_start + 1;
            while seg_end < n && batches.batches[seg_end].uploads.is_empty() {
                seg_end += 1;
            }
            let seg_end = seg_end.min(n);
            let bind = self.bind_group()?;
            let mut enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("forge-ui frame"),
                });
            {
                let t = self
                    .targets
                    .get_mut(&target)
                    .ok_or_else(|| UiError::Gpu(format!("no target {target:?}")))?;
                let load = if t.fresh && first_pass {
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                } else {
                    wgpu::LoadOp::Load
                };
                t.fresh = false;
                first_pass = false;
                let view = t.ui.view.clone();
                let instances = t.instances.clone();
                let mesh_vb = t.mesh_vb.clone();
                let mesh_ib = t.mesh_ib.clone();
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("forge-ui damage"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                for d in damage {
                    for b in &batches.batches[seg_start.min(n)..seg_end] {
                        let Some(s) = scissor_of(b.clip, d, tw, th) else {
                            continue;
                        };
                        match &b.kind {
                            BatchKind::Instances { start, count } => {
                                if *count == 0 {
                                    continue;
                                }
                                pass.set_pipeline(&self.pipeline);
                                pass.set_bind_group(0, &bind, &[]);
                                pass.set_vertex_buffer(0, instances.slice(..));
                                pass.set_scissor_rect(s.x, s.y, s.w, s.h);
                                pass.draw(0..6, *start..start + count);
                                stats.draw_calls += 1;
                            }
                            BatchKind::Mesh {
                                first_index,
                                index_count,
                            } => {
                                if *index_count == 0 {
                                    continue;
                                }
                                pass.set_pipeline(&self.mesh_pipeline);
                                pass.set_bind_group(0, &bind, &[]);
                                pass.set_vertex_buffer(0, mesh_vb.slice(..));
                                pass.set_index_buffer(mesh_ib.slice(..), wgpu::IndexFormat::Uint32);
                                pass.set_scissor_rect(s.x, s.y, s.w, s.h);
                                pass.draw_indexed(*first_index..first_index + index_count, 0, 0..1);
                                stats.draw_calls += 1;
                                stats.mesh_draws += 1;
                            }
                            BatchKind::Viewport { tex, rect } => {
                                let Some(v) = self.external.get(tex) else {
                                    continue;
                                };
                                let Some(s) = scissor_of(Some(*rect), &s, tw, th) else {
                                    continue;
                                };
                                let bg = self.blit_group(v);
                                pass.set_pipeline(&self.ui_blit);
                                pass.set_bind_group(0, &bg, &[]);
                                pass.set_viewport(
                                    rect.x as f32,
                                    rect.y as f32,
                                    rect.w as f32,
                                    rect.h as f32,
                                    0.0,
                                    1.0,
                                );
                                pass.set_scissor_rect(s.x, s.y, s.w, s.h);
                                pass.draw(0..3, 0..1);
                                pass.set_viewport(0.0, 0.0, tw as f32, th as f32, 0.0, 1.0);
                                stats.draw_calls += 1;
                            }
                        }
                    }
                }
            }
            self.queue.submit([enc.finish()]);
            seg_start = seg_end;
            if seg_start >= n {
                break;
            }
        }
        // Composite onto the swapchain (windows only), on damaged frames only.
        let has_surface = self
            .targets
            .get(&target)
            .is_some_and(|t| t.surface.is_some());
        if has_surface && !damage.is_empty() {
            let t = self
                .targets
                .get(&target)
                .ok_or_else(|| UiError::Gpu("target vanished".into()))?;
            let Some(s) = t.surface.as_ref() else {
                return Ok(stats);
            };
            let frame = match s.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(f)
                | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                    return Ok(stats);
                }
                other => {
                    return Err(UiError::Gpu(format!("surface: {other:?}")));
                }
            };
            let fview = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let bg = self.blit_group(&t.ui.view);
            let mut enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("forge-ui composite"),
                });
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("forge-ui composite"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &fview,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&s.blit);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
                stats.draw_calls += 1;
            }
            self.queue.submit([enc.finish()]);
            self.queue.present(frame);
        }
        Ok(stats)
    }
}
