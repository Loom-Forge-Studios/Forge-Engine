//! The atmosphere on the GPU (Ch.11 §11.2, DoD M1-8): Bruneton's precomputed scattering
//! tables built by compute shaders from a body's [`AtmosphereParams`] (which `forge-sky`
//! derives from its composition), and the renderer's view of them.
//!
//! [`GpuAtmosphere::new`] runs the precomputation — transmittance, direct irradiance, single
//! scattering, then per extra order the scattering density, indirect irradiance and multiple
//! scattering (`shaders/atmosphere_precompute.wgsl`) — into three tables: transmittance
//! `T(r, mu)` (2D), scattering `S(r, mu, mu_s, nu)` (3D: Rayleigh plus every multiple order),
//! single Mie scattering (3D, all channels) and ground irradiance `E(r, mu_s)` (2D), all
//! `Rgba16Float`.
//! Tables are per unit solar irradiance, so a star's brightness (and its distance) changes
//! nothing but a multiplier; a change of air (composition, pressure, dust) rebuilds them, in
//! milliseconds on a desktop GPU (the `render.atmosphere.precompute` budget: the dispatches'
//! own GPU time from timestamps; [`AtmosphereReport`] also carries the CPU time before the
//! submit and the wall time).
//!
//! [`GpuAtmosphere::probe`] evaluates the renderer's own sky functions at given rays on the
//! GPU, so `test_atmosphere` can hold them against `forge_sky::ReferenceModel`'s `f64`
//! integrals.

use std::num::NonZeroU64;
use std::time::Instant;

use forge_gpu::{GpuDevice, wgpu};
use forge_num::DVec3;
use forge_sky::{AtmosphereParams, DensityProfile};

use crate::error::RenderError;
use crate::last_mile::narrow;

/// Table resolutions and scattering orders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtmosphereSettings {
    /// Transmittance table: `mu` x `r`.
    pub transmittance: [u32; 2],
    /// Irradiance table: `mu_s` x `r`.
    pub irradiance: [u32; 2],
    /// Scattering table: `r`, `mu`, `mu_s`, `nu` sizes (`mu` even).
    pub scattering: [u32; 4],
    /// Scattering orders (1 = single scattering only).
    pub orders: u32,
}

impl Default for AtmosphereSettings {
    /// Bruneton's reference resolution, four orders.
    fn default() -> Self {
        Self {
            transmittance: [256, 64],
            irradiance: [64, 16],
            scattering: [32, 128, 32, 8],
            orders: 4,
        }
    }
}

impl AtmosphereSettings {
    /// A quarter-size scattering table (tests on the software rasteriser).
    #[must_use]
    pub fn fast() -> Self {
        Self {
            transmittance: [128, 32],
            irradiance: [32, 8],
            scattering: [16, 64, 16, 8],
            orders: 4,
        }
    }

    fn validate(&self) -> Result<(), RenderError> {
        let [r, mu, mu_s, nu] = self.scattering;
        let ok = self.transmittance.iter().all(|&x| (2..=4096).contains(&x))
            && self.irradiance.iter().all(|&x| (2..=4096).contains(&x))
            && (2..=512).contains(&r)
            && (2..=1024).contains(&mu)
            && mu % 2 == 0
            && (2..=512).contains(&mu_s)
            && (2..=64).contains(&nu)
            && (1..=8).contains(&self.orders);
        if ok {
            Ok(())
        } else {
            Err(RenderError::scene(format!("atmosphere settings {self:?}")))
        }
    }
}

/// Words in the atmosphere uniform (21 vec4).
const ATM_WORDS: usize = 84;

/// Pack the uniform: kilometres and 1/km, per unit solar irradiance.
fn uniform_words(p: &AtmosphereParams, s: &AtmosphereSettings, order: u32) -> Vec<u32> {
    let mut w: Vec<u32> = Vec::with_capacity(ATM_WORDS);
    let mut f = |x: f64| w.push(narrow(x).to_bits());
    let km = 1e-3;
    let per_km = 1e3;
    f(p.bottom_radius * km);
    f(p.top_radius * km);
    f(p.mie_g);
    f(p.mu_s_min);
    for k in 0..3 {
        f(p.rayleigh_scattering[k] * per_km);
    }
    f(p.sun_angular_radius);
    for v in [p.mie_scattering, p.mie_extinction, p.absorption_extinction] {
        for x in v {
            f(x * per_km);
        }
        f(0.0);
    }
    for x in p.ground_albedo {
        f(x);
    }
    f(0.0);
    let profile = |f: &mut dyn FnMut(f64), d: &DensityProfile| {
        for l in [d.lower, d.upper] {
            f(l.width * km);
            f(l.exp_term);
            f(l.exp_scale * per_km);
            f(l.linear_term * per_km);
            f(l.constant_term);
            f(0.0);
            f(0.0);
            f(0.0);
        }
    };
    profile(&mut f, &p.rayleigh_density);
    profile(&mut f, &p.mie_density);
    profile(&mut f, &p.absorption_density);
    for x in [
        s.transmittance[0],
        s.transmittance[1],
        s.irradiance[0],
        s.irradiance[1],
    ] {
        f(f64::from(x));
    }
    for x in s.scattering {
        f(f64::from(x));
    }
    w.extend([order, 0, 0, 0]);
    debug_assert_eq!(w.len(), ATM_WORDS);
    w
}

/// A displacement from the atmosphere's ground sphere centre, in its axes, metres — the
/// coordinates of Bruneton's model (the renderer's `frame.atmo_camera`, once narrowed to km).
/// Not a place in the world: the ground sphere's `FramePos` is (I1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CentreOffset(pub DVec3);

/// A ray to evaluate with [`GpuAtmosphere::probe`] (narrowed to kilometres on the GPU, like the
/// renderer's camera).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProbeQuery {
    /// Sky radiance seen from `camera` along the unit `view`, the sun along the unit `sun`.
    Sky {
        /// Viewer.
        camera: CentreOffset,
        /// View direction.
        view: DVec3,
        /// Direction toward the sun.
        sun: DVec3,
    },
    /// In-scattered radiance and transmittance between `camera` and `point`.
    ToPoint {
        /// Viewer.
        camera: CentreOffset,
        /// The far end.
        point: CentreOffset,
        /// Direction toward the sun.
        sun: DVec3,
    },
    /// Transmittance toward the sun from `at`.
    Sun {
        /// Where.
        at: CentreOffset,
        /// Direction toward the sun.
        sun: DVec3,
    },
    /// Sky irradiance on a surface at `at` with unit `normal`.
    Irradiance {
        /// Where.
        at: CentreOffset,
        /// Surface normal.
        normal: DVec3,
        /// Direction toward the sun.
        sun: DVec3,
    },
}

/// What a probe returned (per unit solar irradiance).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeResult {
    /// Radiance (or irradiance), per channel.
    pub radiance: [f64; 3],
    /// Transmittance, per channel.
    pub transmittance: [f64; 3],
}

struct Precompute {
    atm_bgl: wgpu::BindGroupLayout,
    read_bgl: wgpu::BindGroupLayout,
    write_bgl: wgpu::BindGroupLayout,
    probe_bgl: wgpu::BindGroupLayout,
    transmittance: wgpu::ComputePipeline,
    direct: wgpu::ComputePipeline,
    single: wgpu::ComputePipeline,
    density: wgpu::ComputePipeline,
    indirect: wgpu::ComputePipeline,
    multiple: wgpu::ComputePipeline,
    probe: wgpu::ComputePipeline,
}

fn tex_entry(binding: u32, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_tex_entry(binding: u32, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: TABLE_FORMAT,
            view_dimension: dim,
        },
        count: None,
    }
}

/// Every table's format.
pub const TABLE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The layout of the atmosphere bind group the renderer's pipelines use: the uniform, the
/// three tables and a linear clamp sampler.
pub(crate) fn render_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    use wgpu::TextureViewDimension as D;
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render atmosphere"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new((ATM_WORDS * 4) as u64),
                },
                count: None,
            },
            tex_entry(1, D::D2),
            tex_entry(2, D::D3),
            tex_entry(3, D::D2),
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            tex_entry(5, D::D3),
        ],
    })
}

pub(crate) fn linear_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("forge-render atmosphere tables"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..wgpu::SamplerDescriptor::default()
    })
}

fn table(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 3],
    dim: wgpu::TextureDimension,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: size[2],
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: dim,
        format: TABLE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    })
}

fn view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

fn uniform_buffer(device: &wgpu::Device, queue: &wgpu::Queue, words: &[u32]) -> wgpu::Buffer {
    let b = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("forge-render atmosphere uniform"),
        size: (words.len() * 4) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    queue.write_buffer(&b, 0, &bytes);
    b
}

fn precompute_pipelines(device: &wgpu::Device) -> Result<Precompute, RenderError> {
    use wgpu::TextureViewDimension as D;
    let src = format!(
        "{}\n{}",
        include_str!("shaders/atmosphere.wgsl"),
        include_str!("shaders/atmosphere_precompute.wgsl")
    );
    let module =
        forge_gpu::shader::compile_wgsl(device, "forge-render/atmosphere_precompute.wgsl", &src)?;
    let atm_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render atmosphere precompute uniform"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new((ATM_WORDS * 4) as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let read_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render atmosphere precompute reads"),
        entries: &[
            tex_entry(0, D::D2),
            tex_entry(1, D::D3),
            tex_entry(2, D::D3),
            tex_entry(3, D::D3),
            tex_entry(4, D::D2),
            tex_entry(5, D::D3),
            tex_entry(6, D::D3),
        ],
    });
    let write_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render atmosphere precompute writes"),
        entries: &[
            storage_tex_entry(0, D::D2),
            storage_tex_entry(1, D::D2),
            storage_tex_entry(2, D::D3),
            storage_tex_entry(3, D::D3),
            storage_tex_entry(4, D::D3),
        ],
    });
    let buf = |binding, read_only| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let probe_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("forge-render atmosphere probe"),
        entries: &[buf(0, true), buf(1, false)],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("forge-render atmosphere precompute"),
        bind_group_layouts: &[Some(&atm_bgl), Some(&read_bgl), Some(&write_bgl)],
        immediate_size: 0,
    });
    let probe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("forge-render atmosphere probe"),
        bind_group_layouts: &[
            Some(&atm_bgl),
            Some(&read_bgl),
            Some(&write_bgl),
            Some(&probe_bgl),
        ],
        immediate_size: 0,
    });
    let pipe = |entry: &str, layout: &wgpu::PipelineLayout| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        })
    };
    Ok(Precompute {
        transmittance: pipe("cs_transmittance", &layout),
        direct: pipe("cs_direct_irradiance", &layout),
        single: pipe("cs_single_scattering", &layout),
        density: pipe("cs_scattering_density", &layout),
        indirect: pipe("cs_indirect_irradiance", &layout),
        multiple: pipe("cs_multiple_scattering", &layout),
        probe: pipe("cs_probe", &probe_layout),
        atm_bgl,
        read_bgl,
        write_bgl,
        probe_bgl,
    })
}

/// What building the tables cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtmosphereReport {
    /// Wall time of the whole precomputation, submitted and waited for, milliseconds (what a
    /// caller blocks for; it includes any other process's GPU work queued ahead of this one).
    pub precompute_ms: f64,
    /// CPU time before the submit: pipelines, tables, bind groups and the command list,
    /// milliseconds.
    pub record_ms: f64,
    /// The dispatches' own GPU time, milliseconds: the sum over every dispatch of the
    /// timestamps written at its start and end, so another process's GPU work scheduled
    /// between them is not counted. `None` on a device without
    /// [`wgpu::Features::TIMESTAMP_QUERY`] (or one that did not request it).
    pub gpu_ms: Option<f64>,
    /// Compute dispatches.
    pub dispatches: u32,
    /// Bytes of the three kept tables.
    pub table_bytes: u64,
}

/// A body's atmosphere, tabulated on one device.
pub struct GpuAtmosphere {
    params: AtmosphereParams,
    settings: AtmosphereSettings,
    uniform: wgpu::Buffer,
    transmittance: wgpu::Texture,
    scattering: wgpu::Texture,
    /// Single Mie scattering, all three channels.
    single_mie: wgpu::Texture,
    irradiance: wgpu::Texture,
    sampler: wgpu::Sampler,
    pre: Precompute,
    // Placeholders for unused read/write bindings in the probe.
    dummy_read: (wgpu::TextureView, wgpu::TextureView),
    dummy_write: (wgpu::TextureView, wgpu::TextureView),
    report: AtmosphereReport,
}

impl std::fmt::Debug for GpuAtmosphere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuAtmosphere")
            .field("params", &self.params)
            .field("settings", &self.settings)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

struct Dummies {
    r2: wgpu::TextureView,
    r3: wgpu::TextureView,
    w2: wgpu::TextureView,
    w3: wgpu::TextureView,
}

fn dummies(device: &wgpu::Device) -> Dummies {
    use wgpu::TextureDimension as T;
    let r2 = table(device, "atmosphere placeholder 2d", [1, 1, 1], T::D2);
    let r3 = table(device, "atmosphere placeholder 3d", [1, 1, 1], T::D3);
    let w2 = table(device, "atmosphere sink 2d", [1, 1, 1], T::D2);
    let w3 = table(device, "atmosphere sink 3d", [1, 1, 1], T::D3);
    Dummies {
        r2: view(&r2),
        r3: view(&r3),
        w2: view(&w2),
        w3: view(&w3),
    }
}

impl GpuAtmosphere {
    /// Build the tables for `params` on `dev` (blocks until the GPU has finished).
    pub fn new(
        dev: &GpuDevice,
        params: &AtmosphereParams,
        settings: &AtmosphereSettings,
    ) -> Result<Self, RenderError> {
        settings.validate()?;
        let finite = |v: &[f64]| v.iter().all(|x| x.is_finite());
        if !(params.top_radius > params.bottom_radius
            && params.bottom_radius > 0.0
            && finite(&params.rayleigh_scattering)
            && finite(&params.mie_scattering)
            && finite(&params.mie_extinction)
            && finite(&params.absorption_extinction)
            && params
                .rayleigh_scattering
                .iter()
                .chain(&params.mie_scattering)
                .chain(&params.mie_extinction)
                .chain(&params.absorption_extinction)
                .all(|&x| x >= 0.0))
        {
            return Err(RenderError::scene(format!(
                "atmosphere parameters must have top > bottom > 0 and non-negative coefficients: {params:?}"
            )));
        }
        let device = &dev.device;
        let queue = &dev.queue;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let t0 = Instant::now();
        let pre = precompute_pipelines(device)?;
        let sampler = linear_sampler(device);
        let s = *settings;
        let [tw, th] = s.transmittance;
        let [iw, ih] = s.irradiance;
        let [sr, smu, smus, snu] = s.scattering;
        let s3 = [snu * smus, smu, sr];
        use wgpu::TextureDimension as T;
        let transmittance = table(device, "atmosphere transmittance", [tw, th, 1], T::D2);
        let delta_irr = table(device, "atmosphere delta irradiance", [iw, ih, 1], T::D2);
        let irr = [
            table(device, "atmosphere irradiance a", [iw, ih, 1], T::D2),
            table(device, "atmosphere irradiance b", [iw, ih, 1], T::D2),
        ];
        let delta_ray = table(device, "atmosphere delta rayleigh", s3, T::D3);
        let delta_mie = table(device, "atmosphere delta mie", s3, T::D3);
        let delta_mult = table(device, "atmosphere delta multiple", s3, T::D3);
        let delta_density = table(device, "atmosphere delta density", s3, T::D3);
        let scat = [
            table(device, "atmosphere scattering a", s3, T::D3),
            table(device, "atmosphere scattering b", s3, T::D3),
        ];
        let d = dummies(device);
        let uniforms: Vec<wgpu::Buffer> = (0..=s.orders.max(1))
            .map(|o| uniform_buffer(device, queue, &uniform_words(params, &s, o)))
            .collect();
        let atm_bg = |o: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("atmosphere precompute uniform"),
                layout: &pre.atm_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms[o.min(uniforms.len() - 1)].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        // Reads: (transmittance, rayleigh, mie, multiple, irradiance, density, scattering).
        let reads = |v: [&wgpu::TextureView; 7]| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("atmosphere precompute reads"),
                layout: &pre.read_bgl,
                entries: &v
                    .iter()
                    .enumerate()
                    .map(|(i, t)| wgpu::BindGroupEntry {
                        binding: i as u32,
                        resource: wgpu::BindingResource::TextureView(t),
                    })
                    .collect::<Vec<_>>(),
            })
        };
        // Writes: (2d a, 2d b, 3d a, 3d b, 3d c).
        let writes = |v: [&wgpu::TextureView; 5]| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("atmosphere precompute writes"),
                layout: &pre.write_bgl,
                entries: &v
                    .iter()
                    .enumerate()
                    .map(|(i, t)| wgpu::BindGroupEntry {
                        binding: i as u32,
                        resource: wgpu::BindingResource::TextureView(t),
                    })
                    .collect::<Vec<_>>(),
            })
        };
        let v_t = view(&transmittance);
        let v_dirr = view(&delta_irr);
        let v_irr = [view(&irr[0]), view(&irr[1])];
        let v_ray = view(&delta_ray);
        let v_mie = view(&delta_mie);
        let v_mult = view(&delta_mult);
        let v_dens = view(&delta_density);
        let v_scat = [view(&scat[0]), view(&scat[1])];
        let (r2, r3, w2, w3) = (&d.r2, &d.r3, &d.w2, &d.w3);

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("forge-render atmosphere precompute"),
        });
        // Every dispatch is its own pass, stamped at its start and end when the device has
        // timestamps: the GPU cost is the sum of the dispatches' own spans, so work another
        // process gets scheduled between them (the GPU is shared, and per device, not by
        // process priority) is not billed to the precomputation.
        let planned = 3 + 3 * s.orders.saturating_sub(1);
        let stamps = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| {
                device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("forge-render atmosphere precompute timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: planned * 2,
                })
            });
        let mut dispatches = 0u32;
        let groups2 = |w: u32, h: u32| (w.div_ceil(8), h.div_ceil(8), 1);
        let groups3 = (s3[0].div_ceil(4), s3[1].div_ceil(4), s3[2].div_ceil(4));
        let mut run = |enc: &mut wgpu::CommandEncoder,
                       p: &wgpu::ComputePipeline,
                       bgs: [&wgpu::BindGroup; 3],
                       g: (u32, u32, u32)| {
            let timestamp_writes = stamps.as_ref().filter(|_| dispatches < planned).map(|q| {
                wgpu::ComputePassTimestampWrites {
                    query_set: q,
                    beginning_of_pass_write_index: Some(dispatches * 2),
                    end_of_pass_write_index: Some(dispatches * 2 + 1),
                }
            });
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("forge-render atmosphere"),
                timestamp_writes,
            });
            cp.set_pipeline(p);
            for (i, bg) in bgs.iter().enumerate() {
                cp.set_bind_group(i as u32, *bg, &[]);
            }
            cp.dispatch_workgroups(g.0, g.1, g.2);
            dispatches += 1;
        };
        let a1 = atm_bg(1);
        // Transmittance.
        run(
            &mut enc,
            &pre.transmittance,
            [
                &a1,
                &reads([r2, r3, r3, r3, r2, r3, r3]),
                &writes([&v_t, w2, w3, w3, w3]),
            ],
            groups2(tw, th),
        );
        // Direct irradiance -> delta irradiance (the kept irradiance starts at zero: wgpu
        // zero-initialises textures).
        run(
            &mut enc,
            &pre.direct,
            [
                &a1,
                &reads([&v_t, r3, r3, r3, r2, r3, r3]),
                &writes([&v_dirr, w2, w3, w3, w3]),
            ],
            groups2(iw, ih),
        );
        // Single scattering -> delta rayleigh, delta mie, scattering[0].
        run(
            &mut enc,
            &pre.single,
            [
                &a1,
                &reads([&v_t, r3, r3, r3, r2, r3, r3]),
                &writes([w2, w2, &v_ray, &v_mie, &v_scat[0]]),
            ],
            groups3,
        );
        let mut cur_irr = 0usize;
        let mut cur_scat = 0usize;
        for order in 2..=s.orders {
            let a = atm_bg(order as usize);
            // Scattering density of this order (reads order-1 and delta irradiance).
            run(
                &mut enc,
                &pre.density,
                [
                    &a,
                    &reads([&v_t, &v_ray, &v_mie, &v_mult, &v_dirr, r3, r3]),
                    &writes([w2, w2, &v_dens, w3, w3]),
                ],
                groups3,
            );
            // Indirect irradiance of order-1; accumulate.
            run(
                &mut enc,
                &pre.indirect,
                [
                    &a,
                    &reads([&v_t, &v_ray, &v_mie, &v_mult, &v_irr[cur_irr], r3, r3]),
                    &writes([&v_dirr, &v_irr[1 - cur_irr], w3, w3, w3]),
                ],
                groups2(iw, ih),
            );
            cur_irr = 1 - cur_irr;
            // Multiple scattering of this order; accumulate.
            run(
                &mut enc,
                &pre.multiple,
                [
                    &a,
                    &reads([&v_t, r3, r3, r3, r2, &v_dens, &v_scat[cur_scat]]),
                    &writes([w2, w2, &v_mult, &v_scat[1 - cur_scat], w3]),
                ],
                groups3,
            );
            cur_scat = 1 - cur_scat;
        }
        let stamped = dispatches.min(planned);
        let readback = match &stamps {
            Some(q) if stamped > 0 => {
                let bytes = u64::from(stamped) * 16;
                let resolve = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("forge-render atmosphere timestamp resolve"),
                    size: bytes,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let rb = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("forge-render atmosphere timestamp readback"),
                    size: bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                enc.resolve_query_set(q, 0..stamped * 2, &resolve, 0);
                enc.copy_buffer_to_buffer(&resolve, 0, &rb, 0, bytes);
                Some(rb)
            }
            _ => None,
        };
        let record_ms = t0.elapsed().as_secs_f64() * 1e3;
        queue.submit([enc.finish()]);
        dev.wait_idle()?;
        let precompute_ms = t0.elapsed().as_secs_f64() * 1e3;
        if let Some(e) = pollster::block_on(scope.pop()).map(|e| e.to_string()) {
            return Err(forge_gpu::GpuError::Validation {
                what: "forge-render atmosphere precomputation".into(),
                why: e,
            }
            .into());
        }
        let gpu_ms = match readback {
            Some(rb) => {
                let t = forge_gpu::read_timestamps(dev, &rb, stamped as usize)?;
                let period_ns = f64::from(queue.get_timestamp_period());
                let ticks: u64 = t
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|[a, b]| b.saturating_sub(*a))
                    .sum();
                Some(ticks as f64 * period_ns / 1e6)
            }
            None => None,
        };
        let [irr_a, irr_b] = irr;
        let irradiance = if cur_irr == 0 { irr_a } else { irr_b };
        let [scat_a, scat_b] = scat;
        let scattering = if cur_scat == 0 { scat_a } else { scat_b };
        let table_bytes =
            8 * (u64::from(tw * th) + u64::from(iw * ih) + 2 * u64::from(s3[0] * s3[1] * s3[2]));
        let uniform = uniform_buffer(device, queue, &uniform_words(params, &s, 0));
        Ok(Self {
            params: *params,
            settings: s,
            uniform,
            transmittance,
            scattering,
            single_mie: delta_mie,
            irradiance,
            sampler,
            pre,
            dummy_read: (d.r2, d.r3),
            dummy_write: (d.w2, d.w3),
            report: AtmosphereReport {
                precompute_ms,
                record_ms,
                gpu_ms,
                dispatches,
                table_bytes,
            },
        })
    }

    /// The coefficients the tables were built from.
    #[must_use]
    pub fn params(&self) -> &AtmosphereParams {
        &self.params
    }

    /// The table resolutions.
    #[must_use]
    pub fn settings(&self) -> &AtmosphereSettings {
        &self.settings
    }

    /// What the precomputation cost.
    #[must_use]
    pub fn report(&self) -> AtmosphereReport {
        self.report
    }

    /// The bind group the renderer's pipelines read (`layout` from
    /// [`render_bind_group_layout`]).
    pub(crate) fn render_bind_group(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forge-render atmosphere"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view(&self.transmittance)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view(&self.scattering)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&view(&self.irradiance)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&view(&self.single_mie)),
                },
            ],
        })
    }

    /// Evaluate the renderer's sky functions at `queries` on the GPU (per unit solar
    /// irradiance). Positions are narrowed to `f32` kilometres exactly as the renderer's
    /// camera is.
    pub fn probe(
        &self,
        dev: &GpuDevice,
        queries: &[ProbeQuery],
    ) -> Result<Vec<ProbeResult>, RenderError> {
        if queries.is_empty() {
            return Ok(Vec::new());
        }
        let device = &dev.device;
        let km = 1e-3;
        let mut words: Vec<u32> = Vec::with_capacity(queries.len() * 16);
        let mut v4 = |v: DVec3, w: f64| {
            for x in [v.x, v.y, v.z, w] {
                words.push(narrow(x).to_bits());
            }
        };
        for q in queries {
            match *q {
                ProbeQuery::Sky { camera, view, sun } => {
                    v4(camera.0 * km, 0.0);
                    v4(view, 0.0);
                    v4(sun, 0.0);
                    v4(DVec3::ZERO, 0.0);
                }
                ProbeQuery::ToPoint { camera, point, sun } => {
                    v4(camera.0 * km, 1.0);
                    v4(DVec3::ZERO, 0.0);
                    v4(sun, 0.0);
                    v4(point.0 * km, 0.0);
                }
                ProbeQuery::Sun { at, sun } => {
                    v4(at.0 * km, 2.0);
                    v4(DVec3::ZERO, 0.0);
                    v4(sun, 0.0);
                    v4(DVec3::ZERO, 0.0);
                }
                ProbeQuery::Irradiance { at, normal, sun } => {
                    v4(at.0 * km, 3.0);
                    v4(normal, 0.0);
                    v4(sun, 0.0);
                    v4(DVec3::ZERO, 0.0);
                }
            }
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let input = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("atmosphere probe in"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        dev.queue.write_buffer(&input, 0, &bytes);
        let out_size = (queries.len() * 32) as u64;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("atmosphere probe out"),
            size: out_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let atm = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atmosphere probe uniform"),
            layout: &self.pre.atm_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let (r2, r3) = (&self.dummy_read.0, &self.dummy_read.1);
        let (w2, w3) = (&self.dummy_write.0, &self.dummy_write.1);
        let (vt, vi, vs, vm) = (
            view(&self.transmittance),
            view(&self.irradiance),
            view(&self.scattering),
            view(&self.single_mie),
        );
        fn tv(b: u32, t: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding: b,
                resource: wgpu::BindingResource::TextureView(t),
            }
        }
        let reads = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atmosphere probe reads"),
            layout: &self.pre.read_bgl,
            entries: &[
                tv(0, &vt),
                tv(1, r3),
                tv(2, &vm),
                tv(3, r3),
                tv(4, &vi),
                tv(5, r3),
                tv(6, &vs),
            ],
        });
        let _ = r2;
        let writes = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atmosphere probe writes"),
            layout: &self.pre.write_bgl,
            entries: &[tv(0, w2), tv(1, w2), tv(2, w3), tv(3, w3), tv(4, w3)],
        });
        let io = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atmosphere probe io"),
            layout: &self.pre.probe_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("atmosphere probe"),
        });
        {
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("atmosphere probe"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.pre.probe);
            cp.set_bind_group(0, &atm, &[]);
            cp.set_bind_group(1, &reads, &[]);
            cp.set_bind_group(2, &writes, &[]);
            cp.set_bind_group(3, &io, &[]);
            cp.dispatch_workgroups((queries.len() as u32).div_ceil(64), 1, 1);
        }
        dev.queue.submit([enc.finish()]);
        let raw = forge_gpu::transfer::read_buffer(dev, &output, 0, out_size)?;
        if let Some(e) = pollster::block_on(scope.pop()).map(|e| e.to_string()) {
            return Err(forge_gpu::GpuError::Validation {
                what: "forge-render atmosphere probe".into(),
                why: e,
            }
            .into());
        }
        let f = |i: usize| {
            let b = [raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]];
            f64::from(f32::from_le_bytes(b))
        };
        Ok((0..queries.len())
            .map(|q| {
                let o = q * 8;
                ProbeResult {
                    radiance: [f(o), f(o + 1), f(o + 2)],
                    transmittance: [f(o + 4), f(o + 5), f(o + 6)],
                }
            })
            .collect())
    }
}

/// A 1x1 stand-in bound when a frame has no atmosphere (the shaders test the frame's flag and
/// never sample it).
pub(crate) struct AtmospherePlaceholder {
    pub(crate) bind_group: wgpu::BindGroup,
}

impl AtmospherePlaceholder {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
    ) -> Self {
        use wgpu::TextureDimension as T;
        let t2 = table(device, "atmosphere none 2d", [1, 1, 1], T::D2);
        let t3 = table(device, "atmosphere none 3d", [1, 1, 1], T::D3);
        let uniform = uniform_buffer(device, queue, &[0; ATM_WORDS]);
        let sampler = linear_sampler(device);
        let (v2, v3) = (view(&t2), view(&t3));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forge-render no atmosphere"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&v2),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&v3),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&v2),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&v3),
                },
            ],
        });
        Self { bind_group }
    }
}
