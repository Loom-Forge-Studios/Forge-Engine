//! `PhysError` — the physics layer's errors (`PHYS-*` codes, `docs/error-codes.md`).

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};

/// Every way a physics operation can fail. No operation panics: a bad input is one of these.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhysError {
    /// `PHYS-0001`: an input is invalid — a non-finite number, a zero or negative size, a
    /// degenerate shape (a hull of coplanar points, a mesh without triangles, a height grid
    /// smaller than 2x2), an unknown body, collider, joint or zone id.
    Invalid(String),
    /// `PHYS-0002`: a position is in a frame other than the physics world's (a physics world
    /// is region-local: one frame, Ch.17).
    Frame(String),
    /// `PHYS-0003`: no backend is registered under this id on `forge.phys.backend` (the
    /// project's `physics.backend` setting names one the build does not have).
    UnknownBackend(String),
    /// `PHYS-0004`: the backend cannot do what was asked (a feature it does not have); the
    /// reason names the backend and what to use instead.
    Unsupported(String),
    /// `PHYS-0005`: the backend failed inside a step or a query (its message follows).
    Backend(String),
}

impl PhysError {
    /// The stable error code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => error_code!("PHYS-0001"),
            Self::Frame(_) => error_code!("PHYS-0002"),
            Self::UnknownBackend(_) => error_code!("PHYS-0003"),
            Self::Unsupported(_) => error_code!("PHYS-0004"),
            Self::Backend(_) => error_code!("PHYS-0005"),
        }
    }

    pub(crate) fn invalid(why: impl Into<String>) -> Self {
        Self::Invalid(why.into())
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<PhysError> {
        vec![
            Self::Invalid("i".into()),
            Self::Frame("f".into()),
            Self::UnknownBackend("b".into()),
            Self::Unsupported("u".into()),
            Self::Backend("x".into()),
        ]
    }
}

impl fmt::Display for PhysError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.code();
        match self {
            Self::Invalid(w) => write!(f, "{c}: invalid physics input: {w}"),
            Self::Frame(w) => write!(f, "{c}: {w}"),
            Self::UnknownBackend(id) => write!(
                f,
                "{c}: no physics backend \"{id}\" is registered on forge.phys.backend (this build has: {})",
                crate::backend::FIRST_PARTY.join(", ")
            ),
            Self::Unsupported(w) => write!(f, "{c}: {w}"),
            Self::Backend(w) => write!(f, "{c}: the physics backend failed: {w}"),
        }
    }
}

impl std::error::Error for PhysError {}

impl CodedError for PhysError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
