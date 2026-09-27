# ADR 0042 — Timed tests run alone machine-wide, zero-allocation live counters, and simulation inputs on the bus

- **Status:** accepted
- **Date:** 2026-09-23
- **Work package:** WP-19 (WP-18 and WP-13 verifier follow-ups, WP-U8 note)

(Numbered 0042: `dev` holds up to 0041; D-9.)

## Context

The WP-18 and WP-13 verifiers left follow-ups that are each small but touch the frame's hot
paths or the reliability of the gates that guard them: the terrain view sorted its whole mesh
cache in-frame when it overflowed and uploaded ancestor stand-ins past its per-frame cap; the
mesh-churn guard only saw allocation; the tracer allocated per counter and per frame; the play
core rebuilt a per-body map every frame and cloned the scheduler's plan every step; simulation
inputs reached the play core outside the bus; and wall-clock tests failed under another
lane's load or GPU work (the perf gate twice, the streaming walk once).

## Decisions

4. **C-mesh-churn gains a time check** (`C-mesh-churn-time`): the minimum over 15 bursts of a
   removal's time with 16,384 meshes held over 64 held must stay under 10x, timed alone. An
   O(1) removal measures 2.4-2.6x (the GPU buffers' release and cache effects); an
   allocation-free scan of the table on every removal (the control) measures 48-67x, and the
   allocation check passes it. 10x sits a factor of ~4 from each side.
5. **One helper times every wall-clock test, and only one timed body runs on the machine at a
   time.** `forge_trace::timed` (moved from `forge_ui::testing`, which re-exports it, so the
   crates below the UI share it): the timed body runs in a child process of the test binary
   running only that test, at the High priority class (read back), **holding a machine-wide
   OS file lock** (`forge-timed-alone.lock` in the temp directory). Priority puts the timed
   thread ahead of another lane's build; the lock keeps two timed bodies (the perf gate in one
   lane, the walk or a UI gate in the other, or two binaries of one `cargo test`) from
   measuring the CPU or the GPU at once — process priority does not schedule the GPU. The
   child's prints reach the parent's test output. Converted: `test_perf_gate` (its in-file
   mutex removed), `test_streaming_hitch` (its own priority code removed),
   `test_mesh_churn` (time half), `test_lod_select_perf`, `test_chunk_build_async`,
   `test_trace_overhead`, `test_local_stamps`, `test_import_perf`. `test_m1_exit` and
   `test_terrain_cache` time nothing (they stay in nextest's `wall-clock` group because their
   thread pools and GPU work would disturb a timed test). Guarded by `C-timed-alone`.

   **Verification amendment (GPU work that times nothing).** With only timed bodies
   serialised, plain `cargo test --workspace` failed the perf gate in 2 of 3 runs beside
   another lane's `cargo nextest` (`render.atmosphere.precompute` 72.1 and 100.7 ms against
   50.9): the row was the wall clock around a blocking GPU build, and golden renders and UI
   tests that time nothing shared the GPU with it. Two changes, both scheduling or
   measurement, no tolerance touched (W5):
   - **The lock is a reader-writer lock.** `forge_trace::timed::gpu_turn` takes it *shared*
     (per process, counted: a second pool of a process joins the first hold), and
     `forge_gpu::AdapterPool` takes a turn for its lifetime when built with the
     `test-gpu-turn` feature — which every crate with GPU tests (forge-gpu itself,
     forge-render, forge-ui, forge-editor-bin) enables through its
     dev-dependencies, so it is on in `cargo test`/nextest builds and never in a shipped one.
     A timed body takes the lock *exclusively* behind a turnstile file
     (`forge-timed-alone.turn`): new turns queue behind a waiting timed body while running
     ones finish, so it is not starved. The timed child takes no turn (its parent holds the
     lock for it); a thread that holds a turn and asks for the exclusive lock gets an error,
     not a self-deadlock. Guarded by `C-gpu-turn` (`crates/forge-trace/tests/test_gpu_turn.rs`,
     plus `a_test_pool_holds_a_gpu_turn_while_it_lives` in forge-gpu).
   - **GPU rows are timestamps, never the wall clock.** `render.atmosphere.precompute` is now
     the sum of each dispatch's own GPU time (compute-pass timestamps,
     `AtmosphereReport::gpu_ms`), like the frame's per-pass rows, so work another process
     gets scheduled between dispatches is not billed to it. Re-baselined on the timestamps
     (RTX 3080 31.1 ms, WARP 4030 ms — from 40.7 / 4530 wall-clock, so the allowance fell
     from 50.9 to 38.9 ms); the ~6.6 ms of pipeline creation it used to include is printed,
     not gated (a driver cost that differs by device, so no single calibration ratio holds
     it). New control `positive_control_a_slower_precompute_fails_its_row` (eight scattering
     orders fail the row).

   Measured: 10/10 perf-gate runs green while an atmosphere-goldens binary looped on the same
   GPU (62 runs of it; the perf gate's tests waited for their turns, 14 s instead of 8 s);
   plain `cargo test --workspace` 1494/1494 green twice while a `cargo nextest run
   --workspace` of the same tree ran beside it (1475/1475 each; precompute medians 32.3 and
   32.5 ms against the 38.9 ms allowance).
   Residual, said plainly: a GPU process built **without** the turn (another program, or a
   lane whose branch predates this) can still preempt a dispatch mid-way, and timestamps
   then include the preemption — beside a looping pre-turn goldens binary one run in four
   measured 45 ms. Every lane takes turns once it merges `dev`.
6. **Live counters allocate nothing in a steady frame.** Counters are keyed by their
   `&'static str`; zone totals keep their entries and are zeroed at a frame mark; the timeline
   stores `&'static str` parts and recycles its oldest sample once full; the public
   `FrameSample` (with `String` names) is built only when the panel reads a snapshot. 100
   steady frames with the COUNTERS sink on: **0 allocations** (`C-trace-counters-alloc`).
   The `tracy` feature's clippy run is in `just verify` and CI (the `Zone` field is `_tracy`).
7. **A play step batch allocates the same whatever the body count.** The moving bodies'
   transform map is updated in place after a batch (a body an input sets moving joins it; a
   control rebuilds it once); the plan proven at the fork is lent to the scheduler and handed
   back in its report instead of cloned. A 4-step batch: 24 allocations / 2,496 B with 256 and
   with 4,096 bodies; the WP-13 behaviour (control): 57 / 86 KB and 411 / 1.3 MB
   (`C-play-step-alloc`). What remains per step is the scheduler's per-tick bookkeeping, per
   region and system.
8. **A simulation input is the bus's `forge.play.input` session command** (args: one
   `forge_sim::SimInput` as JSON), registered next to `forge.play.control` with the same
   policy (not an undo step, open to every issuer). The shell (`Shell::play_input`) and
   `forge --headless` (an `input` line is sugar for it) hand each one the bus delivers to
   `PlayBackend::input`; the planner checks shape and finiteness, the play core checks the
   body when the input arrives and a refusal is reported (headless: a `refused` line; the
   shell: its console). `test_headless_parity` now sends the GUI's inputs over the bus.

## Why — the owner's two rules

2. **Faster engine:** no sort, set or list allocation in the terrain frame; no allocation per
   counter or per frame with the profiler on; no allocation per simulated body per frame.

## Alternatives rejected

- **Generation buckets for the mesh cache:** eviction within a bucket is unordered and a touch
  still moves the entry; the linked LRU is as cheap and exact.
- **Widen the perf gate's rows or retry them under load:** forbidden (W5), and it would hide
  real regressions.
- **An operation counter inside `remove_mesh`:** it would only count scans written through the
  counted accessor; a time ratio catches any O(n) work.
- **A GPU scheduling-priority call (D3DKMT) for the timed process:** needs `unsafe` FFI into
  the kernel thunk layer for a test-only benefit; the machine lock covers the case that
  failed (two timed bodies).

## Consequences

- New hidden test knobs (never set in an application): `TerrainFaults`,
  `Renderer::scan_mesh_table_on_remove_for_tests`, `SimFaults::rebuild_transforms`,
  `HeadlessOptions::direct_inputs_for_tests`; `TerrainView::evict_for_tests` /
  `release_for_tests`.
- `forge-ui` depends on `forge-trace` (zero engine dependencies; serde and ron only).
- Measured under load: `test_perf_gate` 10 of 10 green while a cold
  `cargo build --workspace --all-targets` ran on the same 12 cores (12-26 `rustc`s);
  `test_streaming_hitch` 3 of 3, slowest whole frame 9.3-9.6 ms of 20.
