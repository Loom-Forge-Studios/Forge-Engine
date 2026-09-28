//! The 2D render path (Ch.35 §35.1): **its own renderer**, not the 3D one with an
//! orthographic camera — no depth buffer to fight pixel snapping, draw order is exactly the
//! layer/order the user set, sprites are instanced quads, and 2D lights are shadowed fans.
//!
//! A frame is four render-graph passes (`forge_gpu::RenderGraph`, retained: a frame whose
//! size did not change executes last frame's compiled graph, and allocates nothing):
//!
//! | Pass | Does |
//! |---|---|
//! | `2d.sprites` | sprites (instanced, one draw per batch) and meshes into albedo + normal targets at the reference size |
//! | `2d.lights` | every light's visibility fan, additively, `N . L` against the normal target — all lights one draw call |
//! | `2d.composite` | albedo x (ambient + light) over the clear colour |
//! | `2d.upscale` | the reference image into the output at a whole-number scale, letterboxed |
//!
//! Positions reach the GPU only through [`last_mile`], the one narrowing site.

pub mod last_mile;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::rc::Rc;

use forge_gpu::{
    Access, CompiledGraph, ExecStats, GpuDevice, GpuError, GpuTimer, Handle, Imports, PassTiming,
    RenderGraph, TextureDesc, TransientPool, wgpu,
};

use crate::atlas::Image;
use crate::sprite::{DrawKind, Frame2d, TextureId};
use crate::{Error2d, Faults2d};
use last_mile::{INSTANCE_BYTES, LIGHT_VERTEX_BYTES, MESH_VERTEX_BYTES, PrepareStats, Prepared};

/// The G-buffer albedo and the composited scene.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// The G-buffer normals (world, encoded `n * 0.5 + 0.5`, alpha = coverage).
pub const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The light buffer (HDR: lights add up past 1).
pub const LIGHT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const FRAME_UNIFORM_BYTES: u64 = 96;

/// Pass names, in execution order (the perf gate's `render2d.frame.gpu.<pass>` rows).
pub const PASS_NAMES: [&str; 4] = ["2d.sprites", "2d.lights", "2d.composite", "2d.upscale"];

/// How a texture is sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Filter2d {
    /// Nearest texel: pixel art.
    #[default]
    Nearest,
    /// Bilinear: painted art.
    Linear,
}

/// Renderer settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions2d {
    /// Output size (pixels) and format (a render attachment the caller owns).
    pub width: u32,
    pub height: u32,
    pub output_format: wgpu::TextureFormat,
    /// Time every pass (CPU recording; GPU where the device has timestamps).
    pub timing: bool,
    /// W2 fault switches (test builds only).
    pub faults: Faults2d,
}

impl RenderOptions2d {
    /// `width x height`, sRGB RGBA8 output, untimed.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            output_format: wgpu::TextureFormat::Rgba8UnormSrgb,
            timing: false,
            faults: Faults2d::default(),
        }
    }
}

/// What rendering a frame did.
#[derive(Clone, Debug, PartialEq)]
pub struct Report2d {
    pub exec: ExecStats,
    pub passes: Vec<PassTiming>,
    pub prepare: PrepareStats,
    /// The reference size drawn at and the whole-number scale.
    pub render_size: (u32, u32),
    pub scale: u32,
    /// Whether the frame graph was built for this frame (false in a steady state).
    pub graph_rebuilt: bool,
    /// CPU time of the last mile (cull, sort, batch, shadow polygons, narrowing), ns.
    pub prepare_ns: u64,
}

struct Tex {
    size: (u32, u32),
    _color: wgpu::Texture,
    _normal: Option<wgpu::Texture>,
}

struct Pipelines {
    sprite: wgpu::RenderPipeline,
    mesh: wgpu::RenderPipeline,
    light: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    upscale: wgpu::RenderPipeline,
    light_bgl: wgpu::BindGroupLayout,
    composite_bgl: wgpu::BindGroupLayout,
    upscale_bgl: wgpu::BindGroupLayout,
}

/// What the retained pass bodies read each frame.
#[derive(Default)]
struct Shared {
    frame_bg: Option<wgpu::BindGroup>,
    textures: BTreeMap<TextureId, wgpu::BindGroup>,
    draws: Vec<last_mile::Draw>,
    instances: Option<wgpu::Buffer>,
    mesh_vb: Option<wgpu::Buffer>,
    mesh_ib: Option<wgpu::Buffer>,
    light_vb: Option<wgpu::Buffer>,
    light_vertices: u32,
    clear: wgpu::Color,
    light_bg: Option<(wgpu::Texture, wgpu::BindGroup)>,
    composite_bg: Option<(wgpu::Texture, wgpu::Texture, wgpu::BindGroup)>,
    upscale_bg: Option<(wgpu::Texture, wgpu::BindGroup)>,
}

struct Cached {
    size: (u32, u32),
    graph: CompiledGraph,
    out: Handle,
}

/// See the module docs.
pub struct Renderer2d {
    opts: RenderOptions2d,
    device: wgpu::Device,
    p: Rc<Pipelines>,
    texture_bgl: wgpu::BindGroupLayout,
    frame_buf: wgpu::Buffer,
    flat_normal: wgpu::Texture,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    textures: BTreeMap<TextureId, Tex>,
    next_texture: u32,
    shared: Rc<RefCell<Shared>>,
    prepared: Prepared,
    cached: Option<Cached>,
    pool: TransientPool,
    timer: GpuTimer,
    white: TextureId,
}

fn gpu(e: GpuError) -> Error2d {
    Error2d::Gpu { why: e.to_string() }
}

fn ensure(
    device: &wgpu::Device,
    buf: &mut Option<wgpu::Buffer>,
    len: u64,
    usage: wgpu::BufferUsages,
    label: &str,
) {
    if buf.as_ref().is_some_and(|b| b.size() >= len) {
        return;
    }
    *buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: len.next_power_of_two().max(256),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }));
}

fn tex_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn pipelines(
    device: &wgpu::Device,
    out_format: wgpu::TextureFormat,
) -> Result<(Pipelines, wgpu::BindGroupLayout, wgpu::BindGroupLayout), Error2d> {
    let common = include_str!("shaders/common.wgsl");
    let module = |name: &str, body: &str| {
        forge_gpu::shader::compile_wgsl(device, name, &format!("{common}\n{body}")).map_err(gpu)
    };
    let gbuf = module(
        "forge-2d/gbuffer.wgsl",
        include_str!("shaders/gbuffer.wgsl"),
    )?;
    let light = module("forge-2d/light.wgsl", include_str!("shaders/light.wgsl"))?;
    let post = module("forge-2d/post.wgsl", include_str!("shaders/post.wgsl"))?;
    let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-2d frame"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(FRAME_UNIFORM_BYTES),
            },
            count: None,
        }],
    });
    let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-2d sprite texture"),
        entries: &[
            tex_entry(0, true),
            tex_entry(1, true),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let light_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-2d light inputs"),
        entries: &[tex_entry(0, false)],
    });
    let composite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-2d composite inputs"),
        entries: &[tex_entry(0, false), tex_entry(1, false)],
    });
    let upscale_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-2d upscale input"),
        entries: &[tex_entry(2, false)],
    });
    let layout = |label: &str, groups: &[&wgpu::BindGroupLayout]| {
        let groups: Vec<Option<&wgpu::BindGroupLayout>> = groups.iter().map(|g| Some(*g)).collect();
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &groups,
            immediate_size: 0,
        })
    };
    let gbuf_layout = layout("forge-2d gbuffer", &[&frame_bgl, &texture_bgl]);
    let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
    let gtargets = [
        Some(wgpu::ColorTargetState {
            format: COLOR_FORMAT,
            blend: alpha,
            write_mask: wgpu::ColorWrites::ALL,
        }),
        Some(wgpu::ColorTargetState {
            format: NORMAL_FORMAT,
            blend: alpha,
            write_mask: wgpu::ColorWrites::ALL,
        }),
    ];
    let opts_c = wgpu::PipelineCompilationOptions::default;
    let sprite_attrs = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2,
        4 => Float32x4, 5 => Unorm8x4, 6 => Uint32
    ];
    let sprite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forge-2d sprites"),
        layout: Some(&gbuf_layout),
        vertex: wgpu::VertexState {
            module: &gbuf,
            entry_point: Some("vs_sprite"),
            compilation_options: opts_c(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_BYTES as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &sprite_attrs,
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &gbuf,
            entry_point: Some("fs_gbuffer"),
            compilation_options: opts_c(),
            targets: &gtargets,
        }),
        multiview_mask: None,
        cache: None,
    });
    let mesh_attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4];
    let mesh = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forge-2d meshes"),
        layout: Some(&gbuf_layout),
        vertex: wgpu::VertexState {
            module: &gbuf,
            entry_point: Some("vs_mesh"),
            compilation_options: opts_c(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: MESH_VERTEX_BYTES as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &mesh_attrs,
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &gbuf,
            entry_point: Some("fs_gbuffer"),
            compilation_options: opts_c(),
            targets: &gtargets,
        }),
        multiview_mask: None,
        cache: None,
    });
    let additive = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::REPLACE,
    };
    let light_attrs = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4
    ];
    let light_layout = layout("forge-2d lights", &[&frame_bgl, &light_bgl]);
    let light_p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forge-2d lights"),
        layout: Some(&light_layout),
        vertex: wgpu::VertexState {
            module: &light,
            entry_point: Some("vs_light"),
            compilation_options: opts_c(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: LIGHT_VERTEX_BYTES as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &light_attrs,
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &light,
            entry_point: Some("fs_light"),
            compilation_options: opts_c(),
            targets: &[Some(wgpu::ColorTargetState {
                format: LIGHT_FORMAT,
                blend: Some(additive),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let full =
        |label: &str, layout: &wgpu::PipelineLayout, fs: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &post,
                    entry_point: Some("vs_full"),
                    compilation_options: opts_c(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &post,
                    entry_point: Some(fs),
                    compilation_options: opts_c(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
    let composite_layout = layout("forge-2d composite", &[&frame_bgl, &composite_bgl]);
    let upscale_layout = layout("forge-2d upscale", &[&frame_bgl, &upscale_bgl]);
    let composite = full(
        "forge-2d composite",
        &composite_layout,
        "fs_composite",
        COLOR_FORMAT,
    );
    let upscale = full(
        "forge-2d upscale",
        &upscale_layout,
        "fs_upscale",
        out_format,
    );
    Ok((
        Pipelines {
            sprite,
            mesh,
            light: light_p,
            composite,
            upscale,
            light_bgl,
            composite_bgl,
            upscale_bgl,
        },
        frame_bgl,
        texture_bgl,
    ))
}

impl Renderer2d {
    /// A renderer on `dev` drawing `opts.width x opts.height`.
    pub fn new(dev: &GpuDevice, opts: RenderOptions2d) -> Result<Self, Error2d> {
        if opts.width == 0 || opts.height == 0 {
            return Err(Error2d::invalid("a zero output size"));
        }
        let device = dev.device.clone();
        let (p, frame_bgl, texture_bgl) = pipelines(&device, opts.output_format)?;
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-2d frame uniforms"),
            size: FRAME_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forge-2d frame"),
            layout: &frame_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buf.as_entire_binding(),
            }],
        });
        let flat_normal = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("forge-2d flat normal"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: NORMAL_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        forge_gpu::transfer::write_texture(dev, &flat_normal, &[128, 128, 255, 255])
            .map_err(gpu)?;
        let sampler = |f: wgpu::FilterMode, label: &str| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                mag_filter: f,
                min_filter: f,
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                ..wgpu::SamplerDescriptor::default()
            })
        };
        let nearest = sampler(wgpu::FilterMode::Nearest, "forge-2d nearest");
        let linear = sampler(wgpu::FilterMode::Linear, "forge-2d linear");
        let shared = Rc::new(RefCell::new(Shared {
            frame_bg: Some(frame_bg),
            ..Shared::default()
        }));
        let mut r = Self {
            opts,
            device,
            p: Rc::new(p),
            texture_bgl,
            frame_buf,
            flat_normal,
            nearest,
            linear,
            textures: BTreeMap::new(),
            next_texture: 0,
            shared,
            prepared: Prepared::default(),
            cached: None,
            pool: TransientPool::new(),
            timer: GpuTimer::new(dev),
            white: TextureId(0),
        };
        r.white = r.add_texture(
            dev,
            &Image::filled(1, 1, [255, 255, 255, 255]),
            None,
            Filter2d::Nearest,
        )?;
        Ok(r)
    }

    /// The options it was made with.
    #[must_use]
    pub fn options(&self) -> &RenderOptions2d {
        &self.opts
    }

    /// A 1x1 white texture (untextured shapes and particles).
    #[must_use]
    pub fn white(&self) -> TextureId {
        self.white
    }

    /// Whether GPU pass timing is available.
    #[must_use]
    pub fn gpu_timing_supported(&self) -> bool {
        self.timer.supported()
    }

    /// Upload an image (an atlas page, a tile set) and optionally its normal map (same
    /// size). Returns its id for sprites and meshes.
    pub fn add_texture(
        &mut self,
        dev: &GpuDevice,
        img: &Image,
        normal: Option<&Image>,
        filter: Filter2d,
    ) -> Result<TextureId, Error2d> {
        if img.w == 0 || img.h == 0 || img.rgba.len() != (img.w * img.h * 4) as usize {
            return Err(Error2d::invalid(
                "a texture needs w*h*4 bytes and a non-zero size",
            ));
        }
        if let Some(n) = normal
            && ((n.w, n.h) != (img.w, img.h) || n.rgba.len() != img.rgba.len())
        {
            return Err(Error2d::invalid(
                "a normal map must match its texture's size",
            ));
        }
        let make = |format: wgpu::TextureFormat,
                    label: &str,
                    bytes: &[u8]|
         -> Result<wgpu::Texture, Error2d> {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: img.w,
                    height: img.h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            forge_gpu::transfer::write_texture(dev, &t, bytes).map_err(gpu)?;
            Ok(t)
        };
        let color = make(COLOR_FORMAT, "forge-2d texture", &img.rgba)?;
        let normal_tex = match normal {
            Some(n) => Some(make(NORMAL_FORMAT, "forge-2d normal map", &n.rgba)?),
            None => None,
        };
        let cv = color.create_view(&wgpu::TextureViewDescriptor::default());
        let nv = normal_tex
            .as_ref()
            .unwrap_or(&self.flat_normal)
            .create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = match filter {
            Filter2d::Nearest => &self.nearest,
            Filter2d::Linear => &self.linear,
        };
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forge-2d texture"),
            layout: &self.texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&cv),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&nv),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        let id = TextureId(self.next_texture);
        self.next_texture += 1;
        self.shared.borrow_mut().textures.insert(id, bind);
        self.textures.insert(
            id,
            Tex {
                size: (img.w, img.h),
                _color: color,
                _normal: normal_tex,
            },
        );
        Ok(id)
    }

    /// A texture's size.
    #[must_use]
    pub fn texture_size(&self, id: TextureId) -> Option<(u32, u32)> {
        self.textures.get(&id).map(|t| t.size)
    }

    /// Drop a texture (sprites naming it are refused afterwards).
    pub fn remove_texture(&mut self, id: TextureId) -> bool {
        self.shared.borrow_mut().textures.remove(&id);
        self.textures.remove(&id).is_some()
    }

    /// Render `frame` into `target` (the output size and format, a render attachment).
    pub fn render(
        &mut self,
        dev: &GpuDevice,
        frame: &Frame2d,
        target: &wgpu::Texture,
    ) -> Result<Report2d, Error2d> {
        let (w, h) = (self.opts.width, self.opts.height);
        if (target.width(), target.height(), target.format()) != (w, h, self.opts.output_format) {
            return Err(Error2d::invalid(format!(
                "target is {}x{} {:?}; the renderer draws {w}x{h} {:?}",
                target.width(),
                target.height(),
                target.format(),
                self.opts.output_format
            )));
        }
        let (rw, rh) = frame.camera.render_size(w, h);
        let vp = frame.camera.fit(w, h);
        let textures = &self.textures;
        let t0 = std::time::Instant::now();
        self.prepared.fill(
            frame,
            rw,
            rh,
            &|t| textures.contains_key(&t),
            self.opts.faults.no_batching(),
        )?;
        let prepare_ns = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
        // Upload.
        let q = &dev.queue;
        let uniform = last_mile::frame_uniform(
            (rw, rh),
            (w, h),
            frame.ambient,
            frame.clear,
            (vp.x, vp.y, vp.scale),
            self.opts.faults.light_repeats(),
        );
        q.write_buffer(&self.frame_buf, 0, &uniform);
        {
            let mut sh = self.shared.borrow_mut();
            let sh = &mut *sh;
            let pr = &self.prepared;
            let d = &self.device;
            ensure(
                d,
                &mut sh.instances,
                pr.instances.len() as u64,
                wgpu::BufferUsages::VERTEX,
                "forge-2d instances",
            );
            ensure(
                d,
                &mut sh.mesh_vb,
                pr.mesh_vertices.len() as u64,
                wgpu::BufferUsages::VERTEX,
                "forge-2d mesh vertices",
            );
            ensure(
                d,
                &mut sh.mesh_ib,
                (pr.mesh_indices.len() * 4) as u64,
                wgpu::BufferUsages::INDEX,
                "forge-2d mesh indices",
            );
            ensure(
                d,
                &mut sh.light_vb,
                pr.light_vertices.len() as u64,
                wgpu::BufferUsages::VERTEX,
                "forge-2d light fans",
            );
            if let Some(b) = &sh.instances
                && !pr.instances.is_empty()
            {
                q.write_buffer(b, 0, &pr.instances);
            }
            if let Some(b) = &sh.mesh_vb
                && !pr.mesh_vertices.is_empty()
            {
                q.write_buffer(b, 0, &pr.mesh_vertices);
            }
            if let Some(b) = &sh.mesh_ib
                && !pr.mesh_indices.is_empty()
            {
                q.write_buffer(b, 0, &pr.mesh_index_bytes);
            }
            if let Some(b) = &sh.light_vb
                && !pr.light_vertices.is_empty()
            {
                q.write_buffer(b, 0, &pr.light_vertices);
            }
            sh.draws.clear();
            sh.draws.extend_from_slice(&pr.draws);
            sh.light_vertices = (pr.light_vertices.len() / LIGHT_VERTEX_BYTES) as u32;
            let c = frame.clear;
            sh.clear = wgpu::Color {
                r: c[0],
                g: c[1],
                b: c[2],
                a: c[3],
            };
        }
        let graph_rebuilt = !matches!(&self.cached, Some(c) if c.size == (rw, rh));
        if graph_rebuilt {
            let (g, out) = self.build_graph(rw, rh);
            self.cached = Some(Cached {
                size: (rw, rh),
                graph: g.compile().map_err(gpu)?,
                out,
            });
        }
        let Some(cached) = self.cached.as_mut() else {
            return Err(Error2d::Gpu {
                why: "no compiled 2D frame graph".into(),
            });
        };
        let mut imports = Imports::new();
        imports.texture(cached.out, target);
        let timer = if self.opts.timing {
            Some(&mut self.timer)
        } else {
            None
        };
        let (exec, passes) =
            match cached
                .graph
                .execute_retained(dev, &mut self.pool, &imports, timer)
            {
                Ok(r) => r,
                Err(e) => {
                    self.cached = None;
                    return Err(gpu(e));
                }
            };
        Ok(Report2d {
            exec,
            passes,
            prepare: self.prepared.stats,
            render_size: (rw, rh),
            scale: vp.scale,
            graph_rebuilt,
            prepare_ns,
        })
    }

    fn build_graph(&self, rw: u32, rh: u32) -> (RenderGraph, Handle) {
        let (w, h) = (self.opts.width, self.opts.height);
        let mut g = RenderGraph::new();
        let out = g.import_texture("output", TextureDesc::d2(w, h, self.opts.output_format));
        let albedo = g.create_texture("2d albedo", TextureDesc::d2(rw, rh, COLOR_FORMAT));
        let normal = g.create_texture("2d normal", TextureDesc::d2(rw, rh, NORMAL_FORMAT));
        let light = g.create_texture("2d light", TextureDesc::d2(rw, rh, LIGHT_FORMAT));
        let scene = g.create_texture("2d scene", TextureDesc::d2(rw, rh, COLOR_FORMAT));

        // 2d.sprites
        let (albedo, normal) = {
            let mut pb = g.add_pass(PASS_NAMES[0]);
            let a = pb.write(albedo, Access::ColorAttachment);
            let n = pb.write(normal, Access::ColorAttachment);
            let shared = self.shared.clone();
            let p = self.p.clone();
            pb.run_retained(move |ctx| {
                let sh = shared.borrow();
                let (av, nv) = (ctx.view(a)?.clone(), ctx.view(n)?.clone());
                let mut rp = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("forge-2d sprites"),
                    color_attachments: &[
                        Some(wgpu::RenderPassColorAttachment {
                            view: &av,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                        Some(wgpu::RenderPassColorAttachment {
                            view: &nv,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                    ],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                let Some(fbg) = &sh.frame_bg else {
                    return Ok(());
                };
                rp.set_bind_group(0, fbg, &[]);
                let mut current: Option<DrawKind> = None;
                for d in &sh.draws {
                    let Some(tbg) = sh.textures.get(&d.texture) else {
                        continue;
                    };
                    match d.kind {
                        DrawKind::Sprite => {
                            let Some(ib) = &sh.instances else { continue };
                            if current != Some(DrawKind::Sprite) {
                                rp.set_pipeline(&p.sprite);
                                rp.set_vertex_buffer(0, ib.slice(..));
                                current = Some(DrawKind::Sprite);
                            }
                            rp.set_bind_group(1, tbg, &[]);
                            rp.draw(0..6, d.first..d.first + d.count);
                        }
                        DrawKind::Mesh => {
                            let (Some(vb), Some(ib)) = (&sh.mesh_vb, &sh.mesh_ib) else {
                                continue;
                            };
                            if current != Some(DrawKind::Mesh) {
                                rp.set_pipeline(&p.mesh);
                                rp.set_vertex_buffer(0, vb.slice(..));
                                rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                                current = Some(DrawKind::Mesh);
                            }
                            rp.set_bind_group(1, tbg, &[]);
                            rp.draw_indexed(d.first..d.first + d.count, 0, 0..1);
                        }
                    }
                }
                Ok(())
            });
            (a, n)
        };

        // 2d.lights
        let light = {
            let mut pb = g.add_pass(PASS_NAMES[1]);
            pb.read(normal, Access::Sampled);
            let l = pb.write(light, Access::ColorAttachment);
            let shared = self.shared.clone();
            let p = self.p.clone();
            let device = self.device.clone();
            pb.run_retained(move |ctx| {
                let mut sh = shared.borrow_mut();
                let sh = &mut *sh;
                let ntex = ctx.texture(normal)?.clone();
                let lv = ctx.view(l)?.clone();
                let bg = match &sh.light_bg {
                    Some((t, bg)) if *t == ntex => bg.clone(),
                    _ => {
                        let v = ntex.create_view(&wgpu::TextureViewDescriptor::default());
                        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("forge-2d light inputs"),
                            layout: &p.light_bgl,
                            entries: &[wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&v),
                            }],
                        });
                        sh.light_bg = Some((ntex.clone(), bg.clone()));
                        bg
                    }
                };
                let mut rp = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("forge-2d lights"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &lv,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                let (Some(fbg), Some(vb)) = (&sh.frame_bg, &sh.light_vb) else {
                    return Ok(());
                };
                if sh.light_vertices > 0 {
                    rp.set_pipeline(&p.light);
                    rp.set_bind_group(0, fbg, &[]);
                    rp.set_bind_group(1, &bg, &[]);
                    rp.set_vertex_buffer(0, vb.slice(..));
                    rp.draw(0..sh.light_vertices, 0..1);
                }
                Ok(())
            });
            l
        };

        // 2d.composite
        let scene = {
            let mut pb = g.add_pass(PASS_NAMES[2]);
            pb.read(albedo, Access::Sampled);
            pb.read(light, Access::Sampled);
            let s = pb.write(scene, Access::ColorAttachment);
            let shared = self.shared.clone();
            let p = self.p.clone();
            let device = self.device.clone();
            pb.run_retained(move |ctx| {
                let mut sh = shared.borrow_mut();
                let sh = &mut *sh;
                let (at, lt) = (ctx.texture(albedo)?.clone(), ctx.texture(light)?.clone());
                let sv = ctx.view(s)?.clone();
                let bg = match &sh.composite_bg {
                    Some((a, l, bg)) if *a == at && *l == lt => bg.clone(),
                    _ => {
                        let av = at.create_view(&wgpu::TextureViewDescriptor::default());
                        let lv = lt.create_view(&wgpu::TextureViewDescriptor::default());
                        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("forge-2d composite inputs"),
                            layout: &p.composite_bgl,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(&av),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::TextureView(&lv),
                                },
                            ],
                        });
                        sh.composite_bg = Some((at.clone(), lt.clone(), bg.clone()));
                        bg
                    }
                };
                let mut rp = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("forge-2d composite"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &sv,
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
                let Some(fbg) = &sh.frame_bg else {
                    return Ok(());
                };
                rp.set_pipeline(&p.composite);
                rp.set_bind_group(0, fbg, &[]);
                rp.set_bind_group(1, &bg, &[]);
                rp.draw(0..3, 0..1);
                Ok(())
            });
            s
        };

        // 2d.upscale
        let out_v = {
            let mut pb = g.add_pass(PASS_NAMES[3]);
            pb.read(scene, Access::Sampled);
            let o = pb.write(out, Access::ColorAttachment);
            pb.side_effect();
            let shared = self.shared.clone();
            let p = self.p.clone();
            let device = self.device.clone();
            pb.run_retained(move |ctx| {
                let mut sh = shared.borrow_mut();
                let sh = &mut *sh;
                let st = ctx.texture(scene)?.clone();
                let ov = ctx.view(o)?.clone();
                let bg = match &sh.upscale_bg {
                    Some((t, bg)) if *t == st => bg.clone(),
                    _ => {
                        let v = st.create_view(&wgpu::TextureViewDescriptor::default());
                        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("forge-2d upscale input"),
                            layout: &p.upscale_bgl,
                            entries: &[wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(&v),
                            }],
                        });
                        sh.upscale_bg = Some((st.clone(), bg.clone()));
                        bg
                    }
                };
                let mut rp = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("forge-2d upscale"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &ov,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(sh.clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                let Some(fbg) = &sh.frame_bg else {
                    return Ok(());
                };
                rp.set_pipeline(&p.upscale);
                rp.set_bind_group(0, fbg, &[]);
                rp.set_bind_group(1, &bg, &[]);
                rp.draw(0..3, 0..1);
                Ok(())
            });
            o
        };
        let _ = out_v;
        (g, out)
    }
}
