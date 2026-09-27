# Linux leg evidence (WP-30, ADR 0044)

Observations of the ubuntu-x86_64 leg made **locally, in Docker**, with `just verify-linux`
(Ch.30 §30.6). One file per gate row / DoD id; each gives the image, the exact command, the
date, and the output excerpt or hashes it rests on.

## The environment every file refers to

| | |
|---|---|
| Base image | `rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e` (local store, never re-pulled) |
| Built image | `forge-linux-verify:1.98.1` from `tools/docker/linux-verify/Dockerfile` (since WP-U12 with an `at-spi2-core` + `dbus` layer for [M2-22](M2-22.md)) |
| Kernel | `Linux 6.18.33.1-microsoft-standard-WSL2 x86_64` (Docker Desktop 29.5.3, WSL2 backend) |
| CPU / limits | AMD Ryzen 5 5600X (12 threads); container capped at 6 CPUs, 12 GB |
| Toolchain | rustc 1.98.1 (48a229cea 2026-09-01), cargo-nextest 0.9.145, just 1.58.0 |
| Vulkan | **lavapipe** — `deviceName = llvmpipe (LLVM 15.0.6, 256 bits)`, Vulkan 1.3.230, Mesa (Debian bookworm). Software Vulkan, **not a hardware Linux GPU** |
| Display | Xvfb (`xvfb-run -a`, 1920x1080x24): X11, no Wayland compositor |
| Environment | `CI=true` (GPU guards fail rather than AWAIT without an adapter), `CARGO_NET_OFFLINE=true` after the crate check |
| Source | the worktree snapshot as `git add -A` would commit it, LF line endings |

Wall-clock budgets are run for correctness in the container and are **not** gated or tuned
for it: they are gated on the reference machine (Ch.29, ADR 0044 decision 8).

## Files

| File | Rows it evidences |
|---|---|
| [verify-linux-run.md](verify-linux-run.md) | the full `just verify` on Linux; DoD M0-1 and M0-16 (local Linux leg); the Linux-only findings; crate and size accounting |
| [C-determinism-linux-leg.md](C-determinism-linux-leg.md) | gate row C-determinism-linux-leg; DoD M0-3 (Windows and Linux corpus digests equal) |
| [C-replay-linux-leg.md](C-replay-linux-leg.md) | gate row C-replay-linux-leg |
| [C-2d-linux-leg.md](C-2d-linux-leg.md) | gate row C-2d-goldens-linux-leg; DoD M4-11 (2D goldens and the 319-row I2 corpus on lavapipe, WP-U15) |
| [C-render-linux-leg.md](C-render-linux-leg.md) | gate row C-render-linux-leg; DoD M1-12 (lavapipe half) |
| [C-perf-gate-linux-leg.md](C-perf-gate-linux-leg.md) | gate row C-perf-gate-linux-leg (observed; baseline still awaited) |
| [C-wasm-plugins-linux-leg.md](C-wasm-plugins-linux-leg.md) | gate rows C-wasm-plugins-in-editor, C-wasm-plugins-no-idle-wakeups, C-project-trust, C-install-live-atomic, C-collab-sync-core-only (WP-34), Linux leg |
| [C-premium-boundary-linux-leg.md](C-premium-boundary-linux-leg.md) | gate rows C-premium-boundary, C-export-leak-scan (WP-41), Linux leg |
| [M2-19.md](M2-19.md) | DoD M2-19 (`ui_gallery` on Linux) |
| [M2-22.md](M2-22.md) | DoD M2-22 (the AT-SPI tree of `ui_gallery` walked over the accessibility bus by a screen-reader-side client; WP-U12) |
| [verify-linux-args.md](verify-linux-args.md) | `just verify-linux` argument passing (backlog L-16): a spaced/quoted argument reaches the container intact |
