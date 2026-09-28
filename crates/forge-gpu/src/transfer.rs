//! Transfer helpers: upload, readback, and **host-staged cross-adapter copies** (Ch.9,
//! Ch.26 Tier 0, spike S1).
//!
//! wgpu has no portable way to share memory between two devices, so a cross-adapter copy is
//! staged through host memory: map the source's readback buffer and write the mapped bytes
//! straight into the destination queue — one host copy, no intermediate `Vec`. The Vulkan
//! external-memory path (`wgpu-hal` passthrough) is spike S1's other arm; it is not built
//! (ADR 0020).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::GpuError;
use crate::pool::GpuDevice;

/// Bytes and time of one transfer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransferStats {
    /// Payload bytes moved.
    pub bytes: u64,
    /// Wall time from submit to the data being usable on the destination.
    pub elapsed: Duration,
}

impl TransferStats {
    /// Throughput in GB/s (1e9 bytes).
    #[must_use]
    pub fn gb_per_s(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s <= 0.0 {
            0.0
        } else {
            self.bytes as f64 / s / 1e9
        }
    }
}

/// A buffer on `dev` holding `bytes`, with `usage` (plus `COPY_DST`/`COPY_SRC`).
#[must_use]
pub fn upload_buffer(
    dev: &GpuDevice,
    label: &str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let size = (bytes.len() as u64)
        .max(4)
        .next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
    let buf = dev.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    write_padded(&dev.queue, &buf, bytes);
    buf
}

/// `queue.write_buffer` with the length padded to `COPY_BUFFER_ALIGNMENT` (wgpu requires
/// it); the pad bytes are zero.
fn write_padded(queue: &wgpu::Queue, buf: &wgpu::Buffer, bytes: &[u8]) {
    let whole = bytes.len() - bytes.len() % wgpu::COPY_BUFFER_ALIGNMENT as usize;
    if whole > 0 {
        queue.write_buffer(buf, 0, &bytes[..whole]);
    }
    if whole < bytes.len() {
        let mut tail = [0u8; wgpu::COPY_BUFFER_ALIGNMENT as usize];
        tail[..bytes.len() - whole].copy_from_slice(&bytes[whole..]);
        queue.write_buffer(buf, whole as u64, &tail);
    }
}

/// Map `buf` (which must have `MAP_READ`) and wait for it.
fn map_read(dev: &GpuDevice, buf: &wgpu::Buffer) -> Result<(), GpuError> {
    let done: Arc<Mutex<Option<Result<(), String>>>> = Arc::default();
    let d2 = done.clone();
    buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        if let Ok(mut g) = d2.lock() {
            *g = Some(r.map_err(|e| e.to_string()));
        }
    });
    dev.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(GpuError::transfer)?;
    let r = done.lock().ok().and_then(|mut g| g.take());
    match r {
        Some(Ok(())) => Ok(()),
        Some(Err(e)) => Err(GpuError::transfer(e)),
        None => Err(GpuError::transfer("map callback never ran")),
    }
}

/// Copy `size` bytes of `src` (needs `COPY_SRC`) into a fresh mappable staging buffer and
/// map it. The caller reads `staging.slice(..).get_mapped_range()` and unmaps.
fn stage(
    dev: &GpuDevice,
    src: &wgpu::Buffer,
    offset: u64,
    size: u64,
) -> Result<wgpu::Buffer, GpuError> {
    let padded = size.max(4).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
    let staging = dev.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("forge-gpu staging"),
        size: padded,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = dev
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("forge-gpu readback"),
        });
    enc.copy_buffer_to_buffer(src, offset, &staging, 0, padded.min(src.size() - offset));
    dev.queue.submit([enc.finish()]);
    map_read(dev, &staging)?;
    Ok(staging)
}

/// Read `size` bytes at `offset` of `src` (needs `COPY_SRC`) back to the host.
pub fn read_buffer(
    dev: &GpuDevice,
    src: &wgpu::Buffer,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>, GpuError> {
    let staging = stage(dev, src, offset, size)?;
    let out = {
        let view = staging
            .slice(..)
            .get_mapped_range()
            .map_err(GpuError::transfer)?;
        view[..size as usize].to_vec()
    };
    staging.unmap();
    Ok(out)
}

/// Bytes per texel of an uncompressed colour format.
fn texel_bytes(format: wgpu::TextureFormat) -> Result<u32, GpuError> {
    format
        .block_copy_size(None)
        .filter(|_| format.block_dimensions() == (1, 1))
        .ok_or_else(|| GpuError::transfer(format!("{format:?} has no single-texel copy size")))
}

/// Read mip 0, layer 0 of `tex` (needs `COPY_SRC`) as tightly packed rows.
pub fn read_texture(dev: &GpuDevice, tex: &wgpu::Texture) -> Result<Vec<u8>, GpuError> {
    let (w, h) = (tex.width(), tex.height());
    let bpt = texel_bytes(tex.format())?;
    let row = w * bpt;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = dev.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("forge-gpu texture readback"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = dev
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("forge-gpu texture readback"),
        });
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
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
    dev.queue.submit([enc.finish()]);
    map_read(dev, &staging)?;
    let mut out = Vec::with_capacity((row * h) as usize);
    {
        let data = staging
            .slice(..)
            .get_mapped_range()
            .map_err(GpuError::transfer)?;
        for y in 0..h as usize {
            let s = y * padded as usize;
            out.extend_from_slice(&data[s..s + row as usize]);
        }
    }
    staging.unmap();
    Ok(out)
}

/// Write tightly packed `bytes` into mip 0, layer 0 of `tex` (needs `COPY_DST`).
pub fn write_texture(dev: &GpuDevice, tex: &wgpu::Texture, bytes: &[u8]) -> Result<(), GpuError> {
    let bpt = texel_bytes(tex.format())?;
    let (w, h) = (tex.width(), tex.height());
    let need = (w * bpt * h) as usize;
    if bytes.len() != need {
        return Err(GpuError::transfer(format!(
            "{} bytes for a {w}x{h} {:?} texture that needs {need}",
            bytes.len(),
            tex.format()
        )));
    }
    dev.queue.write_texture(
        tex.as_image_copy(),
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * bpt),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

/// Host-staged copy of `size` bytes of `src` on `from` into a new buffer on `to` with
/// `usage`. The mapped source range is written straight into `to`'s queue (one host copy).
/// When `from` and `to` are the same device this is a plain GPU copy.
pub fn copy_buffer_across(
    from: &GpuDevice,
    src: &wgpu::Buffer,
    size: u64,
    to: &GpuDevice,
    usage: wgpu::BufferUsages,
) -> Result<(wgpu::Buffer, TransferStats), GpuError> {
    let t0 = Instant::now();
    let padded = size.max(4).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
    let dst = to.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("forge-gpu cross-adapter"),
        size: padded,
        usage: usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    if from.device == to.device {
        let mut enc = to
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("forge-gpu same-device copy"),
            });
        enc.copy_buffer_to_buffer(src, 0, &dst, 0, padded.min(src.size()));
        to.queue.submit([enc.finish()]);
    } else {
        let staging = stage(from, src, 0, size)?;
        {
            let view = staging
                .slice(..)
                .get_mapped_range()
                .map_err(GpuError::transfer)?;
            write_padded(&to.queue, &dst, &view[..size as usize]);
        }
        staging.unmap();
        to.queue.submit([]);
    }
    to.wait_idle()?;
    Ok((
        dst,
        TransferStats {
            bytes: size,
            elapsed: t0.elapsed(),
        },
    ))
}

/// Host-staged copy of `tex` (mip 0, layer 0; needs `COPY_SRC`) from `from` into a new
/// texture on `to` with the same size and format and `usage` (plus `COPY_DST`).
pub fn copy_texture_across(
    from: &GpuDevice,
    tex: &wgpu::Texture,
    to: &GpuDevice,
    usage: wgpu::TextureUsages,
) -> Result<(wgpu::Texture, TransferStats), GpuError> {
    let t0 = Instant::now();
    let bytes = read_texture(from, tex)?;
    let dst = to.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("forge-gpu cross-adapter"),
        size: wgpu::Extent3d {
            width: tex.width(),
            height: tex.height(),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: tex.format(),
        usage: usage | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_texture(to, &dst, &bytes)?;
    to.queue.submit([]);
    to.wait_idle()?;
    Ok((
        dst,
        TransferStats {
            bytes: bytes.len() as u64,
            elapsed: t0.elapsed(),
        },
    ))
}
