//! D-0001 — a command log re-read from a store carries **exactly** the values that were
//! committed, every float bit for bit (Ch.33.4: the command log is the real history; a
//! replay that lands 1 ulp off is a different project).
//!
//! serde_json's default float parser is fast but not correctly rounded: it could return a
//! neighbour of the printed shortest representation (`262682861718.86282` came back as
//! `262682861718.8628`), found by `forge spine`'s reload check. The workspace enables
//! `float_roundtrip`; this test pins it on both M0 backends with 4 000 random bit patterns
//! (every exponent, subnormals, `-0.0`) plus the value that exposed it.
//!
//! Positive control (W2): `positive_control_a_lossy_float_codec_is_caught` runs the same
//! comparison against a codec that keeps 16 significant digits (one short of round-trip) and
//! must find mismatches — so the corpus really contains values that need all 17.

use forge_cmd::{CommandEnvelope, CommandId, EditorCommand, EntityKey, Issuer, TxnId, Value};
use forge_store::{LocalFs, MemoryStore, ProjectStore};

fn floats() -> Vec<f64> {
    let mut s: u64 = 0xD000_0001;
    let mut out = vec![
        262_682_861_718.862_82,
        -0.0,
        5e-324,
        f64::MAX,
        f64::MIN_POSITIVE,
        0.1,
    ];
    while out.len() < 4_000 {
        s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let x = f64::from_bits(z ^ (z >> 31));
        if x.is_finite() {
            out.push(x);
        }
    }
    out
}

fn envelopes(xs: &[f64]) -> Vec<CommandEnvelope> {
    xs.chunks(4)
        .enumerate()
        .map(|(i, c)| CommandEnvelope {
            id: CommandId(i as u64),
            txn: TxnId(i as u64),
            issuer: Issuer::Test,
            cmd: EditorCommand::SetProperty {
                entity: EntityKey(0),
                path: "p".into(),
                value: if c.len() == 4 && i % 2 == 0 {
                    Value::Vec3([c[1], c[2], c[3]])
                } else {
                    Value::Float(c[0])
                },
            },
        })
        .collect()
}

/// Every envelope whose value did not survive bit for bit.
fn mismatches(sent: &[CommandEnvelope], got: &[CommandEnvelope]) -> Vec<String> {
    assert_eq!(sent.len(), got.len());
    sent.iter()
        .zip(got)
        .filter(|(a, b)| a != b) // Value's PartialEq compares float bits
        .map(|(a, b)| format!("{:?} came back as {:?}", a.cmd, b.cmd))
        .collect()
}

fn round_trip(store: &mut dyn ProjectStore, sent: &[CommandEnvelope]) -> Vec<CommandEnvelope> {
    let rev = store.commit("floats", sent).expect("commit");
    store.commands(&rev).expect("log")
}

#[test]
fn command_log_floats_survive_every_backend_bit_for_bit() {
    let sent = envelopes(&floats());
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("log-exact");
    let _ = std::fs::remove_dir_all(&dir);
    let mut local = LocalFs::open(&dir, "t").expect("open");
    let mut memory = MemoryStore::new("t");
    for (name, got) in [
        ("local-fs", round_trip(&mut local, &sent)),
        ("memory", round_trip(&mut memory, &sent)),
    ] {
        let bad = mismatches(&sent, &got);
        assert!(
            bad.is_empty(),
            "{name}: {} lossy values, e.g. {}",
            bad.len(),
            bad[0]
        );
    }
}

#[test]
fn positive_control_a_lossy_float_codec_is_caught() {
    let sent = envelopes(&floats());
    let lossy = |x: f64| format!("{x:.15e}").parse::<f64>().unwrap_or(x);
    let got: Vec<CommandEnvelope> = sent
        .iter()
        .map(|e| {
            let mut e = e.clone();
            if let EditorCommand::SetProperty { value, .. } = &mut e.cmd {
                *value = match &*value {
                    Value::Float(x) => Value::Float(lossy(*x)),
                    Value::Vec3(v) => Value::Vec3(v.map(lossy)),
                    other => other.clone(),
                };
            }
            e
        })
        .collect();
    let bad = mismatches(&sent, &got);
    assert!(
        bad.len() > sent.len() / 4,
        "the corpus must be full of values needing 17 digits; only {} caught",
        bad.len()
    );
}
