//! `SkyError` — the `SKY-*` codes (`docs/error-codes.md`).

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};

/// Every way deriving a sky can fail. Converts into `forge_core::Error`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum SkyError {
    /// `SKY-0001`: an atmosphere description is invalid (non-finite or non-positive radius,
    /// gravity, pressure or temperature; mole fractions that are negative or sum to zero; an
    /// aerosol or ozone amount out of range).
    InvalidAtmosphere {
        /// What is wrong.
        why: String,
    },
}

impl SkyError {
    /// The stable error code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidAtmosphere { .. } => error_code!("SKY-0001"),
        }
    }

    pub(crate) fn atmosphere(why: impl Into<String>) -> Self {
        Self::InvalidAtmosphere { why: why.into() }
    }

    /// One representative of every variant (for the allocator-registration test).
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<SkyError> {
        vec![Self::atmosphere("x")]
    }
}

impl fmt::Display for SkyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::InvalidAtmosphere { why } => write!(f, "invalid atmosphere: {why}"),
        }
    }
}

impl std::error::Error for SkyError {}

impl CodedError for SkyError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
