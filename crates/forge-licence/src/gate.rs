//! The **premium-feature gate** (E-66, ADR 0063): given a verified entitlement, decide whether
//! the premium features are enabled *now*. The premium editor and the droplet server both hold
//! a [`Gate`]; the base editor holds none, because the base build links no premium code at all
//! (the build-time half of the edition switch).
//!
//! What counts as premium-entitled: a valid, non-lapsed **Team** standing (Ch.38 §38.2,
//! Appendix A.1) — Team is the paid recurring tier and premium is what it funds (E-66). This is
//! the one place that maps tier to premium, so the owner can retune it (a dedicated premium
//! capability, an Individual add-on) without touching any feature's call site. On lapse the gate
//! degrades to base (E-57): the project still opens, builds and ships; only the premium features
//! go dark, behind a clear, renewable message the caller localises.

use crate::crypto::PublicKey;
use crate::entitlement::{Entitlement, Standing, VerifyError, evaluate, verify_signed};
use crate::key::EMBEDDED_PUBLIC_KEY;

/// A premium-feature gate holding the verified entitlement, if any.
#[derive(Clone, Debug, Default)]
pub struct Gate {
    entitlement: Option<Entitlement>,
}

impl Gate {
    /// A base gate: no entitlement, so no premium feature is enabled. This is what a premium
    /// binary falls back to when there is no activated entitlement — it then behaves as the
    /// base editor (E-57: never locked out).
    #[must_use]
    pub fn base() -> Self {
        Self { entitlement: None }
    }

    /// A gate whose entitlement was verified from `bytes` against the **embedded** public key.
    ///
    /// # Errors
    /// [`VerifyError`] if the document is malformed or its signature does not verify — a
    /// forged or tampered entitlement never reaches [`Gate`], so it can never enable premium.
    pub fn from_signed(bytes: &[u8]) -> Result<Self, VerifyError> {
        Self::from_signed_with(bytes, &EMBEDDED_PUBLIC_KEY)
    }

    /// [`Gate::from_signed`] against an explicit public key (tests, and the server when it is
    /// configured with its own key).
    ///
    /// # Errors
    /// [`VerifyError`] as [`Gate::from_signed`].
    pub fn from_signed_with(bytes: &[u8], public: &PublicKey) -> Result<Self, VerifyError> {
        let entitlement = verify_signed(bytes, public)?;
        Ok(Self {
            entitlement: Some(entitlement),
        })
    }

    /// Read and verify an entitlement file, returning a base gate if the file is absent (an
    /// un-activated machine is base, not an error) and an error only if the file exists but is
    /// malformed or forged.
    ///
    /// # Errors
    /// [`VerifyError::BadSignature`] and friends if a present file does not verify. A missing
    /// file is `Ok(Gate::base())`.
    pub fn from_file(path: &std::path::Path) -> Result<Self, GateFileError> {
        match std::fs::read(path) {
            Ok(bytes) => Self::from_signed(&bytes).map_err(GateFileError::Verify),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::base()),
            Err(e) => Err(GateFileError::Io(e.kind())),
        }
    }

    /// The verified entitlement, if one was loaded.
    #[must_use]
    pub fn entitlement(&self) -> Option<&Entitlement> {
        self.entitlement.as_ref()
    }

    /// The premium standing right now, for this build's `version`. The project's team is not
    /// consulted (premium is not project-team-scoped), so a bound seat is Team for premium.
    #[must_use]
    pub fn standing(&self, now_ms: u64, version: &str) -> Standing {
        evaluate(self.entitlement.as_ref(), now_ms, version, None)
    }

    /// Are the premium features enabled right now? True for a valid, non-lapsed Team standing
    /// (including its grace period). The single predicate every premium feature is gated on.
    #[must_use]
    pub fn premium_enabled(&self, now_ms: u64, version: &str) -> bool {
        self.standing(now_ms, version).active()
    }
}

/// Why reading an entitlement file failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateFileError {
    /// The file exists but did not verify (see [`VerifyError`]).
    Verify(VerifyError),
    /// The file could not be read (a permission or I/O error; the kind, for the log).
    Io(std::io::ErrorKind),
}

impl std::fmt::Display for GateFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verify(e) => write!(f, "{e}"),
            Self::Io(k) => write!(f, "the entitlement file could not be read: {k:?}"),
        }
    }
}

impl std::error::Error for GateFileError {}
