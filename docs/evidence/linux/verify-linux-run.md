# `just verify` on the Linux leg, in Docker — green

Image: `forge-linux-verify:1.98.1` (`sha256:c960b17d10838574eb866789250ce2e81c409649b57c216d36d900bbb058e222`), built from `rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e`. Environment: see [README](README.md). Date: 2026-09-23.

**Command** (Windows host, lane-b worktree at commit `4d137bf`, snapshot tree
`01dcffd794d866876e2a1321b1bb152eb7808d2e`): `just verify-linux`, which runs `just verify` —
CI's exact list (I12) — inside the container under `xvfb-run`, with `CI=true`, 6 CPUs, 12 GB.

**Result: exit 0.** Every step of the list:

| Step | Linux result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean |
| `cargo clippy -p forge-trace --all-targets --features tracy --locked -- -D warnings` | clean |
| `cargo nextest run --workspace --locked --no-fail-fast` | `1494 tests run: 1494 passed (12 slow), 1 skipped` (1287.6 s) |
| `cargo test --workspace --doc --locked` | ok |
| `cargo xtask gate-parity` | `just verify` == ci.yml (12 commands, same order) on windows-latest, ubuntu-latest |
| `cargo xtask plan-coverage` | 191 directories, all claimed |
| `cargo xtask fp-rules` | no target-cpu, target-feature, llvm-args or fast-math flags |
| `cargo xtask allocators` | 1 defects, 207 error codes — no duplicates |
| `cargo xtask gate` / `cargo xtask dod` | green (no GPU row fell back to AWAITING: `CI=true` would have failed it) |
| `cargo xtask licence-audit` | allow-lists permissive, cargo-deny green, NOTICES fresh |

The one skipped test is the same `#[ignore]` test Windows skips.

## What the Linux leg found (fixed in the code, not skipped)

The first full run (same image, before the fixes) had 6 failures, all Linux-only:

1. **I21** `forge_runtime_links_no_licensing_or_network_crate` and its scratch control:
   on Linux forge-runtime links `x11rb` (winit, arboard), `zbus` (accesskit_unix, AT-SPI)
   and `async-io` (zbus's reactor), which name socket types. Local IPC clients to endpoints
   the desktop session names; the allow-list gained OS-scoped entries
   (`tests/licence/net_allow.txt`, `<crate> [linux] — reason`), excused and stale-checked
   only on that OS, with a parser test and its controls.

<!-- -->

4. **ui_single_glyph_atlas** and **ui_single_glyph_atlas_epochs_draw_every_glyph**: the
   container had no colour-emoji or CJK system font, which the tests draw (the bundled D-7
   set has neither). The image — and CI's ubuntu setup step — now install
   `fonts-noto-color-emoji` and `fonts-wqy-microhei`.

## Also observed on Linux in this run

- `test_ui_os_clipboard` (3 tests) against the **X11 clipboard** of Xvfb: a copy reaches
  another X client and back (no AWAITING), and the in-process control fails as designed.
- `test_ui_a11y_tree` (6 tests) green: the AccessKit tree the Linux adapter serves.
- Every forge-render / forge-gpu / forge-ui GPU test on lavapipe (software Vulkan).

## Crates: nothing re-downloaded

The run prints the host crate cache before and after its `cargo fetch --offline`:
`linux-verify: host crate cache 1240 -> 1240 files; added:` (none) — every crate of the
Linux build came from `%USERPROFILE%\.cargo\registry`, mounted; the rest of the run
had `CARGO_NET_OFFLINE=true`. Before the first image build the cache held 1149 `.crate`
files; building the four tools offline needed their Linux-only crates, which a one-off
`cargo fetch --target x86_64-unknown-linux-gnu` in a throwaway base-image container added:
91 files, 9.2 MB (none of them was in the cache before — cargo adds only missing files).
Downloads in total: those 9.2 MB of crates; for the image, 9.4 MB of apt index and 55.6 MB of
packages, then a second index and the two font packages (roughly 25 MB, not logged), and the
rustfmt and clippy components. The base image was never pulled.

## Sizes

- Image `forge-linux-verify:1.98.1`: 2.65 GB on disk, of which 2.19 GB is shared with the
  base image (unique 466 MB).
- Volume `forge-linux-target` (target dir + snapshot + CARGO_HOME): 28.86 GB — under the
  60 GB prune threshold, kept for incremental rebuilds.

## Final run on the committed records (commit `f486233`, snapshot tree `b19a0bd5`)

`just verify-linux` again after the gate rows, DoD statuses and the new streaming unit test
landed: **exit 0**, `1495 tests run: 1495 passed (13 slow), 1 skipped` (1483.2 s),
`248 rows: 202 BOUND, 4 AWAITING, 42 UNBUILT`, licence-audit green, host crate cache
`1240 -> 1240`. The same commit's `just verify` on Windows: exit 0, 1495 passed, 1 skipped.
Afterwards the dangling build cache was pruned (821 MB); the volume stayed at 28.86 GB.
