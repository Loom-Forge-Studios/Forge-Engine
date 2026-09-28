//! The GPU side: pipelines, buffers, and the per-frame render graph.
//!
//! One frame is one `forge_gpu::RenderGraph`:
//!
//! ```text
//! shadow.cascade0..N  write/modify the cascade array (depth only, reversed)
//! shell.sky           write hdr (clear to background), point sprites
//! <depth pass>...     modify hdr, write its depth    (own clear, reversed-Z), read cascades
//!                     (the view depth's passes, back to front: one for a single-frame world)
//! tonemap             read hdr, write output (the caller's target)
//! ```
//!
//! The depth passes' buffers have identical descriptions and disjoint lifetimes, so the graph's
//! aliasing puts them in **one** physical allocation — each pass still clears it. Passes
//! with nothing in them are not added. Buffers persist across frames and grow on demand; the
//! graph's transient pool makes a steady-state frame allocate nothing on the GPU.

use std::cell::RefCell;
use std::num::NonZeroU64;
use std::rc::Rc;

use forge_frames::{FrameResolver, Tick};
use forge_gpu::{
    Access, CompiledGraph, ExecStats, GpuDevice, GpuTimer, GraphPlan, Imports, PassTiming,
    RenderGraph, TextureDesc, TransientPool, wgpu,
};

use crate::atmosphere::{
    self, AtmospherePlaceholder, AtmosphereReport, AtmosphereSettings, GpuAtmosphere,
};
use crate::camera::Camera;
use crate::cluster::ClusterGrid;
use crate::depth::{DepthSetup, MAX_DEPTH_PASSES};
use crate::error::RenderError;
use crate::last_mile;
use crate::prepare::{
    self, DrawOrder, MeshBounds, PASS_BLOCK_WORDS, PASS_BLOCKS, POINT_WORDS, POOLED_DRAW,
    PrepareInput, PrepareScratch, PrepareStats, PreparedFrame, SKY_BLOCK, SPRITE_WORDS,
};
use crate::scene::{
    Material, MaterialId, MeshData, MeshId, MeshPoolId, Scene, Vertex, vertex_bounds,
};
use crate::shadow::CascadeSettings;

/// The HDR scene target format.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Depth format of every depth pass and cascade.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const VERTEX_STRIDE: u64 = 32;
const PASS_BLOCK_BYTES: u64 = (PASS_BLOCK_WORDS * 4) as u64;

/// How a renderer is set up.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    /// Output width, pixels.
    pub width: u32,
    /// Output height, pixels.
    pub height: u32,
    /// The format of the textures [`Renderer::render`] draws into.
    pub output_format: wgpu::TextureFormat,
    /// Light clusters.
    pub clusters: ClusterGrid,
    /// Sun shadow cascades (`None`: no shadows).
    pub shadows: Option<CascadeSettings>,
    /// Draw order inside a depth pass.
    pub draw_order: DrawOrder,
    /// Measure every pass (CPU recording and GPU timestamps); waits for each frame.
    pub timing: bool,
    /// Depth setup: by default the frame resolver's view depth, reversed-Z. A conventional
    /// (non-reversed) depth exists only for W2 positive controls.
    pub depth: DepthSetup,
    /// Write exposed linear radiance (no tone curve, no sRGB) — for radiometric tests, into a
    /// float target. **Never set in an application.**
    #[doc(hidden)]
    pub linear_output: bool,
    /// W2 positive control for the sprite guards: draw sub-pixel sources as hard 0.75 px
    /// discs instead of the flux-conserving PSF. **Never set.**
    #[doc(hidden)]
    pub hard_disc_sprites: bool,
    /// W2 positive control for the perf gate: evaluate the sky this many extra times per
    /// pixel, so `shell.sky` regresses. **Never set.**
    #[doc(hidden)]
    pub perf_fault_sky_repeats: u32,
    /// W2 positive control for the clear-sky atmosphere golden: scatter sunlight off the air
    /// isotropically in the sky pass instead of with the Rayleigh phase function. **Never set.**
    #[doc(hidden)]
    pub isotropic_rayleigh_for_tests: bool,
}

impl RenderOptions {
    /// Defaults for a `width x height` sRGB target.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            output_format: wgpu::TextureFormat::Rgba8UnormSrgb,
            clusters: ClusterGrid::default(),
            shadows: Some(CascadeSettings::default()),
            draw_order: DrawOrder::FrontToBack,
            timing: false,
            depth: DepthSetup::default(),
            linear_output: false,
            hard_disc_sprites: false,
            perf_fault_sky_repeats: 0,
            isotropic_rayleigh_for_tests: false,
        }
    }
}

/// What one frame did.
#[derive(Clone, Debug)]
pub struct FrameReport {
    /// Graph execution.
    pub exec: ExecStats,
    /// Per pass, in execution order (empty unless [`RenderOptions::timing`]).
    pub passes: Vec<PassTiming>,
    /// Preparation counts.
    pub prepare: PrepareStats,
    /// Whether the frame graph was built and compiled for this frame (its topology — the
    /// depth passes drawn, the cascades, what samples them — changed). A steady scene reuses the
    /// compiled graph: `false`.
    pub graph_rebuilt: bool,
}

#[derive(Clone)]
struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// A mesh pool topology's index list on the GPU (a draw group at and above `POOLED_DRAW`).
#[derive(Clone)]
struct GpuGroup {
    indices: wgpu::Buffer,
    index_count: u32,
}

/// A pool of equal-sized meshes (WP-U21, [`Renderer::add_mesh_pool`]).
struct MeshPool {
    /// Vertices every mesh of the pool has.
    vertex_count: u32,
    /// Its topologies' draw groups (indices into `FrameShared::groups`), by topology.
    topologies: Vec<u32>,
    /// Slots freed by `remove_mesh` (their first vertex in the arena), reused first.
    free: Vec<u32>,
}

/// Bytes of one vertex in the arena and in a mesh's vertex buffer: position, normal, uv.
const ARENA_VERTEX_BYTES: u64 = VERTEX_STRIDE;

struct Pipelines {
    forward: wgpu::RenderPipeline,
    /// The forward pass for pooled meshes: vertices pulled from the arena (WP-U21).
    forward_pulled: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    shadow_pulled: wgpu::RenderPipeline,
    /// The atmosphere's sky radiance over the whole target.
    sky_bg: wgpu::RenderPipeline,
    /// Sky points (additive).
    points: wgpu::RenderPipeline,
    /// Sky sprites (additive).
    sprites: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
    frame_bgl: wgpu::BindGroupLayout,
    pass_bgl: wgpu::BindGroupLayout,
    shadow_bgl: wgpu::BindGroupLayout,
    sky_bgl: wgpu::BindGroupLayout,
    atmo_bgl: wgpu::BindGroupLayout,
    tonemap_bgl: wgpu::BindGroupLayout,
}

struct Buffers {
    frame: wgpu::Buffer,
    pass: wgpu::Buffer,
    instances: wgpu::Buffer,
    draw_index: wgpu::Buffer,
    materials: wgpu::Buffer,
    lights: wgpu::Buffer,
    clusters: wgpu::Buffer,
    light_index: wgpu::Buffer,
    sprites: wgpu::Buffer,
    /// The vertex arena every mesh pool's meshes live in (WP-U21): 8 floats a vertex, pulled
    /// by the pooled pipelines' vertex stage.
    arena: wgpu::Buffer,
    /// The sky points (static: written by `set_sky_points`).
    points: wgpu::Buffer,
    tonemap: wgpu::Buffer,
}

/// The renderer: holds pipelines, meshes, materials and per-frame buffers on one device.
pub struct Renderer {
    opts: RenderOptions,
    device: wgpu::Device,
    queue: wgpu::Queue,
    p: Pipelines,
    b: Buffers,
    frame_bg: wgpu::BindGroup,
    pass_bg: wgpu::BindGroup,
    sky_bg: wgpu::BindGroup,
    dummy_shadow_bg: wgpu::BindGroup,
    shadow_sampler: wgpu::Sampler,
    /// Mesh bounds per slot (the GPU buffers live in [`FrameShared::meshes`], the same
    /// slots); a removed mesh leaves `None` and its id is reused.
    bounds: Vec<Option<MeshBounds>>,
    free_meshes: Vec<u32>,
    /// Meshes held (slots that are `Some`).
    live_meshes: usize,
    /// W2 positive control for `test_mesh_churn`: copy the whole mesh table on every add and
    /// remove, as the pre-WP-18 copy-on-write `Arc<[_]>` did. **Never use.**
    copy_table_on_change: bool,
    /// W2 positive control for `test_mesh_churn`: scan the whole mesh table (allocation-free)
    /// on every removal. **Never set.**
    scan_table_on_remove: bool,
    materials: Vec<Material>,
    /// Mesh pools (WP-U21): their meshes' vertices in `b.arena`, their topologies' index
    /// lists in `FrameShared::groups`.
    pools: Vec<MeshPool>,
    /// Vertices of the arena handed out so far (freed slots go back to their pool).
    arena_top: u64,
    /// W2 positive control for `test_mesh_pool`: every pooled instance reads the arena's
    /// first slot. **Never set.**
    pool_ignores_base: bool,
    /// W2 positive control for `test_mesh_pool`'s allocation guard: one allocation per
    /// instance a frame uploads. **Never set.**
    alloc_per_instance: bool,
    /// The atmosphere's tables, if set, and the bind group every frame binds (the tables' or
    /// a placeholder's).
    atmosphere: Option<GpuAtmosphere>,
    atmo_bg: wgpu::BindGroup,
    atmo_placeholder: AtmospherePlaceholder,
    /// Records in the sky-point buffer.
    points_len: u32,
    materials_dirty: bool,
    /// CPU storage kept across frames so preparation allocates nothing once warm.
    scratch: PrepareScratch,
    pool: TransientPool,
    timer: GpuTimer,
    /// What the retained pass bodies read each frame.
    shared: Rc<RefCell<FrameShared>>,
    /// Last frame's compiled graph and the topology it was built for.
    cached: Option<CachedGraph>,
    /// Byte staging for buffer uploads (kept, so uploads allocate nothing once warm).
    staging: Vec<u8>,
}

fn storage_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    dynamic: bool,
    size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: NonZeroU64::new(size),
        },
        count: None,
    }
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(256).next_multiple_of(256),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Write `words` to the start of `buf` through the kept `staging` bytes.
fn put_words(queue: &wgpu::Queue, buf: &wgpu::Buffer, words: &[u32], staging: &mut Vec<u8>) {
    if words.is_empty() {
        return;
    }
    staging.clear();
    staging.extend(words.iter().flat_map(|w| w.to_le_bytes()));
    queue.write_buffer(buf, 0, staging);
}

/// Grow `buf` to hold `len` bytes; returns whether it was replaced.
fn ensure(device: &wgpu::Device, buf: &mut wgpu::Buffer, len: u64, label: &str) -> bool {
    if buf.size() >= len {
        return false;
    }
    let usage = buf.usage();
    *buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: len.next_power_of_two().max(256),
        usage,
        mapped_at_creation: false,
    });
    true
}

fn shader(device: &wgpu::Device, name: &str, src: &str) -> Result<wgpu::ShaderModule, RenderError> {
    Ok(forge_gpu::shader::compile_wgsl(device, name, src)?)
}

fn pipelines(device: &wgpu::Device, opts: &RenderOptions) -> Result<Pipelines, RenderError> {
    let common = include_str!("shaders/common.wgsl");
    let atmo = include_str!("shaders/atmosphere.wgsl");
    let mesh_src = format!("{common}\n{atmo}\n{}", include_str!("shaders/mesh.wgsl"));
    let sky_src = format!("{common}\n{atmo}\n{}", include_str!("shaders/sky.wgsl"));
    let mesh = shader(device, "forge-render/mesh.wgsl", &mesh_src)?;
    let sky = shader(device, "forge-render/sky.wgsl", &sky_src)?;
    let tonemap = shader(
        device,
        "forge-render/tonemap.wgsl",
        include_str!("shaders/tonemap.wgsl"),
    )?;

    let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
    let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render frame"),
        entries: &[
            uniform_entry(0, vf, false, (prepare::FRAME_WORDS * 4) as u64),
            storage_entry(1, vf),
            storage_entry(2, vf),
            storage_entry(3, vf),
            storage_entry(4, vf),
            storage_entry(5, vf),
            storage_entry(6, vf),
            storage_entry(7, wgpu::ShaderStages::VERTEX),
        ],
    });
    let pass_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render pass"),
        entries: &[uniform_entry(0, vf, true, 80)],
    });
    let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render shadow maps"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    });
    let sky_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render sprites"),
        entries: &[
            storage_entry(0, wgpu::ShaderStages::VERTEX),
            storage_entry(1, wgpu::ShaderStages::VERTEX),
        ],
    });
    let atmo_bgl = atmosphere::render_bind_group_layout(device);
    let tonemap_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render tonemap"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            uniform_entry(1, wgpu::ShaderStages::FRAGMENT, false, 16),
        ],
    });
    let layout = |label: &str, groups: &[&wgpu::BindGroupLayout]| {
        let groups: Vec<Option<&wgpu::BindGroupLayout>> = groups.iter().map(|g| Some(*g)).collect();
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &groups,
            immediate_size: 0,
        })
    };
    let forward_layout = layout(
        "forge-render forward",
        &[&frame_bgl, &pass_bgl, &shadow_bgl, &atmo_bgl],
    );
    let shadow_layout = layout("forge-render shadow", &[&frame_bgl, &pass_bgl]);
    let sky_layout = layout(
        "forge-render sky",
        &[&frame_bgl, &pass_bgl, &sky_bgl, &atmo_bgl],
    );
    let tonemap_layout = layout("forge-render tonemap", &[&tonemap_bgl]);

    let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
    let vbuf = [Some(wgpu::VertexBufferLayout {
        array_stride: VERTEX_STRIDE,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attrs,
    })];
    let pos_attr = wgpu::vertex_attr_array![0 => Float32x3];
    let vbuf_pos = [Some(wgpu::VertexBufferLayout {
        array_stride: VERTEX_STRIDE,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &pos_attr,
    })];
    let compare = if opts.depth.reversed {
        wgpu::CompareFunction::Greater
    } else {
        wgpu::CompareFunction::Less
    };
    let opts_c = wgpu::PipelineCompilationOptions::default;
    let forward = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forge-render forward"),
        layout: Some(&forward_layout),
        vertex: wgpu::VertexState {
            module: &mesh,
            entry_point: Some("vs_main"),
            compilation_options: opts_c(),
            buffers: &vbuf,
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &mesh,
            entry_point: Some("fs_main"),
            compilation_options: opts_c(),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forge-render shadow"),
        layout: Some(&shadow_layout),
        vertex: wgpu::VertexState {
            module: &mesh,
            entry_point: Some("vs_shadow"),
            compilation_options: opts_c(),
            buffers: &vbuf_pos,
        },
        // No culling: a single-sided plane still casts.
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: None,
        multiview_mask: None,
        cache: None,
    });
    // The pooled meshes' pipelines (WP-U21): the same passes, the vertex pulled from the
    // arena by the instance's vertex base, so every mesh of a pool drawn with one index list
    // is one instanced draw.
    let forward_pulled = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forge-render forward (pooled)"),
        layout: Some(&forward_layout),
        vertex: wgpu::VertexState {
            module: &mesh,
            entry_point: Some("vs_pulled"),
            compilation_options: opts_c(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &mesh,
            entry_point: Some("fs_main"),
            compilation_options: opts_c(),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let shadow_pulled = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forge-render shadow (pooled)"),
        layout: Some(&shadow_layout),
        vertex: wgpu::VertexState {
            module: &mesh,
            entry_point: Some("vs_shadow_pulled"),
            compilation_options: opts_c(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: None,
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
    let sky_pipe = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &sky,
                entry_point: Some(vs),
                compilation_options: opts_c(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &sky,
                entry_point: Some(fs),
                compilation_options: opts_c(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let sky_bg_p = sky_pipe("forge-render sky radiance", "vs_sky_bg", "fs_sky_bg", None);
    let points_p = sky_pipe(
        "forge-render sky points",
        "vs_point",
        "fs_point",
        Some(additive),
    );
    let sprites_p = sky_pipe(
        "forge-render sprites",
        "vs_sprite",
        "fs_sprite",
        Some(additive),
    );
    let tonemap_p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forge-render tonemap"),
        layout: Some(&tonemap_layout),
        vertex: wgpu::VertexState {
            module: &tonemap,
            entry_point: Some("vs_full"),
            compilation_options: opts_c(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &tonemap,
            entry_point: Some("fs_tonemap"),
            compilation_options: opts_c(),
            targets: &[Some(wgpu::ColorTargetState {
                format: opts.output_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    Ok(Pipelines {
        forward,
        forward_pulled,
        shadow,
        shadow_pulled,
        sky_bg: sky_bg_p,
        points: points_p,
        sprites: sprites_p,
        tonemap: tonemap_p,
        frame_bgl,
        pass_bgl,
        shadow_bgl,
        sky_bgl,
        atmo_bgl,
        tonemap_bgl,
    })
}

fn frame_bind_group(device: &wgpu::Device, p: &Pipelines, b: &Buffers) -> wgpu::BindGroup {
    fn e(binding: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding,
            resource: buf.as_entire_binding(),
        }
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("forge-render frame"),
        layout: &p.frame_bgl,
        entries: &[
            e(0, &b.frame),
            e(1, &b.instances),
            e(2, &b.draw_index),
            e(3, &b.materials),
            e(4, &b.lights),
            e(5, &b.clusters),
            e(6, &b.light_index),
            e(7, &b.arena),
        ],
    })
}

fn pass_bind_group(device: &wgpu::Device, p: &Pipelines, b: &Buffers) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("forge-render pass"),
        layout: &p.pass_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &b.pass,
                offset: 0,
                size: NonZeroU64::new(80),
            }),
        }],
    })
}

fn sky_bind_group(device: &wgpu::Device, p: &Pipelines, b: &Buffers) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("forge-render sprites"),
        layout: &p.sky_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: b.sprites.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: b.points.as_entire_binding(),
            },
        ],
    })
}

fn shadow_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    tex: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("forge-render cascades"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..wgpu::TextureViewDescriptor::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("forge-render shadow maps"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

impl Renderer {
    /// A renderer on `dev` (a `forge-gpu` pool member).
    pub fn new(dev: &GpuDevice, opts: RenderOptions) -> Result<Self, RenderError> {
        if opts.width == 0 || opts.height == 0 {
            return Err(RenderError::Target {
                why: format!("{}x{} is empty", opts.width, opts.height),
            });
        }
        if let Some(s) = &opts.shadows
            && !(1..=4).contains(&s.count)
        {
            return Err(RenderError::scene(format!(
                "{} shadow cascades (1..=4 supported)",
                s.count
            )));
        }
        let device = dev.device.clone();
        let queue = dev.queue.clone();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let p = pipelines(&device, &opts)?;
        let st = wgpu::BufferUsages::STORAGE;
        let clusters = u64::from(opts.clusters.x * opts.clusters.y * opts.clusters.z) * 8;
        let b = Buffers {
            frame: buffer(
                &device,
                "forge-render frame",
                (prepare::FRAME_WORDS * 4) as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            pass: buffer(
                &device,
                "forge-render pass",
                PASS_BLOCKS as u64 * PASS_BLOCK_BYTES,
                wgpu::BufferUsages::UNIFORM,
            ),
            instances: buffer(&device, "forge-render instances", 256, st),
            draw_index: buffer(&device, "forge-render draw index", 256, st),
            materials: buffer(&device, "forge-render materials", 256, st),
            lights: buffer(&device, "forge-render lights", 256, st),
            clusters: buffer(&device, "forge-render clusters", clusters, st),
            light_index: buffer(&device, "forge-render light index", 256, st),
            sprites: buffer(&device, "forge-render sprites", 256, st),
            arena: buffer(
                &device,
                "forge-render vertex arena",
                256,
                st | wgpu::BufferUsages::COPY_SRC,
            ),
            points: buffer(&device, "forge-render sky points", 256, st),
            tonemap: buffer(
                &device,
                "forge-render tonemap",
                16,
                wgpu::BufferUsages::UNIFORM,
            ),
        };
        let srgb_in_shader: f32 = if opts.output_format.is_srgb() {
            0.0
        } else {
            1.0
        };
        let linear: f32 = if opts.linear_output { 1.0 } else { 0.0 };
        let mut tm = srgb_in_shader.to_le_bytes().to_vec();
        tm.extend_from_slice(&linear.to_le_bytes());
        tm.resize(16, 0);
        queue.write_buffer(&b.tonemap, 0, &tm);
        let atmo_placeholder = AtmospherePlaceholder::new(&device, &queue, &p.atmo_bgl);
        let atmo_bg = atmo_placeholder.bind_group.clone();
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("forge-render shadow compare"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::GreaterEqual),
            ..wgpu::SamplerDescriptor::default()
        });
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("forge-render no shadows"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_shadow_bg = shadow_bind_group(&device, &p.shadow_bgl, &dummy, &shadow_sampler);
        let frame_bg = frame_bind_group(&device, &p, &b);
        let pass_bg = pass_bind_group(&device, &p, &b);
        let sky_bg = sky_bind_group(&device, &p, &b);
        if let Some(e) = pollster::block_on(scope.pop()).map(|e| e.to_string()) {
            return Err(forge_gpu::GpuError::Validation {
                what: "forge-render renderer creation".into(),
                why: e,
            }
            .into());
        }
        Ok(Self {
            timer: GpuTimer::new(dev),
            opts,
            device,
            queue,
            p,
            b,
            frame_bg,
            pass_bg,
            sky_bg,
            dummy_shadow_bg,
            shadow_sampler,
            bounds: Vec::new(),
            free_meshes: Vec::new(),
            live_meshes: 0,
            copy_table_on_change: false,
            scan_table_on_remove: false,
            materials: Vec::new(),
            atmosphere: None,
            atmo_bg,
            atmo_placeholder,
            points_len: 0,
            materials_dirty: true,
            scratch: PrepareScratch::default(),
            pool: TransientPool::new(),
            shared: Rc::new(RefCell::new(FrameShared::default())),
            pools: Vec::new(),
            arena_top: 0,
            pool_ignores_base: false,
            alloc_per_instance: false,
            cached: None,
            staging: Vec::new(),
        })
    }

    /// The options.
    #[must_use]
    pub fn options(&self) -> &RenderOptions {
        &self.opts
    }

    /// Whether per-pass GPU timestamps are available on this device.
    #[must_use]
    pub fn gpu_timing_supported(&self) -> bool {
        self.timer.supported()
    }

    /// Upload a mesh.
    pub fn add_mesh(&mut self, mesh: &MeshData) -> Result<MeshId, RenderError> {
        if mesh.vertices.is_empty()
            || mesh.indices.is_empty()
            || !mesh.indices.len().is_multiple_of(3)
        {
            return Err(RenderError::scene(
                "a mesh needs vertices and whole triangles",
            ));
        }
        let n = mesh.vertices.len();
        if let Some(bad) = mesh.indices.iter().find(|&&i| i as usize >= n) {
            return Err(RenderError::scene(format!(
                "mesh index {bad} >= {n} vertices"
            )));
        }
        if mesh
            .vertices
            .iter()
            .any(|v| !v.local.is_finite() || !v.normal.is_finite())
        {
            return Err(RenderError::scene("mesh vertex is not finite"));
        }
        let mut vb: Vec<u8> = Vec::with_capacity(n * VERTEX_STRIDE as usize);
        for v in &mesh.vertices {
            // Object-space coordinates: the mesh's own frame, not a place.
            let p = last_mile::vertex(v.local);
            let nn = last_mile::vertex(v.normal);
            let uv = last_mile::vertex(forge_num::DVec3::new(v.uv[0], v.uv[1], 0.0));
            for x in [p[0], p[1], p[2], nn[0], nn[1], nn[2], uv[0], uv[1]] {
                vb.extend_from_slice(&x.to_le_bytes());
            }
        }
        let ib: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let vertices = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-render mesh vertices"),
            size: vb.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&vertices, 0, &vb);
        let indices = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-render mesh indices"),
            size: (ib.len() as u64).next_multiple_of(4),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&indices, 0, &ib);
        let (center, radius) = mesh.bounds();
        let gpu = GpuMesh {
            vertices,
            indices,
            index_count: mesh.indices.len() as u32,
        };
        let id = self.take_slot(Some(gpu), |id| MeshBounds {
            center,
            radius,
            group: id,
            base: 0,
            pool: u32::MAX,
        });
        Ok(MeshId(id))
    }

    /// A mesh slot for a new mesh: a freed one, or a new one at the end of the table.
    fn take_slot(&mut self, gpu: Option<GpuMesh>, bounds: impl FnOnce(u32) -> MeshBounds) -> u32 {
        let mut sh = self.shared.borrow_mut();
        let id = match self.free_meshes.pop() {
            Some(id) => {
                sh.meshes[id as usize] = gpu;
                self.bounds[id as usize] = Some(bounds(id));
                id
            }
            None => {
                let id = self.bounds.len() as u32;
                sh.meshes.push(gpu);
                self.bounds.push(Some(bounds(id)));
                // Every slot may be removed at once: the free list grows with the table
                // (here, where the table allocates anyway), so `remove_mesh` never allocates.
                if self.free_meshes.capacity() < self.bounds.len() {
                    self.free_meshes
                        .reserve(self.bounds.len() - self.free_meshes.len());
                }
                id
            }
        };
        if self.copy_table_on_change {
            sh.meshes = sh.meshes.to_vec();
        }
        self.live_meshes += 1;
        id
    }

    /// Remove a mesh: its GPU buffers are released once no frame in flight uses them (wgpu
    /// keeps a buffer that a submitted command buffer uses alive), and its id may be handed
    /// out again by [`Renderer::add_mesh`]. An instance naming a removed mesh is an invalid
    /// scene (`RENDER-0003`). A pooled mesh's slot in the arena goes back to its pool (a
    /// frame already submitted still reads the vertices it drew with: the next upload into
    /// the slot is queued after it).
    ///
    /// O(1) whatever the number of meshes held: the slot is cleared in place, so a streaming
    /// terrain releasing a burst of patches costs the burst, not burst x meshes
    /// (`test_mesh_churn`).
    pub fn remove_mesh(&mut self, id: MeshId) -> Result<(), RenderError> {
        let i = id.0 as usize;
        let Some(Some(b)) = self.bounds.get(i).copied() else {
            return Err(RenderError::scene(format!("no mesh {}", id.0)));
        };
        if let Some(p) = self.pools.get_mut(b.pool as usize) {
            p.free.push(b.base);
        }
        let mut sh = self.shared.borrow_mut();
        if let Some(slot) = sh.meshes.get_mut(i) {
            *slot = None;
        }
        if self.copy_table_on_change {
            sh.meshes = sh.meshes.to_vec();
        }
        if self.scan_table_on_remove {
            // The mistake the timing check exists for: O(meshes held) and allocation-free.
            let live = sh.meshes.iter().filter(|m| m.is_some()).count();
            std::hint::black_box(live);
        }
        self.bounds[i] = None;
        self.free_meshes.push(id.0);
        self.live_meshes -= 1;
        Ok(())
    }

    /// A pool of meshes of `vertex_count` vertices each, drawn together (WP-U21): every
    /// instance of a pooled mesh drawn with the same index list ([`Self::add_pool_topology`])
    /// is one instanced draw per pass, however many meshes of the pool the instances name —
    /// the vertex stage pulls each instance's vertices from the renderer's vertex arena by the
    /// mesh's first vertex there. A streamed terrain's patches (equal grids, a few stitch
    /// index lists) are drawn in a handful of calls instead of one each, and adding a patch is
    /// one write into the arena, not a pair of buffers of its own.
    ///
    /// The arena makes room for `reserve` of the pool's meshes now: growing it later copies
    /// what it holds into a buffer twice the size, which a frame pays for on the GPU (measured
    /// on the RTX 3080: 7-13 ms at 16 MB, for a streamed terrain), so a pool reserves what
    /// it expects to hold.
    pub fn add_mesh_pool(
        &mut self,
        vertex_count: u32,
        reserve: u32,
    ) -> Result<MeshPoolId, RenderError> {
        if vertex_count == 0 {
            return Err(RenderError::scene("a mesh pool needs vertices"));
        }
        self.grow_arena(self.arena_top + u64::from(vertex_count) * u64::from(reserve))?;
        self.pools.push(MeshPool {
            vertex_count,
            topologies: Vec::new(),
            free: Vec::new(),
        });
        Ok(MeshPoolId((self.pools.len() - 1) as u32))
    }

    /// Add an index list (whole triangles, indexing `0..vertex_count`) the meshes of `pool`
    /// can be drawn with; returns its topology number in the pool.
    pub fn add_pool_topology(
        &mut self,
        pool: MeshPoolId,
        indices: &[u32],
    ) -> Result<u32, RenderError> {
        let p = self
            .pools
            .get(pool.0 as usize)
            .ok_or_else(|| RenderError::scene(format!("no mesh pool {}", pool.0)))?;
        if indices.is_empty() || !indices.len().is_multiple_of(3) {
            return Err(RenderError::scene("a pool topology needs whole triangles"));
        }
        if let Some(bad) = indices.iter().find(|&&i| i >= p.vertex_count) {
            return Err(RenderError::scene(format!(
                "pool topology index {bad} >= {} vertices",
                p.vertex_count
            )));
        }
        let ib: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-render pool topology"),
            size: (ib.len() as u64).next_multiple_of(4),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buf, 0, &ib);
        let mut sh = self.shared.borrow_mut();
        let group = sh.groups.len() as u32;
        sh.groups.push(GpuGroup {
            indices: buf,
            index_count: indices.len() as u32,
        });
        let p = &mut self.pools[pool.0 as usize];
        p.topologies.push(group);
        Ok((p.topologies.len() - 1) as u32)
    }

    /// Upload a mesh into `pool` (exactly the pool's vertex count), drawn with the pool's
    /// topology `topology`. Its slot in the arena is a freed one of the pool's, else new (the
    /// arena grows by doubling, rarely). Remove it with [`Self::remove_mesh`].
    pub fn add_pooled_mesh(
        &mut self,
        pool: MeshPoolId,
        topology: u32,
        vertices: &[Vertex],
    ) -> Result<MeshId, RenderError> {
        let p = self
            .pools
            .get(pool.0 as usize)
            .ok_or_else(|| RenderError::scene(format!("no mesh pool {}", pool.0)))?;
        let &group = p.topologies.get(topology as usize).ok_or_else(|| {
            RenderError::scene(format!("mesh pool {} has no topology {topology}", pool.0))
        })?;
        if vertices.len() != p.vertex_count as usize {
            return Err(RenderError::scene(format!(
                "a mesh of {} vertices in a pool of {}-vertex meshes",
                vertices.len(),
                p.vertex_count
            )));
        }
        if vertices
            .iter()
            .any(|v| !v.local.is_finite() || !v.normal.is_finite())
        {
            return Err(RenderError::scene("mesh vertex is not finite"));
        }
        let n = u64::from(p.vertex_count);
        let base = match self.pools[pool.0 as usize].free.pop() {
            Some(b) => b,
            None => {
                let b = self.arena_top;
                let base = u32::try_from(b + n)
                    .map(|_| b as u32)
                    .map_err(|_| RenderError::scene("the vertex arena is full"))?;
                self.grow_arena(b + n)?;
                self.arena_top = b + n;
                base
            }
        };
        self.staging.clear();
        for v in vertices {
            // Object-space coordinates: the mesh's own frame, not a place.
            let q = last_mile::vertex(v.local);
            let nn = last_mile::vertex(v.normal);
            let uv = last_mile::vertex(forge_num::DVec3::new(v.uv[0], v.uv[1], 0.0));
            for x in [q[0], q[1], q[2], nn[0], nn[1], nn[2], uv[0], uv[1]] {
                self.staging.extend_from_slice(&x.to_le_bytes());
            }
        }
        self.queue.write_buffer(
            &self.b.arena,
            u64::from(base) * ARENA_VERTEX_BYTES,
            &self.staging,
        );
        let (center, radius) = vertex_bounds(vertices);
        let id = self.take_slot(None, |_| MeshBounds {
            center,
            radius,
            group: POOLED_DRAW + group,
            base,
            pool: pool.0,
        });
        Ok(MeshId(id))
    }

    /// Make the arena hold `vertices` (copying what it holds into a larger buffer: queued
    /// after every write already made, so nothing is lost).
    fn grow_arena(&mut self, vertices: u64) -> Result<(), RenderError> {
        let need = vertices * ARENA_VERTEX_BYTES;
        let old = self.b.arena.size();
        if old >= need {
            return Ok(());
        }
        let limits = self.device.limits();
        let cap = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let size = need.max(old.saturating_mul(2)).min(cap);
        if size < need {
            return Err(RenderError::scene(format!(
                "the vertex arena would need {need} bytes; this device binds at most {cap}"
            )));
        }
        let next = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge-render vertex arena"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("forge-render vertex arena growth"),
            });
        enc.copy_buffer_to_buffer(&self.b.arena, 0, &next, 0, old);
        self.queue.submit(Some(enc.finish()));
        self.b.arena = next;
        self.frame_bg = frame_bind_group(&self.device, &self.p, &self.b);
        Ok(())
    }

    /// Whether `mesh` lives in a mesh pool (drawn batched with its pool's other meshes).
    #[must_use]
    pub fn is_pooled(&self, mesh: MeshId) -> bool {
        self.bounds
            .get(mesh.0 as usize)
            .and_then(Option::as_ref)
            .is_some_and(|b| b.pool != u32::MAX)
    }

    /// Vertices of the arena handed out (pooled meshes' slots, freed ones included).
    #[must_use]
    pub fn arena_vertices(&self) -> u64 {
        self.arena_top
    }

    /// W2 positive control for `test_mesh_pool`: every pooled instance's vertices are read
    /// from the arena's first slot (the vertex base ignored). **Never use.**
    #[doc(hidden)]
    pub fn pool_ignores_base_for_tests(&mut self) {
        self.pool_ignores_base = true;
    }

    /// W2 positive control for `test_mesh_pool`'s allocation guard: a frame allocates once
    /// per instance it uploads. **Never use.**
    #[doc(hidden)]
    pub fn alloc_per_instance_for_tests(&mut self) {
        self.alloc_per_instance = true;
    }

    /// Meshes held.
    #[must_use]
    pub fn mesh_count(&self) -> usize {
        self.live_meshes
    }

    /// W2 positive control for `test_mesh_churn`: copy the whole mesh table on every
    /// [`Self::add_mesh`] and [`Self::remove_mesh`], as the pre-WP-18 copy-on-write table did
    /// (O(meshes) per call). **Never use.**
    #[doc(hidden)]
    pub fn copy_mesh_table_on_change_for_tests(&mut self) {
        self.copy_table_on_change = true;
    }

    /// W2 positive control for `test_mesh_churn`'s timing check: every
    /// [`Self::remove_mesh`] walks the whole mesh table without allocating (O(meshes held),
    /// invisible to an allocation count). **Never use.**
    #[doc(hidden)]
    pub fn scan_mesh_table_on_remove_for_tests(&mut self) {
        self.scan_table_on_remove = true;
    }

    /// Build the atmosphere's tables for `params` (blocking; milliseconds on a desktop GPU)
    /// and use them for every frame whose [`Scene::atmosphere`] is set. Replaces any earlier
    /// atmosphere.
    pub fn set_atmosphere(
        &mut self,
        dev: &GpuDevice,
        params: &forge_sky::AtmosphereParams,
        settings: &AtmosphereSettings,
    ) -> Result<AtmosphereReport, RenderError> {
        let a = GpuAtmosphere::new(dev, params, settings)?;
        let report = a.report();
        self.atmo_bg = a.render_bind_group(&self.device, &self.p.atmo_bgl);
        self.atmosphere = Some(a);
        Ok(report)
    }

    /// Drop the atmosphere (frames then clear to their background).
    pub fn clear_atmosphere(&mut self) {
        self.atmosphere = None;
        self.atmo_bg = self.atmo_placeholder.bind_group.clone();
    }

    /// The atmosphere's tables, if set.
    #[must_use]
    pub fn atmosphere(&self) -> Option<&GpuAtmosphere> {
        self.atmosphere.as_ref()
    }

    /// Install the sky points the [`Scene::sky_points`] field draws (uploaded once: points at
    /// infinity do not move; a scene turns them by turning their frame).
    pub fn set_sky_points(&mut self, points: &[crate::SkyPoint]) {
        let words = prepare::points_words(points);
        let bytes = (words.len() * 4) as u64;
        if ensure(
            &self.device,
            &mut self.b.points,
            bytes,
            "forge-render sky points",
        ) {
            self.sky_bg = sky_bind_group(&self.device, &self.p, &self.b);
        }
        put_words(&self.queue, &self.b.points, &words, &mut self.staging);
        self.points_len = (words.len() / POINT_WORDS) as u32;
    }

    /// Add a material.
    pub fn add_material(&mut self, m: Material) -> MaterialId {
        self.materials.push(m);
        self.materials_dirty = true;
        MaterialId((self.materials.len() - 1) as u32)
    }

    /// Replace a material.
    pub fn set_material(&mut self, id: MaterialId, m: Material) -> Result<(), RenderError> {
        let slot = self
            .materials
            .get_mut(id.0 as usize)
            .ok_or_else(|| RenderError::scene(format!("no material {}", id.0)))?;
        *slot = m;
        self.materials_dirty = true;
        Ok(())
    }

    fn input(&self, naive: bool) -> PrepareInput<'_> {
        PrepareInput {
            width: self.opts.width,
            height: self.opts.height,
            depth: self.opts.depth,
            order: self.opts.draw_order,
            grid: self.opts.clusters,
            shadows: self.opts.shadows,
            meshes: &self.bounds,
            groups: self.shared.borrow().groups.len(),
            pool_ignores_base: self.pool_ignores_base,
            alloc_per_instance: self.alloc_per_instance,
            materials: &self.materials,
            naive_last_mile: naive,
            has_atmosphere: self.atmosphere.is_some(),
            hard_disc_sprites: self.opts.hard_disc_sprites,
            sky_repeats: self.opts.perf_fault_sky_repeats,
            isotropic_rayleigh: self.opts.isotropic_rayleigh_for_tests,
            points_len: self.points_len,
        }
    }

    /// Resolve `scene` for `camera` at `t` (CPU only): the f64 -> f32 last mile, culling,
    /// depth passes, clusters, cascades. Hand the frame back with [`Renderer::recycle`] once it is
    /// rendered so the next preparation reuses its buffers ([`Renderer::render_scene`] does).
    pub fn prepare(
        &mut self,
        scene: &Scene,
        camera: &Camera,
        tree: &dyn FrameResolver,
        t: Tick,
    ) -> Result<PreparedFrame, RenderError> {
        self.prepare_with(false, scene, camera, tree, t)
    }

    /// W2 positive control for `test_last_mile`: [`Renderer::prepare`] with instance
    /// positions narrowed to `f32` *before* the camera is subtracted. **Never use.**
    #[doc(hidden)]
    pub fn prepare_naive_for_tests(
        &mut self,
        scene: &Scene,
        camera: &Camera,
        tree: &dyn FrameResolver,
        t: Tick,
    ) -> Result<PreparedFrame, RenderError> {
        self.prepare_with(true, scene, camera, tree, t)
    }

    fn prepare_with(
        &mut self,
        naive: bool,
        scene: &Scene,
        camera: &Camera,
        tree: &dyn FrameResolver,
        t: Tick,
    ) -> Result<PreparedFrame, RenderError> {
        // Moving the scratch out and back is three pointer swaps, never an allocation.
        let mut scratch = std::mem::take(&mut self.scratch);
        let r = prepare::prepare(&self.input(naive), &mut scratch, scene, camera, tree, t);
        self.scratch = scratch;
        r
    }

    /// Give a rendered frame's buffers back for the next [`Renderer::prepare`] to reuse.
    /// Optional — a dropped frame only costs the next preparation its allocations.
    pub fn recycle(&mut self, frame: PreparedFrame) {
        self.scratch.recycle(frame);
    }

    /// Prepare and render in one call.
    pub fn render_scene(
        &mut self,
        dev: &GpuDevice,
        scene: &Scene,
        camera: &Camera,
        tree: &dyn FrameResolver,
        t: Tick,
        target: &wgpu::Texture,
    ) -> Result<FrameReport, RenderError> {
        let f = self.prepare(scene, camera, tree, t)?;
        let report = self.render(dev, &f, target);
        self.recycle(f);
        report
    }

    fn upload(&mut self, f: &PreparedFrame) {
        let d = &self.device;
        let mut regrouped = false;
        let need = |words: &[u32]| (words.len() * 4) as u64;
        regrouped |= ensure(
            d,
            &mut self.b.instances,
            need(&f.instance_words),
            "forge-render instances",
        );
        regrouped |= ensure(
            d,
            &mut self.b.draw_index,
            need(&f.draw_index),
            "forge-render draw index",
        );
        regrouped |= ensure(
            d,
            &mut self.b.lights,
            need(&f.light_words),
            "forge-render lights",
        );
        regrouped |= ensure(
            d,
            &mut self.b.clusters,
            need(f.clusters.ranges.as_flattened()),
            "forge-render clusters",
        );
        regrouped |= ensure(
            d,
            &mut self.b.light_index,
            need(&f.clusters.indices),
            "forge-render light index",
        );
        let mat_words = self.materials.len() * prepare::MATERIAL_WORDS;
        regrouped |= ensure(
            d,
            &mut self.b.materials,
            (mat_words * 4) as u64,
            "forge-render materials",
        );
        if regrouped {
            self.frame_bg = frame_bind_group(d, &self.p, &self.b);
            self.materials_dirty = true;
        }
        if ensure(
            d,
            &mut self.b.sprites,
            need(&f.sprite_words),
            "forge-render sprites",
        ) {
            self.sky_bg = sky_bind_group(d, &self.p, &self.b);
        }
        if self.materials_dirty {
            let mut w = Vec::with_capacity(mat_words);
            for m in &self.materials {
                prepare::material_words(m, &mut w);
            }
            put_words(&self.queue, &self.b.materials, &w, &mut self.staging);
            self.materials_dirty = false;
        }
        let (q, st) = (&self.queue, &mut self.staging);
        let mut put = |buf: &wgpu::Buffer, words: &[u32]| put_words(q, buf, words, st);
        put(&self.b.frame, &f.frame_words);
        put(&self.b.pass, &f.pass_words);
        put(&self.b.instances, &f.instance_words);
        put(&self.b.draw_index, &f.draw_index);
        put(&self.b.lights, &f.light_words);
        put(&self.b.clusters, f.clusters.ranges.as_flattened());
        put(&self.b.light_index, &f.clusters.indices);
        put(&self.b.sprites, &f.sprite_words);
    }

    /// Build the render graph for a frame topology (the output is import `"output"`, bound at
    /// execute). Every pass body is retained: it reads the current frame's draws, counts and
    /// bind groups from [`FrameShared`], so a later frame with the same topology executes
    /// this graph again without rebuilding or recompiling it.
    #[allow(clippy::too_many_lines)]
    fn build_graph(&self, topo: &Topology) -> (RenderGraph, forge_gpu::Handle) {
        let (w, h) = (self.opts.width, self.opts.height);
        let mut g = RenderGraph::new();
        let out = g.import_texture("output", TextureDesc::d2(w, h, self.opts.output_format));
        let hdr = g.create_texture("hdr", TextureDesc::d2(w, h, HDR_FORMAT));
        let reversed = self.opts.depth.reversed;
        let depth_clear = if reversed { 0.0 } else { 1.0 };

        // Shadow cascades: one array texture, one pass per layer.
        let mut cascades = None;
        if let (Some(s), true) = (&self.opts.shadows, topo.cascades > 0) {
            let desc = TextureDesc {
                width: s.resolution,
                height: s.resolution,
                layers: topo.cascades as u32,
                mips: 1,
                samples: 1,
                format: DEPTH_FORMAT,
            };
            let mut hnd = g.create_texture("shadow cascades", desc);
            for (i, name) in CASCADE_PASS_NAMES.iter().enumerate().take(topo.cascades) {
                let mut pb = g.add_pass(name);
                let next = if i == 0 {
                    pb.write(hnd, Access::DepthWrite)
                } else {
                    pb.modify(hnd, Access::DepthWrite)
                };
                let pipeline = self.p.shadow.clone();
                let pulled = self.p.shadow_pulled.clone();
                let shared = self.shared.clone();
                let offset = ((prepare::CASCADE_BLOCK + i) as u64 * PASS_BLOCK_BYTES) as u32;
                let layer = i as u32;
                pb.run_retained(move |ctx| {
                    let mut sh = shared.borrow_mut();
                    let sh = &mut *sh;
                    let tex = ctx.texture(next)?;
                    let view = match sh.layer_views.get(i) {
                        Some((t, v)) if t == tex => v.clone(),
                        _ => {
                            let v = tex.create_view(&wgpu::TextureViewDescriptor {
                                label: Some("forge-render cascade layer"),
                                dimension: Some(wgpu::TextureViewDimension::D2),
                                base_array_layer: layer,
                                array_layer_count: Some(1),
                                ..wgpu::TextureViewDescriptor::default()
                            });
                            if sh.layer_views.len() <= i {
                                sh.layer_views.resize(i + 1, (tex.clone(), v.clone()));
                            }
                            sh.layer_views[i] = (tex.clone(), v.clone());
                            v
                        }
                    };
                    let (Some(frame_bg), Some(pass_bg)) = (&sh.frame_bg, &sh.pass_bg) else {
                        return Ok(());
                    };
                    let mut rp = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("forge-render shadow cascade"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    rp.set_pipeline(&pipeline);
                    rp.set_bind_group(0, frame_bg, &[]);
                    rp.set_bind_group(1, pass_bg, &[offset]);
                    if let Some(draws) = sh.cascades.get(i) {
                        draw_all(&mut rp, &sh.meshes, &sh.groups, draws, &pipeline, &pulled);
                    }
                    Ok(())
                });
                hnd = next;
            }
            cascades = Some(hnd);
        }

        // Sky: clears the HDR target to the background (or draws the atmosphere's radiance
        // over it), then the sky points and the sprites, additively.
        let hdr_sky = {
            let mut pb = g.add_pass("shell.sky");
            let next = pb.write(hdr, Access::ColorAttachment);
            let sky_bg_p = self.p.sky_bg.clone();
            let points_p = self.p.points.clone();
            let pipeline = self.p.sprites.clone();
            let shared = self.shared.clone();
            let offset = (SKY_BLOCK as u64 * PASS_BLOCK_BYTES) as u32;
            pb.run_retained(move |ctx| {
                let sh = shared.borrow();
                let bg = sh.background;
                let view = ctx.view(next)?.clone();
                let load = ctx.color_load(
                    next,
                    wgpu::Color {
                        r: bg[0],
                        g: bg[1],
                        b: bg[2],
                        a: bg[3],
                    },
                )?;
                let mut rp = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("forge-render sky"),
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
                if let (Some(frame_bg), Some(pass_bg), Some(sky_bg), Some(atmo_bg)) =
                    (&sh.frame_bg, &sh.pass_bg, &sh.sky_bg, &sh.atmo_bg)
                    && (sh.sky_atmosphere || sh.points > 0 || sh.sprites > 0)
                {
                    rp.set_bind_group(0, frame_bg, &[]);
                    rp.set_bind_group(1, pass_bg, &[offset]);
                    rp.set_bind_group(2, sky_bg, &[]);
                    rp.set_bind_group(3, atmo_bg, &[]);
                    if sh.sky_atmosphere {
                        rp.set_pipeline(&sky_bg_p);
                        rp.draw(0..3, 0..1);
                    }
                    if sh.points > 0 {
                        rp.set_pipeline(&points_p);
                        rp.draw(0..6, 0..sh.points);
                    }
                    if sh.sprites > 0 {
                        rp.set_pipeline(&pipeline);
                        rp.draw(0..6, 0..sh.sprites);
                    }
                }
                Ok(())
            });
            next
        };

        // Depth passes, back to front, each with its own depth clear.
        let mut hdr_v = hdr_sky;
        for (k, (pass, attachment, block, samples_shadows)) in topo.passes().enumerate() {
            let depth = g.create_texture(attachment, TextureDesc::d2(w, h, DEPTH_FORMAT));
            let mut pb = g.add_pass(pass);
            let color = pb.modify(hdr_v, Access::ColorAttachment);
            let depth_w = pb.write(depth, Access::DepthWrite);
            let shadow_read = if samples_shadows { cascades } else { None };
            if let Some(c) = shadow_read {
                pb.read(c, Access::Sampled);
            }
            let pipeline = self.p.forward.clone();
            let pulled = self.p.forward_pulled.clone();
            let dummy = self.dummy_shadow_bg.clone();
            let shadow_layout = self.p.shadow_bgl.clone();
            let sampler = self.shadow_sampler.clone();
            let shared = self.shared.clone();
            let offset = (block as u64 * PASS_BLOCK_BYTES) as u32;
            pb.run_retained(move |ctx| {
                let mut sh = shared.borrow_mut();
                let sh = &mut *sh;
                let cview = ctx.view(color)?.clone();
                let dview = ctx.view(depth_w)?.clone();
                let shadow_bg = match shadow_read {
                    Some(c) => {
                        let tex = ctx.texture(c)?;
                        match &sh.shadow_bg {
                            Some((t, bg)) if t == tex => bg.clone(),
                            _ => {
                                let bg =
                                    shadow_bind_group(ctx.device, &shadow_layout, tex, &sampler);
                                sh.shadow_bg = Some((tex.clone(), bg.clone()));
                                bg
                            }
                        }
                    }
                    None => dummy.clone(),
                };
                let (Some(frame_bg), Some(pass_bg), Some(atmo_bg)) =
                    (&sh.frame_bg, &sh.pass_bg, &sh.atmo_bg)
                else {
                    return Ok(());
                };
                let mut rp = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("forge-render depth pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &cview,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &dview,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(depth_clear),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                rp.set_pipeline(&pipeline);
                rp.set_bind_group(0, frame_bg, &[]);
                rp.set_bind_group(1, pass_bg, &[offset]);
                rp.set_bind_group(2, &shadow_bg, &[]);
                rp.set_bind_group(3, atmo_bg, &[]);
                if let Some(draws) = sh.passes.get(k) {
                    draw_all(&mut rp, &sh.meshes, &sh.groups, draws, &pipeline, &pulled);
                }
                Ok(())
            });
            hdr_v = color;
        }

        // Tone map into the caller's target.
        {
            let mut pb = g.add_pass("tonemap");
            pb.read(hdr_v, Access::Sampled);
            let target = pb.write(out, Access::ColorAttachment);
            let pipeline = self.p.tonemap.clone();
            let layout = self.p.tonemap_bgl.clone();
            let tm = self.b.tonemap.clone();
            let shared = self.shared.clone();
            pb.run_retained(move |ctx| {
                let mut sh = shared.borrow_mut();
                let src_tex = ctx.texture(hdr_v)?;
                let bg = match &sh.tonemap_bg {
                    Some((t, bg)) if t == src_tex => bg.clone(),
                    _ => {
                        let src = ctx.view(hdr_v)?;
                        let bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("forge-render tonemap"),
                            layout: &layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(src),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: tm.as_entire_binding(),
                                },
                            ],
                        });
                        sh.tonemap_bg = Some((src_tex.clone(), bg.clone()));
                        bg
                    }
                };
                let dst = ctx.view(target)?.clone();
                let mut rp = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("forge-render tonemap"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &dst,
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
                rp.set_pipeline(&pipeline);
                rp.set_bind_group(0, &bg, &[]);
                rp.draw(0..3, 0..1);
                Ok(())
            });
        }
        (g, out)
    }

    /// The compiled plan a frame would execute (for inspection: pass order, the depth
    /// depth passes' aliasing, barriers).
    pub fn plan(&self, f: &PreparedFrame) -> Result<GraphPlan, RenderError> {
        let (g, _) = self.build_graph(&Topology::of(f));
        Ok(g.compile()?.plan().clone())
    }

    /// Render a prepared frame into `target` (`width x height`, the output format, usable as
    /// a render attachment).
    pub fn render(
        &mut self,
        dev: &GpuDevice,
        f: &PreparedFrame,
        target: &wgpu::Texture,
    ) -> Result<FrameReport, RenderError> {
        let (w, h) = (self.opts.width, self.opts.height);
        if (target.width(), target.height(), target.format()) != (w, h, self.opts.output_format) {
            return Err(RenderError::Target {
                why: format!(
                    "target is {}x{} {:?}; the renderer draws {w}x{h} {:?}",
                    target.width(),
                    target.height(),
                    target.format(),
                    self.opts.output_format
                ),
            });
        }
        if (f.width, f.height) != (w, h) {
            return Err(RenderError::Target {
                why: format!(
                    "frame was prepared for {}x{}, renderer is {w}x{h}",
                    f.width, f.height
                ),
            });
        }
        if f.reversed != self.opts.depth.reversed {
            return Err(RenderError::scene(
                "frame was prepared for another depth setup",
            ));
        }
        self.upload(f);
        self.share(f);
        // Same topology as the cached graph: execute it again. Otherwise build and compile.
        let topo = Topology::of(f);
        let graph_rebuilt = !matches!(&self.cached, Some(c) if c.topo == topo);
        if graph_rebuilt {
            let (g, out) = self.build_graph(&topo);
            self.cached = Some(CachedGraph {
                topo,
                graph: g.compile()?,
                out,
            });
        }
        let Some(cached) = self.cached.as_mut() else {
            return Err(RenderError::scene("no compiled frame graph"));
        };
        let mut imports = Imports::new();
        imports.texture(cached.out, target);
        let timer = if self.opts.timing {
            Some(&mut self.timer)
        } else {
            None
        };
        let run = cached
            .graph
            .execute_retained(dev, &mut self.pool, &imports, timer);
        let (exec, passes) = match run {
            Ok(r) => r,
            Err(e) => {
                // A failed frame may have left the graph mid-state; build afresh next time.
                self.cached = None;
                return Err(e.into());
            }
        };
        Ok(FrameReport {
            exec,
            passes,
            prepare: f.stats,
            graph_rebuilt,
        })
    }

    /// Copy the frame's per-pass data into the state the retained pass bodies read. Once
    /// warm this allocates nothing (the vectors keep their capacity).
    fn share(&mut self, f: &PreparedFrame) {
        let mut sh = self.shared.borrow_mut();
        let sh = &mut *sh;
        sh.frame_bg = Some(self.frame_bg.clone());
        sh.pass_bg = Some(self.pass_bg.clone());
        sh.sky_bg = Some(self.sky_bg.clone());
        sh.atmo_bg = Some(self.atmo_bg.clone());
        sh.sprites = (f.sprite_words.len() / SPRITE_WORDS) as u32;
        sh.points = f.point_count;
        sh.sky_atmosphere = f.sky_atmosphere;
        sh.background = f.background;
        copy_draws(&mut sh.passes, &f.passes);
        copy_draws(&mut sh.cascades, &f.cascades);
    }
}

/// `dst[i] = src[i].draws`, keeping `dst`'s allocations.
fn copy_draws(dst: &mut Vec<Vec<prepare::Draw>>, src: &[prepare::PassDraws]) {
    if dst.len() < src.len() {
        dst.resize_with(src.len(), Vec::new);
    }
    for (d, s) in dst.iter_mut().zip(src) {
        d.clear();
        d.extend_from_slice(&s.draws);
    }
    for d in dst.iter_mut().skip(src.len()) {
        d.clear();
    }
}

const CASCADE_PASS_NAMES: [&str; 4] = [
    "shadow.cascade0",
    "shadow.cascade1",
    "shadow.cascade2",
    "shadow.cascade3",
];

/// What decides a frame graph's shape: which passes exist and what they read. Two frames
/// with equal topologies execute the same compiled graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Topology {
    cascades: usize,
    /// Depth passes back to front: the pass and its depth attachment (render-graph names),
    /// its pass-uniform block and whether it samples the cascades.
    passes: [Option<(&'static str, &'static str, usize, bool)>; MAX_DEPTH_PASSES],
}

impl Topology {
    fn of(f: &PreparedFrame) -> Self {
        let mut passes = [None; MAX_DEPTH_PASSES];
        for (slot, p) in passes.iter_mut().zip(&f.passes) {
            *slot = Some((p.span.pass, p.span.attachment, p.block, p.samples_shadows));
        }
        Self {
            cascades: f.cascades.len(),
            passes,
        }
    }

    fn passes(&self) -> impl Iterator<Item = (&'static str, &'static str, usize, bool)> + '_ {
        self.passes.iter().flatten().copied()
    }
}

struct CachedGraph {
    topo: Topology,
    graph: CompiledGraph,
    out: forge_gpu::Handle,
}

/// Per-frame state the retained pass bodies read (shared through an `Rc<RefCell<_>>`; the
/// renderer and its graph live on one thread), plus the bind groups and views they cache
/// against the pooled textures they were made for.
#[derive(Default)]
struct FrameShared {
    /// Mesh slots, indexed by [`MeshId`]: the renderer's one mesh table. Pass bodies read it in
    /// place while the graph executes; `add_mesh` / `remove_mesh` change one slot, no copy.
    meshes: Vec<Option<GpuMesh>>,
    /// Mesh pool topologies' index lists, by draw group (`POOLED_DRAW` + index).
    groups: Vec<GpuGroup>,
    frame_bg: Option<wgpu::BindGroup>,
    pass_bg: Option<wgpu::BindGroup>,
    sky_bg: Option<wgpu::BindGroup>,
    atmo_bg: Option<wgpu::BindGroup>,
    sprites: u32,
    points: u32,
    sky_atmosphere: bool,
    background: [f64; 4],
    passes: Vec<Vec<prepare::Draw>>,
    cascades: Vec<Vec<prepare::Draw>>,
    layer_views: Vec<(wgpu::Texture, wgpu::TextureView)>,
    shadow_bg: Option<(wgpu::Texture, wgpu::BindGroup)>,
    tonemap_bg: Option<(wgpu::Texture, wgpu::BindGroup)>,
}

/// Record a pass's draws: a mesh's own draw binds its buffers; a pool topology's draw (a draw
/// group at and above `POOLED_DRAW`, WP-U21) binds its index list and draws every instance
/// with the pulled pipeline, their vertices read from the arena. The pass starts with
/// `classic` set; the pipeline switches only where the kind of draw does.
fn draw_all(
    rp: &mut wgpu::RenderPass<'_>,
    meshes: &[Option<GpuMesh>],
    groups: &[GpuGroup],
    draws: &[prepare::Draw],
    classic: &wgpu::RenderPipeline,
    pulled: &wgpu::RenderPipeline,
) {
    let mut on_pulled = false;
    for d in draws {
        if d.mesh >= POOLED_DRAW {
            let Some(g) = groups.get((d.mesh - POOLED_DRAW) as usize) else {
                continue;
            };
            if !on_pulled {
                rp.set_pipeline(pulled);
                on_pulled = true;
            }
            rp.set_index_buffer(g.indices.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..g.index_count, 0, d.first..d.first + d.count);
            continue;
        }
        let Some(Some(m)) = meshes.get(d.mesh as usize) else {
            continue;
        };
        if on_pulled {
            rp.set_pipeline(classic);
            on_pulled = false;
        }
        rp.set_vertex_buffer(0, m.vertices.slice(..));
        rp.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
        rp.draw_indexed(0..m.index_count, 0, d.first..d.first + d.count);
    }
}
