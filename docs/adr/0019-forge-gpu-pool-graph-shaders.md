# ADR 0019 — Build forge-gpu as a probed adapter pool, a versioned-handle render graph and a polled WGSL library

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.9 (expanded to FULL here), Ch.21 §21.2/§21.13, Ch.26, O-8, E-33, D-3, D-5, I17, I18, M1-1

## Context

Ch.9 fixed the contract (wgpu only, an adapter pool, WGSL via naga, a declarative graph
with automatic barriers and aliasing) and left the mechanics open: how the O-8 floor is
checked when wgpu does not expose the numbers it is stated in, which backend wins when one
GPU appears on several, what "automatic barriers" means on an API that inserts barriers
itself, how order is derived, and how shader files are watched. D-3 requires forge-ui's
renderer to move onto the pool without regressing the D-5 budgets.

## Decision

1. **Floor by probe.** `probe.rs` reads the Vulkan `apiVersion` through
   `Adapter::as_hal::<Vulkan>()` and asks D3D12 whether FL 12_0 is supported with
   `D3D12CreateDevice(adapter, 12_0, NULL)` (creates nothing). This is forge-gpu's only
   `unsafe` (Ch.1.5 exception), with `// SAFETY:` on each block and
   `clippy::undocumented_unsafe_blocks` denied. The floor itself is a pure function of
   `AdapterFacts`; failing it yields `GPU-0002` with a message written for the user.
2. **One physical GPU, one pool member.** Adapters with the same PCI vendor/device id and
   type are one part; the backend with the most qualifying adapters in the group wins (two
   identical cards stay two); ties go **Vulkan, then D3D12, then Metal**. Software
   rasterisers join only as a fallback by default (`SoftwarePolicy`). Every enumerated
   adapter is reported with its verdict. OpenGL is not enumerated by default.
3. **Render graph semantics.** Versioned handles (`write`/`modify` return the next version;
   only the latest may be written); order = topological sort of RAW/WAR/WAW edges (WAR and
   WAW link to the next *live* writer, skipping culled ones) with
   declaration index as the tie-break; culling from roots (side effects, import writers);
   aliasing = greedy interval reuse of physical resources with identical descriptions
   (textures) or best fit (buffers); **barriers**: wgpu inserts the API barriers, the graph
   guarantees every resource's usage is exactly the union of its declared accesses and emits
   the transition plan (including aliasing hand-overs) that drives load ops and is checked
   by an independent `verify_plan`. A pass body can reach only the resources it declared.
4. **Shaders**: every module is created from naga-validated IR (`ShaderSource::Naga`) inside
   an error scope; hot reload **polls** `ProjectStore::stamps_of` for the held paths; a
   failed reload keeps the last good module and reports once.
5. **D-3**: forge-ui depends on forge-gpu and names wgpu only as `forge_gpu::wgpu` inside
   `render_wgpu`; the UI renders on the pool's primary (or the first member that can present
   to the window); `RunOptions::gpu` lets the editor share its pool.

## Why — the owner's two rules

1. **Better for the user:** a machine below the floor gets a sentence naming its GPU and the
   fix to try, not a driver crash or a black window; a shader typo shows file:line:col and
   the viewport keeps rendering the last good version; wgpu errors are reports, not process
   aborts; an editor with two GPUs uses both without duplicating the one card DX12 and
   Vulkan both list.
2. **Faster / more efficient:** the probe runs once per adapter at start-up and creates
   nothing; WGSL is parsed once (IR handed to wgpu); a steady-state graph frame allocates
   nothing (transient pool) and aliasing cuts transient memory (2,304 of 3,328 bytes in the
   test graph; a chain ping-pongs between two allocations); a quiet shader poll is one
   `stamps_of` call with no thread and no dependency; the UI keeps 1 draw call for 1,000
   buttons on the pool device.

## Alternatives rejected

- *Floor from wgpu limits/downlevel flags alone:* no limit distinguishes FL 11_0 from 12_0
  or Vulkan 1.1 from 1.2 reliably, so the floor would be a guess — exactly the machines O-8
  exists to refuse would slip through.
- *Prefer D3D12 on Windows:* two backends' behaviour across the two CI legs (I18), and D3D12
  has no external-memory path for S1.
- *Order passes by declaration only:* simpler, but a pass reading an older version would
  silently read overwritten data; with versioned handles the hazard orders it or is a cycle
  error.
- *An OS file-watcher crate (`notify`):* a thread per watcher, platform quirks, a licence
  to vet (CC0), and it would bypass the `ProjectStore` abstraction (I17) that makes hot
  reload work on non-local backends.
- *Keeping forge-ui on its own device:* two devices on one GPU double driver memory and make
  viewport textures from `forge-render` cross-device copies.

## Consequences

- `forge-gpu` is the only crate that creates shader modules in production code
  (`test_wgsl_only`); `forge-render` (Ch.10) builds on `RenderGraph` and `ShaderLibrary`.
- Gate rows `C-render-graph`, `C-shader-hot-reload`, `C-adapter-floor`, `C-wgsl-only`,
  `C-multi-adapter-frame`, `C-ui-on-forge-gpu` are BOUND; `S1` is AWAITING (ADR 0020).
- The 4 GB VRAM part of O-8 is documented but not gated (no portable query).
- Async-compute queues, bindless and a compiled-plan cache are not built (Ch.9 §9.4).
