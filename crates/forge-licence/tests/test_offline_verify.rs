//! `test_offline_verify` (Ch.38 §38.6 `test_entitlement_offline_verify`, `test_tier_gate`,
//! `test_lapse_degrades_never_locks`): the **shipped** verify path — no `mint` feature — against
//! committed, signed entitlement fixtures, using the embedded (dev) public key.
//!
//! The fixtures are minted by `cargo run -p forge-licence --example mint_dev --features mint`.
//! These tests link the crate as a customer's build does (verify only), so they prove the real
//! shipped code, not a test-only signer.
//!
//! Positive controls (W2):
//! * `positive_control_a_forged_entitlement_fails` — a `Team` entitlement re-signed with a
//!   different key parses but does not verify, so a forged tier can never enable premium.
//! * `positive_control_a_tampered_field_fails` — changing a **signed value** (so the document
//!   still parses) makes verification fail with `VerifyError::BadSignature`, proving the
//!   signature — not merely the JSON parse — is what rejects a tampered entitlement.

use std::path::PathBuf;

use forge_licence::entitlement::Standing;
use forge_licence::{Gate, Tier, VerifyError, verify_signed};

const DAY: u64 = 86_400_000;
const YEAR_2100: u64 = 4_102_444_800_000;

fn fixture(name: &str) -> Vec<u8> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn key() -> [u8; 32] {
    forge_licence::EMBEDDED_PUBLIC_KEY
}

#[test]
fn a_valid_team_entitlement_verifies_offline_and_is_premium() {
    let e = verify_signed(&fixture("dev_team.json"), &key()).expect("dev_team must verify");
    assert_eq!(e.tier, Tier::Team { over_100k: false });
    let gate = Gate::from_signed(&fixture("dev_team.json")).expect("gate");
    // Far before expiry: premium is on.
    assert!(gate.premium_enabled(YEAR_2100 - DAY, "0.1.0"));
    assert_eq!(gate.standing(YEAR_2100 - DAY, "0.1.0"), Standing::Team);
}

#[test]
fn an_individual_entitlement_verifies_but_is_base() {
    let gate = Gate::from_signed(&fixture("dev_individual.json")).expect("gate");
    assert_eq!(gate.entitlement().map(|e| e.tier), Some(Tier::Individual));
    assert!(!gate.premium_enabled(0, "0.1.0"));
    assert_eq!(gate.standing(0, "0.1.0"), Standing::Base);
}

#[test]
fn verification_is_independent_of_the_clock_in_both_directions() {
    // The signature verifies regardless of `now`; only the standing depends on the clock. A
    // far-future Team never degrades (offline forever, §38.6 test_offline_forever spirit).
    let gate = Gate::from_signed(&fixture("dev_team.json")).expect("gate");
    for now in [0u64, DAY, YEAR_2100 - DAY, YEAR_2100 - 1] {
        assert!(gate.premium_enabled(now, "0.1.0"), "premium at now={now}");
    }
}

#[test]
fn a_lapsed_team_degrades_to_base_without_locking() {
    // dev_lapsed expired at 1970 + a day; well past grace by any realistic `now`.
    let gate = Gate::from_signed(&fixture("dev_lapsed.json")).expect("gate");
    let now = 1000 * DAY;
    assert!(matches!(
        gate.standing(now, "0.1.0"),
        Standing::Lapsed { .. }
    ));
    assert!(
        !gate.premium_enabled(now, "0.1.0"),
        "lapsed -> premium off (degrade to base)"
    );
    // Degrade, never lock: the entitlement still parsed and the gate is usable. The project's
    // openability is the caller's; here we assert the gate does not error or refuse to exist.
    assert!(gate.entitlement().is_some());
}

#[test]
fn positive_control_a_forged_entitlement_fails() {
    // forged_team.json is the dev_team entitlement re-signed with an attacker's key.
    let err = verify_signed(&fixture("forged_team.json"), &key())
        .expect_err("a forged entitlement must not verify");
    assert_eq!(err, VerifyError::BadSignature);
    // ... and the gate refuses to load it, so it can never enable premium.
    assert!(matches!(
        Gate::from_signed(&fixture("forged_team.json")),
        Err(VerifyError::BadSignature)
    ));
}

#[test]
fn positive_control_a_tampered_field_fails() {
    // Change a *signed value* so the document still parses as a well-formed signed entitlement,
    // and assert the **signature** is what rejects it (BadSignature) — not a JSON parse error.
    // The earlier version flipped an arbitrary byte, which usually landed in a JSON key and
    // failed as Malformed before the signature was ever checked, so it did not test verify().
    let good = String::from_utf8(fixture("dev_team.json")).expect("utf-8 fixture");

    // A. Tamper `expires_ms` (a numeric value: still valid JSON, still a u64, different bytes).
    assert!(good.contains("4102444800000"), "fixture shape changed");
    let tampered_expiry = good.replace("4102444800000", "4102444800001");
    assert_ne!(tampered_expiry, good);
    assert_eq!(
        verify_signed(tampered_expiry.as_bytes(), &key()),
        Err(VerifyError::BadSignature),
        "a changed expires_ms must fail the signature, not parse as valid"
    );

    // B. Tamper `over_100k` (a bool value): false -> true, still valid JSON.
    let tampered_band = good.replace("\"over_100k\": false", "\"over_100k\": true");
    assert_ne!(tampered_band, good, "over_100k must be present to tamper");
    assert_eq!(
        verify_signed(tampered_band.as_bytes(), &key()),
        Err(VerifyError::BadSignature),
        "a changed over_100k must fail the signature"
    );

    // Control-of-the-control: the untouched document verifies, so the failures above are the
    // tampering, not a broken fixture.
    assert!(verify_signed(good.as_bytes(), &key()).is_ok());
}

#[test]
fn a_missing_file_is_a_base_gate_not_an_error() {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "does-not-exist.json",
    ]
    .iter()
    .collect();
    let gate = Gate::from_file(&path).expect("a missing entitlement is base, not an error");
    assert!(!gate.premium_enabled(0, "0.1.0"));
}
