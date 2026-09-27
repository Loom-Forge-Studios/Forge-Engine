# C-render-linux-leg — forge-render's GPU guards and goldens on ubuntu-x86_64 (lavapipe)

Image `forge-linux-verify:1.98.1` (`sha256:c960b17d10838574eb866789250ce2e81c409649b57c216d36d900bbb058e222`) from `rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e`; environment in [README](README.md); 2026-09-23. Part of the green full run in [verify-linux-run](verify-linux-run.md); the excerpts below are from `just verify-linux bash -c "…"` on the same snapshot (tree `01dcffd7`).

**Device: lavapipe** — `deviceName = llvmpipe (LLVM 15.0.6, 256 bits)`,
`deviceType = PHYSICAL_DEVICE_TYPE_CPU`, `driverInfo = Mesa 22.3.6 (LLVM 15.0.6)`, Vulkan 1.3.230.
**Software Vulkan, not a hardware Linux GPU.** The row's plan wording expects exactly this
("CI's ubuntu leg runs them (with a software rasteriser)").

Every forge-render test passed in the full run with `CI=true` (no adapter would have failed
them, W9). Goldens, `cargo test -p forge-render --test test_render_goldens -- --nocapture`:

| Scene | lavapipe (Linux, Vulkan) | RTX 3080 (Windows, Vulkan) | limit |
|---|---|---|---|
| materials | 0.000 % of pixels differ | 0.000 % | 0.200 % |
| lights | 0.000 % | 0.000 % | 0.200 % |

against the same committed PNGs (e.g. `materials.png` sha256
`0002a4d6bba5b7429835b1d99cffdaf6bb41ae8fce9ed92f219aad23e5e1f1e7` in both checkouts), and
`positive_control_a_changed_material_fails_the_golden` passed (the rougher sphere fails the
golden on lavapipe too). The atmosphere goldens (`test_atmosphere_goldens`, with their
controls) passed on lavapipe in the same run.
