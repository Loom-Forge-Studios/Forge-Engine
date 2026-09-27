# ADR 0056 — forge-render mesh pools: a streamed terrain's patches in one arena, drawn batched

- **Status:** accepted
- **Date:** 2026-09-26

## Context

WP-U18's GPU host uploaded every ground patch as a `forge-render` mesh of its own: two GPU
buffers per patch, and — since `forge-render` groups draws by mesh — one draw call per patch
per pass. A view from eye height draws ~2,300 patches: ~2,300 draw calls and as many
vertex/index buffer bindings a frame (measured on the RTX 3080: the host's render took 1.5
ms of CPU a frame at more than 1,500 patches, p99 frame 11.3 ms, worst 17.1 ms).

Options: (a) merge patches into larger meshes on the host (re-uploads whole batches whenever
the selection changes, i.e. every descent frame; re-narrows vertices to a shared origin);
(b) one shared vertex/index buffer with `base_vertex` per draw (fewer bindings, still one draw
per patch); (c) multi-draw-indirect (needs `INDIRECT_FIRST_INSTANCE`, not on every backend);
(d) **vertex pulling**: the patches' vertices in one storage buffer, each instance carrying its
mesh's first vertex, so all patches drawn with one index list are one instanced draw.

## Decision

**(d), as a general `forge-render` feature: mesh pools.**

1. `Renderer::add_mesh_pool(vertex_count, reserve)` makes a pool of meshes of `vertex_count`
   vertices; `add_pool_topology(pool, indices)` adds an index list its meshes are drawn with;
   `add_pooled_mesh(pool, topology, vertices)` writes a mesh into the renderer's **vertex arena**
   (one storage buffer, 8 floats a vertex — the vertex buffer layout — at binding 7 of the frame
   bind group) and returns an ordinary `MeshId`: instances, culling (its own bounds), materials
   and `remove_mesh` (the slot returns to its pool) work as for any mesh.
2. Preparation groups instances by **draw group**: a mesh id, or (at and above `POOLED_DRAW`) a
   pool topology; the instance record's `material.y` carries the mesh's first vertex in the
   arena. A pooled group is drawn with the `forward_pulled` / `shadow_pulled` pipelines (the same
   layouts; `vs_pulled` reads the arena by `material.y + vertex_index`), so the frame's order
   (front to back by group), cascades and last mile are unchanged.
3. The arena grows by doubling, copying what it holds — which a frame pays for on the GPU
   (7-13 ms at 16 MB on the 3080) — so a pool **reserves** what it expects up front: the
   viewport host reserves 4,096 patches (~42 MB), over the ~3,500 a descent holds.

## Consequences

- Guards: `crates/forge-render/tests/test_mesh_pool.rs` (C-render-mesh-pool): 400 wavy
  patches, two index lists, four materials, shadows on — the pooled frame equals the
  per-mesh frame to one 8-bit step at every pixel, in 10 draw calls against 1,999, and
  re-adding the pool's meshes reuses its slots; control: instances ignoring their slot.
- A stitch-mask change still uploads the patch again (a pooled mesh pairs its vertices with one
  topology); an instance choosing its topology would upload vertices once per node — a follow-up
  if stitch churn shows in a trace.
- The pooled meshes use the same `f32` object-space narrowing as `add_mesh`; nothing about I1 or
  the last mile changes.
