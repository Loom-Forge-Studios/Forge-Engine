//! Cost of one `Seed::child` step. `cargo run --release -p forge-seed --example bench_child`
//! (release: no path recording; a debug build also interns the path, see `Seed::path`).

use std::hint::black_box;
use std::time::Instant;

use forge_seed::Seed;

fn main() {
    const N: u64 = 10_000_000;
    let best = (0..7)
        .map(|_| {
            let t = Instant::now();
            let mut acc = 0u64;
            let u = Seed::root(black_box(7));
            for i in 0..N {
                acc ^= u.child(black_box("region"), i).raw();
            }
            black_box(acc);
            t.elapsed().as_nanos() as f64 / N as f64
        })
        .fold(f64::MAX, f64::min);
    println!("| Seed::child | {best:.2} ns |");
}
