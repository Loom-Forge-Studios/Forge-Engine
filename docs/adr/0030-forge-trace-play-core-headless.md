# ADR 0030 — One tracer over three sinks; a play core that records by step; `forge --headless` is the editor core

- **Status:** accepted
- **Date:** 2026-09-22
- **Plan references:** M2-5, M2-11, M2-17; Ch.29, Ch.34 §34.4, Ch.21 §21.11 and §21.21
  (Profiler, Play controls), Ch.37 §37.5, Ch.3 (determinism), Ch.2.5 (canonical time), I7,
  I-20, W1/W2, W5; D-4, D-9; ADR 0014 (the `forge` binary), ADR 0018 (the headless core),
  ADR 0029 (WP-U6: the viewport, play controls and profiler this binds to)

(Numbered 0030: lane B's WP-U6 ADR took 0029 on `dev` first; D-9.)

## Context

WP-13 builds three things the plan names but leaves open in shape:

- **`forge-trace` (Ch.29, BRIEF):** "Tracy + Perfetto; named budgets per subsystem", with
  counters the editor's profiler panel reads through a live feed (WP-U6 built that panel
  against the `CounterSource` trait, D-4) and "overhead ~0 when disabled".
- **The play core (M2-5):** play-in-editor forks a simulation world from the edit world
  (§21.21, §37.5); a recorded command stream replays deterministically. Nothing says where it
  lives, what a recording contains, or how the editor's clock meets a deterministic step.
  WP-U6 built the play controls as the `forge.play.control` session command on the bus and
  the `PlayBackend` trait the shell runs them on.
- **`forge --headless` (Ch.34.4, M2-17):** "headless is the same binary"; ADR 0014 made
  `forge` (tools/forge-cli) the binary WP-13's `--headless` extends; `test_headless_parity`
  compares headless and GUI runs of the same scene by simulation hash.

## Decision

1. **`crates/forge-trace`, at the bottom of the spine** (no engine dependencies, so
   forge-core, forge-gpu, forge-render and the simulation can all open zones). One API —
   `zone!`/`Tracer::zone` (RAII, `&'static str` dotted names), `counter`, `instant`,
   `frame_mark` — over three runtime-selectable sinks: **COUNTERS** (per-frame zone totals
   on a 240-frame timeline, latest counters, named budgets measured by the zone or counter
   of the same name and grouped by subsystem = first name segment; over-budget frames
   counted), **PERFETTO** (nested slices per thread, counter tracks, instants; encoded by
   hand as the native protobuf trace, with a reader for tests and tools; capped, drops
   counted and written into the trace), **TRACY** (cargo feature `tracy`: `tracy-client`
   with `enable`, `only-localhost`, `ondemand` and no `broadcast`; asking for it in a build
   without the feature is `TRACE-0003`, not a silent no-op). Disabled, a zone is one relaxed
   atomic load and a branch; a zone captures its sinks at open.
2. **The profiler reads the tracer through WP-U6's `CounterSource`.** `forge_trace::FeedCell`
   has `forge_ui::LiveCell`'s semantics (generation, live flag, a waker called once until
   consumed) without a UI dependency. `forge_editor::sim_bridge::TraceCounters` implements
   `LiveSource` over it and `CounterSource`, mapping `TraceSnapshot` to the panel's
   `ProfileSnapshot` in `sim_bridge::profile_snapshot`: frames and lanes field for field; a
   budget's name, value, allowance and unit (the tracer's `peak`, `samples` and
   `over_frames` stay for the `--trace` report); the tracer's counters and dropped-event
   count into two fields added to `ProfileSnapshot` (`counters`, `dropped_events`; empty for
   other sources); replication `None` with its reason. It is the editor's default source
   (`EditorServices::profiler`, over the process tracer, `EditorServices::tracer`); the
   editor binary switches the COUNTERS sink on at start. Producers mark frames — the play
   core per step batch, the viewport's GPU host per rendered view (it reports to the tracer
   instead of into `MemoryCounters`) — and the panel's own redraws never do, so the profiler
   cannot wake itself. `MemoryCounters` stays as a labelled source a test pushes by hand.
3. **`crates/forge-sim`, the play core** (claimed under Ch.34 in the repo tree). The edit
   world is an `EditSnapshot` (from the core's `Project` or from a client's mirror; a
   content hash over every property's bits). `SimWorld::fork` spawns every entity with
   `transform.position.local` into a `forge_core::World`, one region per frame, and steps it
   on the regionized scheduler with the plan proven once at the fork: semi-implicit Euler for
   `motion.velocity`/`motion.acceleration`, spin about the frame's up axis from
   `motion.spin_dps` (fixed-step quaternion precomputed; `det` trig), `f64` throughout.
   `PlaySession` has the play controls' transitions (Play / Pause / Step(n) / Stop; Step while
   playing pauses first, both logged and recorded), `advance(now)`, `tick`, `transforms`,
   `revision`, `log`, `last_step_ms`, plus `input(SimInput)` (set velocity, impulse, set
   acceleration, set spin) and the recorder. **The editor clock decides how many fixed 60 Hz
   steps run (at most 15 per call; the rest is dropped), never what a step computes** (I-20).
   Step `n` is at canonical tick `floor(n * 10^6 / 60)`, so 60 steps are exactly one second (a
   constant 16 666-tick step would lose 40 µs every second).
4. **One set of play types, one play backend.** `PlayCommand`, `PlayState` and
   `PlayLogEntry` are `forge_sim`'s; `forge_editor::play` re-exports them (and the motion
   property names) instead of defining its own. `forge_editor::sim_bridge::SimPlay`
   implements WP-U6's `PlayBackend` over a `PlaySession`, forking from the mirror through
   `sim_bridge::edit_snapshot` (only when a stopped session forks: a running one is never
   handed the edit world). `PlayBackend::control` and `advance` return nothing, so the trait
   gained `take_error` (default `None`), which the shell turns into a console entry. `SimPlay`
   is the editor's default backend (`EditorServices::play`, the same object as
   `EditorServices::play_core`, which exposes the recording and inputs); the in-memory
   `MemoryPlay` is removed.
5. **The replay file (`.forgereplay`) is JSON Lines, version 1:** a header (version, rate,
   checkpoint cadence, edit hash, and the edit world itself by default) then one event per
   line in issue order, each stamped with the **simulation step**: `control`, `input`
   (recorded when issued; applies at the next step boundary), `check` (the full state hash
   every `check_every` steps), `end` (the hash at Stop). `replay` re-issues controls and
   inputs at their steps, runs the steps between them directly, and requires the replaying
   session's own recording to equal the file event for event (`SIM-0007` names the first
   difference and its step); a replay against a given scene first checks its hash
   (`SIM-0006`).
6. **`forge --headless` is `forge_editor::headless`** — the code `forge-editor --headless`
   already ran (ADR 0018), extended with play and `--record`, `--replay`, `--trace`
   (Perfetto + budget summary). `run N` moves a simulated clock exactly N steps' time and
   calls `PlayBackend::advance` (the shell's per-frame call) in catch-up-sized batches;
   `input` and `sim-hash` reach the play core directly (a simulation input is not a bus
   command yet, gate `C-sim-input-command`). *Superseded by ADR 0042 (WP-19): an `input`
   line is sugar for the `forge.play.input` session command, and the gate is bound.* Nothing
   a session does can change the project (it is handed a snapshot, never a command sink;
   play controls are session commands, not project edits — I7).
7. **`test_headless_parity`** (tools/forge-cli/tests): the GUI half builds the scene through
   the shell's command emitter (a `forge_editor::testing::Rig`), plays it with the shell's own
   controls (bus commands run on the shell's `SimPlay`, which forks from the mirror), and the
   editor loop's jittered clock advances it, with inputs between frames, a pause with single
   steps and a 480 ms stall; the headless half is the built `forge` binary building the same
   scene from its script and replaying the GUI's recording against its own project. Same edit
   hash, every checkpoint hash, the final simulation hash and the project state hash must
   agree.

## Why — the owner's two rules

1. **Better for the user:** a play session is a file a user can attach to a bug report and
   anyone can replay bit-for-bit (`forge --headless --replay bug.forgereplay`), with the
   divergence step named if their build differs; the file is text that diffs line by line
   and survives a crash as a replayable prefix. The profiler shows budgets per subsystem in
   red from the same zones a Perfetto or Tracy session shows, so what the panel says and
   what the deep tools say never disagree. Tracy never announces itself on the network.
2. **Faster / more efficient engine:** instrumentation can stay in shipping code — a disabled
   zone measured 0.43 ns, a disabled counter 0.22 ns (`test_trace_overhead`, minimum of nine
   2M-iteration trials); the Perfetto writer needs no protobuf dependency and encodes 900k
   events in ~0.2 s; the simulation proves its schedule once per fork, not per step, and
   precomputes each spin's step rotation and reuses its entity buffers (10,000 moving bodies
   step in 0.93 ms in a release build; tracing on or off changes neither the time measurably
   nor the state hash); the editor copies the edit world only when Play forks; recordings
   store steps and inputs, not states, so they are small (the 24-body, 327-step golden is
   14.6 KB with its scene embedded).

## Alternatives rejected

- *forge-trace depending on forge-ui to implement `LiveSource` itself:* would put the UI
  crate under every engine crate and forbid forge-ui from ever opening a zone (a cycle).
- *Chrome JSON trace events instead of the Perfetto protobuf:* larger and slower to write;
  the protobuf subset is small and is checked by a decoder in the tests.
- *`tracing` crate + subscribers:* a second instrumentation vocabulary, dynamic dispatch per
  event, and no zero-cost disabled path without a compile-time level filter.
- *Recording states (snapshots) instead of inputs:* orders of magnitude larger and proves
  nothing about determinism; checkpoint hashes give the same early divergence detection.
- *Stamping events with wall time and re-timing the replay:* a replay would depend on the
  replaying machine's scheduling; steps are exact.
- *Recording an input when it applies rather than when it is issued:* a Step issued after
  the input would replay before it (found by `test_replay`'s pause/step sequence).
- *A separate `forge` GUI:* the headless mode must be the editor's own core; the GUI binary
  stays `forge-editor` until the launcher work merges the two (listed as a follow-up).
- *Keeping `MemoryPlay` / `MemoryCounters` as the editor's defaults with the real backends
  beside them:* the shell would present stand-ins while the real play core and tracer exist.

## Consequences

- Gate rows: `C-trace-disabled-overhead` BOUND (crates/forge-trace/tests/test_trace_overhead.rs,
  its control a clock-reading zone), `C-perfetto-trace-well-formed` BOUND
  (crates/forge-trace/tests/test_perfetto_trace.rs), `C-replay-bit-exact` BOUND
  (crates/forge-sim/tests/test_replay.rs, its control the variable-timestep fault),
  `C-headless-parity` and `C-play-core` BOUND (tools/forge-cli/tests/test_headless_parity.rs,
  two controls: the variable-timestep fault and a GUI fork that drops a hidden body),
  `C-profiler-trace-source` BOUND (crates/forge-editor/tests/test_play_trace_backends.rs, its
  control a profiler that does not read the tracer), `C-trace-viewers` AWAITING an operator
  (opening a trace in the Perfetto UI, connecting a Tracy viewer), `C-replay-linux-leg`
  AWAITING a Linux runner (the committed golden session), `C-profiler-replication` UNBUILT
  (forge-net, M4-7), `C-sim-input-command` UNBUILT.
- WP-U6's tests adapt minimally: `test_play_controls` compares ticks with
  `forge_editor::play::ticks_after` (a step is 16 666 or 16 667 ticks), and
  `test_profiler_panel` sets its own hand-pushed `MemoryCounters` as the rig's profiler.
- A change to what a step computes changes `golden_session.forgereplay`: a determinism event,
  re-recorded with `FORGE_BLESS=1` and explained, like the other goldens.
- A shipping build that enables `tracy` must regenerate NOTICES with that feature (Tracy's
  BSD-3-Clause C++ client); no default build links it.
