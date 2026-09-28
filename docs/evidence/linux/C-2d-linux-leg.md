# C-2d-goldens-linux-leg — the 2D pipeline on ubuntu-x86_64 (lavapipe) — and DoD M4-11

Image `forge-linux-verify:1.98.1` (`sha256:0129932d13abccd08261a95d360084b6d36111e44e3b52e8bf401fb84baac7e7`, built from `rust:1.98.1-bookworm`); environment in [README](README.md); 2026-09-24. Snapshot tree `9dd5dacd0c8f118de1fa30eea38e57a4d5c42f18` of lane B (WP-U15), as printed by `verify-linux` before the run.

**Command** (one container, `--rm`):

```
just verify-linux bash -c '"CI=true cargo test --locked -p forge-2d -p forge-2d-platformer -- --nocapture
  && CI=true cargo test --locked -p forge-tests --test test_cross_platform_hash --test test_2d_tax_is_zero
     --test test_frame_liveness --test test_no_f32_below_render -- --nocapture
  && CI=true cargo test --locked -p forge-panels-domain --test test_editors_2d -- --nocapture"'
```

Every test in the three commands passed with `CI=true` (no adapter would have failed the GPU
tests, W9). (A trailing `git rev-parse` in the command line exited 128 because the snapshot
carries no `.git`; the snapshot tree above is the script's own record.)

**Device: lavapipe** — `deviceName = llvmpipe (LLVM 15.0.6, 256 bits)`, `driverName = llvmpipe`,
Vulkan 1.3.230, kernel `6.18.33.1-microsoft-standard-WSL2`, rustc 1.98.1. **Software Vulkan,
not a hardware Linux GPU** (ADR 0044).

## 2D goldens (`crates/forge-2d/tests/test_2d_goldens.rs`)

| Scene | lavapipe (Linux, Vulkan) | RTX 3080 (Windows, Vulkan) | limit |
|---|---|---|---|
| tiles | 0.000 % of pixels differ | 0.000 % | 0.200 % |
| lights | 0.000 % | 0.000 % | 0.200 % |
| shapes | 0.000 % | 0.000 % | 0.200 % |

against the same committed PNGs (sha256 `tiles.png` `414ba193f8e7cde519b53380e2d36edb91a8f09147aaf82ce2ca08ba3ea1111f`,
`lights.png` `b70ee33f6d101340cb1c009913a12c4f7a265852b1557621e26012b8ab523c08`,
`shapes.png` `eee0fbad619b76cfd691eebc5326dc5166fcd3e3158224f75396d8793d9b3527`).
Both positive controls passed on lavapipe: `positive_control_a_moved_light_fails_the_golden`
and `positive_control_a_lost_normal_map_fails_the_golden`. `the_scenes_show_what_they_claim`
passed.

One Linux-only finding, fixed before this run (not by widening the threshold, W5): the first
lavapipe run differed on 4.8 % of the `lights` scene's pixels. The scene was drawn at
40 px a unit, 2.5 screen pixels per brick texel, so texel edges fell exactly on pixel centres
and the two rasterisers rounded nearest-neighbour lookups differently. The scene now draws
at 64 px a unit (4 pixels per texel, no edge on a centre); the golden was re-blessed on
Windows and then matches on both.

## 2D determinism (I2 corpus, `tests/determinism/test_cross_platform_hash.rs`)

| Leg | Output |
|---|---|
| ubuntu-x86_64 (Docker) | `I2 corpus on linux-x86_64: 319 rows, BLAKE3 f74989122eb86d0c8b8ef79e0d3299b43bdeeb0b2630992b54a13df039877241` |
| windows-x86_64 (dev box) | `I2 corpus on windows-x86_64: 319 rows, BLAKE3 f74989122eb86d0c8b8ef79e0d3299b43bdeeb0b2630992b54a13df039877241` |

Byte-identical, and each leg matched every committed row of `tests/determinism/golden.txt`
(sha256 `9e130cac4ffae87df520f1d791f8153cb8fabb48e87ab9fbdc0656d6014f3fae`, LF), including the
ten `2d/*` rows (`physics/pyramid`, `physics/mixed`, `particles/fountain`,
`skeleton/walk`, `tilemap/autotile`, `nav/grid`, `light/visibility`, `spline/hill`,
`atlas/pack`, `aseprite/roundtrip`). `positive_control_mutate_det_build_fails`
passed on Linux.

## The rest of the 2D guards on Linux

- `test_physics_2d` (7 tests with its three controls), `test_2d_render` (steady state, 32 lights,
  batching control), `test_2d_prepare_alloc` (steady last mile 0 heap allocations; control).
- `tools/forge-2d-platformer`: `the_scripted_run_clears_the_level_with_a_gamepad` and
  `the_run_replays_to_its_golden_bits` — the same trajectory fingerprint
  `0xb051_597b_3a74_23b3` as Windows — with both controls.
- `test_2d_tax_is_zero` (4, both controls), `test_frame_liveness` and
  `test_no_f32_below_render` (rule 4 and its 2D control), and `forge-panels-domain`'s
  `test_editors_2d` (8, the 2D editors on real forge-2d, with the own-rule control).

## The full `just verify` on Linux after the verifier's fixes (2026-09-24)

`just verify-linux` (the whole of `just verify`, not a subset) on snapshot tree
`0747f37f016619a327e8d0df74a00c67dcedf8a2` (lane B, commit `a17439a`), same image, lavapipe
(`llvmpipe (LLVM 15.0.6, 256 bits)`, Vulkan 1.3.230, software Vulkan), rustc 1.98.1,
cargo-nextest 0.9.145: fmt, both clippy runs, nextest **1710 tests run: 1710 passed, 1 skipped**,
doc tests, gate-parity, plan-coverage, fp-rules, layering, allocators, gate (295 rows: 248
BOUND, 4 AWAITING, 43 UNBUILT), dod and licence-audit — exit 0. Windows `just verify` on the
same tree: 1710 passed, 1 skipped, exit 0.

The first full Linux run (tree of commit `3365a6d`) found one 2D defect the subset runs had
not: `positive_control_an_injected_2d_cpu_regression_fails_the_gate` did not bite on lavapipe.
Sort-and-sweep turned off makes the 2,000-body `phys2d.step` only ~1.5-1.8x (ratio 0.163-0.175
in the container against the row's 0.170 allowance; 0.19-0.23 on the dev box). The control now
runs the solver's velocity iterations nine times over (`Faults2d::solver_repeats` = 8): ratio
0.32 in the container, 0.35-0.36 on the dev box. No budget, band or slack changed (W5).
That run also failed `forge-panels-scene::test_ui_hierarchy_100k`
`positive_control_hierarchy_rebuilding_under_a_filter_fails` once (the unfiltered rename step
measured 5.0 ms p95 before the filtered one ran, so the control saw the wrong failure); it
passed in the run above. It is not a 2D test; recorded as observed.
