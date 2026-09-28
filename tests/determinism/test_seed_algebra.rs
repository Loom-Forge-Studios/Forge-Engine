//! I3 — seeds derive. They are never accumulated, stored, or passed as mutable state
//! (Ch.4).
//!
//! * The derivation is exactly Ch.4.1's formula, with `splitmix64` and `fnv1a` pinned to
//!   their published test vectors (so neither can silently change and move every world).
//! * Deriving the same paths in any order, on any number of threads, gives the same seeds.
//! * **Every registered generator** (`forge_seed::BASIS`, [`registry`]; a crate with
//!   generators of its own runs the same `forge_seed::check_order_independence` over them)
//!   produces identical output when its inputs are evaluated in a different order —
//!   forwards, backwards, shuffled, interleaved with the other generators, and split across
//!   threads (Ch.4.2's corollary: a generator with hidden state has smuggled a stream back
//!   in, and order exposes it).
//! * A `SeedPath` string replays to the same seed (Ch.4.3).
//!
//! **Positive control (W2):** a generator that keeps a call counter — a PRNG stream in
//! disguise — is run through the same order-independence check, which must reject it and
//! name it.

// A test harness: its helpers panic on a broken fixture by design (the no-unwrap rule of
// Ch.1.2 governs engine code; clippy exempts only `#[test]` bodies, not their helpers).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use forge_seed::{
    BASIS, Generator, Seed, SeedPath, check_order_independence, fnv1a, shuffled, splitmix64,
};
use proptest::prelude::*;

/// Every generator the base edition registers.
fn registry() -> Vec<Generator> {
    BASIS.to_vec()
}

// ---- the formula and its primitives -------------------------------------------------------

#[test]
fn primitives_match_their_published_test_vectors() {
    // FNV-1a 64: the reference vectors from the FNV specification.
    assert_eq!(fnv1a(""), 0xCBF2_9CE4_8422_2325);
    assert_eq!(fnv1a("a"), 0xAF63_DC4C_8601_EC8C);
    assert_eq!(fnv1a("foobar"), 0x8594_4171_F739_67E8);
    // SplitMix64: the first outputs of a generator seeded with 0 (Vigna's reference code).
    assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
    assert_eq!(splitmix64(0x9E37_79B9_7F4A_7C15), 0x6E78_9E6A_A1B9_65F4);
}

#[test]
fn child_is_exactly_the_4_1_formula() {
    let u = Seed::root(0x5EED);
    for (tag, index) in [
        ("world", 3u64),
        ("zone", 118),
        ("tree", 2),
        ("region", 14_883),
    ] {
        let want = splitmix64(u.raw() ^ fnv1a(tag).rotate_left(17) ^ splitmix64(index));
        assert_eq!(u.child(tag, index).raw(), want, "{tag}:{index}");
    }
}

// ---- derivation order independence -------------------------------------------------------

/// A spread of derivation paths: worlds x zones x trees x regions.
fn paths() -> Vec<SeedPath> {
    let mut v = Vec::new();
    for g in 0..4u64 {
        for s in [0u64, 1, 118, u64::MAX] {
            for b in 0..3u64 {
                for r in [0u64, 14_883, 1 << 40] {
                    v.push(
                        SeedPath::universe()
                            .child("world", g)
                            .child("zone", s)
                            .child("tree", b)
                            .child("region", r),
                    );
                }
            }
        }
    }
    v
}

#[test]
fn derivation_is_independent_of_order_and_threads() {
    let u = Seed::root(0xC0FF_EE00_1234_5678);
    let ps = paths();
    let forward: Vec<u64> = ps.iter().map(|p| p.resolve(u).raw()).collect();
    let mut backward = vec![0; ps.len()];
    for i in (0..ps.len()).rev() {
        backward[i] = ps[i].resolve(u).raw();
    }
    assert_eq!(forward, backward);
    let mut shuf = vec![0; ps.len()];
    for i in shuffled(ps.len(), 7) {
        shuf[i] = ps[i].resolve(u).raw();
    }
    assert_eq!(forward, shuf);
    let threaded: Vec<u64> = thread::scope(|s| {
        let hs: Vec<_> = ps
            .chunks(17)
            .map(|c| s.spawn(move || c.iter().map(|p| p.resolve(u).raw()).collect::<Vec<_>>()))
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(forward, threaded);
    // Distinct paths give distinct seeds over this set (collision resistance, sampled).
    let mut uniq = forward.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), forward.len());
}

// ---- generator order independence --------------------------------------------------------

#[test]
fn test_seed_algebra() {
    let gens = registry();
    assert!(
        !gens.is_empty(),
        "no generators registered: the check would be vacuous"
    );
    assert_eq!(check_order_independence(&gens), Ok(()));
}

static STREAM: AtomicU64 = AtomicU64::new(0);

/// A generator that smuggles a stream back in: its output depends on how many calls came
/// before it.
fn stream_in_disguise(s: Seed, p: [i64; 3]) -> u64 {
    let drawn = STREAM.fetch_add(1, Ordering::Relaxed);
    splitmix64(s.raw() ^ p[0] as u64 ^ drawn)
}

#[test]
fn positive_control_a_stateful_generator_is_rejected() {
    let mut gens = registry();
    gens.push(Generator {
        name: "control::stream_in_disguise",
        eval: stream_in_disguise,
    });
    let err = check_order_independence(&gens).expect_err("a stateful generator passed");
    assert!(
        err.iter()
            .all(|e| e.starts_with("control::stream_in_disguise")),
        "the check blamed a registered generator: {err:?}"
    );
    assert!(!err.is_empty());
}

// ---- properties and provenance ------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x05EE_DA16_EB7A),
        ..ProptestConfig::default()
    })]

    /// Derivation is a pure function: same parent, tag and index give the same seed; a
    /// different index gives a different one.
    #[test]
    fn child_is_pure_and_index_sensitive(root in any::<u64>(), i in any::<u64>(), j in any::<u64>()) {
        let u = Seed::root(root);
        prop_assert_eq!(u.child("region", i), u.child("region", i));
        if i != j {
            prop_assert_ne!(u.child("region", i), u.child("region", j));
        }
        prop_assert_ne!(u.child("region", i), u.child("regiom", i));
    }

    /// A path printed and parsed replays to the same seed.
    #[test]
    fn seed_paths_round_trip_through_text(root in any::<u64>(), steps in prop::collection::vec((0usize..4, any::<u64>()), 0..6)) {
        const TAGS: [&str; 4] = ["world", "zone", "tree", "region"];
        let u = Seed::root(root);
        let mut path = SeedPath::universe();
        let mut direct = u;
        for (t, i) in steps {
            path = path.child(TAGS[t], i);
            direct = direct.child(TAGS[t], i);
        }
        let parsed: SeedPath = path.to_string().parse().unwrap();
        prop_assert_eq!(parsed.resolve(u), direct);
        if cfg!(debug_assertions) {
            prop_assert_eq!(direct.path(), Some(path));
        }
    }
}
