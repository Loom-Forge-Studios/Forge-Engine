//! Throughput of `forge_num::det` against the platform's std/libm, and of the noise basis.
//!
//! `cargo run --release -p forge-num --example bench`
//!
//! No benchmark framework (forge-num depends on nothing, and one dev-dependency for a
//! stopwatch is not worth it): each function runs over the same 4096 pre-generated inputs,
//! many passes, through `black_box`, best of 7 runs. The numbers in
//! docs/adr/0003-forge-num-deterministic-math.md come from this program.

#![allow(clippy::disallowed_methods)] // std is the comparison, never the answer

use forge_num::{IVec3, det, fbm_i, hash3, simplex_i, value_noise_i};
use std::hint::black_box;
use std::time::Instant;

const N: usize = 4096;
const PASSES: usize = 500;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }
}

/// Best-of-7 nanoseconds per call of `f` over `inputs`.
fn time<T: Copy>(inputs: &[T], f: impl Fn(T) -> f64) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..7 {
        let t = Instant::now();
        let mut acc = 0.0;
        for _ in 0..PASSES {
            for &x in inputs {
                acc += f(black_box(x));
            }
        }
        black_box(acc);
        best = best.min(t.elapsed().as_nanos() as f64 / (PASSES * inputs.len()) as f64);
    }
    best
}

fn main() {
    let mut r = Rng(7);
    let small: Vec<f64> = (0..N).map(|_| r.uniform(-10.0, 10.0)).collect();
    let big: Vec<f64> = (0..N).map(|_| r.uniform(1e7, 1e15)).collect();
    let expin: Vec<f64> = (0..N).map(|_| r.uniform(-700.0, 700.0)).collect();
    let pos: Vec<f64> = (0..N).map(|_| r.uniform(1e-10, 1e10)).collect();
    let pairs: Vec<(f64, f64)> = (0..N)
        .map(|_| (r.uniform(1e-3, 1e3), r.uniform(-50.0, 50.0)))
        .collect();
    let yx: Vec<(f64, f64)> = (0..N)
        .map(|_| (r.uniform(-1e3, 1e3), r.uniform(-1e3, 1e3)))
        .collect();

    println!("| function | inputs | det ns/call | std ns/call | det / std |");
    println!("|---|---|---:|---:|---:|");
    let row = |name: &str, dom: &str, d: f64, s: f64| {
        println!("| {name} | {dom} | {d:.2} | {s:.2} | {:.2}x |", d / s);
    };
    row(
        "sin",
        "[-10, 10]",
        time(&small, det::sin),
        time(&small, f64::sin),
    );
    row(
        "sin",
        "[1e7, 1e15] (Payne-Hanek)",
        time(&big, det::sin),
        time(&big, f64::sin),
    );
    row(
        "cos",
        "[-10, 10]",
        time(&small, det::cos),
        time(&small, f64::cos),
    );
    row(
        "exp",
        "[-700, 700]",
        time(&expin, det::exp),
        time(&expin, f64::exp),
    );
    row(
        "ln",
        "[1e-10, 1e10]",
        time(&pos, det::ln),
        time(&pos, f64::ln),
    );
    row(
        "pow",
        "x in [1e-3, 1e3], y in [-50, 50]",
        time(&pairs, |(x, y)| det::pow(x, y)),
        time(&pairs, |(x, y)| x.powf(y)),
    );
    row(
        "atan2",
        "[-1e3, 1e3]^2",
        time(&yx, |(y, x)| det::atan2(y, x)),
        time(&yx, |(y, x)| y.atan2(x)),
    );
    row(
        "sqrt",
        "[1e-10, 1e10]",
        time(&pos, det::sqrt),
        time(&pos, f64::sqrt),
    );

    let pts: Vec<IVec3> = (0..N)
        .map(|_| {
            IVec3::new(
                (r.next() >> 40) as i64,
                (r.next() >> 40) as i64,
                (r.next() >> 40) as i64,
            )
        })
        .collect();
    println!("\n| noise | ns/call |");
    println!("|---|---:|");
    println!(
        "| hash3 | {:.2} |",
        time(&pts, |p| hash3(p.x, p.y, p.z, 9) as f64)
    );
    println!(
        "| value_noise_i | {:.2} |",
        time(&pts, |p| value_noise_i(p, 9))
    );
    println!(
        "| simplex_i (scale 2^6) | {:.2} |",
        time(&pts, |p| simplex_i(p, 6, 9))
    );
    println!(
        "| fbm_i (6 octaves) | {:.2} |",
        time(&pts, |p| fbm_i(p, 10, 6, 9))
    );
}
