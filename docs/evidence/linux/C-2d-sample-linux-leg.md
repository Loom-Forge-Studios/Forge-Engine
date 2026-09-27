# C-2d-sample-linux-leg — the 2D sample game on ubuntu-x86_64 (lavapipe) — DoD M4-12, M4-13

Image `forge-linux-verify:1.98.1` (built from `rust:1.98.1-bookworm`); environment in
[README](README.md); 2026-09-24. Lane B (WP-U16). Snapshot trees, as printed by `verify-linux`
before each run: `da6d144b0330a6c47e1cef34cf054d9a446f04ec` (runs 1-2),
`d69989992bb33ba470052b7d03166060a4c881be` (run 3: the export stripped).

**Commands** (each one container, `--rm`, foreground):

```
1. just verify-linux bash -c '"CI=true cargo test --locked -p forge-2d-game -- --nocapture"'
2. just verify-linux bash -c '"CI=true cargo test --locked -p forge-tests --test test_cross_platform_hash
     && CI=true cargo test --locked -p forge-2d-game --test test_sample_game -- --nocapture the_run_replays the_menus"'
```

**Bit-identical with windows-x86_64** (printed on Linux, equal to the Windows run and the pins):

| What | Linux | Pinned (Windows) |
|---|---|---|
| scripted run, 900 steps (`SCRIPT_FINGERPRINT`) | `0xb051597b3a7423b3` | `0xb051597b3a7423b3` |
| through the menus to the level-clear banner (`SCRIPT_WIN`) | step 695, `0xc366f9b68e66f6ac` | step 695, `0xc366f9b68e66f6ac` |
| the **exported release binary**'s `--play-script` | `won=true steps=695 coins=5 fingerprint=0xc366f9b68e66f6ac on linux-x86_64` | the same line, `on windows-x86_64` |
| I2 corpus row `2d/sample-game/run` | `c8fa2160…418018` (golden matched) | committed golden |

**Device: lavapipe** — `deviceName = llvmpipe (LLVM 15.0.6, 256 bits)`, `driverName = llvmpipe`,
Vulkan 1.3.230. **Software Vulkan, not a hardware Linux GPU** (ADR 0044).
