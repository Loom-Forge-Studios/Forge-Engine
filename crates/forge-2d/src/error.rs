//! `Error2d` — the `TWOD-*` codes (`docs/error-codes.md`).

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};

/// Every way the 2D pipeline can refuse or fail. Converts into `forge_core::Error`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Error2d {
    /// `TWOD-0001`: an input is invalid (a non-finite value, a zero size, an unknown id, a
    /// polygon that is not convex, a skeleton whose parent comes after its child).
    Invalid {
        /// What is wrong.
        why: String,
    },
    /// `TWOD-0002`: a position is in a frame other than the 2D world's (a 2D world is
    /// depth 1: everything is in its one root frame, Ch.35 §35.3).
    Frame {
        /// What is wrong.
        why: String,
    },
    /// `TWOD-0003`: an Aseprite file could not be read (truncated, bad magic, an unsupported
    /// colour depth or cel type).
    Aseprite {
        /// What is wrong.
        why: String,
    },
    /// `TWOD-0004`: the images do not fit in the atlas (a sprite larger than a page).
    AtlasFull {
        /// What is wrong.
        why: String,
    },
    /// `TWOD-0005`: the GPU layer failed (its `GPU-*` code follows).
    Gpu {
        /// The GPU error, as text.
        why: String,
    },
}

impl Error2d {
    /// The stable error code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid { .. } => error_code!("TWOD-0001"),
            Self::Frame { .. } => error_code!("TWOD-0002"),
            Self::Aseprite { .. } => error_code!("TWOD-0003"),
            Self::AtlasFull { .. } => error_code!("TWOD-0004"),
            Self::Gpu { .. } => error_code!("TWOD-0005"),
        }
    }

    pub(crate) fn invalid(why: impl Into<String>) -> Self {
        Self::Invalid { why: why.into() }
    }

    pub(crate) fn aseprite(why: impl Into<String>) -> Self {
        Self::Aseprite { why: why.into() }
    }

    /// One representative of every variant (for the allocator-registration test).
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<Error2d> {
        vec![
            Self::invalid("x"),
            Self::Frame { why: "x".into() },
            Self::aseprite("x"),
            Self::AtlasFull { why: "x".into() },
            Self::Gpu { why: "x".into() },
        ]
    }
}

impl fmt::Display for Error2d {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::Invalid { why } => write!(f, "invalid 2D input: {why}"),
            Self::Frame { why } => write!(f, "2D frame: {why}"),
            Self::Aseprite { why } => write!(f, "Aseprite file: {why}"),
            Self::AtlasFull { why } => write!(f, "atlas: {why}"),
            Self::Gpu { why } => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Error2d {}

impl CodedError for Error2d {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
