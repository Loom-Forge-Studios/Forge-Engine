# ADR 0054 — GPU mode: Single by default, Multi opt-in (experimental)

- **Status:** accepted (owner decision 2026-09-25)
- **Date:** 2026-09-25
- **Plan references:** Ch.9 §9.2 (the adapter pool), Ch.26 (Tier 0/1/2), Ch.21 §21.18
  (settings), Ch.27/§38.7 (`forge.json`), DoD M1-2, spike S1, ADR 0019, ADR 0020; owner rules 1
  and 2. WP-39.

## Context

`PoolOptions::default()` had `max_devices: None`, so `AdapterPool::new` opened a device on
**every** qualifying adapter: a machine with two GPUs got a multi-GPU pool by default, before
multi-GPU transfer has been measured on two physical adapters (S1 is AWAITING; ADR 0020). The
owner has no second GPU to test with for a few months and decided (2026-09-25) that the engine
gets a GPU-mode setting, **Single** or **Multi**, **default Single**.

## Decision

1. **`forge_gpu::GpuMode { Single, Multi }`, `PoolOptions::mode`, default `Single`.** Single
   opens exactly one device: the best adapter by `plan_selection`'s order — the primary a Multi
   pool would have (discrete before integrated before software; one physical GPU counted
   once; Vulkan preferred). If that adapter's device fails to open, the next in line is tried,
   so Single never fails where Multi would have succeeded. Every other qualifying adapter is
   reported `LeftOutByGpuMode` — "GPU mode: Single" — in `AdapterPool::report()`. Multi keeps
   the every-adapter pool; `max_devices` still caps either.
2. **Where the setting lives.** An editor preference (Settings → Editor → Graphics → GPU
   mode), user config persisted with the other editor settings; and a project setting
   (Settings → Graphics, `graphics.gpu_mode`, a `SetSetting` command like every project
   setting, I7) that the packager writes into the exported game's `forge.json` as
   `"gpu_mode"`. The runtime (`forge-runtime --menu`, the 2D sample game) reads it at startup;
   `--gpu-mode single|multi` overrides it.
3. **Applies at next start.** The pool is built once, at startup. Changing either row says
   "Applies at next start" (a row note and a notification); nothing restarts silently.
4. **Multi is labelled experimental**, with the reason in the UI: moving work between GPUs is
   unmeasured until a second physical GPU is tested (M1-2, ADR 0020).
5. Tests that exist to exercise several adapters (`test_multi_adapter`, the `Include` pool
   tests, the `adapters --multi` example) ask for `GpuMode::Multi` explicitly.

## Why — the owner's two rules

1. **Better for the user:** an untested multi-GPU path is not switched on behind their back;
   one GPU is the configuration every user has and every test covers. A user who wants every
   GPU can say so, and the settings window tells them what that means and when it applies.
2. **Faster / more efficient:** Single creates one device instead of one per adapter (each
   device costs driver memory and startup time, and a software rasteriser's device is not
   free either); on the dev machine nothing changes, since the pool already had one hardware
   device and the primary is the same adapter in both modes.

## Alternatives rejected

- *`max_devices: Some(1)` as the default:* it caps silently — the report would say "Capped",
  not why, and there would be no setting for the user.
- *Pick the Single adapter by the window's surface:* the pool is built before any window
  exists (the editor shares it with the viewport renderer and Tier-0 compute). The primary
  presents on every machine we know of; `device_for_surface` still searches the pool, so a
  pool whose one device cannot present fails with `GPU-0008` rather than rendering nowhere.
- *Restart the editor when the mode changes:* a silent restart loses unsaved state; the pool
  is cheap to rebuild at the next start.

## Consequences

- DoD M1-2 stays **Blocked**; its reason now names this decision and the Single default. Gate
  row S1 stays **AWAITING(second physical adapter)**. When a second physical GPU is available,
  `cargo test --release -p forge-gpu --test test_multi_adapter -- --nocapture` measures it
  (the test builds a Multi pool); if the verdict of ADR 0020 stands, Multi can lose its
  experimental label.
- New gate row `C-gpu-mode-single` (`crates/forge-gpu/tests/test_gpu_mode.rs`, control
  `positive_control_multi_forced_fails_the_single_guard`).
