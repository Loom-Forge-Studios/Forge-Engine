# ADR 0020 — Spike S1: host-staged transfer is the cross-adapter path; Tier 1 is editor/offline only

- **Status:** accepted (provisional on one input: the two-physical-GPU measurement is AWAITING)
- **Date:** 2026-09-21
- **Plan references:** Ch.9 §9.3/§9.6, Ch.26 (Tier 0/1/2), Appendix B spike S1, M1-2

## Context

Spike S1 asks whether wgpu can share resources across adapters efficiently: host-staged
transfers versus Vulkan external memory through a `wgpu-hal` passthrough. Its written
fallback: "host-staged transfers only → Tier 0 + Tier 2 ship, Tier 1 limited to
editor/offline". The dev machine has one physical GPU (RTX 3080) plus Microsoft's WARP
software rasteriser.

## What was built and measured

- `forge_gpu::transfer::copy_buffer_across` / `copy_texture_across`: host-staged, one host
  copy (the mapped source range is written straight into the destination queue).
- `test_multi_adapter` (DoD seed): a compute pass on the second pool member (WARP here), the
  result moved to the primary host-staged, rendered there — **bit-identical** to computing
  and rendering on the primary alone.
- `s1_host_staged_bandwidth`, release build, 64 MiB, RTX 3080 on PCIe 4.0 ×16:

| Path | Throughput |
|---|---|
| host → RTX 3080 (`write_buffer` + wait) | 8.2 GB/s |
| RTX 3080 → host (staging copy + map + memcpy, warm) | 5.4 GB/s |
| RTX 3080 → WARP, host-staged | 0.01 GB/s (WARP's own software write path; not a physical-GPU number) |

Between two physical GPUs a host-staged copy is bounded by readback then upload: ≈3.3 GB/s
serial, ≈5 GB/s if the two halves are pipelined. A 4K colour+depth frame (≈66 MB) therefore
costs ≈12–20 ms — more than a 60 Hz frame budget if serial, most of it if pipelined.

## Decision

1. **Host-staged transfer is the committed cross-adapter path.** It is portable (every
   backend, every vendor pair, including a mismatched second card) and adequate for
   **Tier 0** (heterogeneous compute: bakes, generation, simulation — results are small
   relative to the work) and **Tier 2** (the LAN farm, which moves results over a network
   that is slower still).
2. **Tier-1 fallback, stated:** split-frame rendering across GPUs is limited to **editor
   viewports, offline/cinematic renders and multi-display/ICVFX**, where one frame of latency
   is free. It is not offered on the player-facing hot path, and the docs say so.
3. **The Vulkan external-memory passthrough is not built now.** It is worth its `unsafe`,
   its Vulkan-only scope and its feature flag only if the two-GPU measurement shows host
   staging cannot feed a 60 Hz editor viewport. The decision point is that measurement.

## Why — the owner's two rules

1. **Better for the user:** a second GPU of any make speeds up bakes and generation today,
   and nobody is promised realtime multi-GPU frames the hardware arithmetic cannot deliver.
2. **Faster / more efficient:** the portable path costs one host copy and no extra
   dependency; the external-memory path would add a Vulkan-only code path and test matrix
   before there is evidence it pays for itself.

## Alternatives rejected

- *Build the external-memory passthrough now:* it cannot be validated without two physical
  adapters, it is Vulkan-only (a D3D12-only machine gets nothing), and it would ship
  untested `unsafe`.
- *Declare S1 resolved on the WARP measurement:* WARP is software; its transfer rate says
  nothing about PCIe peer paths. That would be a fake green (W9).

## Consequences

- Gate row `S1` stays **AWAITING(reason: second physical adapter)**; DoD M1-2 is Blocked on
  the same reason with this verdict written. On a machine with two physical GPUs,
  `cargo test --release -p forge-gpu --test test_multi_adapter -- --nocapture` prints the
  two-GPU figures; if host staging sustains a 4K editor viewport at 60 Hz pipelined, the
  verdict stands and S1 is resolved; if not, the passthrough is built behind a feature.

## Amendment 1 (2026-09-25) — GPU mode Single by default

Owner decision: no second GPU for testing for a few months, so the pool opens **one** device
by default (`GpuMode::Single`) and multi-GPU is an opt-in, labelled experimental, setting
(ADR 0054). This verdict is unchanged: host staging is still the committed cross-adapter
path for a Multi pool, and S1 is still AWAITING(second physical adapter). `test_multi_adapter`
asks for `GpuMode::Multi` explicitly.
