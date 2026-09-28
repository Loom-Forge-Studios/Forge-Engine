//! `forge-remote` — the **split editor** (Ch.34 §34.1–§34.5, DoD M2-16): the editor's UI runs
//! locally, native, against a core on another machine, and the only thing that crosses the
//! wire is what already crossed the in-process boundary — commands one way, `Applied` state
//! deltas the other (Ch.7, I7). It is a transport swap, not a rewrite: the host serves an
//! [`forge_editor::core::EditorCore`] through the core's own `LocalBus`, and the client is a
//! [`forge_editor::client::BusClient`], so the shell and every panel run unchanged over it.
//!
//! | Piece | Where | What |
//! |---|---|---|
//! | Transport | [`host::QuicTransport`], [`client::RemoteBus`] | QUIC (`quinn`) over TLS 1.3, one ordered stream per session, `postcard` frames ([`wire`]) |
//! | Identity | [`identity::Identity`] | a self-signed certificate per machine (user config); peers are known by its SHA-256 |
//! | Pairing | [`pairing`], [`device::Device::pair`] | the host shows a one-time code; the device proves it knows it by SPAKE2, and the key confirmation binds both certificates, so a man in the middle gets one online guess per code; a **person** allows it on the host; both sides then pin the other's certificate |
//! | Exposure | [`host::QuicTransport`] | **loopback by default**; LAN is the pairing store's explicit, confirmed choice (the host rebinds); internet exposure is not offered (Ch.34 §34.5) |
//! | Grants | host sessions | every request of a remote session is checked in the core's **one grant table** as `Principal::Remote(<device>)`, with the one command classification every non-human principal gets (`forge_plugin::CommandClass::of`), before it reaches the bus — and then the bus's own policies apply: a remote client is a peer, never a privileged path |
//! | Headless host | [`console::serve`] | `forge --headless --remote-host` and `forge-editor --headless --remote-host`: the host with no window or GPU; a person at the terminal shows codes and allows pairings with the Remote panel's own calls |
//! | Prediction | [`client::RemoteBus`] | O-13: each command is dry-run locally with the editor's planners ([`forge_editor::core::Predictor`]) and shown at once as an overlay; the core's answer reconciles it the next pump |
//!
//! The in-memory `LoopbackTransport` of `forge_editor::connect::remote` stays as the labelled
//! D-4 stand-in for tests that script "the other device"; the editor binary hosts this one.

#![forbid(unsafe_code)]

pub mod client;
pub mod console;
pub mod device;
pub mod host;
pub mod identity;
pub mod pairing;
mod session;
pub mod wire;

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};

pub use client::{ConnectOptions, RemoteBus, RemoteHandle};
pub use device::{Device, PairedHost};
pub use host::{HostConfig, HostFaults, QuicTransport};
pub use identity::Identity;
pub use wire::WireFaults;

/// The ALPN protocol id of this wire format (a peer speaking another version is refused at
/// the handshake).
pub const ALPN: &[u8] = b"forge-remote/1";

/// The port the host listens on unless configured otherwise.
pub const DEFAULT_PORT: u16 = 47_900;

/// Every way a remote operation can fail.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemoteError {
    /// `REMOTE-0001`: the network or the QUIC connection failed (bind, connect, a stream).
    Transport(String),
    /// `REMOTE-0002`: TLS or certificate set-up failed, or the peer's certificate is not
    /// the pinned one.
    Tls(String),
    /// `REMOTE-0003`: a frame did not decode, was too large, or broke the protocol.
    Protocol(String),
    /// `REMOTE-0004`: pairing failed: no code on show, a wrong or expired code, a person
    /// denied it, or it timed out.
    Pairing(String),
    /// `REMOTE-0005`: the host refused the session: the device is not paired, or it lacks
    /// the capability to read the project.
    Refused(String),
    /// `REMOTE-0006`: the device's identity or its paired-host list could not be read or
    /// written.
    Config(String),
    /// `REMOTE-0007`: waited too long for the host.
    Timeout(String),
}

impl RemoteError {
    /// The stable error code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Transport(_) => error_code!("REMOTE-0001"),
            Self::Tls(_) => error_code!("REMOTE-0002"),
            Self::Protocol(_) => error_code!("REMOTE-0003"),
            Self::Pairing(_) => error_code!("REMOTE-0004"),
            Self::Refused(_) => error_code!("REMOTE-0005"),
            Self::Config(_) => error_code!("REMOTE-0006"),
            Self::Timeout(_) => error_code!("REMOTE-0007"),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<RemoteError> {
        vec![
            Self::Transport("x".into()),
            Self::Tls("x".into()),
            Self::Protocol("x".into()),
            Self::Pairing("x".into()),
            Self::Refused("x".into()),
            Self::Config("x".into()),
            Self::Timeout("x".into()),
        ]
    }
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::Transport(w) => write!(f, "the connection failed: {w}"),
            Self::Tls(w) => write!(f, "the secure channel failed: {w}"),
            Self::Protocol(w) => write!(f, "the peer broke the protocol: {w}"),
            Self::Pairing(w) => write!(f, "pairing failed: {w}"),
            Self::Refused(w) => write!(f, "the host refused the session: {w}"),
            Self::Config(w) => write!(f, "the remote configuration could not be used: {w}"),
            Self::Timeout(w) => write!(f, "timed out: {w}"),
        }
    }
}

impl std::error::Error for RemoteError {}

impl CodedError for RemoteError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The bytes of lowercase hex `s`.
pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_remote_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in RemoteError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-remote |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }

    #[test]
    fn hex_round_trips() {
        let b = [0u8, 1, 0xab, 0xff];
        assert_eq!(hex(&b), "0001abff");
        assert_eq!(unhex("0001abff").as_deref(), Some(&b[..]));
        assert!(unhex("0g").is_none());
    }
}
