# The input runtime on ubuntu-x86_64 (WP-65, DoD M7-12)

Observed 2026-09-28 in the `forge-linux-verify:1.98.1` container (`just verify-linux`, ADR 0044)
on the dev box: Linux 6.18.33.1-microsoft-standard-WSL2 x86_64, rustc 1.98.1, cargo-nextest
0.9.145. Vulkan in the container is llvmpipe (software), not a hardware Linux GPU; none of these
tests draws.

The image was rebuilt once for this work package (`FORGE_LINUX_REBUILD=1`): its new last layer
installs `libudev-dev` (gilrs reads pads through evdev with udev hot-plug, and `libudev-sys`
links libudev at build time). Every layer above it stayed cached; the rebuild downloaded that one
package. CI's ubuntu step installs the same package.

| Command (in the container, on this worktree) | Result |
|---|---|
| `cargo test -p forge-input --locked --all-features` (gilrs and winit adapters built on Linux) | 67 passed: unit 12, `test_input_budgets` 11 (timed alone), `test_local_multiplayer` 5, `test_rebinding_persists` 7, `test_touch_gyro_haptics` 9, `test_triggers_curves_deadzones` 23 |
| `cargo test -p forge-runtime --locked --test test_controls_screen` | 6 passed |
| `cargo test -p forge-panels-domain --locked --test test_input_backend --test test_input_map` | 8 passed |

Gate rows covered: `C-input-backend`, `C-input-deadzone-curve`, `C-input-triggers`,
`C-input-rebinding-persists`, `C-input-device-assignment`, `C-input-latency-budget`,
`C-input-alloc-free-frame`, `C-input-idle-cost`, `C-input-update-budget`,
`C-input-touch-gestures`, `C-input-onscreen-controls`, `C-input-gyro`, `C-input-haptics`,
`C-input-game-ui`, `C-input-debugger` — each with its positive control, on both legs.
