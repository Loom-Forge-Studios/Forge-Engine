// timed-gates: exempt(S1 transfer rates are printed for the record, not asserted)
//! `test_multi_adapter` — the Ch.9 DoD seed and spike S1's measurements (M1-2).
//!
//! **DoD seed:** the same frame renders identically on 1 adapter and on 2, with the second
//! adapter doing only compute. A compute pass generates a 256x256 pattern; the primary
//! renders it through a fragment shader. Run A does both on the primary; run B computes on
//! the second pool member and moves the result to the primary with a host-staged transfer.
//! The rendered bytes must be identical.
//!
//! On a one-GPU machine the second member is the software rasteriser (WARP / lavapipe),
//! which exercises the real cross-device path — two devices, two queues, host staging —
//! but is not a second *physical* adapter: spike S1's verdict for two physical GPUs stays
//! AWAITING(second physical adapter) (ADR 0020). The test prints which case ran.
//!
//! **S1 measurements** (`s1_host_staged_bandwidth`): upload, readback, same-device copy and
//! cross-device host-staged copy of 64 MiB, printed for ADR 0020. No thresholds (W5): the
//! assertions are on correctness of every byte moved.

mod common;

use forge_gpu::wgpu;
use forge_gpu::{AdapterPool, GpuDevice, GpuMode, PoolOptions, SoftwarePolicy, transfer};

const W: u32 = 256;
const H: u32 = 256;

const GEN: &str = "@group(0) @binding(0) var<storage, read_write> o: array<u32>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= 256u || g.y >= 256u) { return; }
    // Integer-only hash: bit-identical on every adapter.
    var h = g.x * 73856093u ^ g.y * 19349663u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    o[g.y * 256u + g.x] = h | 0xff000000u;
}";

const SHOW: &str = "@group(0) @binding(0) var<storage, read> p: array<u32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<u32> {
    let x = u32(pos.x);
    let y = u32(pos.y);
    let v = p[y * 256u + x];
    return vec4<u32>(v & 0xffu, (v >> 8u) & 0xffu, (v >> 16u) & 0xffu, v >> 24u);
}";

fn compute_pattern(dev: &GpuDevice) -> wgpu::Buffer {
    let module = forge_gpu::shader::compile_wgsl(&dev.device, "gen.wgsl", GEN).expect("gen");
    let pipeline = dev
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gen"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let size = u64::from(W * H * 4);
    let buf = dev.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pattern"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let bg = dev.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    });
    let mut enc = dev.device.create_command_encoder(&Default::default());
    {
        let mut cp = enc.begin_compute_pass(&Default::default());
        cp.set_pipeline(&pipeline);
        cp.set_bind_group(0, &bg, &[]);
        cp.dispatch_workgroups(W / 8, H / 8, 1);
    }
    dev.queue.submit([enc.finish()]);
    buf
}

fn render(dev: &GpuDevice, pattern: &wgpu::Buffer) -> Vec<u8> {
    let module = forge_gpu::shader::compile_wgsl(&dev.device, "show.wgsl", SHOW).expect("show");
    let target = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Uint,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let pipeline = dev
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("show"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba8Uint.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
    let bg = dev.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: pattern.as_entire_binding(),
        }],
    });
    let view = target.create_view(&Default::default());
    let mut enc = dev.device.create_command_encoder(&Default::default());
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("show"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rp.set_pipeline(&pipeline);
        rp.set_bind_group(0, &bg, &[]);
        rp.draw(0..3, 0..1);
    }
    dev.queue.submit([enc.finish()]);
    transfer::read_texture(dev, &target).expect("frame readback")
}

fn two_device_pool() -> Option<AdapterPool> {
    let pool = common::pool(&PoolOptions {
        software: SoftwarePolicy::Include,
        // Every adapter: this test exists to exercise two devices (ADR 0054).
        mode: GpuMode::Multi,
        ..PoolOptions::default()
    })?;
    if pool.len() < 2 {
        println!(
            "AWAITING(second adapter): the pool has one device ({})",
            pool.primary().label()
        );
        return None;
    }
    let physical = pool
        .devices()
        .iter()
        .filter(|d| !d.facts.is_software())
        .count();
    println!(
        "pool: {} ; second physical adapter: {}",
        pool.devices()
            .iter()
            .map(GpuDevice::label)
            .collect::<Vec<_>>()
            .join(" + "),
        if physical >= 2 {
            "yes"
        } else {
            "AWAITING(second physical adapter) — the second member is software"
        }
    );
    Some(pool)
}

#[test]
fn the_same_frame_renders_identically_on_one_adapter_and_on_two() {
    let Some(pool) = two_device_pool() else {
        return;
    };
    let (primary, helper) = (&pool.devices()[0], &pool.devices()[1]);
    let size = u64::from(W * H * 4);

    let one = render(primary, &compute_pattern(primary));

    let remote = compute_pattern(helper);
    let (moved, stats) =
        transfer::copy_buffer_across(helper, &remote, size, primary, wgpu::BufferUsages::STORAGE)
            .expect("cross-adapter copy");
    println!(
        "pattern moved {} -> {}: {:?}",
        helper.label(),
        primary.label(),
        stats
    );
    let two = render(primary, &moved);

    assert_eq!(one.len(), (W * H * 4) as usize);
    assert!(one.iter().any(|&b| b != 0), "the frame is not blank");
    assert!(
        one == two,
        "frames differ between the one- and two-adapter runs"
    );
    // The compute really ran on the helper: read its buffer back there.
    let there = transfer::read_buffer(helper, &remote, 0, size).expect("helper readback");
    let here = transfer::read_buffer(primary, &moved, 0, size).expect("primary readback");
    assert!(there == here);
    for d in pool.devices() {
        assert!(d.take_uncaptured_errors().is_empty(), "{}", d.label());
    }
}

#[test]
fn positive_control_a_different_helper_result_changes_the_frame() {
    // If the transfer delivered wrong bytes, the equality above would catch it: a pattern
    // with one word changed renders a different frame.
    let Some(pool) = two_device_pool() else {
        return;
    };
    let primary = &pool.devices()[0];
    let size = (W * H * 4) as usize;
    let good =
        transfer::read_buffer(primary, &compute_pattern(primary), 0, size as u64).expect("read");
    let mut bad = good.clone();
    bad[4 * (100 * 256 + 100)] ^= 0x01;
    let a = transfer::upload_buffer(primary, "a", &good, wgpu::BufferUsages::STORAGE);
    let b = transfer::upload_buffer(primary, "b", &bad, wgpu::BufferUsages::STORAGE);
    assert!(render(primary, &a) != render(primary, &b));
}

#[test]
fn s1_host_staged_bandwidth() {
    let Some(pool) = two_device_pool() else {
        return;
    };
    let (a, b) = (&pool.devices()[0], &pool.devices()[1]);
    let size: u64 = 64 << 20;
    let data: Vec<u8> = (0..size)
        .map(|i| (i.wrapping_mul(2654435761) >> 7) as u8)
        .collect();
    let gbps = |bytes: u64, d: std::time::Duration| bytes as f64 / d.as_secs_f64() / 1e9;

    // Warm up (first-use allocation and driver paths), then measure.
    let _ = transfer::upload_buffer(a, "warm", &data[..1 << 20], wgpu::BufferUsages::empty());
    a.wait_idle().expect("idle");

    let t = std::time::Instant::now();
    let src = transfer::upload_buffer(a, "src", &data, wgpu::BufferUsages::empty());
    a.wait_idle().expect("idle");
    let up = t.elapsed();

    // Best of three: the first readback also pays the staging allocation's page faults.
    let mut down = std::time::Duration::MAX;
    let mut back = Vec::new();
    for _ in 0..3 {
        let t = std::time::Instant::now();
        back = transfer::read_buffer(a, &src, 0, size).expect("readback");
        down = down.min(t.elapsed());
    }
    assert!(back == data, "readback corrupted");

    let (_same, same) =
        transfer::copy_buffer_across(a, &src, size, a, wgpu::BufferUsages::empty()).expect("same");
    let (moved, cross) =
        transfer::copy_buffer_across(a, &src, size, b, wgpu::BufferUsages::empty()).expect("cross");
    let check = transfer::read_buffer(b, &moved, 0, size).expect("check");
    assert!(check == data, "cross-adapter copy corrupted");

    println!("S1 measurements, 64 MiB, {} -> {}:", a.label(), b.label());
    println!(
        "  host -> {:<40} {:6.2} GB/s ({:?})",
        a.label(),
        gbps(size, up),
        up
    );
    println!(
        "  {:<40} -> host {:6.2} GB/s ({:?})",
        a.label(),
        gbps(size, down),
        down
    );
    println!(
        "  same-device GPU copy                           {:6.2} GB/s ({:?})",
        same.gb_per_s(),
        same.elapsed
    );
    println!(
        "  cross-device host-staged                       {:6.2} GB/s ({:?})",
        cross.gb_per_s(),
        cross.elapsed
    );
    let frame_4k = 3840u64 * 2160 * 4 * 2; // colour + depth, ~66 MB
    println!(
        "  a 4K colour+depth frame host-staged at the readback+upload rate: {:.1} ms",
        frame_4k as f64 / (gbps(size, down).min(gbps(size, up)) * 1e9) * 2.0 * 1e3
    );
}
