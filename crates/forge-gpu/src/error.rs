//! `GpuError` — the `GPU-*` codes (`docs/error-codes.md`).

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};

/// Every way a GPU-layer operation can fail. Converts into `forge_core::Error` with `?`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GpuError {
    /// `GPU-0001`: the graphics API reported no adapter at all (no driver, or a headless box
    /// with no software rasteriser).
    NoAdapter {
        /// Which backends were asked.
        backends: String,
    },
    /// `GPU-0002`: adapters exist, but none meets the minimum hardware floor (O-8). The
    /// message is written for the user: what is needed, what was found, what to try.
    BelowFloor {
        /// The user-facing explanation, one line per adapter found.
        message: String,
    },
    /// `GPU-0003`: an adapter refused to create a device.
    RequestDevice {
        /// The adapter.
        adapter: String,
        /// Why.
        why: String,
    },
    /// `GPU-0004`: a shader did not parse or validate as WGSL. `message` carries the file,
    /// line and column and the offending source line.
    Shader {
        /// The shader's path.
        path: String,
        /// naga's diagnostic.
        message: String,
    },
    /// `GPU-0005`: a shader file could not be read.
    ShaderIo {
        /// The shader's path.
        path: String,
        /// Why.
        why: String,
    },
    /// `GPU-0006`: a render graph is invalid (read before write, a stale handle, a cycle,
    /// an access the pass did not declare, an import left unbound).
    Graph {
        /// What is wrong, naming the pass and resource.
        why: String,
    },
    /// `GPU-0007`: a transfer or readback failed (the buffer could not be mapped).
    Transfer {
        /// Why.
        why: String,
    },
    /// `GPU-0008`: no adapter in the pool can present to the given window surface.
    NoPresentableAdapter,
    /// `GPU-0009`: a shader handle names nothing in this library.
    UnknownShader(String),
    /// `GPU-0010`: wgpu reported a validation or out-of-memory error for an operation.
    Validation {
        /// What was being done.
        what: String,
        /// wgpu's message.
        why: String,
    },
}

impl GpuError {
    /// The stable error code (`docs/error-codes.md`).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::NoAdapter { .. } => error_code!("GPU-0001"),
            Self::BelowFloor { .. } => error_code!("GPU-0002"),
            Self::RequestDevice { .. } => error_code!("GPU-0003"),
            Self::Shader { .. } => error_code!("GPU-0004"),
            Self::ShaderIo { .. } => error_code!("GPU-0005"),
            Self::Graph { .. } => error_code!("GPU-0006"),
            Self::Transfer { .. } => error_code!("GPU-0007"),
            Self::NoPresentableAdapter => error_code!("GPU-0008"),
            Self::UnknownShader(_) => error_code!("GPU-0009"),
            Self::Validation { .. } => error_code!("GPU-0010"),
        }
    }

    pub(crate) fn graph(why: impl Into<String>) -> Self {
        Self::Graph { why: why.into() }
    }

    pub(crate) fn transfer(why: impl fmt::Display) -> Self {
        Self::Transfer {
            why: why.to_string(),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<GpuError> {
        let s = || "x".to_string();
        vec![
            Self::NoAdapter { backends: s() },
            Self::BelowFloor { message: s() },
            Self::RequestDevice {
                adapter: s(),
                why: s(),
            },
            Self::Shader {
                path: s(),
                message: s(),
            },
            Self::ShaderIo {
                path: s(),
                why: s(),
            },
            Self::Graph { why: s() },
            Self::Transfer { why: s() },
            Self::NoPresentableAdapter,
            Self::UnknownShader(s()),
            Self::Validation {
                what: s(),
                why: s(),
            },
        ]
    }
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::NoAdapter { backends } => write!(
                f,
                "no graphics adapter found (backends asked: {backends}); install or update the GPU driver"
            ),
            Self::BelowFloor { message } => f.write_str(message),
            Self::RequestDevice { adapter, why } => {
                write!(f, "{adapter} could not create a device: {why}")
            }
            Self::Shader { path, message } => write!(f, "shader {path} is invalid:\n{message}"),
            Self::ShaderIo { path, why } => write!(f, "shader {path} could not be read: {why}"),
            Self::Graph { why } => write!(f, "render graph: {why}"),
            Self::Transfer { why } => write!(f, "GPU transfer failed: {why}"),
            Self::NoPresentableAdapter => {
                f.write_str("no adapter in the pool can present to this window")
            }
            Self::UnknownShader(s) => write!(f, "no shader {s} in this library"),
            Self::Validation { what, why } => write!(f, "{what}: {why}"),
        }
    }
}

impl std::error::Error for GpuError {}

impl CodedError for GpuError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
