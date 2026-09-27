# C-determinism-linux-leg (I2 on ubuntu-x86_64) — and DoD M0-3

Image `forge-linux-verify:1.98.1` (`sha256:c960b17d10838574eb866789250ce2e81c409649b57c216d36d900bbb058e222`) from `rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e`; environment in [README](README.md); 2026-09-23. Part of the green full run in [verify-linux-run](verify-linux-run.md); the excerpts below are from `just verify-linux bash -c "…"` on the same snapshot (tree `01dcffd7`).

**Command:** `cargo test -p forge-tests --test test_cross_platform_hash --locked -- --nocapture`
(in the container), and the same on the Windows host.

| Leg | Output |
|---|---|
| ubuntu-x86_64 (Docker) | `I2 corpus on linux-x86_64: 309 rows, BLAKE3 c8d606700baf1e56b3f5963889d74d28c1baec0e5ab12f3ad16d71d58eedd703` — `test result: ok. 4 passed` |
| windows-x86_64 (dev box) | `I2 corpus on windows-x86_64: 309 rows, BLAKE3 c8d606700baf1e56b3f5963889d74d28c1baec0e5ab12f3ad16d71d58eedd703` — `test result: ok` |

**The two corpora are byte-identical**: the digest is BLAKE3 of the corpus *as computed* on
each platform (rendered in golden-file form), independent of the working tree's line
endings, and each leg also matched every one of the 309 rows of the committed golden
(`tests/determinism/golden.txt`, sha256 `dc31accd9d0452ef15908dfc56190db307d647876ec7eea97c364ad3ab004d2d`
in both checkouts, LF).

**Later observation (WP-U15, 2026-09-24):** with the ten `2d/*` rows the corpus is 319 rows,
BLAKE3 `f74989122eb86d0c8b8ef79e0d3299b43bdeeb0b2630992b54a13df039877241` on both legs —
[C-2d-linux-leg](C-2d-linux-leg.md).

**Positive control on Linux:** `positive_control_mutate_det_build_fails` PASSED in the full
run (38.2 s): the `--features mutate-det` build (octave 0 of `fbm_i` perturbed by 1 ulp) failed
the golden comparison on Linux as designed.
