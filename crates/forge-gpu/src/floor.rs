//! The O-8 minimum-hardware floor, applied at adapter selection.
//!
//! **Floor:** Vulkan 1.2 or Direct3D 12 feature level 12_0, with compute shaders. DX11,
//! OpenGL / GLES, Vulkan 1.0/1.1 and anything without compute are refused (decisions O-8).
//! The check is a pure function of [`AdapterFacts`], so it is tested on synthetic adapters
//! (a GTX 660-class FL 11_0 part, a Vulkan 1.1 driver, a GL context) as well as on the
//! machine's real ones; [`crate::probe`] fills the facts in from the driver.

use std::fmt;

/// The API level an adapter reports, as far as the floor cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiLevel {
    /// Vulkan, with the device's `apiVersion` (major, minor).
    Vulkan {
        /// Major version.
        major: u32,
        /// Minor version.
        minor: u32,
    },
    /// Direct3D 12, with whether the device supports feature level 12_0.
    Dx12 {
        /// `D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_12_0, NULL)` succeeded.
        fl_12_0: bool,
    },
    /// Metal (macOS is out of scope, E-33; listed so the report names it).
    Metal,
    /// OpenGL / GLES / WebGL — never supported.
    Gl,
    /// The browser's WebGPU or an API level the probe could not read.
    Unknown,
}

impl fmt::Display for ApiLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vulkan { major, minor } => write!(f, "Vulkan {major}.{minor}"),
            Self::Dx12 { fl_12_0: true } => f.write_str("Direct3D 12, feature level 12_0+"),
            Self::Dx12 { fl_12_0: false } => f.write_str("Direct3D 12, feature level 11_x"),
            Self::Metal => f.write_str("Metal"),
            Self::Gl => f.write_str("OpenGL"),
            Self::Unknown => f.write_str("unknown API level"),
        }
    }
}

/// What the floor and the pool's ordering need to know about one adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterFacts {
    /// The driver's adapter name.
    pub name: String,
    /// The backend it was enumerated on.
    pub backend: wgpu::Backend,
    /// Discrete / integrated / virtual / CPU (software) / other.
    pub device_type: wgpu::DeviceType,
    /// PCI vendor id (backend-specific superset).
    pub vendor: u32,
    /// PCI device id (backend-specific superset).
    pub device: u32,
    /// The API level the probe read.
    pub api: ApiLevel,
    /// Compute shaders are available (`DownlevelFlags::COMPUTE_SHADERS`).
    pub compute: bool,
}

impl AdapterFacts {
    /// A software rasteriser (WARP, lavapipe, llvmpipe, SwiftShader).
    #[must_use]
    pub fn is_software(&self) -> bool {
        self.device_type == wgpu::DeviceType::Cpu
    }

    /// `"NVIDIA GeForce RTX 3080 (Vulkan 1.3)"`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.api)
    }
}

/// Why one adapter is below the floor. Every reason is phrased for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloorMiss {
    /// The adapter, as [`AdapterFacts::label`].
    pub adapter: String,
    /// Each unmet requirement.
    pub reasons: Vec<String>,
}

/// The floor, stated once for messages and docs.
pub const FLOOR_TEXT: &str = "Vulkan 1.2 or Direct3D 12 feature level 12_0, with compute shaders \
(roughly a GeForce GTX 1060, Radeon RX 580, Intel Arc or a recent integrated GPU)";

/// Check one adapter against O-8. `Ok(())` if it may be used.
pub fn check_floor(f: &AdapterFacts) -> Result<(), FloorMiss> {
    let mut reasons = Vec::new();
    match f.api {
        ApiLevel::Vulkan { major, minor } => {
            if (major, minor) < (1, 2) {
                reasons.push(format!(
                    "its driver offers Vulkan {major}.{minor}; Forge needs Vulkan 1.2 or later \
                     (a driver update often provides it)"
                ));
            }
        }
        ApiLevel::Dx12 { fl_12_0 } => {
            if !fl_12_0 {
                reasons.push(
                    "it supports only Direct3D 12 feature level 11_x; Forge needs feature level 12_0"
                        .to_string(),
                );
            }
        }
        ApiLevel::Metal => {
            reasons.push("Metal (macOS) is not a supported platform".to_string());
        }
        ApiLevel::Gl => {
            reasons.push("OpenGL is not supported; Forge needs Vulkan or Direct3D 12".to_string());
        }
        ApiLevel::Unknown => {
            reasons.push("its API level could not be determined".to_string());
        }
    }
    if !f.compute {
        reasons.push("it has no compute shaders, which world generation requires".to_string());
    }
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(FloorMiss {
            adapter: f.label(),
            reasons,
        })
    }
}

/// The user-facing message when no adapter meets the floor (`GPU-0002`).
#[must_use]
pub fn below_floor_message(misses: &[FloorMiss]) -> String {
    let mut s = format!(
        "Forge cannot start the GPU: no graphics adapter meets the minimum of {FLOOR_TEXT}.\n\
         Adapters found:\n"
    );
    for m in misses {
        s.push_str(&format!("  - {}: {}\n", m.adapter, m.reasons.join("; ")));
    }
    s.push_str(
        "Update the graphics driver first. If the adapter is still listed as below the \
         minimum, this GPU cannot run Forge.",
    );
    s
}
