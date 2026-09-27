# ADR 0022 — forge-render: a CPU last mile into camera-relative f32, CPU-binned clustered forward+

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.2.4, Ch.10 §10.1–10.9, Ch.1.5 (no `f32` below `forge-render`), I1,
  M1-3, M1-7

## Context

Ch.2.4 fixes the contract (resolve in the camera frame in `f64`, emit camera-relative
`f32`) and Ch.10 the renderer's shape (clustered forward+, PBR metal-rough, virtual shadow
maps). Left open: where the narrowing lives and how it is enforced, where light binning
runs, how draws are grouped, and the dev-build optimisation level.

## Decision

1. **One narrowing module.** Every `f64 -> f32` conversion in the engine is in
   `crates/forge-render/src/last_mile.rs`. `CameraView` resolves each `FramePos` into the
   camera's frame, subtracts the camera in `f64` and rotates into view axes (`ViewOffset`);
   instances upload a model-view matrix whose translation is that camera-relative offset.
   No view matrix on the GPU carries a translation. `tests/liveness/test_no_f32_below_render.rs`
   enforces (a) no `f32` in any workspace crate in `forge-render`'s dependency closure
   (three allow-listed glTF/mesh decode files in forge-asset), (b) the scanned set is
   complete, (c) no `as f32` / `f32::from` in forge-render outside `last_mile.rs`.

<!-- -->

3. **Light binning on the CPU, in `f64`.** 16x9 tiles x 48 log slices from 1 cm to 1e7 m;
   exact sphere-vs-padded-box, conservative by one pixel and 0.1 % depth. The shader loops
   only its cluster's list. Checked against brute force on the CPU (40,000 fragments) and on
   the GPU (default grid vs a 1x1x1 grid, identical pixels).
4. **Draws.** Visible instances are uploaded once to a storage buffer; every pass draws
   index ranges into it (`draw_index`), one draw per mesh per pass, front to back for early
   depth rejection.

5. **`forge-render` is optimised in dev builds** (`[profile.dev.package.forge-render]
   opt-level = 2`), as forge-ui and forge-asset are.

## Why — the owner's two rules

1. **Better for the user:** a character far from the world origin does not jitter, because
   the GPU only ever sees small camera-relative offsets; a debug run of the editor viewport
   is smooth (preparation 1.1 ms instead of 8.6 ms for 2,500 instances).

2. **Faster engine:** one narrowing pass per frame on data the CPU already walks; CPU
   binning costs no GPU pass, no GPU list allocation and no readback, and at M1 light counts
   it is sub-millisecond optimised.

## Alternatives rejected

- **Emulated double precision on the GPU / "relative-to-eye" high+low floats:** doubles the
  vertex bandwidth and every shader's position math to solve a problem that disappears
  when the CPU subtracts first.
- **Compute-shader light binning now:** adds a pass, an atomic list allocator and a
  capacity-overflow policy for no gain at hundreds of lights. It is the follow-up when light
  counts reach the thousands per view.

## Consequences

- Every future renderer feature receives camera-relative `f32` and must not narrow on its
  own — the lint fails otherwise.
- `prepare_naive_for_tests` and `last_mile::mutants` exist only as W2 controls
  (`#[doc(hidden)]`).
- Gate rows: `C-camera-relative-last-mile`, `C-no-f32-below-render`, `C-clustered-lights`,
  `C-pbr-matches-reference`, `C-render-goldens`, `C-render-pass-timings`.
