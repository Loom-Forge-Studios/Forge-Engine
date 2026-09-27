# ADR 0004 — Own DVec3/DQuat in forge-num, microsecond Ticks, debug-only seed provenance; faster pow and atan2

- **Status:** accepted
- **Date:** 2026-09-20

## Context

WP-02 builds `forge-frames` (Ch.2) and `forge-seed` (Ch.4), the I1 lint and the I3 guard,
and follows up WP-01's recorded perf debt in `forge-num` (`pow` 10x std, `atan2` 5.6x). The
plan leaves open: where `DVec3`/`DQuat` come from; the unit of `Tick`; how `SeedPath`
provenance rides on a `Copy` seed in debug builds only; and what "every registered
generator" means before the generator crates exist.

## Decision

1. **`DVec3` and `DQuat` are forge-num's own minimal types** (`forge_num::linalg`,
   re-exported by `forge-frames`). Every operation is a fixed sequence of IEEE `+ - * /`
   and `sqrt`, no FMA; the one transcendental use (`DQuat::from_axis_angle`/`_turns`) calls
   `forge_num::det::{sin, cos}`. There is no slerp, no `angle_between`, no Euler
   constructor — nothing that would reach a platform libm.
2. **`Tick` is an `i64` count of microseconds from the epoch** (`TICKS_PER_SECOND =
   1_000_000`): +-292,000 years of range. Periodic motion reduces its phase **exactly in
   integers** (`Tick::phase(period)`) before any floating point, so a spin angle is as
   precise 250,000 years out as at the epoch (tested). A 60 Hz step uses the integer
   schedule `floor(k * 10^6 / 60)`.
9. **Seed provenance is recorded in debug builds by an interning recorder.** `Seed` stays
   `Copy` (16 bytes in debug: value + `u32` handle; 8 in release). Each distinct `(parent,
   tag, index)` step is stored once in a global, mutex-guarded table capped at 2^20 steps
   (past it, new seeds report `path() == None` instead of growing without bound). Values
   never read the recorder; equality and hashing use the value only.
10. **"Registered generator" = an entry in a `&[Generator]` list exported by its crate**
    (`forge_seed::BASIS` now: `hash3`, `value_noise_i`, `simplex_i`, `fbm_i`, and
    `Seed::child` keyed by position). `test_seed_algebra::registry()` concatenates the lists;
    generator crates append theirs. The check evaluates backwards, shuffled, on threads and
    interleaved; its control is a generator with a call counter.
11. **The I1 lint is syn-based** (`tests/liveness/test_frame_liveness.rs`): public fns,
    trait fns, inherent methods, public struct fields and enum-variant fields; "bare" is
    `DVec3`/alias/`[f64; 3]`/`(f64, f64, f64)` anywhere in the type; "position-shaped" is a
    name token from a fixed list; plus a ban on global position type names. Allow-list
    `tests/liveness/i1_allow.txt` (one entry: `FrameTransform.translation`, the tree edge
    itself); entries need a reason and a stale entry fails.
12. **forge-num perf follow-up** — golden hashes unchanged, accuracy table unchanged
    (max 0.778 ulp overall; pow 0.607, atan2 0.507):
    - `ln_dd` (used by `pow`) is table-driven: 128 buckets over `[OFF, 2 OFF)` with `OFF`
      placing 1.0 at a bucket centre (`1/c = 1`, `ln c = 0` there, so `x -> 1` keeps its
      relative accuracy); `1/c` has <= 21 bits so `z_hi/c` and `z_lo/c` are exact
      products; `r = z/c - 1` is exact as a double-double, `|r| <= 2^-8`; `log1p(r)` carries
      `r - r^2/2 + r^3/3` in double-double (no division anywhere). ~2^-78 relative, was
      ~2^-70. The `ln c` table is derived on every test run from a bignum atanh series.
    - `atan2`: the quotient is computed once (was twice), the double-double division is one
      reciprocal plus one exact-remainder correction, and the known-ordered double-double
      sums (`atan c + u + tail`, `pi/2 - a`, `pi - a`) use a two-`fast_two_sum` add.
      Checked bit-for-bit against the previous implementation over the golden corpus.

## Why — the owner's two rules

1. **Better for the user**: the engine's own vector types cannot hand a game developer a
   libm-backed call that silently desynchronises client and server; a seed path in a bug
   report replays exactly; microsecond ticks never overflow a long-lived server or a
   time-warp game.

2. **Faster engine**: `pow` 119.4 -> 46.7 ns (10.0x -> 4.0x std), `atan2` 54.7 -> 32.5 ns
   (5.6x -> 3.4x std); `Seed::child` 4.0 ns in release, where provenance costs nothing.

## Alternatives rejected

- **glam (`DVec3`/`DQuat`, MIT/Apache).** Permissive and familiar, but its API carries
  libm-backed functions (`from_axis_angle`, `slerp`, `angle_between`, Euler conversions)
  that would each need a lint ban below `forge-render`, its f64 types are scalar (no speed
  gain), and interop with `bevy_math`'s f32 types happens only at the render boundary,
  where a conversion is needed anyway.
- **Flicks (1/705,600,000 s) for `Tick`.** Exact for every common frame rate, but +-414
  years of range; an integer schedule gives exact 60 Hz stepping with microseconds.
- **Carrying a full path inside every debug `Seed`** (an `Arc` chain) — makes `Seed`
  non-`Copy` in debug only, so code would compile differently per profile.
- **A stricter correctly-rounded `atan2`.** The WP-01 table-path denominator `1 + t c`
  omits the `tl c` term (~2^-60 in `u`); fixing it makes one golden-corpus sample correctly
  rounded (0.501 -> 0.499 ulp) and so **moves one golden row** (`det/atan2`). This WP was
  required to keep every golden hash; the fix is recorded as a follow-up that needs a plan
  amendment.

## Consequences

- Gate rows: I1 BOUND, I3 BOUND. DoD M0-6, M0-13 settled.
- Every crate above forge-num that needs a vector uses `forge_num::DVec3`; a position is a
  `FramePos`, enforced by the I1 lint on every commit.
- A generator crate must export its `&[Generator]` list and append it to
  `test_seed_algebra::registry()`, or I3 does not cover it.
- `forge-frames` has no `Reflect` derive yet (Ch.2.2 shows one): `forge-reflect` does not
  exist; its WP adds the derives.
- Follow-up: the `atan2` denominator accuracy fix above (moves one golden row).
