# ADR 0044 — The Linux leg observed locally: a pinned Docker verify environment (`just verify-linux`)

- **Status:** accepted
- **Date:** 2026-09-23
- **Work package:** WP-30

(Numbered 0044: `dev` holds up to 0043; D-9.)

## Context

Every Linux leg has been AWAITING(no Linux runner) since M0: the dev machine is Windows, CI
runs only when the owner pushes, and Docker Desktop's daemon was off. On 2026-09-23 the owner
started Docker Desktop (daemon 29.5.3, linux/amd64, WSL2 backend, 12 CPUs, 16 GB). The owner
is on a shared, metered Starlink link, so the environment must not re-download what the
machine already holds, and it shares 12 CPUs with two Windows lanes.

## Decisions

1. **One pinned image, built once.** `tools/docker/linux-verify/Dockerfile` is
   `FROM rust:1.98.1-bookworm@sha256:93ce27a8…3e` (the image already in the local store; the
   repo toolchain is 1.98.1) plus exactly what the build and tests need: lavapipe
   (`mesa-vulkan-drivers`, `libvulkan1`, `vulkan-tools` to record the device), Xvfb (so the
   X11 clipboard test and winit have a display rather than reporting AWAITING), the libraries
   winit dlopens, rsync, the rustfmt/clippy components, and the same tool versions as the
   Windows box (just 1.58.0, cargo-nextest 0.9.145, cargo-deny 0.20.2, cargo-about 0.9.2).
   No `# syntax=` line (it would pull a frontend image). The tools compile **offline** from
   the host's crate cache, passed as the named build contexts `hostcache`/`hostindex`.
2. **The host's crate cache is the container's.** `registry/cache` and `registry/index` of
   the host's `CARGO_HOME` are bind-mounted into the container's `CARGO_HOME`; extracted
   sources and the package lock stay in the volume. The run first tries
   `cargo fetch --locked --offline --target x86_64-unknown-linux-gnu`; only a crate the host
   never had is fetched (into the host cache, once), and the listing of the cache before and
   after is printed — cargo only ever adds a `.crate` it does not have, so an unchanged count
   proves nothing was re-downloaded. Everything after that runs with `CARGO_NET_OFFLINE=true`.
3. **One persistent named volume, `forge-linux-target`**, holds `CARGO_TARGET_DIR`
   (`/vol/target`), the source snapshot (`/vol/src`) and `CARGO_HOME` (`/vol/cargo-home`).
   Rebuilds are incremental. No Windows target dir is ever copied in.
4. **The container builds what CI would check out.** The worktree is snapshotted as the tree
   `git add -A` would commit (built in a throwaway index; the real index is untouched) and
   archived with **LF** endings — the Windows working tree is CRLF, the ubuntu checkout is
   LF, and a CRLF tree on Linux would be neither. `rsync --checksum` copies it into the volume
   so unchanged files keep their mtimes (incremental builds survive a re-snapshot).
5. **`CI=true` inside the container**, as on GitHub's runners: a GPU guard with no adapter
   fails instead of reporting AWAITING (W9), so a green run means the guards rendered.
6. **Load cap:** `--cpus 6 --memory 12g` and `CARGO_BUILD_JOBS=6` (overridable:
   `FORGE_LINUX_CPUS`, `FORGE_LINUX_MEMORY`), so the Windows lanes keep half the machine.
   The timed-alone lock (ADR 0042) is per kernel: it does not reach across the WSL2 VM
   boundary, so a Windows wall-clock gate must not run while the container builds or tests.
7. **Entry points:** `just verify-linux [command…]` — `[windows]` runs
   `tools/docker/linux-verify/verify-linux.ps1` (Windows PowerShell 5.1 safe: no
   native-to-native pipes, the snapshot goes through a temp file), `[unix]` runs
   `verify-linux.sh` (also Git Bash). With no command it runs `just verify`, the exact CI list.
8. **What lavapipe is.** GPU rows observed in the container are **software Vulkan**
   (lavapipe, reported by Vulkan as `llvmpipe (LLVM 15.0.6, 256 bits)`), never a hardware
   Linux GPU; the evidence files and row reasons say so. Wall-clock budgets are gated on the
   reference machine only: in the container they run for correctness and are neither gated
   nor tuned for it (no lavapipe perf baseline is recorded from a VM sharing the dev box).
9. **Evidence is committed**, one file per row under `docs/evidence/linux/`: image digest,
   exact command, date, output excerpt and hashes.
10. **`tools/docker` is excluded from the workspace** (`exclude` in the root `Cargo.toml`):
    the `tools/*` member glob otherwise requires a `Cargo.toml` there.

## Bootstrap on a fresh Windows host

A Windows host's cache lacks the tools' Linux-only crates (their `Cargo.lock` pulls
`addr2line`, `rustix`, `openssl-sys`, … on Linux). Once, before the first image build, fetch
exactly those into the host cache with the base image (no compile):
`docker run --rm -v %USERPROFILE%\.cargo\registry\cache:/usr/local/cargo/registry/cache -v
%USERPROFILE%\.cargo\registry\index:/usr/local/cargo/registry/index rust:1.98.1-bookworm@sha256:…`
running, per tool crate, `tar -xzf <crate>.crate` and `cargo fetch --locked --target
x86_64-unknown-linux-gnu` in it. On 2026-09-23 that added 91 crates (9.2 MB) — `cargo fetch`
also takes the tools' dev-dependencies, which `cargo install` does not need; a known, small
overshoot. The workspace itself needed **zero** crates beyond the host cache.

## Measured (2026-09-23)

- **Full `just verify` on Linux: green** (exit 0): 1494 tests passed, 1 skipped (the same
  `#[ignore]` as Windows), nextest 1287.6 s with 6 CPUs; every xtask green
  (`docs/evidence/linux/verify-linux-run.md`).
- **I2 corpus digest**, BLAKE3 of the 309-row corpus as computed: windows-x86_64 and
  ubuntu-x86_64 both `c8d606700baf1e56b3f5963889d74d28c1baec0e5ab12f3ad16d71d58eedd703`.
- **Crates:** host cache 1240 → 1240 `.crate` files across the verify runs (none added, none
  re-downloaded). The one-off tool bootstrap added 91 missing Linux-only crates (9.2 MB).
- **Image** `forge-linux-verify:1.98.1` 2.65 GB on disk (2.19 GB shared with the base, 466 MB
  unique). **Volume** `forge-linux-target` 28.86 GB after a full verify (target + snapshot +
  CARGO_HOME), under the 60 GB prune threshold.
- **Times:** image build ~7 min once (the tools compile offline); a cold full verify ~75 min,
  an incremental one ~45 min (the positive-control cargo builds dominate).
- **Streaming walk** (600 frames, 100 m/s): RTX 3080 `DemoWorld::frame` 3.77 ms / whole frame
  4.88 ms (gated, green); lavapipe 10.92 ms alone, 43.79 ms inside a full verify; WARP
  38.17 ms — all with 0 waits, 0 holes, 0 bad covers, identical upload counts.

## What the Linux leg found, and how each was fixed (never by skipping)

1. **I21 on Linux**: forge-runtime links `x11rb` (winit, arboard), `zbus` (accesskit_unix)
   and `async-io` (zbus's reactor) only on Linux; their sources name socket types. They are
   local IPC clients to endpoints the user's desktop session names. `net_allow.txt` entries
   may now be scoped to a host OS (`<crate> [linux] — reason`) and are excused, and checked
   for staleness, only there (the graph the guard walks is the host platform's). Dropping
   them would drop the Linux clipboard and screen-reader support — worse for the user.
   Parser test with controls: an unknown OS, a malformed scope and a scoped licensing crate
   are refused.

<!-- -->

4. **Glyph-atlas tests** draw a colour emoji and 600 CJK ideographs from system fonts (the
   bundled D-7 set has neither); a font-less container fails them loudly. The image installs
   `fonts-noto-color-emoji` and `fonts-wqy-microhei`, and so does a new CI setup step for the
   ubuntu leg (with lavapipe, Xvfb and `DISPLAY`) — outside the `just verify` markers, so
   gate-parity is unchanged — so CI's ubuntu leg sees what the Docker leg sees.
5. **Not an engine bug, an environment one:** mid-run the WSL2 VM's page cache served a
   corrupted `librustc_driver.so` (every rustc codegen SIGSEGV'd, even hello world in the
   pristine base image; `drop_caches` restored the on-disk bytes). The image records the
   toolchain's sha256 at build time and `in-container.sh` refuses to run on different bytes
   (control: one changed byte stops the run with exit 3 and the diagnosis). Memory or VM
   instability on the dev box is worth the owner's attention.

## Consequences

- Every AWAITING(no Linux runner) row is re-examined against an observation (see the gate
  rows and `docs/plan/dod-status.ron`); rows that need a remote CI run (M0-1, M0-16) stay
  Blocked on the owner's push, now with the Linux leg observed locally.
- The Docker leg is a *local* observation of the Linux build; it does not replace CI's
  ubuntu leg, which runs on GitHub's hardware once the owner pushes.

## Amendment 1 (WP-34 verifier, 2026-09-23): one volume per worktree, one Linux verify at a time

**Problem.** Decision 3's single volume held `/vol/src`, the source snapshot, as well as the
target dir. Two lanes running `just verify-linux` at once rsync'd their own trees over each
other's snapshot mid-build and mid-test: lane A's full run on 2026-09-23 failed
`test_no_privileged_plugin` ("allow-list entry forge_editor::services::PanelFaults matches
nothing" — a file from the other lane's tree) and both UI goldens tests (32 % of pixels
differed), and every one of them passed when rerun alone on the same tree. The two
containers (6 CPUs each) also loaded the VM's 12 CPUs under each other's timed tests
(`test_ui_hierarchy_100k` missed its 2 ms p95 at 2.05 ms and 7.42 ms, green alone).

**Decided.**

1. **A volume per worktree**, `forge-linux-target-<folder name>-<8 hex of SHA-256 of the
   lowercased worktree path>` (both host scripts compute the same name; `FORGE_LINUX_VOLUME`
   overrides). A new worktree's volume is **seeded once** from the old shared
   `forge-linux-target` by a local `cp -a` (no download) so its first build is incremental;
   the old volume is only a seed now and can be removed (`docker volume rm
   forge-linux-target`) once every lane runs this script.
2. **One Linux verify at a time on the machine.** `in-container.sh` takes an exclusive `flock`
   on `/lock/verify.lock` in the shared `forge-linux-lock` volume before it touches its
   volume, held (inherited fd) until the container exits; a second run prints that it waits
   and waits. All containers share the WSL2 VM's kernel, so the lock reaches across lanes
   (unlike ADR 0042's lock, which cannot reach across the VM boundary). The seed copy takes
   the same lock. Better for the user: a green or red Linux run now means the tree it names.
3. Budgets are unchanged (W5). The UI wall-clock gates (`ui_*_100k`, D-5) still gate in the
   container, as they did in WP-30's green run; decision 8's reference-machine rule for them
   is the perf-gate owner's call (lane B, WP-U12), not this harness's.

Cost: one more volume per worktree (~30-48 GB each; the host has the room) and a wait when
two lanes want the Linux leg at once.
