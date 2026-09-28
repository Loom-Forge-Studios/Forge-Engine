//! `forge-licence` — **offline entitlement verification and the premium-feature gate**
//! (Ch.38 §38.2, E-57, E-66, M5-25; ADR 0063).
//!
//! This crate is the whole trust boundary between "a signed file on disk" and "the paid
//! features are on". It:
//!
//! * defines the [`Entitlement`] wire type and its [`SignedEntitlement`] on-disk form
//!   ([`entitlement`]);
//! * verifies the Ed25519 signature **offline**, against a public key **embedded in the
//!   binary** ([`crypto`], [`key`]) — no network, ever (I21);
//! * evaluates tier, expiry, grace and fallback **locally** into a [`Standing`]
//!   ([`entitlement::evaluate`]); a lapsed Team term degrades to base and never locks a
//!   project (E-57);
//! * gates the premium features off that standing ([`Gate`]);
//! * obfuscates the licensing strings and premium identifiers in the binary ([`obf!`]).
//!
//! **It is a small library the editor and the droplet server both link, and it is *not* in
//! `forge-runtime`'s dependency graph and cannot be** (I21; `tests/licence/
//! test_no_runtime_phone_home.rs` fails if it ever is). It depends only on the curve
//! arithmetic and hash already resolved in the workspace, plus serde — nothing that reaches
//! the network or the editor.
//!
//! Minting (key derivation and signing) is behind the `mint` feature: the offline store's
//! signer and the tests use it; the shipped verifier is compiled without it.

#![forbid(unsafe_code)]

pub mod activation;
pub mod crypto;
pub mod entitlement;
pub mod gate;
pub mod key;
pub mod obf;

pub use activation::{entitlement_path, load_gate};
pub use entitlement::{
    Entitlement, GRACE_MS, SeatKind, SignedEntitlement, Standing, Tier, VerifyError, date,
    evaluate, verify_signed,
};
pub use gate::{Gate, GateFileError};
pub use key::{DEV_PUBLIC_KEY, EMBEDDED_PUBLIC_KEY, embedded_key_is_dev};

#[cfg(feature = "mint")]
pub use entitlement::sign_entitlement;
