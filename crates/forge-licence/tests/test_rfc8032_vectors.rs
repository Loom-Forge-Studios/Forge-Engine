//! `test_rfc8032_vectors` (WP-47, the WP-45 verifier follow-up (c)): **the shipped Ed25519 verify
//! (`forge_licence::crypto::verify`) accepts the RFC 8032 §7.1 known-answer vectors.** The
//! verifier is built on `curve25519-dalek` directly (no Ed25519 crate), so this pins it to the
//! standard: a verifier that hashed `R || A || M` in another order, reduced the challenge
//! differently, or decoded points or scalars wrongly would reject these signatures.
//!
//! The vectors are RFC 8032 §7.1 "TEST 1" (empty message), "TEST 2" (one byte), "TEST 3" (two
//! bytes) and "TEST SHA(abc)" (a 64-byte message: SHA-512 of "abc"). The §7.1 "TEST 1024"
//! vector (a 1023-byte message) is not transcribed here; the four above exercise every code path
//! of the verifier (the message length only feeds SHA-512). The secret keys are listed for
//! reference only: this crate's shipped build carries no signer (the `mint` feature is off), and
//! this test links it as a customer's build does.
//!
//! Positive control (W2): `positive_control_a_changed_vector_does_not_verify` — every vector with
//! one bit of R, one bit of S, one bit of the public key, or the message changed must be
//! rejected, so the known-answer check cannot pass by a verifier that accepts everything.

use forge_licence::crypto::verify;

/// One §7.1 vector: `(name, secret key, public key, message, signature)`, hex.
const VECTORS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "TEST 1",
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "TEST 2",
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        "TEST 3",
        "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
    (
        "TEST SHA(abc)",
        "833fe62409237b9d62ec77587520911e9a759cec1d19755b7da901b96dca3d42",
        "ec172b93ad5e563bf4932c70e1245034c35467ef2efd4d64ebf819683467e2bf",
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        "dc2a4459e7369633a52b1bf277839a00201009a3efbf3ecb69bea2186c26b58909351fc9ac90b3ecfdfbc7c66431e0303dca179c138ac17ad9bef1177331a704",
    ),
];

/// A parsed vector: `(name, public key, message, signature)`.
type Vector = (&'static str, [u8; 32], Vec<u8>, [u8; 64]);

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd hex length: {s}");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or_else(|e| panic!("{s}: {e}")))
        .collect()
}

fn key(s: &str) -> [u8; 32] {
    unhex(s)
        .try_into()
        .unwrap_or_else(|v: Vec<u8>| panic!("a key is 32 bytes, not {}", v.len()))
}

fn sig(s: &str) -> [u8; 64] {
    unhex(s)
        .try_into()
        .unwrap_or_else(|v: Vec<u8>| panic!("a signature is 64 bytes, not {}", v.len()))
}

/// The vectors that do not verify (empty: all do).
fn failing(vectors: &[Vector]) -> Vec<String> {
    vectors
        .iter()
        .filter(|(_, pk, msg, s)| !verify(pk, msg, s))
        .map(|(name, ..)| (*name).to_string())
        .collect()
}

fn parsed() -> Vec<Vector> {
    VECTORS
        .iter()
        .map(|(name, _secret, pk, msg, s)| (*name, key(pk), unhex(msg), sig(s)))
        .collect()
}

#[test]
fn the_rfc_8032_section_7_1_vectors_verify() {
    let v = parsed();
    assert_eq!(v.len(), 4);
    // The SHA(abc) vector's message is SHA-512("abc") (FIPS 180-2): the transcription is right.
    {
        use sha2::Digest as _;
        assert_eq!(sha2::Sha512::digest(b"abc").to_vec(), v[3].2);
    }
    let f = failing(&v);
    assert!(f.is_empty(), "RFC 8032 §7.1 vectors rejected: {f:?}");
}

#[test]
fn positive_control_a_changed_vector_does_not_verify() {
    for (name, pk, msg, s) in parsed() {
        // One bit of R, one bit of S (the low bit keeps S canonical), one bit of A.
        for (what, pk2, s2) in [
            ("R", pk, {
                let mut x = s;
                x[3] ^= 0x10;
                x
            }),
            ("S", pk, {
                let mut x = s;
                x[32] ^= 0x01;
                x
            }),
            (
                "A",
                {
                    let mut x = pk;
                    x[5] ^= 0x04;
                    x
                },
                s,
            ),
        ] {
            let v = vec![(name, pk2, msg.clone(), s2)];
            assert_eq!(
                failing(&v),
                vec![name.to_string()],
                "{name} verified with one bit of {what} changed"
            );
        }
        // Another message.
        let mut m = msg.clone();
        m.push(0x00);
        assert!(!verify(&pk, &m, &s), "{name} verified another message");
    }
}
