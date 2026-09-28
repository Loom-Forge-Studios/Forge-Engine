//! Pairing over a one-time code (Ch.34 §34.5: "pairing is device-based"), with a person on
//! both ends.
//!
//! The code is six digits: far too few to send as a password or to hash — anyone who saw a
//! hash could try all million codes offline. So the code is never sent. Both sides run
//! **SPAKE2** over it (a password-authenticated key exchange): each gets the same key only
//! if both used the same code, and an attacker in the middle learns nothing it can test
//! offline — every guess costs it one live exchange, and the host withdraws a code after
//! [`MAX_ATTEMPTS`] failed ones. Each side then proves it holds the key with an HMAC over
//! **both certificate fingerprints**, so the exchange is bound to this TLS connection: a
//! relay that terminated TLS itself would present other fingerprints and fail the proof.
//!
//! A verified request is still only a request: a person at the host allows it (the Remote
//! panel), and only then does each side pin the other's certificate.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity as SpId, Password, Spake2};

use crate::RemoteError;

/// Failed key confirmations one code survives; then the host withdraws it.
pub const MAX_ATTEMPTS: u32 = 3;

const ID_DEVICE: &[u8] = b"forge-remote device";
const ID_HOST: &[u8] = b"forge-remote host";

/// A SPAKE2 exchange in progress.
pub struct Exchange {
    state: Spake2<Ed25519Group>,
}

impl std::fmt::Debug for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Exchange").finish_non_exhaustive()
    }
}

impl Exchange {
    /// The device's side: its outbound message.
    #[must_use]
    pub fn device(code: &str) -> (Self, Vec<u8>) {
        let (state, msg) = Spake2::<Ed25519Group>::start_a(
            &Password::new(code.trim().as_bytes()),
            &SpId::new(ID_DEVICE),
            &SpId::new(ID_HOST),
        );
        (Self { state }, msg)
    }

    /// The host's side: its outbound message.
    #[must_use]
    pub fn host(code: &str) -> (Self, Vec<u8>) {
        let (state, msg) = Spake2::<Ed25519Group>::start_b(
            &Password::new(code.trim().as_bytes()),
            &SpId::new(ID_DEVICE),
            &SpId::new(ID_HOST),
        );
        (Self { state }, msg)
    }

    /// The shared key, from the peer's message.
    pub fn finish(self, peer: &[u8]) -> Result<Vec<u8>, RemoteError> {
        self.state
            .finish(peer)
            .map_err(|e| RemoteError::Pairing(format!("the key exchange failed: {e:?}")))
    }
}

/// Which side a confirmation is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Host,
    Device,
}

/// `side`'s key confirmation: HMAC-SHA-256 under the exchanged key over the side's label
/// and both certificate fingerprints.
#[must_use]
pub fn confirm(key: &[u8], side: Side, host_fp: &str, device_fp: &str) -> Vec<u8> {
    let Ok(mut m) = Hmac::<Sha256>::new_from_slice(key) else {
        // HMAC takes a key of any length; unreachable.
        return Vec::new();
    };
    m.update(match side {
        Side::Host => b"forge-remote/1 pairing: host",
        Side::Device => b"forge-remote/1 pairing: device",
    });
    m.update(host_fp.as_bytes());
    m.update(b"|");
    m.update(device_fp.as_bytes());
    m.finalize().into_bytes().to_vec()
}

/// Check `side`'s confirmation in constant time.
#[must_use]
pub fn check(key: &[u8], side: Side, host_fp: &str, device_fp: &str, got: &[u8]) -> bool {
    let Ok(mut m) = Hmac::<Sha256>::new_from_slice(key) else {
        return false;
    };
    m.update(match side {
        Side::Host => b"forge-remote/1 pairing: host",
        Side::Device => b"forge-remote/1 pairing: device",
    });
    m.update(host_fp.as_bytes());
    m.update(b"|");
    m.update(device_fp.as_bytes());
    m.verify_slice(got).is_ok()
}

/// A fresh six-digit code from the OS's random source.
pub fn fresh_code() -> Result<String, RemoteError> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|e| RemoteError::Pairing(format!("no randomness: {e}")))?;
    Ok(format!("{:06}", u64::from_le_bytes(b) % 1_000_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(device_code: &str, host_code: &str, host_fp: &str, relay_fp: &str) -> bool {
        let (d, dm) = Exchange::device(device_code);
        let (h, hm) = Exchange::host(host_code);
        let (Ok(dk), Ok(hk)) = (d.finish(&hm), h.finish(&dm)) else {
            return false;
        };
        // The device saw `relay_fp` as the host's certificate.
        let from_host = confirm(&hk, Side::Host, host_fp, "dev");
        let host_ok = check(&dk, Side::Host, relay_fp, "dev", &from_host);
        let from_device = confirm(&dk, Side::Device, relay_fp, "dev");
        host_ok && check(&hk, Side::Device, host_fp, "dev", &from_device)
    }

    #[test]
    fn the_same_code_confirms_and_anything_else_does_not() {
        assert!(run("123456", "123456", "h", "h"));
        assert!(!run("123457", "123456", "h", "h"), "a wrong code");
        assert!(
            !run("123456", "123456", "h", "relay"),
            "a relay with its own certificate"
        );
    }

    #[test]
    fn codes_are_six_digits() {
        let c = fresh_code().expect("random");
        assert_eq!(c.len(), 6);
        assert!(c.chars().all(|ch| ch.is_ascii_digit()));
    }
}
