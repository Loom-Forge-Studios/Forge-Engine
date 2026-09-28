//! The generator registry that `test_seed_algebra` (I3) sweeps.
//!
//! A *generator* is anything that turns `(seed, integer position)` into generated data.
//! Ch.4.2's corollary: a generator that consumes randomness in a loop whose length depends
//! on data, or keeps any state between calls, has smuggled a PRNG stream back in — and its
//! output then depends on evaluation order. Every generator is registered in a `&[Generator]`
//! list exported by its crate (this crate's [`BASIS`]; a crate that adds generators exports
//! its own list) and [`check_order_independence`] evaluates each one in several orders and
//! on several threads, asserting identical output: `test_seed_algebra` runs it over every
//! list the build links.

use forge_num::{IVec3, fbm_i, hash3, simplex_i, value_noise_i};

use crate::Seed;

/// A registered generator: a name for failure messages, and a pure function of a seed and
/// an integer position to the bits it produces.
#[derive(Clone, Copy)]
pub struct Generator {
    pub name: &'static str,
    pub eval: fn(Seed, [i64; 3]) -> u64,
}

fn ivec(p: [i64; 3]) -> IVec3 {
    IVec3::new(p[0], p[1], p[2])
}

/// The generators that exist at the bottom of the spine: `forge-num`'s integer noise basis
/// keyed by a derived seed, and seed derivation itself keyed by position.
pub const BASIS: &[Generator] = &[
    Generator {
        name: "forge_num::hash3",
        eval: |s, p| hash3(p[0], p[1], p[2], s.raw()),
    },
    Generator {
        name: "forge_num::value_noise_i",
        eval: |s, p| value_noise_i(ivec(p), s.raw()).to_bits(),
    },
    Generator {
        name: "forge_num::simplex_i(scale 2^4)",
        eval: |s, p| simplex_i(ivec(p), 4, s.raw()).to_bits(),
    },
    Generator {
        name: "forge_num::fbm_i(scale 2^6, 5 octaves)",
        eval: |s, p| fbm_i(ivec(p), 6, 5, s.raw()).to_bits(),
    },
    Generator {
        name: "forge_seed::Seed::child(cell, position)",
        eval: |s, p| {
            let index =
                (p[0] as u64) ^ (p[1] as u64).rotate_left(21) ^ (p[2] as u64).rotate_left(42);
            s.child("cell", index).raw()
        },
    },
];

// ---- the order-independence check (the I3 guard's core, Ch.4.2) ---------------------------

/// The inputs every generator is evaluated on: seeds from derivation paths x positions
/// (origin, near, negative, huge).
fn inputs() -> Vec<(Seed, [i64; 3])> {
    let u = Seed::root(0xA11CE);
    let seeds: Vec<Seed> = (0..6u64)
        .map(|b| u.child("system", 3).child("body", b))
        .collect();
    let pts = [
        [0, 0, 0],
        [1, 2, 3],
        [-7, 11, -13],
        [1 << 20, -(1 << 21), 1 << 22],
        [i64::MAX / 3, i64::MIN / 5, 123_456_789_012],
        [-1, -1, -1],
        [255, 256, 257],
    ];
    seeds
        .iter()
        .flat_map(|&s| pts.iter().map(move |&p| (s, p)))
        .collect()
}

/// A deterministic permutation of `0..n` (no RNG state shared with anything under test).
#[must_use]
pub fn shuffled(n: usize, key: u64) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by_key(|&i| crate::splitmix64(key ^ i as u64));
    idx
}

/// Evaluate every generator of `gens` on a fixed set of inputs forwards, backwards, shuffled,
/// split across threads and interleaved with the other generators. `Err` names every
/// generator whose output for some input changed with evaluation order (Ch.4.2's corollary:
/// a generator with hidden state has smuggled a stream back in, and order exposes it).
///
/// # Errors
/// One line per generator and order that disagreed with the forward evaluation.
pub fn check_order_independence(gens: &[Generator]) -> Result<(), Vec<String>> {
    let xs = inputs();
    let n = xs.len();
    let reference: Vec<Vec<u64>> = gens
        .iter()
        .map(|g| xs.iter().map(|&(s, p)| (g.eval)(s, p)).collect())
        .collect();
    let mut bad = Vec::new();
    let mut compare = |gi: usize, how: &str, got: &[u64]| {
        if got != reference[gi].as_slice() {
            bad.push(format!(
                "{} changed output when evaluated {how}",
                gens[gi].name
            ));
        }
    };
    for (gi, g) in gens.iter().enumerate() {
        let mut out = vec![0; n];
        for i in (0..n).rev() {
            out[i] = (g.eval)(xs[i].0, xs[i].1);
        }
        compare(gi, "backwards", &out);
        let mut out = vec![0; n];
        for i in shuffled(n, 0xBEEF + gi as u64) {
            out[i] = (g.eval)(xs[i].0, xs[i].1);
        }
        compare(gi, "shuffled", &out);
        let threaded: Option<Vec<u64>> = std::thread::scope(|sc| {
            let hs: Vec<_> = xs
                .chunks(5)
                .map(|c| {
                    sc.spawn(move || c.iter().map(|&(s, p)| (g.eval)(s, p)).collect::<Vec<_>>())
                })
                .collect();
            let mut all = Vec::with_capacity(n);
            for h in hs {
                all.extend(h.join().ok()?);
            }
            Some(all)
        });
        // A generator that panicked on a worker thread is reported like one that disagreed.
        let out = threaded.unwrap_or_default();
        compare(gi, "on several threads", &out);
    }
    // Interleaved: every generator's i-th input before any generator's (i+1)-th.
    let mut inter = vec![vec![0u64; n]; gens.len()];
    for i in shuffled(n, 0xFACE) {
        for (gi, g) in gens.iter().enumerate().rev() {
            inter[gi][i] = (g.eval)(xs[i].0, xs[i].1);
        }
    }
    for (gi, out) in inter.iter().enumerate() {
        compare(gi, "interleaved with the other generators", out);
    }
    if bad.is_empty() { Ok(()) } else { Err(bad) }
}
