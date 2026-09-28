//! `cargo run -p forge-licence --example mint_dev --features mint`
//!
//! Developer/CI helper: derive the **dev** public key from the committed dev seed and mint a
//! few signed entitlement fixtures the tests and the premium-editor integration test use. It
//! prints the dev public key as 64 hex characters — paste it into `DEV_PUBLIC_KEY` in
//! `src/key.rs` whenever the dev seed changes. It never touches the production key.

use std::path::PathBuf;

use forge_licence::entitlement::{Entitlement, SeatKind, Tier};
use forge_licence::{crypto, sign_entitlement};

// The committed dev seed. A *test* key: the production private key is held offline and injected
// through FORGE_LICENCE_PUBKEY at release (src/key.rs). Must match crypto.rs's DEV_SEED.
const DEV_SEED: [u8; 32] = *b"forge-dev-signing-key-0001!!!!!!";

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn main() {
    let pk = crypto::public_key(&DEV_SEED);
    println!("DEV_PUBLIC_KEY (paste into src/key.rs): {}", hex(&pk));

    let fixtures: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures"]
        .iter()
        .collect();
    std::fs::create_dir_all(&fixtures).expect("create fixtures dir");

    // A perpetual-ish Team entitlement far in the future: the "premium is entitled" fixture.
    let team = Entitlement {
        tier: Tier::Team { over_100k: false },
        seat: SeatKind::Purchaser,
        bound: None,
        expires_ms: Some(4_102_444_800_000), // 2100-01-01
        fallback: None,
    };
    // An Individual licence: base, no premium.
    let individual = Entitlement {
        tier: Tier::Individual,
        seat: SeatKind::Purchaser,
        bound: None,
        expires_ms: None,
        fallback: None,
    };
    // A Team entitlement that expired in 1970 + a day: lapsed past grace -> degrade to base.
    let lapsed = Entitlement {
        tier: Tier::Team { over_100k: false },
        seat: SeatKind::Purchaser,
        bound: None,
        expires_ms: Some(86_400_000),
        fallback: None,
    };

    for (name, e) in [
        ("dev_team", &team),
        ("dev_individual", &individual),
        ("dev_lapsed", &lapsed),
    ] {
        let signed = sign_entitlement(e, &DEV_SEED).expect("sign");
        let json = serde_json::to_string_pretty(&signed).expect("json");
        let path = fixtures.join(format!("{name}.json"));
        std::fs::write(&path, json).expect("write fixture");
        println!("wrote {}", path.display());
    }

    // A forged variant: the dev_team entitlement re-signed with a DIFFERENT key. It parses, but
    // must fail verification against the embedded dev key — the positive control for the gate.
    let forged_seed = *b"an-attackers-forging-seed-32byte";
    let msg = forge_licence::entitlement::canonical_bytes(&team).expect("canon");
    let forged_sig = crypto::sign(&forged_seed, &msg);
    let forged = forge_licence::SignedEntitlement {
        entitlement: team.clone(),
        sig: hex(&forged_sig),
    };
    let path = fixtures.join("forged_team.json");
    std::fs::write(&path, serde_json::to_string_pretty(&forged).expect("json")).expect("write");
    println!("wrote {}", path.display());
}
