//! The **embedded public key** the offline entitlement check verifies against (Ch.38 §38.2:
//! "the public key is in the binary"). There is exactly one, compiled in — no file, no
//! network, nothing a run-time setting can point elsewhere.
//!
//! * A **release build injects the production key** through the `FORGE_LICENCE_PUBKEY`
//!   environment variable (128 hex characters), parsed at compile time. The private half is
//!   held offline by the store's signer and is never in this repository.
//! * With no override, the key is [`DEV_PUBLIC_KEY`], whose secret is the committed *dev*
//!   seed (`crypto` tests, the `mint_dev` example): it verifies entitlements minted in tests
//!   and by developers, and it is deliberately not the production key.

use crate::crypto::PublicKey;

/// The development public key — the public half of the committed dev seed
/// (`b"forge-dev-signing-key-0001!!!!!!"`). Regenerate with `cargo run --example mint_dev
/// --features mint` if the dev seed ever changes.
pub const DEV_PUBLIC_KEY: PublicKey = [
    0x4f, 0x9d, 0xd3, 0x21, 0xe3, 0xb9, 0xc6, 0x27, 0xb8, 0x6f, 0x8b, 0x34, 0x2c, 0xfd, 0x37, 0x8c,
    0x56, 0xb6, 0xf6, 0x13, 0xcf, 0x6c, 0x4e, 0x89, 0x9e, 0xbb, 0xea, 0x59, 0xa4, 0x07, 0xf2, 0x28,
];

/// The public key the offline check verifies against: the production key when a release build
/// set `FORGE_LICENCE_PUBKEY`, else [`DEV_PUBLIC_KEY`].
pub const EMBEDDED_PUBLIC_KEY: PublicKey = match option_env!("FORGE_LICENCE_PUBKEY") {
    Some(hex) => parse_hex_32(hex),
    None => DEV_PUBLIC_KEY,
};

/// Whether the binary embedded the committed **dev** key rather than a production key (i.e.
/// `FORGE_LICENCE_PUBKEY` was not injected at build time). Anyone can mint entitlements the
/// dev key accepts (its seed is committed), so a *shipped* premium binary must never embed it:
/// `cargo xtask premium-protections` fails a hardened build whose embedded key is the dev key,
/// and its control is a `dev` build (no env var) whose embedded key *is* the dev key (ADR
/// 0063). `const`, so it can guard at build time too.
#[must_use]
pub const fn embedded_key_is_dev() -> bool {
    let mut i = 0;
    while i < 32 {
        if EMBEDDED_PUBLIC_KEY[i] != DEV_PUBLIC_KEY[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Parse 64 bytes... no: 32 bytes from a 64-character hex string, at compile time. Panics
/// (a compile error) on a wrong length or a non-hex character, so a misconfigured release
/// build fails to compile rather than shipping a bad key.
const fn parse_hex_32(hex: &str) -> [u8; 32] {
    let bytes = hex.as_bytes();
    assert!(
        bytes.len() == 64,
        "FORGE_LICENCE_PUBKEY must be 64 hex characters (32 bytes)"
    );
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(bytes[i * 2]) << 4) | nibble(bytes[i * 2 + 1]);
        i += 1;
    }
    out
}

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("FORGE_LICENCE_PUBKEY contains a non-hex character"),
    }
}
