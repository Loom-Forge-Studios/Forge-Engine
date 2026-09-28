//! Ed25519 (RFC 8032) **verification** — and, behind the `mint` feature, signing — built
//! directly on `curve25519-dalek` (the curve arithmetic already vetted in the workspace) and
//! `sha2`. Nothing here reaches the network or the filesystem; verification is pure.
//!
//! Only verification ships. Deriving a key and signing an entitlement is the offline store's
//! job and the tests', so it lives behind `mint` and is compiled out of the editor and the
//! server's verify-only builds. Every scalar operation is modulo the group order `L`, and the
//! basepoint has order `L`, so reducing the clamped secret scalar modulo `L` (which
//! `Scalar::from_bytes_mod_order` does, `curve25519-dalek` having no non-reducing constructor)
//! yields the same points and the same `S` value as RFC 8032's integer arithmetic: a signature
//! this module mints is byte-identical to a reference Ed25519 implementation's.

use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
use curve25519_dalek::scalar::Scalar;
use sha2::{Digest, Sha512};

/// An Ed25519 public key (a compressed Edwards point).
pub type PublicKey = [u8; 32];
/// An Ed25519 signature (`R` then `S`, 64 bytes).
pub type Signature = [u8; 64];

/// Verify an Ed25519 signature over `msg` against `public`, offline. `false` on any failure:
/// a malformed public key, a non-canonical `S`, or an equation that does not hold. Constant
/// enough for a licence check (this is not a timing-sensitive secret): the honour-system model
/// (Ch.38 §38.2) does not turn on side channels.
#[must_use]
pub fn verify(public: &PublicKey, msg: &[u8], sig: &Signature) -> bool {
    // A = the public point. A public key that is not a valid point verifies nothing.
    let Some(a) = CompressedEdwardsY(*public).decompress() else {
        return false;
    };
    let neg_a = -a;

    let mut r_bytes = [0u8; 32];
    r_bytes.copy_from_slice(&sig[0..32]);
    let mut s_bytes = [0u8; 32];
    s_bytes.copy_from_slice(&sig[32..64]);

    // S must be a canonical scalar (0 <= S < L): the standard malleability guard.
    let Some(s) = Option::<Scalar>::from(Scalar::from_canonical_bytes(s_bytes)) else {
        return false;
    };

    // k = SHA-512(R || A || M) reduced mod L.
    let mut hasher = Sha512::new();
    hasher.update(&sig[0..32]);
    hasher.update(public);
    hasher.update(msg);
    let k = scalar_from_hash(hasher);

    // R' = S*B - k*A = k*(-A) + S*B; the signature holds iff R' encodes to R.
    let r_check = EdwardsPoint::vartime_double_scalar_mul_basepoint(&k, &neg_a, &s);
    r_check.compress().to_bytes() == r_bytes
}

/// Reduce a finished SHA-512 hasher's 64-byte output to a scalar mod `L`.
fn scalar_from_hash(hasher: Sha512) -> Scalar {
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&hasher.finalize());
    Scalar::from_bytes_mod_order_wide(&wide)
}

/// Derive the Ed25519 public key for a 32-byte seed (the secret key). `mint` only.
#[cfg(feature = "mint")]
#[must_use]
pub fn public_key(seed: &[u8; 32]) -> PublicKey {
    EdwardsPoint::mul_base(&secret_scalar(seed))
        .compress()
        .to_bytes()
}

/// Sign `msg` with the 32-byte seed, producing a 64-byte Ed25519 signature. `mint` only.
#[cfg(feature = "mint")]
#[must_use]
pub fn sign(seed: &[u8; 32], msg: &[u8]) -> Signature {
    let expanded = Sha512::digest(seed);
    let a = secret_scalar(seed);
    let public = EdwardsPoint::mul_base(&a).compress().to_bytes();

    // r = SHA-512(prefix || M); R = r*B.
    let mut hr = Sha512::new();
    hr.update(&expanded[32..64]);
    hr.update(msg);
    let r = scalar_from_hash(hr);
    let r_point = EdwardsPoint::mul_base(&r).compress().to_bytes();

    // k = SHA-512(R || A || M); S = r + k*a.
    let mut hk = Sha512::new();
    hk.update(r_point);
    hk.update(public);
    hk.update(msg);
    let k = scalar_from_hash(hk);
    let s = r + k * a;

    let mut sig = [0u8; 64];
    sig[0..32].copy_from_slice(&r_point);
    sig[32..64].copy_from_slice(&s.to_bytes());
    sig
}

/// The clamped secret scalar for a seed (RFC 8032 §5.1.5). `mint` only.
#[cfg(feature = "mint")]
fn secret_scalar(seed: &[u8; 32]) -> Scalar {
    let expanded = Sha512::digest(seed);
    let mut a = [0u8; 32];
    a.copy_from_slice(&expanded[0..32]);
    a[0] &= 248;
    a[31] &= 127;
    a[31] |= 64;
    Scalar::from_bytes_mod_order(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixed development seed. It is a *test* key, never the production signing key (which is
    // held offline and injected at release through FORGE_LICENCE_PUBKEY, src/key.rs).
    #[cfg(feature = "mint")]
    const DEV_SEED: [u8; 32] = *b"forge-dev-signing-key-0001!!!!!!";

    #[cfg(feature = "mint")]
    #[test]
    fn round_trip_verifies() {
        let pk = public_key(&DEV_SEED);
        let msg = b"tier=team;seat=purchaser";
        let sig = sign(&DEV_SEED, msg);
        assert!(
            verify(&pk, msg, &sig),
            "a freshly minted signature must verify"
        );
    }

    #[cfg(feature = "mint")]
    #[test]
    fn a_tampered_message_does_not_verify() {
        let pk = public_key(&DEV_SEED);
        let sig = sign(&DEV_SEED, b"tier=team");
        // Positive control: the same signature over a different message must be rejected —
        // proving verify() checks the message, not merely the key.
        assert!(
            !verify(&pk, b"tier=team-forged", &sig),
            "a signature must not verify a message it did not sign"
        );
    }

    #[cfg(feature = "mint")]
    #[test]
    fn a_tampered_signature_does_not_verify() {
        let pk = public_key(&DEV_SEED);
        let msg = b"tier=team";
        let mut sig = sign(&DEV_SEED, msg);
        assert!(verify(&pk, msg, &sig));
        // Flip one bit of R, then (reset and) one bit of S: neither may verify.
        sig[0] ^= 1;
        assert!(!verify(&pk, msg, &sig), "a flipped R must fail");
        let mut sig = sign(&DEV_SEED, msg);
        sig[40] ^= 1;
        assert!(!verify(&pk, msg, &sig), "a flipped S must fail");
    }

    #[cfg(feature = "mint")]
    #[test]
    fn the_wrong_key_does_not_verify() {
        let pk = public_key(&DEV_SEED);
        let other = public_key(b"diff-seed-0000000000000000000000");
        let msg = b"tier=team";
        let sig = sign(&DEV_SEED, msg);
        assert!(verify(&pk, msg, &sig));
        // Positive control: verified against another key it must fail — the signature is
        // bound to the signer's key.
        assert!(
            !verify(&other, msg, &sig),
            "a signature is bound to its key"
        );
    }

    #[test]
    fn a_malformed_public_key_and_a_zero_signature_verify_nothing() {
        // A non-canonical / non-point public key cannot decompress: verify returns false, it
        // does not panic.
        let bad_key = [0xffu8; 32];
        assert!(!verify(&bad_key, b"x", &[0u8; 64]));
        // The all-zero signature over a real (but here arbitrary) key must not verify.
        let key = [
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ];
        assert!(!verify(&key, b"", &[0u8; 64]));
    }

    #[cfg(feature = "mint")]
    #[test]
    fn a_non_canonical_s_is_rejected() {
        // S set to all-0xff is >= L, so from_canonical_bytes refuses it: the malleability
        // guard. (A valid signature never has such an S.)
        let pk = public_key(&DEV_SEED);
        let mut sig = sign(&DEV_SEED, b"m");
        sig[32..64].copy_from_slice(&[0xffu8; 32]);
        assert!(
            !verify(&pk, b"m", &sig),
            "a non-canonical S must be rejected"
        );
    }
}
