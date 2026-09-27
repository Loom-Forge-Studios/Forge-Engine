# ADR 0028 — Terrain streaming never waits, LOD selection updates locally, every GPU perf row catches 1.5x, and virtual shadow maps stay owed

- **Status:** accepted
- **Date:** 2026-09-22

## Context

WP-18 polishes M1 from the WP-09/WP-11 verifier follow-ups. The M1 demo hitched ~65 ms while
walking: `TerrainView` waited for the job pool whenever 64 or fewer patches were missing, then
uploaded every landed patch at once, and every LRU release copied the renderer's whole mesh
table (`Arc<[_]>`, O(n) per `remove_mesh`, O(n k) per burst). LOD selection re-walked the
quadtree from the six roots on every walking frame. The perf gate's per-pass GPU rows allowed
`max(1.5 x base, base + slack)`, so a pass measuring a little under its baseline could grow
1.5x unseen. No atmosphere golden isolated the Rayleigh phase function. VSM was owed (ADR 0023).

A local model's attempt (03464ad, d445568) was reverted: its `remove_mesh` left the GPU mesh in
the table (a removed id stayed drawable), and its terrain change zipped a filtered list against
the unfiltered one and cached a parent's mesh under the child's slot.

## Decision

1. **A frame never waits for the streamer.** `TerrainView::update` polls finished builds,
   uploads at most 48 meshes, requests what is missing (finest first), and draws every leaf that
   is not resident from a substitute: the same patch with another stitch mask; the finer
   patches it replaces (a merge, at any depth: a per-ancestor count of resident patches below
   keeps the search to subtrees that hold one); or the nearest resident
   ancestor (a split), with every selected leaf under a drawn ancestor suppressed so nothing
   overlaps. Chunk keys are computed once per leaf per frame, and only for leaves without a
   mesh (a resident mesh remembers its origin). The streamer's cache is bounded with the mesh
   cache. `TerrainView::load` / `DemoWorld::load` is the explicit loading step (the demo's
   start, the tests). Substitutes meet their neighbours without matching stitch masks for the
   frames a build takes: a possible sliver of sky at a seam for a few frames, instead of a
   frozen frame (rule 1). Measured (`test_streaming_hitch`, 600 frames at 100 m/s, ~7,000
   uploads, 2560x1440): slowest `DemoWorld::frame` 3.7 ms, slowest whole frame with a GPU wait
   4.6 ms (the pre-WP-18 view: 70 ms); every frame's drawn patches disjoint and covering the
   body.

   *Verifier fixes.* The timed walk runs its process at the High priority class (the
   foreground frame's place): under a concurrent release build plus 12 busy loops, 9.2-11.2
   ms whole frame, 6.6-7.8 ms `DemoWorld::frame`, in 9 of 9 runs. Scheduling, not a
   tolerance: the 20 ms budget is unchanged.
2. **The renderer's mesh table is one slot table** in the per-frame state the pass bodies read;
   `add_mesh` / `remove_mesh` change one slot (0 bytes allocated per removal with 64 or 4,096
   meshes held, ~1 us; the copy-on-write table allocated 9.4 MB and ~100 us per removal at
   4,096). Public API unchanged.
3. **Local LOD selection.** The scratch keeps the evaluation frontier (roots and children of
   refined nodes — what a walk from the roots tests) with each node's slack as an odometer
   reading in a heap; a call re-decides only the nodes whose slack the viewer's path used up.
   The one-level restriction is a closure — a split node at level L >= 1 requires its parent
   and the level L-1 node across each edge, strictly coarser, so acyclic — kept with reference
   counts; each refinement change adds or removes only the nodes whose count crosses zero,
   splits or merges their leaves in place in the Morton-ordered leaf list, and recomputes only
   neighbouring stitch masks. More than 256 changes in one call (a teleport) rebuild. 2 m walk
   at 1.4 m a frame: 78 us -> ~5-6 us, at most 36 of 722 nodes re-decided per frame; a cold
   selection costs ~7 % more (0.93 -> ~1.0 ms) to build the local state — paid once per viewer.
4. **GPU perf rows: 1.25x relative band, floor tied to the timer, and a structural 1.5x rule.**
   `gpu_band: 0.25` (CPU rows keep 0.5); the per-class slack floor stays the timestamp
   quantisation plus spread (RTX 3080 3 us, WARP 50 us). The gate refuses a budget file in
   which any row of any class would allow a pass 15 % under its baseline to grow 1.5x
   (`max(1 + gpu_band, 1 + slack / base) < 1.5 x 0.85`). Control: this device's clean frame
   with each pass in turn 1.5x slower fails exactly that pass's committed row (all eight, RTX
   3080 and WARP). Clean runs: 6/6 green on the RTX 3080 (pass medians 0.92-1.05x baseline),
   green on WARP (~0.91x).
5. **A clear-sky golden guards the Rayleigh phase.** Control: an isotropic Rayleigh phase in
   the sky pass (hidden knob, `frame.flags.y`) fails the golden and the signature;
   continental haze fails the signature (forward/backward 1.50).
6. **Virtual shadow maps stay owed; cascades stay (ADR 0023, restated).** A VSM is buildable on
   the RTX 3080 through wgpu without sparse resources — GPU page marking over the depth
   buffers with storage atomics, a GPU allocator into a physical page atlas, per-page caster
   routing with indirect draws and clip distances, static-page caching and invalidation, and a
   page-table lookup with cross-page filtering in the shaders. It is not built now because on
   M1's content it can buy neither rule: the four cascades cost **0.116 ms** of the 0.18 ms M1
   frame at 1280x720 (perf gate: 0.034 / 0.028 / 0.029 / 0.025 ms), and a VSM's fixed
   per-frame passes (a full-resolution marking pass, allocation, page-table maintenance) plus
   per-page draws start at that order of cost before it renders a page; its advantage —
   texel density for dense far casters and cached static pages — needs content M1 does not
   have (towns, forests, many shadowed kilometres). It is a milestone-sized subsystem with its
   own guards (floor shadow, page-feedback latency, invalidation). It becomes worth building
   when a frame's shadow cost or far-shadow quality is measured to need it; `test_shadows`'
   floor-shadow check is its acceptance test. Gate row `C-vsm` stays `Unbuilt` and DoD M1-7
   stays Blocked on the VSM half, with this reason.

## Why — the owner's two rules

1. **Better for the user:** walking and flying never freeze the frame (a few frames of coarser
   ground instead); the regression gate now actually sees a 1.5x pass; the sky's most visible
   physics has a guard.
2. **Faster engine:** frame streaming work is bounded (48 uploads, O(1) removals); walking LOD
   selection is ~15x cheaper; VSM is deferred because it would make M1's frame slower.

## Alternatives rejected

- **Keep the wait but make it shorter (a time budget on `wait_idle`):** still a stall, and its
  length would track the job pool's queue, not the frame.
- **Draw nothing for a missing leaf:** holes in the ground under a moving viewer.
- **A tighter GPU band everywhere (1.1x):** the RTX 3080's pass medians move ~10 % between runs
  (clock states); a gate that fails clean runs is ignored.
- **Build the VSM now:** see decision 6.

## Consequences

- `forge-render`: `RenderOptions::isotropic_rayleigh_for_tests` (hidden, additive) and
  `Renderer::copy_mesh_table_on_change_for_tests` (hidden) exist only for W2 controls.

## Amendment (WP-19, ADR 0042): the conditions the perf gate measures under

The gate's rows are only meaningful if the measurement has the machine to itself.
`the_named_budgets_hold` failed once under another lane's load (a row over 1.25x) and once
beside a GPU-using test in a full-workspace run (the atmosphere precompute 51.9 ms against
its 50.9 ms allowance), because the file only serialised its own tests with an in-file mutex.
The fix is scheduling, never a wider row (W5):

- Every test of `test_perf_gate` that measures runs its body through
  `forge_trace::timed::run_timed_alone`: a child process of the test binary running only that
  test (so plain `cargo test` measures as nextest does), at the **High** priority class (read
  back; a Windows run that cannot get it fails instead of measuring at another class), holding
  the **machine-wide timed lock** (`forge-timed-alone.lock` in the temp directory, an OS file
  lock), so no other timed test of any binary, lane or runner (another perf gate, the
  streaming walk, a UI gate) measures the CPU or the GPU at the same moment.
- The timed lock is a reader-writer lock (WP-19 verification, ADR 0042 item 5): every test
  process with a GPU device holds it shared (`forge_gpu::AdapterPool` under the
  `test-gpu-turn` feature of test builds), so golden renders and UI tests that time nothing
  do not run on the GPU while a perf-gate body measures.
- GPU rows are GPU timestamps around their own work, never the wall clock: the atmosphere
  precompute row became the sum of its dispatches' timestamps (re-baselined lower: 31.1 ms on
  the RTX 3080, 4030 ms on WARP).
- nextest still gives the binary every test slot (`threads-required = "num-cpus"`).
- Recorded in `tests/gates.ron` above the `C-perf-gate` row. Measured after the change: 10 of 10
  runs green while a workspace build ran on the same cores (ADR 0042).
