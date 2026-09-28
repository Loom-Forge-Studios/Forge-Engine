// timed-gates: exempt(S10 call costs and the wasm/native ratio are printed for the record, not asserted)
//! Spike S10, early numbers (Ch.32.3, M4-15 resolves it): what a call into a WASM plugin
//! costs, next to the same work done natively.
//!
//! Three shapes: an empty call (the boundary alone: lock, fuel, lifting three arguments
//! and lowering the result), a generator-node-shaped kernel (4096 samples of hashed noise
//! into an `f64` buffer: the shape of a generator node or a PCG rule's inner loop), and a
//! call that makes one capability-checked host import (`project-read`).
//!
//! The kernel's output must be **bit-identical** to the native kernel's: a WASM generator
//! node may not change a generated world (I2). That is asserted in every build; the timings
//! are printed, and the numbers recorded in ADR 0033 come from
//! `cargo test --release -p forge-wasm --test test_s10_call_overhead -- --nocapture`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use forge_plugin::{Capability, FsScope, Manifest, PluginId, Principal, SharedGrants};
use forge_wasm::{WasmHost, wat};

const SAMPLES: u32 = 4096;
const SEED: u64 = 0x5eed_f00d_0123_4567;

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The native generator node.
fn native(n: u32, seed: u64, out: &mut Vec<f64>) {
    out.clear();
    for i in 0..u64::from(n) {
        let h = mix(seed.wrapping_add(i.wrapping_mul(0x9e37_79b9_7f4a_7c15)));
        #[allow(clippy::cast_precision_loss)] // exact: h >> 11 < 2^53
        out.push((h >> 11) as f64 * f64::from_bits(0x3ca0_0000_0000_0000)); // 2^-53
    }
}

/// The same node in WAT. Input: `n: u32`, `seed: u64` (little endian). Output: `n` f64s.
fn kernel_plugin() -> String {
    wat::component(
        &[],
        r#"(func $mix (param $z i64) (result i64)
             (local.set $z (i64.mul (i64.xor (local.get $z) (i64.shr_u (local.get $z) (i64.const 30)))
                                    (i64.const 0xbf58476d1ce4e5b9)))
             (local.set $z (i64.mul (i64.xor (local.get $z) (i64.shr_u (local.get $z) (i64.const 27)))
                                    (i64.const 0x94d049bb133111eb)))
             (i64.xor (local.get $z) (i64.shr_u (local.get $z) (i64.const 31))))
           (func (export "call") (param i32 i32 i32 i32 i32 i32) (result i32)
             (local $n i32) (local $seed i64) (local $out i32) (local $i i32)
             (if (i32.lt_u (local.get 5) (i32.const 12)) (then (return (call $ok (i32.const 0) (i32.const 0)))))
             (local.set $n (i32.load (local.get 4)))
             (local.set $seed (i64.load (i32.add (local.get 4) (i32.const 4))))
             (local.set $out (call $alloc (i32.shl (local.get $n) (i32.const 3))))
             (block $done (loop $l
               (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
               (f64.store (i32.add (local.get $out) (i32.shl (local.get $i) (i32.const 3)))
                 (f64.mul
                   (f64.convert_i64_u (i64.shr_u
                     (call $mix (i64.add (local.get $seed)
                                         (i64.mul (i64.extend_i32_u (local.get $i)) (i64.const 0x9e3779b97f4a7c15))))
                     (i64.const 11)))
                   (f64.const 0x1p-53)))
               (local.set $i (i32.add (local.get $i) (i32.const 1)))
               (br $l)))
             (call $ok (local.get $out) (i32.shl (local.get $n) (i32.const 3))))"#,
    )
}

fn per_iter(iters: u32, mut f: impl FnMut()) -> Duration {
    for _ in 0..iters / 10 {
        f(); // warm
    }
    let t = Instant::now();
    for _ in 0..iters {
        f();
    }
    t.elapsed() / iters
}

#[test]
fn s10_wasm_call_overhead_and_a_bit_exact_generator_node() {
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let m = |id: &str, caps: &str| {
        Manifest::parse(&format!(
            r#"Plugin(id: "{id}", version: "0.1.0", engine: "^0.1", kind: Wasm, capabilities: [{caps}])"#
        ))
        .unwrap_or_else(|e| panic!("{e}"))
    };
    let kernel = host
        .load(m("com.example.noise", ""), kernel_plugin().as_bytes())
        .unwrap_or_else(|e| panic!("{e}"))
        .item("GeneratorNode", "noise");

    let mut input = SAMPLES.to_le_bytes().to_vec();
    input.extend_from_slice(&SEED.to_le_bytes());
    let out = kernel.call(&input).unwrap_or_else(|e| panic!("{e}"));
    let mut want = Vec::new();
    native(SAMPLES, SEED, &mut want);
    let got: Vec<f64> = out
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| f64::from_le_bytes(*c))
        .collect();
    assert_eq!(got.len(), want.len());
    assert!(
        got.iter()
            .zip(&want)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "the WASM generator node must be bit-identical to the native one"
    );

    // Timings (printed; see the module docs for where they are recorded).
    let empty = per_iter(20_000, || {
        let _ = kernel.call(&[]);
    });
    let wasm_kernel = per_iter(500, || {
        let _ = kernel.call(&input);
    });
    let native_kernel = per_iter(500, || native(SAMPLES, SEED, &mut want));

    let reader = host
        .load(
            m("com.example.reader", "Fs(ProjectRead)"),
            wat::component(
                &["project-read"],
                r#"(func (export "call") (param i32 i32 i32 i32 i32 i32) (result i32)
                     (call $read (local.get 4) (local.get 5) (i32.const 64)) (i32.const 64))"#,
            )
            .as_bytes(),
        )
        .unwrap_or_else(|e| panic!("{e}"))
        .item("Probe", "read");
    host.set_project_reader(Some(Arc::new(|_: &str| Ok(vec![7u8; 16]))));
    grants.grant(
        Principal::Plugin(PluginId::new("com.example.reader").unwrap_or_else(|e| panic!("{e}"))),
        Capability::Fs(FsScope::ProjectRead),
    );
    assert_eq!(
        reader.call(b"a.txt").unwrap_or_else(|e| panic!("{e}")),
        vec![7u8; 16]
    );
    let import = per_iter(20_000, || {
        let _ = reader.call(b"a.txt");
    });

    #[allow(clippy::cast_precision_loss)]
    let ratio = wasm_kernel.as_secs_f64() / native_kernel.as_secs_f64().max(1e-12);
    println!(
        "S10 ({} build): empty call {:?}; call + one host import {:?}; generator node {} samples: wasm {:?} vs native {:?} ({ratio:.2}x)",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        empty,
        import,
        SAMPLES,
        wasm_kernel,
        native_kernel,
    );
}
