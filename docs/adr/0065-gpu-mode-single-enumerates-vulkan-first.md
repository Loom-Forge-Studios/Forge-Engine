# ADR 0065 — GPU mode Single enumerates Vulkan first and skips Direct3D 12 when it suffices

- **Status:** accepted (orchestrator decision by owner rules 1 and 2, 2026-09-28)
- **Date:** 2026-09-28
- **Plan references:** Ch.9 §9.2 (the adapter pool), Ch.21 §21.22 (the startup budget, gate
  `C-ui-startup`, DoD M2-30), ADR 0019 (Vulkan preferred), ADR 0054 (GPU mode). WP-48.

## Context

The editor's cold start to its first interactive frame has a 1.5 s budget on the reference
machine (RTX 3080, 12 logical CPUs; `tools/forge-editor-bin/tests/test_ui_startup_budget.rs`).
Before this change it measured 1240-1305 ms (medians of three launches): 5-10 % under the
budget when the machine is loaded. The adapter pool is the largest part: one wgpu instance
(Vulkan, Direct3D 12 and Metal) brought up and every backend enumerated. Measured in fresh
processes on the reference machine (release build):

| Step | Time |
|---|---|
| instance creation (Vulkan loader and driver, with or without the Direct3D 12 backend) | 0.96-1.51 s |
| Vulkan enumeration (1 adapter) | 3-10 ms |
| Direct3D 12 enumeration (2 adapters: the same RTX 3080, WARP) | 0.39-1.05 s |
| the floor probes | < 1 ms |

On this machine, and on any machine whose Vulkan driver lists a discrete GPU above the floor,
the Direct3D 12 enumeration only finds that GPU again (a `Duplicate`: Vulkan wins the same
part, ADR 0019) and WARP (left out by the default software policy when hardware exists). In
GPU mode Single (the default, ADR 0054) it cannot change which device opens. Bringing the two
backends up in parallel was tried in an earlier work package and deadlocked about once in
25-30 launches; it is not retried.

## Decision

1. **In GPU mode Single, with the default software policy (`FallbackOnly`, or `Exclude`),
   the pool enumerates Vulkan alone first.** If `vulkan_suffices` — a Vulkan **discrete**
   adapter above the floor, which is the adapter the full enumeration makes primary (discrete
   first, Vulkan preferred for the same part) — the other backends are not enumerated, and
   `AdapterPool::skipped_backends()` names them. The one instance still carries every wanted
   backend, so window surfaces are unchanged.
2. **Everything else enumerates every backend:** Vulkan yields no qualifying discrete adapter
   (none, integrated only, below the floor, or a software rasteriser — Direct3D 12 may hold a
   better adapter, and WARP is the fallback); the Vulkan device fails to open (the skipped
   backends are then enumerated and the selection runs again over all adapters); the software
   policy is `Include` or `Only` (both ask for WARP, a Direct3D 12 adapter); GPU mode Multi;
   `WGPU_BACKEND` naming Vulkan alone or excluding it (nothing to skip).
3. **The guard changes from "every adapter is reported" to "every enumerated adapter is
   reported, and the skip happens exactly when a qualifying Vulkan adapter exists".**
   `test_adapter_pool::skip_guard` compares a GPU-mode-Single pool with the full GPU-mode-Multi
   enumeration on the same machine: the Single report is the full report less the skipped
   backends' rows, holds no skipped backend's row, skips only when `vulkan_suffices` holds on
   the full report's Vulkan adapters, and does skip whenever it holds (so the saving cannot be
   lost silently). Positive controls: a skip on an integrated-only machine, a missed skip, a
   lost adapter and a skipped backend's row each fail the guard; and forcing the full
   enumeration in the pool fails it on the dev machine ("Vulkan yields a qualifying discrete
   adapter but Backends(METAL | DX12) was enumerated anyway"). `vulkan_suffices` is pure and
   tested on synthetic machines. The budget is not widened (W5).

## Consequences

- Editor cold start (the same test, medians of three launches on the reference machine):
  **1246-1305 ms before, 1009-1116 ms after** (six sets: 1046, 1009, 1088, 1116, 1018, 1102 ms;
  one further set measured 2099 ms while another lane's build loaded the machine). About
  0.2 s, a sixth of the startup, comes off every editor and game launch in GPU mode Single.
- The Single report on such a machine no longer lists the Direct3D 12 duplicate or WARP; the
  adapters example prints the skipped backends, and GPU mode Multi still lists everything.
- A machine whose Vulkan driver is missing or broken behaves exactly as before.
- Linux leg (the `just verify-linux` container, software Vulkan — lavapipe, not a hardware
  Linux GPU): `cargo nextest run -p forge-gpu -p xtask`, 183 tests pass. lavapipe is a
  software adapter, so `vulkan_suffices` is false there and the pool enumerates as before.
