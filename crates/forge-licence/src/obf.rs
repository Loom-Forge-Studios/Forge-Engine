//! A small **compile-time XOR** for the entitlement/licensing strings and the premium
//! feature identifiers, so a `strings` scan of a shipped premium binary does not hand an
//! attacker the exact tokens the licence check compares against (ADR 0063 item 2).
//!
//! This is deliberately *light*. The owner's second rule is that a protection must not slow
//! the engine, and heavier measures (control-flow flattening, packers) are rejected in ADR
//! 0063. XOR obfuscation costs nothing at run time beyond a short copy on first use and raises
//! the bar past a plain-text `strings` dump; it does not pretend to defeat a determined
//! reverse engineer, which — the source being public — is not a goal.
//!
//! [`obf!`](crate::obf) takes a string literal and expands to a `String` revealed at run time.
//! Only the XORed bytes are stored in the binary; the keystream is code (recomputed on reveal),
//! not a blob sitting beside the ciphertext. Whether the plaintext is truly absent from a built
//! artefact is proven end-to-end by `cargo xtask premium-protections`, which scans a real
//! release binary; the unit tests here prove the transform is correct and non-identity.
//!
//! **Obfuscation is a shipped-binary protection, so it is applied only when
//! `debug_assertions` is off** (release, and the hardened `release-premium` profile). A `dev`
//! or a test build keeps the plain literal, which:
//!
//! * keeps crash reports and debugging legible for developers (owner rule 2 — a protection
//!   must not get in the engine's way), and
//! * gives `cargo xtask premium-protections` a real positive control: the same source built
//!   `dev` still contains the tokens, the `release-premium` build does not, so the guard is
//!   demonstrably non-vacuous without a build-time feature flag.
//!
//! The `xor`/`keystream`/`reveal` functions below are unconditional, so their unit tests prove
//! the transform is a real, non-identity XOR whichever profile the tests are built with.

/// A deterministic, non-zero keystream of length `N`, computed at compile time (an xorshift64*
/// PRNG seeded by a fixed salt mixed with `N`). Pure and `const`.
#[must_use]
pub const fn keystream<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15 ^ (N as u64).wrapping_mul(0x0000_0100_0000_01B3);
    // Guard against a degenerate all-zero state (xorshift's fixed point).
    if state == 0 {
        state = 0xDEAD_BEEF_CAFE_F00D;
    }
    let mut i = 0;
    while i < N {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let x = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        out[i] = (x >> 33) as u8;
        i += 1;
    }
    out
}

/// XOR `plain` (which must be `N` bytes) with `key`, at compile time. Pure and `const`.
#[must_use]
pub const fn xor<const N: usize>(plain: &[u8], key: &[u8; N]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N {
        out[i] = plain[i] ^ key[i];
        i += 1;
    }
    out
}

/// Reveal an obfuscated literal at run time: recompute the keystream and XOR it back. Returns
/// the empty string only if the stored bytes were not valid UTF-8, which cannot happen for a
/// literal put through [`obf!`](crate::obf).
#[must_use]
pub fn reveal<const N: usize>(obf: &[u8; N]) -> String {
    // The optimiser must not see through the ciphertext: `obf` is a constant and so is the
    // keystream, so without this barrier LLVM may evaluate the XOR at compile time and store
    // the **plaintext** in the binary — which the Linux leg of `premium-protections` caught
    // (`entitlement.json` in plain text in a fat-LTO `release-premium` ELF, WP-47). `black_box`
    // makes the bytes opaque, so only the XORed bytes are ever in the artefact; the cost is
    // one copy of N bytes on reveal.
    let obf: [u8; N] = std::hint::black_box(*obf);
    let key = keystream::<N>();
    let mut bytes = Vec::with_capacity(N);
    let mut i = 0;
    while i < N {
        bytes.push(obf[i] ^ key[i]);
        i += 1;
    }
    String::from_utf8(bytes).unwrap_or_default()
}

/// Obfuscate a string literal. `obf!("Team")` expands to a `String` holding `"Team"` at run
/// time. In a **release** build (`debug_assertions` off — including the hardened
/// `release-premium` profile) only the XORed bytes are stored in the binary and the plaintext
/// is recomputed on reveal; in a **dev/test** build the plain literal is kept (see the module
/// docs: legible debugging, and a real control for `premium-protections`). See the module docs.
#[macro_export]
macro_rules! obf {
    ($s:literal) => {{
        #[cfg(debug_assertions)]
        {
            ::std::string::String::from($s)
        }
        #[cfg(not(debug_assertions))]
        {
            const N: usize = $s.len();
            const OBF: [u8; N] =
                $crate::obf::xor::<N>($s.as_bytes(), &$crate::obf::keystream::<N>());
            $crate::obf::reveal::<N>(&OBF)
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obf_round_trips_the_literal() {
        assert_eq!(obf!("Team"), "Team");
        assert_eq!(
            obf!("forge.premium.entitlement"),
            "forge.premium.entitlement"
        );
        assert_eq!(obf!(""), "");
        // A longer, structured token like the licence code paths compare.
        assert_eq!(
            obf!("premium feature refused: no valid entitlement"),
            "premium feature refused: no valid entitlement"
        );
    }

    #[test]
    fn the_stored_bytes_are_not_the_plaintext() {
        // The property the obfuscation exists for: the ciphertext differs from the plaintext.
        const PLAIN: &[u8] = b"forge.premium.entitlement";
        const N: usize = PLAIN.len();
        const OBF: [u8; N] = xor::<N>(PLAIN, &keystream::<N>());
        assert_ne!(
            &OBF[..],
            PLAIN,
            "obfuscated bytes must differ from the plaintext"
        );
        // ... and reveal recovers it.
        assert_eq!(reveal::<N>(&OBF).as_bytes(), PLAIN);
    }

    #[test]
    fn positive_control_a_zero_keystream_would_be_identity() {
        // Prove the "differs from plaintext" guard is real: with a zero keystream (a broken
        // obfuscation), the ciphertext EQUALS the plaintext, so the guard above would fire.
        const PLAIN: &[u8] = b"forge.premium.entitlement";
        const N: usize = PLAIN.len();
        let zero = [0u8; N];
        assert_eq!(
            xor::<N>(PLAIN, &zero)[..],
            PLAIN[..],
            "zero key is identity — the control"
        );
        // And the real keystream is not all zero, so the real transform is not identity.
        assert!(
            keystream::<N>().iter().any(|&b| b != 0),
            "keystream must be non-zero"
        );
    }
}
