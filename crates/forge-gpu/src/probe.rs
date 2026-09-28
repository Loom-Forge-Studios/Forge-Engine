//! Read an adapter's [`AdapterFacts`] from the driver.
//!
//! The two numbers O-8 is stated in are not in wgpu's portable API, so this module is where
//! forge-gpu uses `unsafe` (Ch.1.5 exception):
//!
//! * **Vulkan:** the device's `apiVersion` from `VkPhysicalDeviceProperties`, read through
//!   `Adapter::as_hal::<Vulkan>()`.
//! * **Direct3D 12:** `D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_12_0, NULL)` — with a
//!   null output pointer it only answers whether the adapter supports that level and
//!   creates nothing (documented behaviour: `S_FALSE` on success).

use crate::floor::{AdapterFacts, ApiLevel};

/// Everything the floor and the pool need, read from the driver.
#[must_use]
pub fn facts(adapter: &wgpu::Adapter) -> AdapterFacts {
    let info = adapter.get_info();
    let compute = adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);
    let api = match info.backend {
        wgpu::Backend::Vulkan => vulkan_level(adapter),
        wgpu::Backend::Dx12 => dx12_level(adapter),
        wgpu::Backend::Metal => ApiLevel::Metal,
        wgpu::Backend::Gl => ApiLevel::Gl,
        _ => ApiLevel::Unknown,
    };
    AdapterFacts {
        name: info.name,
        backend: info.backend,
        device_type: info.device_type,
        vendor: info.vendor,
        device: info.device,
        api,
        compute,
    }
}

/// Split a packed `VK_MAKE_API_VERSION` value into (major, minor).
#[must_use]
pub const fn vk_version(packed: u32) -> (u32, u32) {
    ((packed >> 22) & 0x7f, (packed >> 12) & 0x3ff)
}

#[cfg(any(windows, target_os = "linux", target_os = "android"))]
fn vulkan_level(adapter: &wgpu::Adapter) -> ApiLevel {
    // SAFETY: `as_hal` hands out a guard over wgpu-hal's adapter. We only read an immutable
    // property struct that wgpu-hal filled in at enumeration; nothing is destroyed or
    // mutated, and the guard is dropped before this function returns (wgpu-hal's
    // requirement: the adapter must not be destroyed while the guard is used).
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() };
    match hal {
        Some(a) => {
            let (major, minor) =
                vk_version(a.physical_device_capabilities().properties().api_version);
            ApiLevel::Vulkan { major, minor }
        }
        None => ApiLevel::Unknown,
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
fn vulkan_level(_adapter: &wgpu::Adapter) -> ApiLevel {
    ApiLevel::Unknown
}

/// `D3D_FEATURE_LEVEL_12_0`.
pub const D3D_FL_12_0: i32 = 0xc000;

fn dx12_level(adapter: &wgpu::Adapter) -> ApiLevel {
    match dx12_supports_level(adapter, D3D_FL_12_0) {
        Some(fl_12_0) => ApiLevel::Dx12 { fl_12_0 },
        None => ApiLevel::Unknown,
    }
}

/// Whether a Direct3D 12 adapter supports the raw `D3D_FEATURE_LEVEL` value `level`
/// (`0xc000` = 12_0, `0xc100` = 12_1, `0xc200` = 12_2). `None` for a non-D3D12 adapter or
/// off Windows.
#[cfg(windows)]
#[must_use]
pub fn dx12_supports_level(adapter: &wgpu::Adapter, level: i32) -> Option<bool> {
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL;
    use windows::Win32::Graphics::Direct3D12::{D3D12CreateDevice, ID3D12Device};

    // SAFETY: as in `vulkan_level`: a read-only use of wgpu-hal's adapter inside the guard's
    // lifetime; the guard is dropped before return.
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Dx12>() }?;
    let dxgi: &windows::Win32::Graphics::Dxgi::IDXGIAdapter3 = hal.raw_adapter();
    // SAFETY: `dxgi` is a live COM interface owned by wgpu-hal for the guard's lifetime.
    // A null `ppDevice` is the documented "test only" form of D3D12CreateDevice: it creates
    // no device, touches no wgpu state, and returns S_FALSE when the level is supported.
    let r = unsafe {
        D3D12CreateDevice::<_, ID3D12Device>(dxgi, D3D_FEATURE_LEVEL(level), std::ptr::null_mut())
    };
    Some(r.is_ok())
}

/// Off Windows there is no Direct3D 12.
#[cfg(not(windows))]
#[must_use]
pub fn dx12_supports_level(_adapter: &wgpu::Adapter, _level: i32) -> Option<bool> {
    None
}
