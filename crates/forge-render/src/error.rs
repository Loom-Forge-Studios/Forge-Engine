//! `RenderError` — the `RENDER-*` codes (`docs/error-codes.md`).

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};
use forge_frames::FrameError;
use forge_gpu::GpuError;

/// Every way preparing or rendering a frame can fail. Converts into `forge_core::Error`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum RenderError {
    /// `RENDER-0001`: a position, light or the camera could not be resolved into the camera
    /// frame (unknown frame, disconnected trees). The frame error follows.
    Frame(FrameError),
    /// `RENDER-0002`: the GPU layer failed (its `GPU-*` code follows).
    Gpu(GpuError),
    /// `RENDER-0003`: the scene or camera is invalid (non-finite value, zero scale, a mesh
    /// or material id the renderer does not hold, an empty mesh, a bad field of view).
    InvalidScene {
        /// What is wrong.
        why: String,
    },
    /// `RENDER-0004`: the output target does not match the renderer (size or format).
    Target {
        /// What is wrong.
        why: String,
    },
}

impl RenderError {
    /// The stable error code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Frame(_) => error_code!("RENDER-0001"),
            Self::Gpu(_) => error_code!("RENDER-0002"),
            Self::InvalidScene { .. } => error_code!("RENDER-0003"),
            Self::Target { .. } => error_code!("RENDER-0004"),
        }
    }

    pub(crate) fn scene(why: impl Into<String>) -> Self {
        Self::InvalidScene { why: why.into() }
    }

    /// One representative of every variant (for the allocator-registration test).
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<RenderError> {
        vec![
            Self::Frame(FrameError::UnknownFrame(forge_frames::FrameId(0))),
            Self::Gpu(GpuError::NoPresentableAdapter),
            Self::scene("x"),
            Self::Target { why: "x".into() },
        ]
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::Frame(e) => write!(f, "could not resolve into the camera frame: {e}"),
            Self::Gpu(e) => write!(f, "{e}"),
            Self::InvalidScene { why } => write!(f, "invalid scene: {why}"),
            Self::Target { why } => write!(f, "output target: {why}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl CodedError for RenderError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}

impl From<FrameError> for RenderError {
    fn from(e: FrameError) -> Self {
        Self::Frame(e)
    }
}

impl From<GpuError> for RenderError {
    fn from(e: GpuError) -> Self {
        Self::Gpu(e)
    }
}
