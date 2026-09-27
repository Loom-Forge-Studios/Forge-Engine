# ADR 0003 — Vendor faithfully-rounded transcendentals and an exact integer noise kernel in forge-num; Spike S2 verdict

- **Status:** accepted
- **Date:** 2026-09-20
- **Plan references:** Ch.1.1, Ch.3 (all), Ch.4.1; I2; M0-2, M0-3; Spike S2 (Appendix B);
  E-6, E-33; D-2; W2, W5, W9

## Context

Ch.3.1 rules that `forge-num` vendors every transcendental reachable from generation,
"correctly-rounded or at minimum bit-reproducible", because platform libms disagree in the
low bits and a hash amplifies that into a different world. Spike S2 asks whether bit-exact
`f64` transcendentals are harder than expected; its fallback is a softfloat path for the
noise basis only.

## Decision

1. **Only IEEE-754 `+ - * /` and `sqrt` in round-to-nearest, integer ops and bit
   manipulation.** No FMA (explicit or otherwise; products are split with Dekker's
   algorithm), no platform `floor`/`round` (rounding to integer is the `1.5·2^52` add/sub
   trick), no data-dependent loop counts, no lookup into the C runtime. These operations are
   fixed to the bit by IEEE-754, and Rust neither contracts nor reassociates floating point
   at any opt level, so the results are identical on every conforming platform. `sqrt` is
   forwarded to the hardware instruction (IEEE-exact).
2. **Faithful rounding (< 1 ulp) is the accuracy contract**, pinned by
   `crates/forge-num/tests/accuracy.rs` against an independent ~2^-90 reference. Algorithms:
   - `sin`/`cos`: reduction to [-pi/4, pi/4] in double-double — Cody-Waite with four pieces
     of pi/2 below 1.6e6, Payne-Hanek (exact 53x192-bit integer product against a 1280-bit
     2/pi table) above; Taylor kernels with an exact leading term and the `1 - x^2/2`
     rounding error recovered.
   - `exp`: `k ln2 + r` with a 42-bit `LN2_HI` (so `k·LN2_HI` is exact), Taylor `e^r - 1`
     with `1 + r` error-free, single-rounding scaling into subnormals.
   - `ln`: `f - f²/2 + s(f²/2 + R(s²))`, `s = f/(2+f)`, exact `f`.
   - `pow`: `exp(y ln x)` with `ln x` in double-double (~2^-72 relative) and the product
     `y · ln x` error-free, so the exponent stays accurate when `|y ln x|` ~ 709. Full
     C99 Annex F special cases.
   - `atan2`: `t = min/max` with the division's rounding error recovered, table of
     `atan(j/8)` in double-double, quadrant fix-ups in double-double, one final rounding.
3. **Every NaN returned is the canonical `0x7FF8_0000_0000_0000`.** x86-64's default NaN
   is negative, aarch64's positive: a NaN produced by an operation would hash differently.
4. **Every embedded constant is re-derived from first principles on every test run** (a
   test-only bignum: Machin's formula, ln 2 and atan series, Newton reciprocal for 2/pi).
   Nothing is typed from memory.
5. **forge-num takes `u64` seeds, defines no `Seed`.** `forge-seed` wraps and derives
   (Ch.4.1). This keeps forge-num a leaf with no forge deps (Ch.1.1) and is a plan
   amendment to the Ch.3.2 signatures, recorded there.
6. **`simplex_i` is exact integer arithmetic up to the kernel weights.** 3D simplex's skew
   factors are exactly 1/3 and 1/6, so with a fixed-point denominator of `6·2^scale` the
   skew, cell choice, simplex ordering, corner offsets and the kernel support test are all
   integers. Floats enter only as a fixed sequence of correctly-rounded operations per
   corner. Noise is therefore bit-identical everywhere *and* equally detailed at the origin
   and 2^50 units out. The kernel is generic over i64/i128: i64 whenever every intermediate
   provably fits (|p| < 2^56, feature size <= 2^20), i128 otherwise — same bits, pinned by
   `i64_and_i128_kernels_agree_bit_for_bit`. `IVec3` is i64 (a large world at 0.25 m
   outgrows 32 bits). `scale_log2` is clamped to [-24, 56].
7. **Corpus now: what exists.** 276 BLAKE3 rows in `tests/determinism/golden.txt`: sweeps of
   every det function over specials, raw bit patterns, huge/tiny/subnormal and working
   ranges; `hash3`, `value_noise_i`, `simplex_i` at eight
   scales and four origins up to -2^62; and 256 fBm "chunks" (8 seeds x 4 LODs x 8 chunks of
   8^3 samples, plus their `tree_sum`). Later crates append rows; appending never changes existing
   rows.
   There is no bless switch — a mismatch prints the complete regenerated file.
8. **The `mutate-det` positive control is the real feature build.** The gate's control test
   spawns `cargo test --features mutate-det` (octave 0 of `fbm_i` perturbed by 1 ulp) in a
   separate target dir and asserts it fails, on exactly the 256 chunk rows and no other.
   Verified: with the perturbation turned into a no-op, the control goes red.
9. **Two guards keep forge-num off the platform libm**: a crate-local `clippy.toml`
   `disallowed-methods` list (f64 transcendentals and `mul_add`), and
   `tests/test_no_platform_libm.rs` (source scan + empty `[dependencies]`), each with a
   positive control. `tree_sum` provides the Ch.3.3 fixed-order reduction.
10. **Horner polynomials are written as `horner(x, &COEFFS)`** (a fixed-length fold, the
    same operation order as the nested form): rustfmt does not terminate in reasonable
    time on 13-deep nested Horner expressions.

## Measured

Accuracy, `FORGE_ACCURACY_SCALE=50`, 3.1 M samples, Windows x86-64 (release):

| function | domain | samples | max ulp | mean ulp | correctly rounded | bit-equal to MSVC std |
|---|---|---:|---:|---:|---:|---:|
| sin | [-pi/4, pi/4] | 200000 | 0.705 | 0.2519 | 97.69% | 96.64% |
| sin | \|x\| in [2^-30, 2^20] log | 200000 | 0.713 | 0.2347 | 99.27% | 98.39% |
| sin | \|x\| in [2^20, 2^1023] log (Payne-Hanek) | 100000 | 0.778 | 0.2511 | 98.47% | 96.60% |
| sin | x ~ n pi/2, n < 2^20 (cancellation) | 100000 | 0.500 | 0.1256 | 100.00% | 100.00% |
| cos | [-pi/4, pi/4] | 200000 | 0.550 | 0.2504 | 99.62% | 95.56% |
| cos | \|x\| in [2^-30, 2^20] log | 200000 | 0.723 | 0.2371 | 99.34% | 98.32% |
| cos | \|x\| in [2^20, 2^1023] log (Payne-Hanek) | 100000 | 0.746 | 0.2511 | 98.47% | 96.49% |
| cos | x ~ n pi/2, n < 2^20 (cancellation) | 100000 | 0.500 | 0.1248 | 100.00% | 100.00% |
| exp | [-708, 709.7] | 200000 | 0.599 | 0.2502 | 98.73% | 98.66% |
| exp | \|x\| in [2^-50, 1] log | 200000 | 0.593 | 0.2509 | 99.87% | 98.64% |
| exp | [-745, -708] (subnormal results) | 50000 | 0.749 | 0.2511 | 99.14% | 99.97% |
| ln | x in [2^-1074, 2^1023] log | 200000 | 0.682 | 0.2499 | 99.89% | 99.89% |
| ln | x = 1 ± 2^-k, k in [1, 52] (cancellation) | 200000 | 0.739 | 0.2424 | 99.47% | 99.48% |
| pow | x in [2^-20, 2^20] log, y in [-30, 30] | 200000 | 0.607 | 0.2503 | 98.76% | 98.77% |
| pow | x = 1 ± 2^-k, \|y ln x\| up to ~650 | 200000 | 0.594 | 0.2510 | 98.77% | 98.78% |
| pow | x in [0.5, 2], y integer in [-1000, 1000] | 100000 | 0.602 | 0.2498 | 98.80% | 98.80% |
| atan2 | \|y\|, \|x\| in [2^-60, 2^60] log, all quadrants | 200000 | 0.502 | 0.2552 | 99.99% | 99.91% |
| atan2 | \|y\| ~ \|x\| | 100000 | 0.507 | 0.2494 | 99.85% | 99.84% |
| atan2 | y/x in [2^-40, 2^-3] | 100000 | 0.502 | 0.2488 | 99.99% | 99.58% |

**Worst case over everything: 0.778 ulp.** The last column is the point of Ch.3.1: even
this accurate platform libm disagrees with a faithfully-rounded result on 0.1–4.5% of
inputs (the libm is not wrong — both are within 1 ulp — they simply round differently), and
a different libm would disagree on a different set.

Throughput, `cargo run --release -p forge-num --example bench` (the dev machine, 12 logical CPUs,
best of 7, 2M calls each):

| function | det ns/call | MSVC std ns/call | det / std |
|---|---:|---:|---:|
| sin [-10, 10] | 13.6 | 4.8 | 2.8x |
| sin [1e7, 1e15] (Payne-Hanek) | 31.4 | 12.6 | 2.5x |
| cos [-10, 10] | 14.0 | 4.8 | 2.9x |
| exp [-700, 700] | 11.9 | 3.4 | 3.5x |
| ln [1e-10, 1e10] | 8.9 | 3.8 | 2.3x |
| pow | 119.5 | 11.7 | 10.2x |
| atan2 | 54.8 | 9.8 | 5.6x |
| sqrt | 1.9 | 1.9 | 1.0x |
| hash3 / value_noise_i | 4.5 | — | — |
| simplex_i | 49.1 | — | — |
| fbm_i, 6 octaves | 314 | — | — |

Optimisation already applied (and the golden hashes did not move): a two-step
double-double division and double-double constant multiplies in `ln_dd` (pow 225 -> 120
ns); the i64 fast path in the simplex kernel (150 -> 49 ns).

## Spike S2 verdict

**Not harder than expected; the fallback (softfloat for the noise basis) is not needed.**
The functions are faithfully rounded by construction, bit-reproducibility follows from
using only IEEE-fixed operations (argued in Decision 1, pinned by the golden-hash gate), and
the cost is 2.3–3.5x std for the common functions. The noise basis needs no softfloat at
all: it is exact integer arithmetic. **The Windows leg is observed green; the ubuntu leg is
AWAITING** (gate row `C-determinism-linux-leg`: no Linux runner here, Docker daemon off —
CI runs it on the owner's push). If that leg ever disagrees, the disagreement is a defect
in this crate (I18), not a caveat; the canonical-NaN rule and the no-FMA rule are the two
places to look first.

## Why — the owner's two rules

1. **Better for the user:** a seed produces the same world on every player's machine and
   the server, so "baseline, hash = X" (Ch.3.5) costs nothing in the common case; noise is
   as detailed a billion units from the origin as at it; results are *more* accurate than
   typical platform libms, never less.
2. **Faster engine:** the common transcendentals cost single-digit to low-double-digit
   nanoseconds; the noise kernel avoids i128 on every realistic input; no softfloat
   anywhere. pow and atan2 are the slow ones (follow-up below).

## Alternatives rejected

- **Port FreeBSD msun / musl / the `libm` crate.** Its licence notice is Sun's permissive
  one, not on the Appendix A.5 list; and the code would still need auditing for FMA and
  platform `floor`. Written from the mathematics instead.
- **Correctly-rounded (CORE-MATH style) everywhere.** Needs a rare-case slow path and
  table sets several times larger; faithful rounding with a documented < 0.78 ulp worst case
  already meets the contract, and determinism never depended on correct rounding.
- **Float simplex with a float skew.** Loses fractional precision with distance from the
  origin and depends on rounding of an irrational-looking constant; the exact integer form
  costs nothing extra.
- **A `Seed` newtype in forge-num.** Would duplicate forge-seed's type (Ch.1.3 says every
  crate uses the one type).

## Consequences

- A golden change needs a plan amendment and a line here or in a new ADR (Ch.3.4).
- The `mutate-det` feature must never be enabled in a shipping build (`forge_num::MUTATE_DET`
  exposes it; forge-runtime's I21 dependency guard can assert it when it lands).
- Follow-ups: table-driven `ln_dd` (a 128-entry `ln(c_i)` double-double table, derived by the
  bignum) is expected to cut pow substantially (not yet measured); `atan2` can drop one double-double division the same
  way. Both are pure optimisations that must not move a golden hash.
