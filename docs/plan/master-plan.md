# Forge — Master Implementation Plan

> Codename placeholder. Rust game engine + editor for true-scale planetary bodies,
> seed-generated universes, and agent-native authoring. Apache-2.0 OR MIT.

**Status:** draft 1, 2026-09-20. Nothing below is ratified until it appears in
`forge-decision-ledger.md` with a ratification date.

---

> **"The Foundations"** throughout this document means the planetary-architecture work
> ratified before this plan began — the stateless terrain layer, the frame stack, the
> ephemeris model, the simulation-region rules, and the invariants that come with them.
> Those decisions are **binding here and restated in full** in `decisions.md` §1. The
> source documents are not published.

## Who this document is for

A contributor, or a team of contributors, building the engine. It is also the contract
that keeps those contributors from disagreeing with each other. When two chapters conflict, **Chapter 1
wins**; when Chapter 1 conflicts with a Foundation decision, **the Foundation wins and
Chapter 1 has a defect** — file it, do not paper over it.

---

## What is being built

An engine and editor whose **unit of content is a celestial body**, not a level.

- A universe is a seed. `galaxy → system → body → region → chunk → instance` all derive
  from it, deterministically, with nothing stored.
- Bodies are true-scale. Earth is 6,371 km and stays 6,371 km. There is no "scaled down
  for gameplay" mode and no global coordinate type to lose precision in.
- Physics, atmosphere, weather, hydrology, and asset placement are **generated from the
  same fields**, so they agree with each other by construction rather than by art-passing.
- The editor is a headless core with a UI *client*. An agent is a peer of that UI, not a
  plugin bolted to it.
- Everything is Rust. Gameplay is Rust. Blueprints **compile to** Rust. Extensions are
  Rust compiled to WASM.
- It runs on one machine and it runs on four cheap used ones.

---

## Reality check — read before planning anything

This section exists so that no agent quietly discovers it halfway through M4 and stalls.

**1. Feature parity with Unreal is not a goal and cannot be one.** UE is roughly
twenty years and low-thousands of engineer-years. Unity similar. Godot is ~14 years and
hundreds of contributors. The attached feature comparison lists, conservatively, 400+
discrete subsystems. Treating that list as a backlog is how this project dies at 30%
completion with nothing anyone can ship.

**2. The list is still the right list — as a *destination*, ordered by a different rule.**
The ordering rule is: *does this exist because the engine is planet-scale and
agent-native, or does it exist because every engine has one?* The first category is built
first and built well. The second is adopted from the Rust ecosystem, deferred, or
accepted as a gap with a written reason. See *What we are not building*.

**3. What is actually winnable.** Not "a better Unreal." This:

> The only engine where a planet is a first-class object and an agent is a first-class user.

Nobody else is trying to be that. UE bolts planets on via plugins and fights its own
`float` render transform doing it. Unity has no story at all above ~10 km. Godot is the
right *shape* but has no scale layer. Bevy has the architecture and no editor. The gap
is real, it is large, and it is reachable.

**4. The specific payoff.** It is a settled Foundation result that Folia-style *regionized
multithreading inside one process* is strictly better than cross-process ghost projection,
and that Unreal cannot do it: it has no regionized threading, and packing several worlds
into one process still shares one game thread, so it buys packing and not parallelism.
Rust can do it. Region-partitioned mutable world access is precisely what an
ownership-and-borrow model is for. **The one thing the Foundations name as
better-but-unavailable becomes available by building in Rust.** Alongside it:

| Known pain | Cause | Gone in Forge because |
|---|---|---|
| Multi-UWorld packing to dodge 2.5 GB baseline | UE process baseline | a `World` is kilobytes; sparse players stop needing a trick |
| Replication quadratic pinned to one game thread | UE game thread | regionized scheduler parallelises it |
| `double` world pos → `float` at the last mile | UE render transform | the renderer is ours; f64→camera-relative is the *only* path |
| Projection needed at 400+ crowd | no in-process regions | regions first, projection only if regions run out |
| BP VM ~10× slower than C++ | interpreted graph | graphs compile to Rust |

**5. Timescale, stated honestly.** With sustained effort and aggressive reuse of the Rust
ecosystem: a **vertical slice nothing else can do** (walk on a true-scale
generated planet with real climate, edit it live, drive it from an agent) is an M3
deliverable and is genuinely reachable. A **self-hosting engine that a real game ships on** is M4–M5. **Broad parity** is a multi-year community project and should be planned as one,
with the 1.0 line drawn at "a real game ships on it" — not at "the comparison table is full."

**6. This is a marathon that must produce runnable things monthly.** Every milestone ends
in something a human can launch and an agent can drive. No milestone is allowed to end in
"the architecture is now correct."

---

## Working agreements (binding)

Each of these was earned the hard way on an earlier project. They are binding here, and
none of them is a style preference.

**W1. A guard can only fail on a question somebody thought to ask.** Every invariant in
this plan has a named test file. A rule with no test is a wish.

**W2. Every guard needs a non-vacuous positive control.** Prove the guard fails when you
break the thing it guards, in CI, every run. A previous project shipped a liveness test whose positive control named a module that had
never existed. It reported green for months.

**W3. A name match is not a reference.** Liveness checks resolve through the import graph,
follow re-exports, and normalise spelling. Two complete modules once passed a name-matching caller-liveness test while being entirely
dead.

**W4. `just verify` is CI's command list in CI's order.** It is the command whose green
predicts the push. `just check` is a deliberate subset and says so in its own help text.
**A local gate that is a subset of the remote gate is worse than no local gate** — it
produces confidence exactly proportional to what it skips.

**W5. Never widen a tolerance or add a retry to make a test pass.** If a scenario is
flaky, find the cause. Flakiness is *triggered* by load and never *caused* by it. A
harness's patience with the OS scheduler may be raised, with the reasoning written at the
constant; an assertion about the engine may not.

**W6. A control run is only a control if the machine was in the same state.** Hold load
fixed, not just the commit. Use a worktree *and* an idle machine.

**W7. "Serialised" is a claim that needs an "against what".** Write the *against what* at
every such claim or it will hide a data race for the length of the project.

**W8. Defect numbers have one allocator.** `docs/defects.md` is the only allocator. Grep
before allocating.

**W9. When a subsystem cannot be built, its status changes to a *stated reason*, never to
silence.** `Unbuilt(reason)` is a legitimate terminal state. "Nobody got to it" is not one
of the reasons.

**W10. Interfaces are frozen before fan-out.** The binding constraint on parallel work is
interface ambiguity, not headcount. Contracts freeze at M0-18; parallel work starts after.

---

## What we are *not* building

Each of these is a decision, not an oversight. Re-proposing one requires new information.

| Not building | Instead |
|---|---|
| A shading language | WGSL, compiled by `naga` |
| A physics solver (v1) | `avian` (generic over f32/f64) with `rapier` as fallback |
| A rigid-body solver *at orbital velocity* | orbital motion is `f(t)`; physics only sees slow relative motion |
| An FBX parser | `ufbx` (MIT). **Never** the Autodesk SDK — it poisons the licence |
| Image/texture codecs | `image`, `ktx2`, `basis-universal`, `texpresso` |
| A blueprint virtual machine | graphs compile to Rust (Ch. 23) |
| A general circulation model | closed-form energy-balance climate + stochastic weather (Ch. 13) |
| N-body gravity integration | Kepler ephemeris, fixed-iteration (Ch. 14) |
| Real-world geodata import | rejected in the Foundations: does not translate into generator graphs cleanly |
| Console platform backends in the open tree | a HAL trait boundary licensed porters implement privately (Ch. 26) |
| A MetaHuman equivalent | interop with external tools; revisit no earlier than M8 |
| An asset marketplace | a package index and an open asset library |
| Our own ECS | `bevy_ecs` (see Ch. 5 for why we take the crate and not the framework) |
| A drop-in **native** plugin ABI | Rust has no stable ABI; drop-in is WASM, native is compiled in (Ch. 32) |
| A render-pipeline choice that forks the ecosystem | one pipeline; presets change defaults, never capability (Ch. 31) |
| A hosted remote-editor service, relay, or account | self-hosted only; point at your own TURN if you need one (Ch. 34) |
| Telemetry of any kind | — |
| An LFS-style service dependency | engine-native content-addressed blobs that work on every backend (Ch. 33) |
| Live multi-user editing before M8 | the command log keeps the door open; nothing is promised (Ch. 33 §33.4) |

---

## The fourteen invariants

These are the engine. Everything else is implementation. Each names its guard.

| # | Invariant | Guard |
|---|---|---|
| **I1** | **No global position type exists.** Every position is `(FrameId, DVec3)`. | `tests/liveness/test_frame_liveness.rs` |
| **I2** | Generation is a pure function of `(body_seed, integer position)` and is **byte-identical on every supported platform**. | `tests/determinism/test_cross_platform_hash.rs` |
| **I3** | Seeds derive. They are never accumulated, stored, or passed as mutable state. | `tests/determinism/test_seed_algebra.rs` |
| **I4** | Thread and process count scales with **players and viewers** — never with geometry, extent, body count, or channel count. | `tests/e2e/test_empty_universe_cost.rs` |
| **I5** | A profile may change cost. It may never change outcome. | `tests/e2e/test_profile_equivalence.rs` |
| **I6** | Exactly one authority per entity. Ghost replicas resolve nothing. | `tests/net/test_authority_uniqueness.rs` |
| **I7** | **Every mutation of project state is a command on the bus. The UI has no privileged path.** | `tests/liveness/test_command_liveness.rs` |
| **I8** | Every command is dry-runnable, undoable, and provenance-tagged. | `tests/liveness/test_command_contract.rs` |
| **I9** | Agent-authored and third-party code runs in the WASM sandbox. Native `cdylib` is a *shipping* path, never an *authoring* path. | `tests/script/test_sandbox_boundary.rs` |
| **I10** | Blueprint graphs compile. Nothing interprets a graph at runtime. | `tests/graph/test_no_interpreter.rs` |
| **I11** | A subsystem with no production caller **reached through a real import path** is not done. | `tests/liveness/test_caller_liveness.rs` |
| **I12** | The local gate is a **superset** of the remote gate. | `xtask/src/gate_parity.rs` |
| **I13** | No GPL or NDA-encumbered code in the open tree. | `xtask/src/licence_audit.rs` |
| **I14** | Bodies are **addressed**, not instantiated. Adding a body adds rows, never processes. | `tests/e2e/test_body_addition_cost.rs` |
| **I15** | **A workspace preset sets defaults. It never gates capability.** Every project is promotable to every preset. | `tests/preset/test_no_preset_gating.rs` |
| **I16** | **No first-party subsystem uses a capability a plugin cannot use.** The engine is a kernel plus plugins. | `tests/plugin/test_no_privileged_plugin.rs` |
| **I17** | `ProjectStore` is a trait. The local filesystem is one implementation, not a privileged one. | `tests/store/test_store_backend_parity.rs` |
| **I18** | **Windows and Linux are co-primary.** A behavioural difference between them is a defect, never a caveat. macOS is out of scope (Ch.27). | `tests/platform/test_platform_parity.rs` |
| **I19** | **`project_view = baseline ⊕ sandbox_deltas`.** A sandbox never mutates the baseline in place, and an idle sandbox is a row, not a process. | `tests/collab/test_sandbox_isolation.rs` |
| **I20** | **Live and Pull are subscription policies over one command stream, not two systems.** The same publish sequence yields the same final state under either. | `tests/collab/test_live_pull_equivalence.rs` |

> **I1 and I7 are the two that are cheap today and brutal to retrofit.** The Foundations learned this
> about `frame_id` and about the one-authority rule. Adopt both before anything exists to
> retrofit.

> **I16 and I17 join them.** A kernel/plugin boundary drawn after the engine has twenty
> subsystems is drawn around whatever those subsystems happened to do, and every private
> escape hatch found later is a permanent one. The same is true of a storage trait added
> after the editor has learned to call `std::fs` directly in forty places.

> **Three of the four are cheap only because I7 already exists.** Split-editor operation
> (Ch.34), semantic version control (Ch.33) and complete agent access (Ch.22) are the same
> decision collecting its dividend three times.

---

## Document map

| Ch | Title | Depth | Crate(s) |
|---|---|---|---|
| **1** | **System Contracts** | **FULL** | — |
| **2** | **Coordinates, Frames & Time** | **FULL** | `forge-frames` |
| **3** | **Determinism & the Math Contract** | **FULL** | `forge-num` |
| **4** | **Seed Algebra** | **FULL** | `forge-seed` |
| **5** | **ECS Core & the Regionized Scheduler** | **FULL** | `forge-core` |
| 6 | Reflection, Annotation & the Four Outputs | FULL | `forge-reflect` |
| 7 | The Command Bus, Undo & Provenance | FULL | `forge-cmd` |
| 8 | Asset System, VFS & Import | BRIEF | `forge-asset` |
| 9 | GPU Device Layer & Render Graph | BRIEF | `forge-gpu` |
| 10 | Renderer — Scene, Materials, Lighting, GI | BRIEF | `forge-render` |
| 11 | Planetary Rendering — Frusta, Atmosphere, Ocean, Cloud | BRIEF | `forge-render` |
| 12 | Body Representation — Lattice / Quadtree / Field | BRIEF | `forge-planet` |
| 13 | Terrain Generation — Global Solve & Local Function | BRIEF | `forge-gen` |
| 14 | PCG Framework | BRIEF | `forge-pcg` |
| 15 | Climate & Weather | BRIEF | `forge-climate` |
| 16 | Astrophysics — Galaxies, Systems, Bodies, Ephemeris | BRIEF | `forge-astro` |
| 17 | Physics & Simulation Bubbles | BRIEF | `forge-phys` |
| 18 | Navigation & AI | BRIEF | `forge-nav` |
| 19 | Animation | BRIEF | `forge-anim` |
| 20 | Audio | BRIEF | `forge-audio` |
| 21 | UI Toolkit & Editor Shell | BRIEF | `forge-ui`, `forge-editor` |
| 22 | MCP Server & the Agent Protocol | FULL | `forge-mcp` |
| 23 | Scripting — Rust, WASM, Hot Reload | BRIEF | `forge-script` |
| 24 | Blueprints — Graph, IR, Codegen | FULL | `forge-graph` |
| 25 | Networking, Replication & Authority | BRIEF | `forge-net` |
| 26 | Heterogeneous Compute & the Distributed Farm | FULL | `forge-jobs` |
| 27 | Platform HAL & Export | BRIEF | `forge-hal` |
| 28 | Gameplay Framework | BRIEF | `forge-play` |
| 29 | Profiling, Debugging, Observability | BRIEF | `forge-trace` |
| 30 | Testing, Gates & CI | FULL | `xtask` |
| **31** | **Workspace Presets: 2D, 3D, Planetary** | **FULL** | `forge-editor`, `presets/` |
| **32** | **Plugin System & Extension Points** | **FULL** | `forge-plugin` |
| **33** | **Project Storage, Version Control & Collaboration** | **FULL** | `forge-store` |
| **34** | **Remote & Headless Operation** | **FULL** | `forge-remote` |
| **35** | **The 2D Pipeline** | **FULL** | `forge-2d` |
| **36** | **Volumetric Editing & the Mineable-World Toggle** | **FULL** | `forge-volume` |
| **37** | **Teams, Sandboxes & Multi-User Sessions** | **FULL** | `forge-collab`, `forge-identity` |
| A | Licensing, Governance & Community | FULL | — |
| B | Risk Register & Spikes | FULL | — |

**FULL** = written to implementation depth here. **BRIEF** = contract, decisions, DoD and
an expansion brief; expanding it is a task, and the first task of whoever owns it.

> **Chapter numbers are identities, not an order.** Chapters 31–36 were added after 1–30
> and are numbered by allocation, not by where they read. Renumbering would break every
> DoD id, gate row and handoff that cites one — the same trap as a DoD item id, which is a
> *position* and not an identity unless you refuse to renumber. Read in this order:
> **1–7** (contracts) → **31, 32** (shape and extensibility) → **8–21** (subsystems, with
> **35** beside 10 and **36** beside 12) → **22–30** → **33, 34, 37** (storage, remoting and
> collaboration — one arc; read them together).

---

## Repo tree — coverage checklist

Every directory must be fully specified by some chapter. `just plan-coverage` fails on an
unclaimed directory.

```
forge/
├── crates/
│   ├── forge-num/        Ch.3   deterministic math, vendored transcendentals, noise basis
│   ├── forge-frames/     Ch.2   FrameId, FramePos, FrameTree, rebase, Tick
│   ├── forge-seed/       Ch.4   seed algebra and derivation
│   ├── forge-core/       Ch.5   world, regionized scheduler, profiles
│   ├── forge-reflect/    Ch.6   #[forge_api], type registry, schema emit
│   ├── forge-cmd/        Ch.7   EditorCommand, txn, undo, provenance, bus
│   ├── forge-asset/      Ch.8   vfs, importers, cache, hot reload
│   ├── forge-gpu/        Ch.9   adapter pool, transfers, render graph
│   ├── forge-render/     Ch.10-11  scene, materials, lighting, planetary passes
│   ├── forge-planet/     Ch.12  BodyRepr trait + lattice/quadtree/field impls
│   ├── forge-gen/        Ch.13  generator graph, global solve, local eval
│   ├── forge-pcg/        Ch.14  scatter, rules, biome binding
│   ├── forge-climate/    Ch.15  EBM, circulation, weather
│   ├── forge-astro/      Ch.16  galaxy/system/body gen, Kepler ephemeris
│   ├── forge-phys/       Ch.17  bubbles, collision, character, vehicles
│   ├── forge-nav/        Ch.18  navmesh, pathing, steering
│   ├── forge-anim/       Ch.19  skeleton, state machine, blend, IK, retarget
│   ├── forge-audio/      Ch.20  buses, spatial, adaptive
│   ├── forge-ui/         Ch.21  widget layer used by editor AND games
│   ├── forge-editor/     Ch.21  the shell — a client of forge-cmd, nothing more
│   ├── forge-mcp/        Ch.22  five tools, three resource roots
│   ├── forge-script/     Ch.23  wasm host, dylib host, reload
│   ├── forge-graph/      Ch.24  graph model, typed IR, Rust codegen
│   ├── forge-net/        Ch.25  transport, replication, authority, prediction
│   ├── forge-jobs/       Ch.26  work-stealing, heterogeneous dispatch, farm client
│   ├── forge-hal/        Ch.27  platform traits — no platform code, only traits
│   ├── forge-play/       Ch.28  abilities, input, save, localisation
│   ├── forge-trace/      Ch.29  tracy/perfetto, counters, budgets
│   ├── forge-plugin/     Ch.32  kernel boundary, extension points, wasm+source loaders
│   ├── forge-store/      Ch.33  ProjectStore trait; LocalFs/Git/S3/Sql backends, blobs
│   ├── forge-remote/     Ch.34  split editor, thin client, encode, transport, pairing
│   ├── forge-2d/         Ch.35  sprites, tilemaps, 2D lights, avian2d, cutout rigs
│   ├── forge-volume/     Ch.36  volumetric layer, brushes, edit store, sync
│   ├── forge-identity/   Ch.37  users, teams, roles, auth — its own security boundary
│   ├── forge-collab/     Ch.37  baseline, sandboxes, live/pull policy, rebase, conflicts
│   └── forge-runtime/    Ch.28  the shipping runtime binary
├── tools/
│   ├── forge-cli/        Ch.30
│   ├── forge-bake/       Ch.26
│   ├── forge-farm/       Ch.26  the LAN daemon
│   └── forge-server/     Ch.37  the multi-user host: baseline + N sandboxes + N sessions
├── tests/
│   ├── determinism/       Ch.3, Ch.30
│   ├── liveness/          Ch.30
│   ├── perf/              Ch.29, Ch.30
│   ├── platform/          Ch.30   Windows/Linux parity (I18)
│   ├── collab/            Ch.30   sandbox isolation, rebase, role enforcement
│   ├── plugin/  store/  preset/   Ch.30
│   └── e2e/               Ch.30   serial leg — NOT part of `just check`
├── xtask/                 Ch.30   gate, dod, plan-coverage, licence-audit
├── presets/               Ch.31  2d/ 3d/ planetary/ — DATA, not code
├── plugins/               Ch.32  first-party plugins; each one proves I16
├── docs/
│   ├── defects.md         W8 — the one allocator
│   └── adr/               Ch.30
└── justfile               Ch.30
```

---
---

# Chapter 1 — System Contracts

Everything in this chapter is pinned. Changing a signature here is a plan amendment, not
a refactor, and invalidates every chapter that consumes it.

## 1.1 The dependency spine

Crates may only depend **downward**. `xtask` enforces it.

```
  forge-num          (no forge deps — leaf)
      ↑
  forge-seed ── forge-frames
      ↑              ↑
      └──── forge-core ────┐
                 ↑          │
          forge-reflect    │
                 ↑          │
            forge-cmd      │
                 ↑          │
   ┌─────────────┴──────────┴────────────────────┐
   │  forge-planet  forge-astro  forge-gen    │  ← the universe layer
   │  forge-climate forge-pcg    forge-phys   │
   └──────────────────┬──────────────────────────┘
                      ↑
        forge-gpu → forge-render
                      ↑
   forge-ui  forge-mcp  forge-script  forge-graph
                      ↑
        forge-editor        forge-runtime
```

**`forge-num` depends on nothing, not even `std::f64` transcendentals.** That is the
whole point of Chapter 3.

## 1.2 Error contract

One error enum per crate, all convertible into `forge_core::Error`, every variant
carrying a stable `ErrorCode`. Codes are allocated in `docs/error-codes.md` — one
allocator, same rule as defects (W8).

```rust
#[non_exhaustive]
pub struct Error {
    pub code: ErrorCode,          // stable, greppable, never reused
    pub ctx: SmallVec<[Ctx; 4]>,  // breadcrumbs, cheap
    pub source: Option<Box<dyn StdError + Send + Sync>>,
}
```

Rules: no `unwrap` outside tests and `main`; no `panic!` reachable from a command handler
(a panicking command must be caught at the bus boundary and turned into a `Rejection`, or
one bad agent call takes down the editor); `#[deny(clippy::unwrap_used)]` crate-wide
outside `#[cfg(test)]`.

## 1.3 The four cross-cutting types

Every crate in the tree uses these and defines no alternative.

```rust
// forge-frames
pub struct FrameId(pub u32);
pub struct BodyId(pub u32);
pub struct Tick(pub i64);            // canonical time, monotonic from epoch

// forge-seed
pub struct Seed(u64);

// forge-core
pub struct EntityId(/* bevy_ecs Entity */);
```

`BodyId` is a **u32 partition key** (Foundations). Chunk keys are
`(BodyId, u64 morton)` — never a single int64.

## 1.4 Profiles

A profile is a property of a **region**, never of a process or a build. One binary.

```rust
pub enum Profile { Surface, Cave, Crowd, Sparse, Fluid }
```

Selected by **measurement, not label** (Foundations): sky-occlusion ratio, mutual-visibility
count, loaded-surface-area ÷ volume, dispersion. **With hysteresis** — without it, a
player standing in a cave mouth thrashes the profile.

**Invariant I5 is the hard part**: a profile may change cost, never outcome. Guard:
`test_profile_equivalence` runs the same seeded scenario under every profile and asserts
bit-identical gameplay-visible results.

## 1.5 Repo conventions

- Rust edition 2024. MSRV pinned in `rust-toolchain.toml`, bumped deliberately.
- `#![forbid(unsafe_code)]` by default. Exceptions: `forge-gpu`, `forge-script`,
  `forge-jobs`. Each `unsafe` block carries a `// SAFETY:` naming the invariant *and the
  thing it is serialised against* (W7).
- No `f32` in any crate below `forge-render`. Enforced by a lint (Ch.3).
- Public items reachable from a game carry `#[forge_api]` (Ch.6).

---

# Chapter 2 — Coordinates, Frames & Time

**This chapter is the engine.** Everything else is downstream of it. It is written first,
frozen first, and changed only by plan amendment.

## 2.1 Why there is no global position type

| Coordinate | Magnitude | `f32` ulp | `f64` ulp |
|---|---|---|---|
| Planet surface from centre | 6.37e6 m | **0.38 m** | 7e-10 m |
| 1 AU | 1.5e11 m | 8,900 m | 1.7e-5 m |
| 1 light year | 9.46e15 m | 1.1e9 m | 1.9 m |
| Galactic radius (~50 kly) | 4.7e20 m | — | **6.6e4 m** |

`f32` quantises at 0.38 m on a planet surface — coarser than the Foundations' 0.25 m voxel. `f64`
is fine to interplanetary scale and **fails at galactic scale**, which is why the answer
is not "use f64" but "use a frame stack." A galactic-frame `f64` local offset is fine
because nothing is ever *at* 4.7e20 m in the frame it is being simulated in.

## 2.2 The types

```rust
/// The ONLY legal way to express a position anywhere in the engine.
/// There is no `WorldPos`, no `GlobalPos`, and no bare DVec3 in any public signature.
#[derive(Clone, Copy, Reflect)]
pub struct FramePos { pub frame: FrameId, pub local: DVec3 }

#[derive(Clone, Copy, Reflect)]
pub struct FrameVel { pub frame: FrameId, pub local: DVec3 }

pub enum FrameKind {
    Galactic,        // constant — see §2.6
    System,          // star-centric inertial
    BodyInertial,    // BCI  — moons, rings, orbiting stations
    BodyFixed,       // BCBF — the lattice, and ~100% of surface gameplay
}

pub struct FrameTransform {
    pub translation: DVec3,   // of this frame's origin in its parent
    pub rotation:    DQuat,
    pub ang_vel:     DVec3,   // ω — needed by rebase, and by nothing else
    pub lin_vel:     DVec3,   // v_body
}

pub trait FrameTree: Send + Sync {
    fn parent(&self, f: FrameId) -> Option<FrameId>;
    fn kind(&self, f: FrameId) -> FrameKind;
    fn body(&self, f: FrameId) -> Option<BodyId>;
    /// Pure. Deterministic. O(1) at any t, forwards or backwards.
    fn transform(&self, f: FrameId, t: Tick) -> FrameTransform;
}
```

## 2.3 `rebase` — the highest-risk function in the engine

The Foundations call this "the highest-risk four lines in the feature," scattered across client
and server. **In an engine we own it is one function, called from everywhere, with a
property test.** That is the single clearest win of owning the stack.

```rust
/// Move a position and velocity into a different frame at time `t`.
///
/// Miss `lin_vel` and a player leaving a surface arrives in the system frame at 30 km/s.
/// Miss `ω × p` and they arrive at 463 m/s. Both bugs are silent and both are shipped
/// by every team that writes this inline.
pub fn rebase(
    tree: &impl FrameTree,
    pos: FramePos, vel: FrameVel,
    to: FrameId, t: Tick,
) -> (FramePos, FrameVel);
```

**Guards** (`tests/frames/`):
- `test_rebase_roundtrip` — `rebase(rebase(x, B), A) == x` to 1e-9 relative, over a
  property-test sweep of frame pairs, times, and positions.
- `test_rebase_velocity_terms` — **non-vacuous positive control (W2)**: a mutant that
  drops `lin_vel` must fail, and a mutant that drops `ω × p` must fail, *differently*.
  Assert both mutants fail and that they fail on different assertions — otherwise one
  test is covering for the other.
- `test_no_bare_position` (I1) — AST lint: no `pub fn` in the tree takes or returns a
  bare `DVec3` in a position-shaped parameter. Positions are `FramePos`. The lint's
  allow-list is in the repo, is short, and every entry has a comment.

## 2.4 Rendering — the last mile

The renderer resolves everything into **camera frame in f64**, then emits **f32
camera-relative**. There is no other path to the GPU. `forge-render` is the only crate
permitted to hold an `f32` position, and it holds only camera-relative ones.

**Multi-frustum depth.** One depth range cannot span 0.01 m to 1e13 m. Four shells,
rendered back-to-front with independent depth clears, reversed-Z within each:

| Shell | Range | Contents |
|---|---|---|
| Near | 0.01 – 1e3 m | characters, interiors, held objects |
| Mid | 1e3 – 1e7 m | terrain, the body you are on, close moons |
| Far | 1e7 – 1e13 m | other bodies in-system, stations |
| Sky | — | point sprites, star field, other galaxies |

Distant bodies are **point sprites with correct position, brightness and phase**, not
low-poly globes: a planet at 0.3 AU is ~1 px at 4K/60°. Detail pyramids load on approach.

## 2.5 Canonical time

One authoritative `Tick` (`i64`, from epoch) owned by the global-services layer. Clients
estimate it with offset + drift.

**A client's wall clock never feeds a physics-relevant ephemeris evaluation.** That seam
is trivially exploitable. Because `t` is canonical, eclipses and conjunctions are
simultaneous for everyone with zero coordination.

## 2.6 Galactic frames are constant

Any body whose orbital period exceeds a human lifetime is a **constant, not a simulation
entity**. The Milky Way's galactic year is ~225–250 Myr; a century of live operation
translates the whole system ~4.8 AU, which is a rigid translation of everything
interactable and therefore unobservable by definition. Galaxies are first-class bodies in
the same tables with the same channel type and a mean motion of effectively zero.

## 2.7 An open question inherited from the Foundations

Is inter-galactic a **fourth** frame tier? The Foundations lean yes — it keeps each frame
numerically well-conditioned and makes the crossing *a place*, which mirrors the
bridge/ship structure. **Engine recommendation: build the frame tree with arbitrary
depth and no hard-coded tier count**, which makes this a content decision rather than an
engine one. Record the answer in the ledger; do not bake four kinds into a Rust enum.
`FrameKind` above is a *hint for physics*, not a depth limit.

---

# Chapter 3 — Determinism & the Math Contract

A seed-generated universe is worthless if two machines disagree about it. This chapter is
the reason that does not happen.

## 3.1 The three failure modes, and why each is not obvious

**1. `f32` mantissa.** 24 bits. At 0.25 m across a 6,371 km radius you need 25 bits of
integer range. Any `f32` path — GPU compute, a material graph, SIMD with fast-math —
*will* diverge from the CPU. The Foundations already states the fix:

> **Do the noise basis in integer / fixed-point voxel coordinates, never float world
> position.** Hash integer voxel coords → derive the float you feed the noise.

**2. Convergence loops.** `while err > eps` takes a different iteration count on a
different compiler, platform, or optimisation level. The Foundations already ban it for
Kepler: **exactly 5 Newton steps in `double`, never a loop.** Generalise it: *no
iteration count in the generation pipeline may depend on a computed value.*

**3. The one that will bite hardest, and is not in the notes yet: platform `libm`.**
`sin`, `cos`, `exp`, `pow`, `atan2` are **not** IEEE-754-specified to a single result.
glibc, Apple's libm, and MSVC's CRT differ by 1–2 ulp on the same input, and they differ
between x86-64 and aarch64 on the *same* OS. A generator that calls `f64::sin` is
non-deterministic across platforms, silently, and only in the low bits — which is exactly
where a hash amplifies it into a completely different chunk.

> **Ruling: `forge-num` vendors its own transcendentals for every function reachable
> from generation.** Correctly-rounded or at minimum bit-reproducible, with the
> implementation in-tree and versioned. The cost is a few hundred lines and some
> nanoseconds. The alternative is a class of bug that reproduces on one developer's
> machine and nowhere else.

## 3.2 The contract

```rust
// forge-num — depends on nothing.
pub type Real = f64;

pub mod det {
    /// Bit-reproducible across x86-64, aarch64, glibc, musl, Apple, MSVC.
    pub fn sin(x: f64) -> f64;
    pub fn cos(x: f64) -> f64;
    pub fn exp(x: f64) -> f64;
    pub fn ln(x: f64)  -> f64;
    pub fn pow(x: f64, y: f64) -> f64;
    pub fn atan2(y: f64, x: f64) -> f64;
    pub fn sqrt(x: f64) -> f64;  // IEEE-exact; the one we may forward
}

/// Exactly `N` Newton steps. No convergence test. `N` is a const generic so the
/// iteration count is in the type, where nobody can make it data-dependent.
pub fn newton<const N: usize>(f: impl Fn(f64) -> (f64, f64), x0: f64) -> f64;

/// The noise basis. Takes INTEGERS. There is no float overload and there never will be.
pub fn hash3(x: i64, y: i64, z: i64, seed: Seed) -> u64;
pub fn value_noise_i(p: IVec3, seed: Seed) -> f64;
pub fn simplex_i(p: IVec3, scale_log2: i8, seed: Seed) -> f64;
```

## 3.3 Build-level rules

- `-ffast-math` equivalents are forbidden. No `-C target-feature` that changes FP
  semantics. Pinned in `.cargo/config.toml` and asserted by `xtask`.
- FMA contraction is **explicit only**: `a.mul_add(b, c)` where intended; never left to
  the backend. Rust does not auto-contract today, but pin it rather than rely on it.
- No `-C target-cpu=native` in any profile that produces artefacts (it changes SIMD
  lowering and therefore reduction order).
- Parallel reductions in generation use a **fixed-order tree reduction**, never
  `rayon::sum()` — floating-point addition is not associative and a work-stealing
  reduction order is not deterministic.

## 3.4 The gate — the one that pays for this whole chapter

`tests/determinism/test_cross_platform_hash.rs`, run in CI on **macOS-aarch64,
ubuntu-x86_64, and windows-x86_64** as three legs:

1. Generate a fixed corpus: 256 chunks drawn from 8 bodies at 4 LODs, plus 64 climate
   cells, plus a 1,000-body system ephemeris sampled at 128 ticks.
2. Hash each with BLAKE3.
3. Compare against `tests/determinism/golden.txt`, committed.
4. **Positive control (W2):** a `#[cfg(feature = "mutate-det")]` build perturbs one
   noise octave by 1 ulp and the job asserts the comparison **fails**. A determinism gate
   that cannot demonstrate failure is decoration.

A golden change requires a plan amendment and a one-line reason in `docs/adr/`.

## 3.5 Client/server divergence is *detected*, never assumed away

The Foundations' rule, adopted verbatim: for an unedited chunk the server sends `"baseline, hash =
X"`; the client generates locally and verifies. Bandwidth is ~zero in the common case
**and float-divergence surfaces as a mismatch rather than as a wallhack-adjacent bug.**
Keep this even after §3.4 is green. The gate proves the platforms we test; the hash check
covers the ones a player brings.

---

# Chapter 4 — Seed Algebra

## 4.1 The rule

A universe is one `u64`. Everything else **derives**.

```rust
impl Seed {
    /// Deterministic, associative-free, collision-resistant descent.
    /// `tag` is a compile-time string so the derivation path is greppable.
    pub fn child(self, tag: &'static str, index: u64) -> Seed {
        Seed(splitmix64(self.0 ^ fnv1a(tag).rotate_left(17) ^ splitmix64(index)))
    }
}
```

`universe.child("galaxy", 3).child("system", 118).child("body", 2).child("region", m)` is
computable in O(depth) from nothing but the root seed and the path. **No seed is ever
stored, and no seed is ever mutated** (I3).

## 4.2 Why not a PRNG stream

Because a stream has *position*, and position is state. Two workers generating the same
chunk must agree without communicating; a stream forces them to agree on how many numbers
they have drawn. Derivation has no such requirement — which is exactly what makes the Foundations'
Layer 1 stateless.

**Corollary, and it is a strong one:** any generator that consumes randomness *in a
loop whose length depends on data* has smuggled a stream back in. Guard:
`test_seed_algebra` asserts that every registered generator produces identical output
when its inputs are evaluated in a different order.

## 4.3 Provenance

Every generated artefact carries its derivation path in debug builds
(`SeedPath: universe/galaxy:3/system:118/body:2/region:14883`). This is what makes
"why is this mountain here" answerable, makes bug reports reproducible from a string, and
gives the MCP `observe` tool something genuinely useful to return.

---

# Chapter 5 — ECS Core & the Regionized Scheduler

## 5.1 Bevy: take the crates, not the framework

**Ratified recommendation:** depend on `bevy_ecs`, `bevy_reflect`, `bevy_app`,
`bevy_tasks`, `bevy_asset` as libraries. **Do not depend on `bevy` or `bevy_render`.**

Why take them:
- `bevy_ecs` is the best archetypal ECS in Rust: change detection, system parameter
  inference, and — critically — **automatic parallel scheduling from borrow
  signatures**, which is 80% of the regionized scheduler already built.
- `bevy_reflect` is the keystone of Chapter 6. It is why one annotation can produce four
  outputs.
- They are `MIT OR Apache-2.0` and independently published.

Why not take the framework:
- `bevy_render` assumes `f32` transforms and a single adapter. Both assumptions are wrong
  for us and neither is a patch.
- Version churn is real. Vendoring-and-pinning the five crates we use is a bounded,
  budgeted cost; tracking the whole framework is not.

**Risk S6 in Appendix B covers the churn. Budget it; do not pretend it is zero.**

## 5.2 Worlds are cheap, and that changes the Foundations' sparse case

A `bevy_ecs::World` with no entities is on the order of kilobytes. The Foundations spend a
section on multi-UWorld packing to amortise UE's ~2.5 GB process baseline across 50 solo
players (135 GB → 12.5 GB, a 10× win, described as deciding "whether the sparse case is
affordable at all").

**In Forge that problem does not exist.** 50 solo players is 50 worlds and some tens of
megabytes. The *packing* remains useful as a scheduling convenience; the *cliff* is gone.
Update the cost model accordingly and do not port the machinery that existed to climb it.

## 5.3 The regionized scheduler — the headline feature

The Foundations, on Folia:

> regionized multithreading *inside one process*. No network hop, no ghost sync, no
> cross-process handoff. **Strictly better than projection — and UE cannot do it.**

Build it. The design:

```rust
/// A region owns a disjoint set of entities and may mutate only those.
/// Cross-region effects are messages, applied at a barrier.
pub struct Region { id: RegionId, frame: FrameId, profile: Profile, /* ... */ }

pub trait RegionSystem {
    /// Declares what it touches. The scheduler proves disjointness before running.
    fn access(&self) -> RegionAccess;
    fn run(&mut self, r: &mut RegionView<'_>, out: &mut Outbox);
}
```

- Regions are derived from the **interaction graph**, min-cut along the seam where
  entities interact least — the Foundations' bubble-split rule, applied to threads instead of
  processes.
- **Merge on proximity, split on load** (Foundations). Two regions within interaction range
  merge; the boundary ceases to exist rather than needing a protocol.
- **Two regions may only merge if they share a frame** (a Foundations correction to invariant 6).
  One comparison; kills a bug class in advance.
- The borrow checker is the enforcement mechanism: `RegionView` hands out `&mut` only to
  entities the region owns. **This is the thing Rust buys us** and the reason the whole
  project is worth the rewrite.

Process-level projection (ghost replicas) stays in the plan as Ch.25, **staged last**,
for the case where one machine runs out. The Foundations' arithmetic stands: projection
parallelises work, it does not reduce it, and it costs RAM.

## 5.4 Guards

- `test_region_disjointness` — property test: no two concurrently-running regions hold
  overlapping entity sets. Fuzz the merge/split sequence.
- `test_region_determinism` — the same seeded scenario produces identical results under
  1, 2, 4, and 16 regions. **This is the I5 guard applied to threading** and is the test
  that will find the real bugs.
- `test_thread_boundary` — every shared structure names what it is serialised *against*
  (W7). A real instance: a voxel map integrated from an async loop and queried from a dispatch
  thread, with a "serialized" claim that meant "against each other," not "against the event
  loop." It crashed under load and hid for the length of the project.

---

# Chapter 6 — Reflection, Annotation & the Four Outputs

**This is the elegance keystone. One annotation, four outputs.** It is what makes the
blueprint system, the inspector, the serialiser and the MCP surface stay in sync forever
instead of drifting for the life of the project.

```rust
#[forge_api]
/// Applies an impulse to a body, in the body's own frame.
pub fn apply_impulse(
    #[forge(entity)] target: EntityId,
    #[forge(units = "N·s")] impulse: FrameVel,
) -> Result<(), Error> { … }
```

The proc-macro emits, from that single annotation:

| Output | Consumed by | Chapter |
|---|---|---|
| A `bevy_reflect` `TypeInfo` registration | inspector widgets, serialiser | 21, 8 |
| A **blueprint node descriptor** (pins, types, docs, purity) | the graph editor | 24 |
| A **JSON Schema fragment** | the MCP `schema://` resource | 22 |
| A **command variant** if it mutates | the bus, undo, provenance | 7 |

**Rules:**
- Anything a game can call is `#[forge_api]`. A public item that is not is an internal
  detail and the API-surface test will say so.
- Doc comments are the node tooltip *and* the MCP tool description. Write them for both
  audiences, because there is only one.
- Unit annotations are not cosmetic: the graph editor refuses to connect an `N·s` pin to
  an `m/s` pin, and the MCP schema carries the unit so an agent does not guess.

**Guard:** `test_four_outputs_agree` — for every `#[forge_api]` item, assert that the
node descriptor, the schema fragment, and the reflect registration describe the same
arity and the same types. A drift here is silent and catastrophic.

---

# Chapter 7 — The Command Bus, Undo & Provenance

**Invariant I7 lives here and it is the reason agent access can be *complete* rather than
bolted on.**

## 7.1 Shape

The editor is a **headless core** plus clients. The UI is a client. MCP is a client.
Scripts, tests, CI and the CLI are clients. There is exactly one way to change project
state.

```rust
pub struct CommandEnvelope {
    pub id:     CommandId,
    pub txn:    TxnId,
    pub issuer: Issuer,          // Human{user} | Agent{session,tool} | Script{path} | Test
    pub cmd:    EditorCommand,   // #[derive(Reflect, Serialize, Deserialize)]
}

pub trait CommandSink {
    /// Compute the effect without applying it. Total; never mutates.
    fn dry_run(&self, e: &CommandEnvelope) -> Result<Diff, Rejection>;
    fn apply(&mut self, e: CommandEnvelope) -> Result<Applied, Rejection>;
    fn undo(&mut self, txn: TxnId) -> Result<Applied, Rejection>;
}
```

## 7.2 Why this is the decision that makes the MCP work

Every engine that adds agent control late ends up with two code paths: the one the UI
uses and the thin, incomplete one the agent uses. Then the agent path lags forever, and
every new editor feature is invisible to agents until somebody remembers.

Inverting it — **the UI has no privileged path** — means new editor features are
automatically agent-accessible, automatically undoable, automatically scriptable, and
automatically testable. The cost is paid once, at M2, and it is small then.

## 7.3 Guards

- `test_command_liveness` (I7). Walks the AST of `forge-editor`: every UI event handler
  that reaches a mutation must terminate in `CommandSink::apply`. **Resolved through the
  import graph, not by name (W3).**
- `test_command_contract` (I8). Every `EditorCommand` variant: has a `dry_run` that does
  not mutate (checked by a snapshot hash before/after), produces an undo entry, and
  round-trips through serde.
- `test_undo_fuzz`. Random command sequences, then full undo, then assert the project
  state hashes equal to the starting state.
- **Positive control (W2):** a mutation build adds a UI handler that writes state
  directly; `test_command_liveness` must fail on it, and the CI job asserts that it does.

---

# Chapter 8 — Asset System, VFS & Import  *(BRIEF)*

**Contract:** content-addressed store; stable `AssetId` independent of path; async load
with typed handles; hot reload on file change; importers are `#[forge_api]` plugins.

**Decisions:** glTF 2.0 and USD are first-class; FBX via `ufbx` (MIT) only. Textures
transcode to KTX2/Basis at import. `.forge` scene format is RON-over-reflect, diffable,
and merge-friendly — because "my scene file is an unmergeable binary blob" is a real
reason teams leave Unity.

**Expansion brief:** enumerate importers; define the import-settings sidecar; specify the
dependency graph and reimport invalidation; specify how a *generated* asset (a chunk, a
scattered instance) participates — it has a `SeedPath`, not a file, and the asset system
must treat those uniformly or the PCG layer will grow a parallel universe of its own.

**DoD seed:** import a 500-object glTF scene, mutate the source, see it hot-reload,
undo the import, and drive all of it from MCP.

---

# Chapter 9 — GPU Device Layer & Render Graph  *(BRIEF)*

**Contract:** `wgpu` is the only graphics API surface. An **adapter pool**, not an
adapter — `Vec<Device>` from day one, even when the length is 1, because retrofitting
plurality is the expensive version.

**Decisions:** WGSL only, compiled by `naga`; bindless where available with a bound
fallback; a declarative render graph with automatic barrier/alias computation; every pass
declares reads/writes (same discipline as `RegionSystem`, deliberately).

**Expansion brief:** resource lifetime and aliasing; transient allocator; cross-adapter
transfer primitives (see Ch.26 and **spike S1**); shader hot reload; a `wgpu-hal`
passthrough escape hatch for Vulkan external memory, isolated behind a feature so the
portable path never depends on it.

**DoD seed:** the same frame renders identically on 1 adapter and on 2, with the second
adapter doing only compute.

---

# Chapter 10 — Renderer: Scene, Materials, Lighting, GI  *(BRIEF)*

**Contract:** clustered forward+ as the baseline; PBR metal-rough with a layered material
model; virtual shadow maps; reversed-Z per shell (Ch.2 §2.4).

**Decisions:** ship a *good* renderer, not a novel one. GI staged: (1) baked irradiance
volumes + SSAO, (2) screen-space GI + reflections, (3) a surfel/probe dynamic GI, (4)
hardware ray tracing where present. **Do not start at (4).** A path tracer for reference
renders is cheap once the material model is settled and pays for itself in validation.

**Expansion brief:** material graph → WGSL codegen (shares the Ch.24 IR — do not build
two graph compilers); shadow atlas policy at planetary ranges where cascades break down;
decals; transparency; post stack.

**Open risk:** virtualised geometry (Nanite-equivalent) is **Ch.26/M6 and spike S5.**
Note that for *procedural terrain we do not need it* — analytic LOD already exists. It is
an *asset* feature.

---

# Chapter 11 — Planetary Rendering  *(BRIEF)*

**Contract:** atmosphere, ocean, cloud and sky are **driven by the same fields the
simulation uses** (Ch.15), not by hand-authored parameters. This is the differentiator:
if the climate model says the air is thin and cold, the sky is thin and cold, without an
artist.

**Decisions:** Bruneton-style precomputed multiple-scattering LUTs, parameterised per
body by composition and scale height from Ch.16's retention model; volumetric clouds
seeded by Ch.15's humidity/convection fields; ocean via spectral (FFT) waves with fetch
and wind from the weather layer; aerial perspective applied across all four shells.

**Expansion brief:** LUT parameterisation and cost; night-side scattering; terminator
quality; eclipse shadowing (free — the ephemeris already knows); ring shadows; from-orbit
cloud continuity as a player descends through shell boundaries. **That last one is the
hard one** and deserves its own section.

**DoD seed:** fly from 2,000 km orbit to standing on the ground in one continuous shot,
no loading screen, no visible shell transition, correct sky the whole way.

---

# Chapter 12 — Body Representation  *(BRIEF)*

**This is where the Foundations' hardest-won decision lives. Get it right.**

```rust
pub trait BodyRepr: Send + Sync {
    fn sample(&self, p: FramePos, seed: Seed) -> Sample;   // density/material/fields
    fn extract(&self, chunk: ChunkKey) -> Option<Mesh>;    // None for fields
    fn collide(&self, region: Aabb, lod: Lod) -> Collider;
}
```

Three implementations, chosen **per body**:

| Impl | For | Why |
|---|---|---|
| `UnwarpedLattice` | mineable/cave-bearing bodies | the Foundations' choice, see below |
| `CubeSphereQuadtree` | heightfield-only bodies | cheap, great UVs, no 3D cost |
| `FluidField` | stars, gas giants | drops mesh+collision entirely |

**The Foundations explicitly reject cube-sphere projection for volumetric bodies** and the reason
must be carried into the engine verbatim: *warped grids give anisotropic voxels — a "1
voxel" cube is a different physical size at face centre than at corner — which breaks
mining volume, ore density, and collision consistency.* Projection is for heightfield
planets. Volumetric bodies get a **uniform unwarped cubic lattice**, with the sphere as
an SDF carved out of it. The cubes are *knives*: pure spatial partition, invisible to
generation.

Carried constants from the Foundations (per-body config, not global — corrected §7 inv.1):
chunk = 32 m = 128³ voxels @ 0.25 m; key `(BodyId u32, morton u64)`; optional 16.384 km
cell = the top Morton bits (65,536 voxels/side = exactly three `u16`).

> **⚠️ The trap the Foundations already hit once.** A 16.384 km cell holds 512³ = 134 M chunk slots.
> A dense index at 64 B/record is 8.6 GB — a whole RAM budget spent indexing chunks that
> do not exist. **The chunk index must be a sparse hash of generated-or-edited chunks
> only. Never an array.** The same rule applies to promoted ring/asteroid particles
> (Ch.16), which is the identical trap one abstraction level up.

**`FluidField` is counterintuitively the cheapest profile.** A star's surface area is
~12,000× Earth's and would be unshippable meshed — but it has no walkable surface, so it
is plausibly the cheapest inhabited body in the engine (~10–20 MB/viewer against
128–448 MB). Its interior is smooth and radial: a near-trivial pure function. Depth is
the axis; density, temperature and radiation are fields you sample, not solids you carve.

---

# Chapter 13 — Terrain Generation  *(BRIEF)*

**The split that makes everything else possible** (Foundations): erosion, rivers, and cave
networks are **not local**. Hydraulic erosion needs upstream state; a river needs to know
where the ocean is; a cave may have started 40 km away. None decompose into
independently-generatable chunks.

**Therefore split the generator by scale:**

1. **Global solve, once, at low resolution** on the whole-body field: tectonics, erosion,
   flow accumulation, river networks, lakes (priority-flood depression fill), cave trunk
   topology, biome assignment. Runs on GPU in seconds; output is a small baked field set.
2. **Local evaluation** stays a pure function: sample the coarse fields at this position,
   add detail noise conditioned on macro slope and flow accumulation, done. Caves branch
   procedurally off globally-solved trunks.

Skipping this yields either chunk-seam artefacts or a generator that must load its
neighbours — which defeats the entire stateless architecture.

**Expansion brief:** the generator graph (nodes, types, GPU lowering — reuse Ch.24 IR);
exact list of what is solved globally vs locally *(a Foundations open question — settle it here)*;
the edit-delta store (`redb` or LSM, keyed `(BodyId, morton)`); LOD extraction
(Transvoxel or surface nets — pick one and write why); the erosion kernel.

**DoD seed:** a river that flows downhill from a mountain to a sea, on a planet, from a
seed, bit-identically on three platforms, with a delta in the middle of it.

---

# Chapter 14 — PCG Framework  *(BRIEF)*

**Contract:** placement is a **pure function of position and seed**, never a stored list.
Instances are promoted to entities on proximity and demoted on departure — the Foundations'
field-with-promotion pattern (§11), applied to vegetation, rocks, and props.

**Decisions:** the PCG graph is the Ch.24 IR again (three graph compilers would be two
too many); rules are declarative and query the same fields as climate and hydrology, so a
forest appears where the climate model says a forest appears. Density functions are
evaluated on GPU; promotion is CPU and bounded by radius.

**Expansion brief:** biome→asset binding; slope/altitude/moisture rules; deterministic
position-hash instantiation; **touching an instance makes it real** (it gains an identity
and a KV row and becomes persistent, visible Layer-1 state); scatter LOD and impostors.

---

# Chapter 15 — Climate & Weather  *(BRIEF)*

**The highest value-per-line subsystem in the engine, and no other engine has it.**

**Contract:** a closed-form **energy-balance model**, not a GCM. Inputs: stellar spectrum
and luminosity, orbital elements, obliquity, rotation rate, albedo, land/sea mask,
atmospheric composition and pressure. Outputs: temperature, precipitation, wind, humidity
fields — good enough to be Köppen-classifiable and to respond *correctly* to changes.

**What falls out for free and is worth stating, because it is why this is cheap:**
- Seasons, from axial tilt + orbital position (Foundations). No new system.
- Milankovitch cycles, from eccentricity and obliquity variation.
- Hadley/Ferrel/Polar cell **count** from rotation rate via the Coriolis parameter — so a
  slow-rotating world gets one cell and a fast one gets five, and its climate bands look
  right without anyone deciding they should.
- Orographic precipitation and rain shadows, from prevailing wind × terrain slope. Deserts
  appear on the correct side of mountains because the physics put them there.
- Tidal-locked bodies get an eyeball-ocean pattern, correctly, with no special case.

**Weather** is a stochastic process conditioned on the climatology (a Markov chain over
weather states per cell) plus a local shallow-water/advection solve inside the active
region for dynamics you can see. **No global GCM ever.**

**Expansion brief:** the EBM discretisation and its diffusion coefficients; ocean heat
transport as tuned diffusion; the weather state machine; how sun elevation is sampled
(the Foundations: once per inhabited body per climate tick at 0.5 Hz, then **one dot product** per
character — 1.25 µs/s of wall clock for 10 bodies, not findable on a flame graph);
**invariant 4's phase**: unattended crops grow with *accumulated daylight*, integrated
closed-form or by one-day quadrature, never ticked.

---

# Chapter 16 — Astrophysics  *(BRIEF)*

**Contract:** "the same features as the real one" means **closed-form astrophysics with
real statistics**, not simulation. Every relation below is O(1), deterministic, and
defensible.

| Layer | Method |
|---|---|
| Galaxy | density-wave spiral arms; metallicity gradient by galactocentric radius and age |
| Star | IMF (Kroupa/Chabrier) sampling → mass; mass–luminosity–radius relations → L, R, T_eff; spectral class |
| Habitable zone | conservative and optimistic bounds from L and T_eff |
| System | occurrence-rate-driven planet formation (Kepler/TESS statistics), **not** an accretion sim |
| Stability | mutual Hill-radius spacing filter (≥ 8–10 R_H) — rejects systems that would not survive |
| Atmosphere | Jeans escape parameter λ = v_esc²/v_thermal² decides which gases a body retains |
| Rings | Roche limit; field-with-promotion for particles (Ch.12 sparse rule) |
| Rotation | tidal-locking timescale vs system age decides locked / slow / fast |
| Motion | **six Keplerian elements + mean motion. Never integrate gravity.** |

**Kepler, exactly as the Foundations ratified it:** solve with a **fixed 5 Newton steps in `double`,
never a `while (err > eps)` loop** — a convergence-threshold loop takes different
iteration counts on different compilers and platforms, the same bug class as the
integer-noise-basis rule. Fixed count is accurate far beyond need for e < 0.5. Cost:
~50 bodies × ~250 ns ≈ 12.5 µs per evaluation.

**Why never integrate:** determinism (an integrator diverges between client, server, and
restart) and random access (a player logs off for a month; a crop needs 40 days of
accumulated daylight; a ship plans a transfer). `f(t)` is O(1) at any `t`, forwards or
backwards. An integrator gives you "replay from epoch."

**Expansion brief:** the generator chain and its statistics sources; moon systems; binary
stars (and whether to allow them — they complicate the frame tree); the ephemeris cache;
how a body's generated parameters feed Ch.15 and Ch.11 without a second source of truth.

---

# Chapter 17 — Physics & Simulation Regions  *(BRIEF)*

**Contract:** `avian` (generic over `f32`/`f64` — this is why it is preferred over
`rapier`) inside a region whose origin is frame-local. **Nothing is ever simulated at
orbital velocity**: orbital motion is `f(t)`, and physics only ever sees slow *relative*
motion inside a region. That single decision deletes the CCD-at-7.8-km/s problem that
kills naive space games.

**Collision LOD rings** carried from the Foundations — doubling radius while doubling voxel size
holds triangle count constant, so every ring costs the same (~16 MB, 206k tris). Seven
rings reach 2 km for ~112 MB/player. **Full 0.25 m collision is affordable only within
~32 m**; pushing it to 128 m is 3.3 M triangles and ~264 MB for one player.

**Geometry residency and gameplay residency are two independent radii. Do not force them
to match.**

**Expansion brief:** the region↔physics-world binding; determinism (fixed step, fixed
iteration counts, stable contact ordering — a solver whose island order depends on
insertion order is not deterministic); character controller; vehicles; cloth; destruction
(defer past M6); the frame-crossing handoff, which is Ch.2's `rebase` plus a physics-world
migration and is the **highest-risk sequence in the runtime**.

---

# Chapters 18–21, 25, 27–29 — Parity Subsystems  *(BRIEF)*

These are the "every engine has one" category. Each gets a full chapter at its milestone.
Contracts and the non-obvious decisions only:

**Ch.18 Navigation.** Recast/Detour-class navmesh, but **generated per-region on demand
from the same density field**, never baked per-level — there are no levels. Long-range
pathing is a hierarchical graph over the coarse field. Flow fields for crowds.

**Ch.19 Animation.** Skeleton, state machine, 1D/2D blend spaces, per-bone masking, root
motion, IK. **Retargeting is the acquisition feature** — Unity's Humanoid Avatar is the
single most-cited reason animators prefer Unity, and it is a solvable, bounded problem:
a skeleton profile + automatic bone mapping + a retarget solver. Motion matching (pose
search) is M7. Animate-any-property (Godot's `AnimationPlayer` model) is **better than
UE's and Unity's split**, costs little on top of reflection (Ch.6), and should be the
design: one timeline that keys any reflected property on any entity.

**Ch.20 Audio.** Buses, spatialisation (HRTF), occlusion from the same fields, adaptive
music. Wwise/FMOD as *plugins*, never as a dependency (licence).

**Ch.21 UI & Editor Shell.** One widget layer used by both the editor and games — Godot's
"the editor is built with the engine" property, which is what makes tool-building
plausible for users. `egui` for the editor in M2 as a deliberate stopgap; a retained
layout/styling system for shipping games. The editor shell is a **client of `forge-cmd`
and nothing more** (I7).

**Ch.25 Networking.** Transport, snapshot replication, client prediction, lag
compensation. **Adopt the Foundations' invariant 6 on day one:** exactly one authority per entity;
damage resolved by the **target's** authority, not the shooter's — the shooter sends a
shot record (ray, timestamp, weapon) and the target's authority runs lag compensation
against its own history. Letting the shooter decide means trusting a stale ghost and
surfaces as unreproducible "I was behind cover." Cheap now, brutal later.
**Interest caps (K≈50) come before anything clever** — they linearise the N² and are the
largest single win available.

**Ch.27 Platform HAL.** Traits only, no platform code. **Windows and Linux are the targets
(I18); macOS is explicitly out of scope** — nobody on the project runs it as a target, and
a platform nobody tests is a platform that is broken. The HAL means someone who cares can
add it later without a rewrite, which is the right way to carry an unfunded platform.
Web/iOS/Android are best-effort and staged. Console backends are implemented privately by
licensed porters against the same traits — Godot's answer, and the correct one here.

**Ch.28 Gameplay Framework.** Ability system, input remapping, save/load (reflection
again), localisation, scene composition/prefabs with nesting and inheritance (Godot's
model — cleanest of the three).

**Ch.29 Observability.** Tracy + Perfetto; named budgets per subsystem; **a perf gate
that fails CI on regression**, because a budget nobody enforces is a comment.

---

# Chapter 22 — MCP Server & the Agent Protocol

## 22.1 Five tools, not 626

> **This lesson is already paid for.** A 626-tool MCP server was built on another project
> and abandoned; the rebuild started from the opposite principle. Do not re-learn it here.

Enumerating the editor's surface as tools guarantees the surface and the tools drift, and
buries the agent in a menu it cannot reason about. **Compose, do not enumerate.**

| Tool | Signature | Notes |
|---|---|---|
| `query` | `(selector) -> [EntityView]` | selector language over the reflect registry |
| `apply` | `(commands[], {dry_run, txn}) -> Diff \| Applied` | the *only* mutation path |
| `observe` | `({viewport \| probe \| screenshot \| stats \| seed_path})` | what the agent can see |
| `simulate` | `({ticks, region, profile}) -> Trace` | run the game headlessly |
| `assert` | `(predicate) -> Verdict` | closes the loop |

Three resource roots: `scene://` (live project state), `schema://` (the entire
`#[forge_api]` registry as JSON Schema — Ch.6), `docs://`.

## 22.2 Why `simulate` + `assert` is the feature that matters

Everything else is remote-controlling an editor, which is useful. `simulate` and `assert`
let an agent **run the game, watch what happened, and check a claim about it** — without
a human, without a screenshot, deterministically, in CI. That is agent-driven content
iteration and agent-driven regression testing, and no other engine can offer it because
no other engine has a deterministic headless runtime with a reflected command surface.

## 22.3 Safety

Agent commands are ordinary commands with `Issuer::Agent`, which means they are already
undoable, provenance-tagged, and dry-runnable (I8). On top of that:

- Every agent session runs in a **transaction** with preview-then-commit.
- Destructive commands (delete, overwrite, export, publish) require an explicit
  capability grant on the session.
- An audit log of every agent-issued envelope, greppable by session.
- **Agent-authored *code* runs in the WASM sandbox (I9), never as a native dylib.**

---

# Chapter 24 — Blueprints: Graph, IR, Codegen

## 24.1 Blueprints *are* Rust

**Ruling: a graph compiles. Nothing interprets a graph at runtime (I10).**

```
   visual graph  ─┐
   Rust source   ─┼─→  typed IR  ─→  Rust codegen  ─→  wasm (iterate) │ native (ship)
   agent (MCP)   ─┘
```

Three authoring surfaces, **one semantics**. Why this and not a VM:

- UE's Blueprint VM is roughly an order of magnitude slower than C++, and "port your
  Blueprints to C++ before shipping" is a real, well-known, expensive migration. Designing
  that tax in from the start would be choosing to inherit a known defect.
- A VM means two sets of semantics that must agree forever. They will not.
- The generated Rust is **readable and editable**, and for the subset that maps back
  cleanly, graph↔text is a round trip. That is the feature Blueprint users actually want
  and have never had.
- The same IR is the material graph (Ch.10), the generator graph (Ch.13) and the PCG
  graph (Ch.14). **One graph compiler, four users.**

## 24.2 Where the nodes come from

Nowhere, manually. Every `#[forge_api]` item is a node (Ch.6). Adding an engine function
adds a node, a schema entry, and an inspector row, in one edit, with no possibility of
drift.

## 24.3 The risk to measure early

Compile latency. A designer tweaking a graph cannot wait 30 seconds. **Spike S3** must
establish, before M5 commits: cold and warm codegen+`cargo build`+wasm times for a
realistic graph, and whether incremental compilation of a single generated crate lands
under ~2 s warm. **Fallback if it does not:** a cached per-node wasm module with a thin
dispatch layer for iteration only, with the full compile on save. State the fallback in
the chapter so nobody discovers the problem at M5 and panics.

---

# Chapter 26 — Heterogeneous Compute & the Distributed Farm

> "Many devs are using cheaper used gear and being able to combine them would be amazing
> for all of us." This chapter is that. It is split into three tiers because **they have
> wildly different difficulty and payoff, and conflating them is how the feature dies.**

## Tier 0 — Heterogeneous compute offload  ·  *real, large, do it first*

Dispatch **batch GPU work across every adapter in the machine**, including the iGPU and a
five-year-old card in the second slot.

Eligible work — all of it embarrassingly parallel and none of it latency-critical:
terrain generation, erosion solve, PCG density evaluation, lightmap and probe bake,
texture transcode, mesh simplification and meshlet building, climate EBM solve,
ephemeris tables, path-traced reference renders.

This is where combining cheap used gear genuinely pays, and the payoff is **editor and
bake time**, which is most of a developer's day. A second-hand card that is useless for
rendering a modern frame is perfectly good at running noise kernels.

```rust
pub struct AdapterPool { devices: Vec<Device>, caps: Vec<Caps> }
pub trait GpuJob { fn split(&self, n: usize) -> Vec<Self>; fn merge(parts: Vec<Out>) -> Out; }
```

Scheduling is by measured throughput per adapter, not by device name. Results merge in a
**fixed order** — parallel float reduction is not deterministic (Ch.3 §3.3).

## Tier 2 — The LAN farm  ·  *real, do it second*

The same `GpuJob` and CPU job types, dispatched to `forge-farm` daemons on other
machines on the LAN. A second workstation, an old laptop, a spare box: they contribute
bakes, generation, and simulation. **Identical job interface as Tier 0** — that is the
design constraint that makes it cheap.

Also the path to server-side simulation for a shipped game, and to distributed CI.

## Tier 1 — Split-frame realtime rendering  ·  *bounded, honest, spike first*

Splitting a *live* frame across GPUs. The arithmetic must be in the plan so nobody
promises it casually:

- 4K RGBA8 at 60 Hz is ~2 GB/s of colour per full frame; correct compositing needs depth
  too, so call it ~4 GB/s. PCIe 3.0 ×16 is ~16 GB/s theoretical, so it *fits* — but it is
  latency-sensitive, and a second GPU in a ×4 slot (which is what a cheap second slot
  usually is) does not fit.
- `wgpu` has no good cross-adapter resource-sharing story. Doing it properly needs Vulkan
  external memory through a `wgpu-hal` passthrough. **This is spike S1.**

**Ruling:** Tier 1 is committed for **editor viewports, offline/cinematic renders, and
multi-display/ICVFX**, where a frame of latency is free. It is **not** committed for the
player-facing hot path. Say this in the docs too — over-promising multi-GPU realtime is
how this feature would lose the project its credibility.

## CPU

Work-stealing scheduler over all cores (`bevy_tasks` or a custom one), NUMA-aware pinning,
and the same job types dispatchable to the farm. The regionized scheduler (Ch.5) sits on
top of it.

---

# Chapter 30 — Testing, Gates & CI

## 30.1 The command ladder

| Command | What it is | Rule |
|---|---|---|
| `just check` | a **deliberate subset** — fast, deselects `slow`, **does not run `tests/e2e`** | says so in its own help text |
| `just verify` | **CI's command list, in CI's order** | the command whose green predicts the push |
| `just gate` | invariant rows: bound / awaiting / unbuilt(reason) | a row is a test file |
| `just dod` | DoD items: settled / blocked / superseded / **UNMET** | 0 UNMET, 0 unread to ship a milestone |
| `just plan-coverage` | every directory claimed by a chapter | fails on an orphan |

**W4 is load-bearing here.** A local gate that is a subset of the remote gate produces
confidence exactly proportional to what it skips. One project had three CI-red causes
living in precisely that diff — a formatter check that ran remotely and not
locally, a stale lint cache that made the local check *lie*, and an e2e leg that
`just check` never ran. `xtask gate-parity` (I12) asserts the local list is a superset.

`tests/e2e` is a **serial leg**, run alone. A background e2e run competing with another
suite fails scenarios for load reasons, which then get misdiagnosed as engine bugs.

## 30.2 The liveness family

Four guards, each answering a question the others do not. Real defects slipped past each
of the first three in turn, so each carries its own hard-won refinement:

| Guard | Asks | Refinement it needed |
|---|---|---|
| `test_caller_liveness` | does this module have a production caller? | **must resolve through the import graph, following re-exports and normalising spelling — a name match is not a reference (W3).** Non-Rust entry points read from `[[bin]]`, `main.rs` and shell scripts |
| `test_seam_liveness` | is this seam actually *reached*, or does `None`/`Default` satisfy every question? | a seam satisfied by a default value is dead while looking alive |
| `test_config_liveness` | does any code read this config key? | same import-graph rule |
| `test_command_liveness` | does every UI mutation go through the bus? (I7) | new to this project; see Ch.7 |

**And the subtlest question, added last:** a module can
fail liveness not because it lacks a *caller* but because it lacks **an input that exists
on disk**. When triaging a liveness failure, the standing question is now: *is this
blocked on hardware, on a file nobody wrote, or on an input that does not exist?*

## 30.3 Determinism, perf, and the positive-control rule

- Determinism: Ch.3 §3.4, platform matrix, golden hashes, mutation control.
- Perf: named budgets per subsystem (Ch.29), CI fails on regression beyond a stated band.
- **Every guard in this section has a non-vacuous positive control that runs in CI (W2).**
  A guard whose positive control names something that does not exist is worse than no guard,
  because it reports green.

## 30.4 A note on flakiness

**Never widen a tolerance or add a retry to make a scenario pass (W5).** Find the cause.
Flakiness is triggered by load and never caused by it — and a "control run" that held the
commit fixed while letting machine load vary is not a control (W6). A harness's patience
with the OS scheduler may be raised, with the reasoning written at the constant; an
assertion about the *engine* may not.

---

---

# Chapter 31 — Workspace Presets: 2D, 3D, Planetary

## 31.1 The anti-pattern this must avoid

Unity's URP/HDRP choice is made at project creation, is painful to reverse, splits the
asset ecosystem, splits the documentation, and invalidates half of every tutorial. It is
the single most-complained-about structural decision in that engine.

> **I15: A preset sets defaults. It never gates capability.**
> Every `#[forge_api]` item is reachable under every preset. Any project can be promoted
> to any preset. A preset is **data, not code**.

## 31.2 The three presets, and the honest shape of the difference

| | **2D** | **3D** | **Planetary** |
|---|---|---|---|
| Frame tree | depth 1, fixed | depth 1, fixed | arbitrary depth |
| Default `BodyRepr` | none — layers | static mesh (+ optional volumetric) | quadtree / lattice / field |
| Render path | **2D** (Ch.35) | 3D | 3D + planetary passes (Ch.11) |
| Physics | `avian2d` | `avian3d` | `avian3d`, region-bubbled |
| Camera | ortho, pixel-snapped | perspective | perspective, four-shell frusta |
| Loads | `forge-2d` | core 3D | + `astro`, `climate`, `planet`, `gen` |

**3D is the degenerate case of Planetary** — one frame, one static body, one frustum
shell. There is no fork and no second code path; a 3D project is a Planetary project with
the depth set to 1. Promotion is a settings change.

**2D is the one genuinely different kind**, because it has its own render path and its own
physics solver. That is a real cost and Chapter 35 pays it deliberately.

## 31.3 What a preset actually is

`presets/planetary/workspace.ron` — a manifest naming: the default plugin set, the default
panel layout, default project settings, the new-scene template, and which extension-point
defaults are installed. Nothing else. **Users author their own presets by copying one**,
which is the "any feature modifiable" principle applied to the editor's own shape.

## 31.4 Promotion

| From → To | Cost |
|---|---|
| 3D → Planetary | settings change: deepen the frame tree, assign a body. **Non-destructive.** |
| Planetary → 3D | narrowing. Lossless with one body; lossy with several, and the UI says so. |
| 2D → 3D | additive: the 2D content keeps working as a 2D layer in a 3D project |
| 3D → 2D | narrowing, lossy, and the UI says so |

**Guards:** `test_no_preset_gating` (I15) asserts every `#[forge_api]` item resolves under
every preset — with a positive control that adds a preset-gated item and must fail.
`test_preset_promotion` round-trips a project through each arrow and asserts no loss where
none is expected.

## 31.5 The 2D tax must be zero

The reason unified engines lose 2D developers is that 2D projects pay for 3D machinery
they never use — build size, load time, editor complexity, irrelevant settings.

**Guard: `test_2d_has_no_planetary_cost`** — a 2D project loads none of `forge-astro`,
`forge-climate`, `forge-planet`, `forge-gen`, `forge-volume`; asserted against the
resolved plugin manifest, not against a comment. Binary size and cold-start time for the
2D preset are named budgets in the perf gate (Ch.29).

---

# Chapter 32 — Plugin System & Extension Points

## 32.1 The strong form, because the weak form is not what was asked for

"Users can write plugins" is table stakes. **"Any feature is modifiable" is a much
stronger claim and it has exactly one honest implementation:**

> **I16: No first-party subsystem uses a capability a plugin cannot use.**
> The engine is a small kernel plus plugins. The terrain generator, the renderer passes,
> the physics backend, the importers and the editor panels are plugins, written against
> the public extension points, with no private escape hatch.

Guard: `test_no_privileged_plugin` — no first-party plugin crate references a non-`pub`
item or a `pub(crate)` back door across the kernel boundary. **Positive control:** a
mutation build adds such a reference and the test must fail on it.

This is the sibling of I7. I7 says the editor UI has no privileged path to state; I16 says
engine subsystems have no privileged path to the engine. Together they are what make
"modifiable" and "agent-drivable" true rather than aspirational.

## 32.2 Extension points support `replace`, not just `add`

This is the detail that separates a real answer from a hook system. Most plugin APIs let
you append; "modify any feature" requires being able to **replace or remove** the built-in.

```rust
pub trait ExtensionPoint: 'static {
    type Item;
    const ID: &'static str;          // stable, namespaced, greppable
}

pub struct Registry<P: ExtensionPoint> { /* ordered, keyed, replaceable */ }

impl<P: ExtensionPoint> Registry<P> {
    pub fn add(&mut self, owner: PluginId, item: P::Item, order: Order) -> ItemId;
    pub fn replace(&mut self, target: ItemId, item: P::Item) -> Result<Replaced>;
    pub fn remove(&mut self, target: ItemId) -> Result<Removed>;
    pub fn chain(&mut self, target: ItemId, wrap: impl Fn(P::Item) -> P::Item);
}
```

`chain` is the middleware case — wrap the built-in rather than discard it — which is what
most "modify" requests actually want and what prevents an ecosystem of mutually-exclusive
replacements.

**Seed list of named extension points** (the contract; each chapter adds its own):
importers · exporters · render passes · material nodes · graph nodes · **body
representations** · generator nodes · PCG rules · physics backends · navmesh builders ·
inspector widgets · editor panels · **commands** · asset types · **store backends** ·
**presets** · MCP tools · script hosts · platform backends · localisers · input devices ·
profilers · brushes.

Guard: `test_extension_point_replaceable` — every registered point supports `add`,
`replace`, `remove` and `chain`, exercised, with a positive control.

## 32.3 Two plugin flavours, and the Rust ABI truth

| | **WASM component** | **Source (cargo) plugin** |
|---|---|---|
| Distribution | drop in a file, no recompile | a `cargo` dependency; recompile |
| Speed | ~1.5–3× native | native |
| Trust | sandboxed, capability-scoped | full |
| Hot reload | yes | yes, via dev `dylib` |
| Reaches | most extension points | **all**, incl. render passes and physics backends |
| Right for | marketplace, agent-authored, untrusted, tools, gameplay | engine subsystems, perf-critical, low-level |

> **Rejected: drop-in native `.dll`/`.so` plugins over a Rust ABI.**
> Rust has no stable ABI. `abi_stable`/`stabby` work but constrain every interface they
> touch, and the failure mode at a version skew is a **silent miscompile**, not an error.
> The honest resolution: **if you want drop-in, use WASM; if you want native, compile it
> in.** Recompiling is normal and fast in Rust, and it is what Bevy does. A fragile dylib
> loader would be a permanent source of unreproducible bug reports.

## 32.4 Manifest, capabilities, versioning

```ron
Plugin(
  id: "com.example.rivers",
  version: "0.3.1",
  engine: "^0.2",                       // semver against the kernel, checked at load
  kind: Wasm,                           // Wasm | Source
  provides: [ GeneratorNode("river_v2"), EditorPanel("rivers") ],
  replaces: [ GeneratorNode("forge.river") ],   // explicit, and conflicts are errors
  capabilities: [ Fs(ProjectRead), Gpu(Compute) ],  // NOT granted by default
)
```

Capabilities reuse the **Ch.22 MCP session model** deliberately — one grant mechanism for
agents, plugins, and remote sessions, so there is one thing to audit rather than three.
`net`, `fs(write)`, `process`, and `command(destructive)` are never default-granted.

Conflicting `replaces` between two plugins is a **load error with both names printed**,
never a silent last-wins.

## 32.5 The registry

An open **index**, not a storefront: a signed manifest list, self-hostable, with
`forge add <id>` resolving versions. No revenue share, no curation gate, no account.

---

# Chapter 33 — Project Storage, Version Control & Collaboration

## 33.1 The local filesystem is not privileged

> **I17: `ProjectStore` is a trait. The local filesystem is one implementation of it.**

```rust
pub trait ProjectStore: Send + Sync {
    fn read(&self, path: &StorePath) -> Result<Bytes>;
    fn write(&mut self, path: &StorePath, b: Bytes) -> Result<()>;
    fn blob_get(&self, h: Blake3) -> Result<Bytes>;
    fn blob_put(&mut self, b: Bytes) -> Result<Blake3>;
    fn commit(&mut self, msg: &str, envelopes: &[CommandEnvelope]) -> Result<RevId>;
    fn history(&self, range: RevRange) -> Result<Vec<Rev>>;
    fn lock(&mut self, path: &StorePath) -> Result<Lock>;     // §33.3
}
```

Backends, all first-party, none special-cased:

| Backend | For |
|---|---|
| `LocalFs` | solo, offline, the default |
| `Git` | any remote — GitHub, GitLab, **self-hosted Gitea/Forgejo**, or a bare repo on a NAS |
| `S3` | S3-compatible: **self-hosted MinIO**, R2, B2, S3 |
| `Sql` | **SQLite** local file, or **Postgres** self-hosted or managed |

Guard: `test_store_backend_parity` — the same project round-trips through every backend
and produces identical content hashes. A backend that cannot do that is not a backend.

## 33.2 The binary problem, which kills teams

**Git is bad at binaries**, and a game project is mostly binaries. Ignoring this is how a
team ends up with a 40 GB clone and a five-minute `git status`. The representation splits
three ways:

| Class | Stored as | Why |
|---|---|---|
| **Project data** — scenes, prefabs, graphs, settings | RON over `bevy_reflect`: text, diffable | mergeable, reviewable in a PR |
| **Assets** — textures, meshes, audio | **content-addressed blobs** (BLAKE3) in the blob backend; the tree holds hashes | LFS semantics, but engine-native, so it works identically on S3 and SQL backends where LFS does not exist |
| **Generated content** — terrain, scatter, systems, climate | **not stored at all** — it has a `SeedPath` (Ch.4) | this is where the universe architecture pays off in the VCS layer: a planet is a seed, not 16 PB |

That third row is worth dwelling on. The Foundations' storage analysis put a dense 0.25 m Earth at
**268 EB**, and the best-case sparse octree at **~16 PB**. The only thing a repository ever
holds is the seed, the generator graph, and the **edit deltas** — which for 10,000 players
over a year is tens of gigabytes. **An engine whose worlds are functions has a version
control story that an engine whose worlds are files cannot have.**

## 33.3 Two things the big engines charge for

**Semantic merge.** Because project data is a reflect tree, a three-way merge happens on
the *tree*, not on text lines — "A moved the light, B changed its colour" merges cleanly
instead of conflicting on line 4,102. Unity and UE both struggle here and it is a common
reason studios buy Perforce.

**Asset locking.** For genuinely un-mergeable binaries, `lock()` is Perforce's actual
value proposition and costs almost nothing once a store exists. Expensive to retrofit.

## 33.4 The command log is the real history

The bus already records semantic, provenance-tagged edits (Ch.7). `commit()` takes the
envelopes, so history reads *"agent:sess-4 placed 37 trees along river_2"* rather than
*"scene.ron changed, +412 −9."*

**This is also the substrate for multi-user work (Ch.37).** Sandboxes, publishing and
Live/Pull are all policies over this log, which is why Ch.37 is mostly a protocol rather
than a subsystem. What stays deliberately **unpromised** is narrower and named in Ch.37
§37.4: *concurrent editing of the same property of the same object*, Google-Docs style.
That is a CRDT problem over a scene graph with referential integrity and it is
research-grade. Prevent it with scoping, detect it with preconditions, resolve it with a
conflict UI — and say so in the user-facing docs, because that gap is exactly where users
form wrong expectations.

## 33.5 Setup must be three clicks

A storage story nobody configures is no storage story. The new-project dialog offers:
*local folder* (default), *GitHub* (device-flow OAuth, repo created for you), *any git
remote* (paste a URL), *self-hosted* (MinIO / Postgres / Gitea, with a connection test and
a working `docker-compose.yml` shipped in the docs). Ignore rules are generated from the
store's own knowledge of what is derivable — nobody hand-writes a `.gitignore`.

---

# Chapter 34 — Remote & Headless Operation

## 34.1 Three modes, named so they are never conflated

| Mode | UI runs | Core runs | For |
|---|---|---|---|
| **Headless** | nowhere | local | CI, bakes, agents, dedicated servers, farm nodes |
| **Split editor** | **locally, native** | remote | **the laptop + workstation case** |
| **Thin client** | streamed pixels | remote | tablets, weak laptops, WAN |

## 34.2 Split editor is a free consequence of I7 — and it beats Parsec

The UI is already a client of the command bus (Ch.7). Making that bus a network transport
is a **protocol, not a rearchitecture**.

The difference from a general remote-desktop tool is structural, not incremental:

| | Parsec / Moonlight / RDP | **Split editor** |
|---|---|---|
| Menus, hierarchy, inspector | streamed video — pays full latency | **local, native, zero latency** |
| Typing a property value, code editor | streamed — every keystroke round-trips | **local** |
| Dragging a slider | streamed | **local, with optimistic preview** |
| 3D viewport | streamed | streamed |

A general-purpose streamer cannot do this because it does not know your application's
structure. Forge does, because everything is a reflected command. **Only the viewport is
video**, and that is the difference between "usable over a link" and "actually pleasant."

## 34.3 Transport and budget

- **Commands + state deltas:** QUIC on LAN; WebRTC data channel when NAT traversal is
  needed. Deltas are reflect-tree diffs, so they are small and already exist (Ch.33).
- **Viewport:** hardware encode — NVENC / VA-API / AMF / QuickSync — AV1 where present,
  H.264 fallback. Software encode is a last resort and says so in the UI.
- **Assets:** the laptop does not have the project. Content-addressed blobs (Ch.33) stream
  on demand into a local cache. Same mechanism, no second system.
- **Budget, as a gate:** ≤ **16 ms** encode + transit + decode at 1080p60 on a wired LAN;
  ≤ 33 ms at 1440p. Measured in CI on a real pair where the hardware exists, and
  `AWAITING(hardware)` where it does not — never assumed.

**Spike S11** establishes the real number before M5 commits, with a written fallback: if
the budget cannot be met, split editor still ships (panels are local and useful even with
a sluggish viewport) and thin-client mode is documented as WAN-only.

## 34.4 Headless is the same binary

`forge --headless` runs the core with no UI at all: CI, bakes, agent sessions (Ch.22
`simulate`), farm nodes (Ch.26), and dedicated game servers.

**Guard: `test_headless_parity`** — headless and GUI runs of the same scene produce
identical simulation hashes. This is a remote-mode test that doubles as a determinism
test, which is the best kind.

## 34.5 Self-hosted, and that is the whole point

**No account. No relay service. No telemetry. No cloud dependency.** The remote host binds
to loopback by default; LAN exposure is an explicit choice; internet exposure requires a
written acknowledgement in the config. Pairing is device-based, transport is TLS/DTLS,
sessions are capability-scoped with the **same grant model as MCP and plugins** (Ch.22,
Ch.32), and every session is in the audit log. If someone needs TURN for NAT traversal,
they point at their own TURN server; the engine ships the config, not the service.

**Guard: `test_split_editor_parity`** — the same command sequence produces byte-identical
project state whether issued locally or over the wire.

---

# Chapter 35 — The 2D Pipeline

**Resolves open question O-7: true 2D, not 3D with an orthographic camera.**

## 35.1 Why it is worth a separate render path

"3D with an ortho camera" is a known-inferior experience and every 2D developer can tell
within ten minutes: pixel snapping fights the depth buffer, sorting is fragile, 2D lights
and normal maps are approximations, and sprite batching pays 3D pipeline overhead it never
needs. Godot's true-2D engine is one of its three most-cited draws, and Unity's 2D toolset
is the single most common reason 2D developers choose Unity over Unreal. **Half-doing this
forfeits an entire audience, and it is the cheapest audience in the engine to serve.**

## 35.2 Contents

Sprite batcher with atlas packing · tilemaps with autotiling and layers · spline-based
geometry (Sprite-Shape equivalent) · 2D lights, shadows and normal maps · 2D physics on
`avian2d` (**a 2D solver, not a 3D solver with a frozen axis**) · `Skeleton2D` cutout
rigging · sprite sheets and frame animation · Aseprite import · 2D navigation · parallax
layers · pixel-perfect camera with integer scaling · 2D particles · 2D-specific inspector
and panel layout in the 2D preset.

## 35.3 The coordinate system costs 2D nothing

`FrameId` is always the root; positions are `DVec2` under the same `FramePos` discipline
as everything else. A 2D project never evaluates an ephemeris, never builds a frame tree
deeper than one, and never links the universe crates (I15 / `test_2d_has_no_planetary_cost`,
Ch.31 §31.5).

**Scope control (spike S13):** 2D is a whole engine's worth of features and will sprawl
if it is allowed to. It is bounded by the list in §35.2 and by a shipped sample game —
a small platformer with tilemaps, cutout animation, 2D lights and a gamepad — which is the
acceptance test. Anything not in §35.2 is post-1.0.

## 35.4 Parallelisable

2D touches the core, the GPU device layer and the asset system, and **nothing in the
universe layer**. It can be built by an independent agent stream from M2 onward without
contending with planetary work. That makes it unusually good parallel work.

---

# Chapter 36 — Volumetric Editing & the Mineable-World Toggle

The Voxel-Plugin-2-equivalent, as an explicit per-world capability rather than an engine
mode.

## 36.1 The two states

| State | What a world is made of | Cost |
|---|---|---|
| **Static collision** (default) | meshes, heightfields, convex hulls | cheapest; what most games want |
| **Volumetric** (toggle) | + an editable density/material field | fully mineable, buildable, caves, overhangs |

**They coexist.** A volumetric world still contains static meshes; the voxel layer carves
around them and meshes can be stamped into the field. That hybrid is how the mature
Unreal voxel plugins work in practice and it is the right shape — "everything must be voxels" is a
worse engine than "voxels where you want them."

**It is a property of a world/body, not a global setting.** In a Planetary project, one
body can be volumetric and another static.

## 36.2 Toggle honesty

- **Off → On** on an existing world: **non-destructive.** The existing geometry becomes the
  static layer and a field is initialised around it.
- **On → Off** after edits: **lossy.** The voxel edits have nowhere to go. The UI states
  this, offers a bake-to-mesh path, and requires confirmation. Writing it only in a design
  document is how users lose work.

## 36.3 What "similar to Voxel 2" concretely means

A checkable list, because otherwise this requirement is unfalsifiable:

| | Feature | Chapter |
|---|---|---|
| V-1 | Node-graph world generation | 13, on the Ch.24 IR — **a fourth user of one compiler, not a fourth compiler** |
| V-2 | Surface nets / dual contouring extraction, seamless across LOD | 12 (O-5) |
| V-3 | Clipmap / octree LOD streaming | 12 |
| V-4 | Runtime edit API + brushes: sphere, box, smooth, flatten, paint material, stamp mesh→SDF | 36 |
| V-5 | Collision generated from the voxel surface, with LOD rings | 17 |
| V-6 | Navmesh generated from the voxel surface, per region | 18 |
| V-7 | Foliage and detail scatter that survives edits | 14 |
| V-8 | Multiplayer edit sync — **deltas, never geometry** | 25 |
| V-9 | Undo/redo on voxel edits **through the command bus** | 7 — so scripts and agents can mine too |
| V-10 | Import: mesh → SDF, heightmap → voxels | 8 |
| V-11 | Material/biome painting and blending | 14 |
| V-12 | Per-body config: voxel size, chunk size, shell thickness | 12 |

V-9 is the one that is easy to miss and expensive to add later. Because edits are commands
(I7), an agent can dig, a script can dig, a test can dig and assert, and every edit is
undoable and provenance-tagged — none of which is true of a plugin that owns its own
undo stack.

## 36.4 Where this should be outright better than the UE plugin

Not a marketing claim — a structural one. In Forge the edit store, the LOD rings, the
determinism contract, and the stateless generator are **engine-level**, not bolted onto an
engine that assumes static levels. Specifically: edits are `(BodyId, morton)` KV rows with
a sparse index (I-5), generation is a pure function so any node can produce any chunk
without ownership, the client can generate locally and verify by hash instead of
downloading geometry, and the whole thing is deterministic across platforms by contract.

And it is free, source-available, and modifiable — which is itself the answer to a real
grievance, since the capability in question currently costs money and cannot be changed.

## 36.5 Performance honesty, carried from the Foundations

Full 0.25 m collision is affordable only within **~32 m** of a viewer; pushing it to 128 m
is 3.3 M triangles and ~264 MB for one player. Seven LOD rings reach 2 km for ~112 MB
because doubling radius while doubling voxel size holds triangle count constant. Caves
multiply surface area 3–5×. **Geometry residency and gameplay residency are two
independent radii and must not be forced to match.**

---

---

# Chapter 37 — Teams, Sandboxes & Multi-User Sessions

## 37.1 Most of this is already built

Four earlier decisions turn out to have been building toward this, which is why the
chapter is mostly a protocol rather than a subsystem:

| Already have | From | Gives |
|---|---|---|
| every mutation is a serialisable, provenance-tagged, undoable command | Ch.7 (I7) | a shareable edit stream |
| the bus already runs over a network transport | Ch.34 | N sessions is a generalisation of 1 |
| content-addressed blobs + semantic merge on the reflect tree + locks | Ch.33 | the merge and transfer layer |
| one capability-grant model | Ch.22, 32, 34 | roles, with one thing to audit |

Multi-user is: **the command log becomes a shared, ordered stream**, plus a per-user layer
over it.

## 37.2 The sandbox equation

> **I19: `project_view = baseline ⊕ sandbox_deltas`.**
> A sandbox never mutates the baseline in place.

That is the same equation as the terrain layer — `chunk_state = generate(seed, pos) ⊕
edits[chunk_id]` — one level up, and not by coincidence: it is the same problem (many
readers, few writers, isolation must be cheap) and it has the same answer.

It inherits the same property, which is the one that makes hosting affordable:
**a sandbox is a row, not a process.** An idle sandbox costs what an empty planet costs.
That is invariants I4 and I14 again, and **spike S16** exists to prove it rather than
assume it.

```rust
pub struct Sandbox {
    id:         SandboxId,
    owner:      UserId,
    base:       RevId,                  // the baseline revision it is layered on
    deltas:     Vec<CommandEnvelope>,   // ordered, unpublished
    visibility: Visibility,             // Private | Team | Shared(read-only)
}
```

## 37.3 Live and Pull are one mechanism, two policies

> **I20: Live and Pull are subscription policies over one command stream, not two systems.**
> Switching between them mid-session costs nothing and changes no outcome.

| Mode | When your baseline advances | Good for |
|---|---|---|
| **Pull / Push** | when you say, and you choose *which* revisions | deliberate work, risky refactors, working offline |
| **Live** | automatically, as each publish lands | tight collaboration, review sessions, level-design pairing |

"Permanent changes" in the requirement means **published** changes, and that is the whole
difference between the two modes: Live auto-advances your baseline, Pull advances it on
request.

**Publishing is `ProjectStore::commit`** (Ch.33) — promoting your sandbox deltas into the
baseline. So push and pull are literally the store operations, and "pull any pushed work
of your choosing" is a revision-range selection, not a new concept.

`test_live_pull_equivalence` (I20) asserts the same publish sequence produces identical
final state under either policy. **That is invariant I5's shape applied to collaboration:
a policy may change when you see something. It may never change what you end up with.**

## 37.4 The hard part, stated honestly: rebase

When the baseline advances under your sandbox, your unpublished deltas may no longer be
valid — you moved an entity someone deleted, you edited a material someone replaced.

Three layers of defence, in the order they should be built:

1. **Command preconditions.** Every `EditorCommand` declares what it assumes (this entity
   exists; this property holds this value). On rebase they are re-checked, and a failure
   is a **conflict, surfaced** — never a silent drop. This is cheap because `dry_run`
   (I8) already computes exactly this.
2. **Scoped ownership.** Claim a subtree — a scene, a prefab, a body, an asset — and
   others get live read-only. **This is what makes it work in practice.** Most real
   conflicts are prevented, not merged.
3. **Locks** (Ch.33 §33.3) for genuinely un-mergeable binaries.

> **What is deliberately not promised: two people editing the same property of the same
> object at the same time, Google-Docs style.** That is a CRDT problem over a scene graph
> with referential integrity and it is research-grade. Prevent it with scoping, detect it
> with preconditions, resolve it with a conflict UI — **and say exactly this in the
> user-facing docs**, because the gap between "live collaboration" and "concurrent
> same-object editing" is precisely where users form expectations that will not be met.

**Spike S15** measures the real conflict rate before Live mode ships, with a written
fallback: if it is too high, scoped ownership becomes mandatory in Live rather than
optional, and Live degrades to "live read, scoped write."

## 37.5 Sandboxed testing

Play-in-editor runs against **your sandbox view**, not the baseline. Each user simulates
independently, which is natural because the runtime is already separate from editor state
(Ch.7) and a world is kilobytes (Ch.5 §5.2).

**Nobody's test run disturbs anybody's work because nobody's test run writes anywhere but
their own sandbox.** That is the requirement, and it is satisfied structurally rather than
by policy — the same shape as the Layer 1 / Layer 2 visibility rule.

## 37.6 Identity, teams and roles

**No Forge-operated account service, ever** (E-31). Identity is one of:

- **Local accounts** on the self-hosted server — the default, zero external dependency
- **OIDC, bring your own** — self-hosted Keycloak or Authentik, or GitHub/Google if a team
  wants it
- **Device pairing** (Ch.34) authenticates the *machine*; user auth authenticates the
  *person*. They are two different things and both are needed.

Roles map to capability sets, **reusing the Ch.22/32/34 grant model** — the fourth user of
one mechanism, so there is one thing to audit instead of four:

| Role | Can |
|---|---|
| **Owner** | everything, including delete and transfer ownership |
| **Maintainer** | publish to baseline, manage members, manage locks, resolve conflicts |
| **Developer** | own sandboxes, claim scopes, publish (optionally review-gated) |
| **Reviewer** | read the baseline and shared sandboxes, comment, approve |
| **Viewer** | read the baseline only |

Per-path scoping on top: a Developer may hold publish rights to `scenes/levels/**` and not
to `engine-config/**`.

## 37.7 The UI

**Create Team → Add Member**, and it has to be three clicks or nobody will use it.

1. **Create Team** — name it; the project binds to it.
2. **Add Member** — invite by email or by join code/link, pick a role, optionally scope
   paths. A pending invite is visible and revocable.
3. **Member list** — role, online state, current sandbox, what they have claimed.

Plus: a **presence** indicator (who is looking at what), a **publish/review queue**, a
**conflict resolution** panel, and a per-user **Live / Pull** toggle that is one click and
reversible at any time (I20 is what makes that safe).

**Team management is commands** (I7) — so it is scriptable, undoable, audited and
MCP-drivable like everything else. An agent can hold a role, which costs nothing extra
because `Issuer::Agent` already exists (Ch.7).

## 37.8 The server

`forge-server` is Ch.34's remote host with N sessions and a sandbox layer. It holds the
baseline, the sandboxes, the blob store and the identity database. Clients attach as
split-editor or thin-client.

**This makes the editor multi-tenant, which is a genuine change in threat model:**

- **Authn on connect; authz per *command*, not per session** — a role can be reduced
  mid-flight and the next command must feel it.
- **Sandbox isolation** — a `Private` sandbox is unreadable by anyone but its owner and
  the Owner role.
- **A user's WASM plugins and scripts run with that user's capabilities, not the
  server's.** This is the one that is easy to get wrong and catastrophic when it is: if a
  Developer can load a plugin that executes with server authority, every role above is
  decorative.
- **Rate limits per session** — a runaway script must not take down a team.
- **Full audit log**, greppable by user, session and sandbox.
- Loopback by default; LAN is an explicit choice; internet exposure requires a written
  acknowledgement in the config.

## 37.9 Guards

| Guard | Asserts |
|---|---|
| `test_sandbox_isolation` (I19) | a private sandbox is unreadable across users; the baseline is never mutated in place. Positive control: a mutation that writes through must fail |
| `test_live_pull_equivalence` (I20) | the same publish sequence yields identical final state under both policies |
| `test_rebase_preconditions` | every command declares preconditions; a synthetic conflict is **detected**, not swallowed |
| `test_role_enforcement` | authz is evaluated per command. Positive control: an escalation attempt must fail |
| `test_plugin_runs_as_user` | a plugin loaded in a sandbox cannot reach a capability its owner lacks |
| `test_sandbox_cost` (S16) | 50 idle sandboxes stay within the stated per-sandbox budget |

---

# Appendix A — Licensing, Governance & Community

**Licence: `Apache-2.0 OR MIT`**, the Rust ecosystem convention. Apache-2.0 supplies an
explicit patent grant; MIT supplies maximal compatibility; the dual form is what every
downstream Rust consumer already expects. **Not GPL** — it would make console ports and
many commercial integrations impossible, which defeats the purpose.

**Dependency licence policy (I13):** permissive only in the open tree. `cargo-deny` in
`just verify` with an explicit allow-list. **`ufbx`, never the Autodesk FBX SDK.**
Proprietary middleware (Wwise, FMOD, platform SDKs) may only ever be a plugin behind a
trait, never a dependency.

**Contribution: DCO, not a CLA.** A CLA on a project whose pitch is "no rug-pull risk"
sends exactly the wrong signal — it is the mechanism a relicensing would need. Godot's
governance is the model and its credibility is a real asset, not a soft one.

**Trademark the name, license the code freely** (Blender/Godot model). That is what
protects the project without restricting users.

**Funding:** foundation + sponsorships, never royalties and never seats. "Free forever"
is a positioning decision, and Unity's 2023 runtime-fee episode is why it is worth more
than the revenue it forgoes.

---

# Appendix B — Risk Register & Spikes

Each spike has a **decision deadline** and a **written fallback**. A spike with no
fallback is a hope.

| # | Risk | Spike | Fallback if it fails | Due |
|---|---|---|---|---|
| **S1** | `wgpu` cannot share resources across adapters efficiently | prototype cross-adapter transfer: host-staged vs Vulkan external memory via `wgpu-hal` | host-staged transfers only → Tier 0 + Tier 2 ship, Tier 1 limited to editor/offline | M1 |
| **S2** | Cross-platform bit-exact `f64` transcendentals are harder than expected | implement and validate `forge-num::det` on 3 platforms | vendor a softfloat path for the noise basis only; accept the slowdown in generation | **M0 — blocks everything** |
| **S3** | Blueprint compile latency too slow for iteration | measure cold/warm codegen + build for a realistic graph | cached per-node wasm + dispatch for iteration, full compile on save | M4 |
| **S4** | Regionized scheduler has correctness or contention problems | fuzz merge/split under load; measure contention at 4/16/64 regions | coarse per-region locks, then process-level projection (Ch.25) as the Foundations planned | M2 |
| **S5** | Virtualised geometry is out of reach | meshlet DAG + `meshoptimizer` prototype | classic LOD chains + GPU meshlet culling; procedural terrain does not need it anyway | M6 |
| **S6** | `bevy_*` crate churn costs more than budgeted | track two upgrade cycles, measure the diff | vendor and pin the five crates; fork if churn exceeds budget twice | M2 |
| **S7** | Physics precision or determinism insufficient at region scale | determinism harness on `avian` f64 with fixed step | region-local origin rebasing; if still insufficient, fork for stable contact ordering | M3 |
| **S8** | The global terrain solve does not produce believable rivers/caves at planet scale | run the global solve on a full body, inspect | reduce global scope to hydrology only; caves become locally-seeded with a coarse trunk hint | M3 |
| **S15** | Rebase conflicts frequent enough to make Live mode unusable | synthetic two-user workload on a real scene; measure conflicts per hour | scoped ownership becomes **mandatory** in Live rather than optional; Live degrades to "live read, scoped write" | M6 |
| **S16** | A sandbox costs more than a row — per-user memory or process growth | 50 idle sandboxes on one server, measured | if a sandbox cannot be a row, Live is capped at a stated team size and that number is published | M5 |
| **S9** | Scope collapse — the project stalls chasing parity | **the dogfood gate**: the reference game must port by end of M4 | cut parity features, not universe features. The differentiator is the product | M4 |
| **S10** | WASM plugin overhead too high at hot extension points | measure a generator node and a PCG rule in WASM vs native | that point becomes source-plugin-only and is documented as such | M4 |
| **S11** | Split-editor viewport latency budget unmet on real hardware | measure encode+transit+decode on a real LAN pair at 1080p60 | split editor still ships (panels are local and useful); thin client documented as WAN-only | M5 |
| **S12** | A realistic project makes a git repo unusable | build a 20 GB sample project, measure clone and status on every backend | blob store is the mitigation; if it is not enough, default new projects to `Sql` or `S3` | M3 |
| **S13** | The 2D pipeline sprawls into a second engine | bound it by the Ch.35 §35.2 list and one shipped sample game | anything outside the list is post-1.0, stated in the docs | M4 |
| **S14** | Windows/Linux divergence found late (case sensitivity, paths, DX12 vs Vulkan) | run the full gate on both from M0; lint asset-reference casing | none — this one is prevented, not mitigated (I18) | **M0** |

**S2 and S9 are the two that actually kill the project.** S2 because a non-deterministic
universe makes the entire premise false and the failure is silent. S9 because it is how every ambitious
engine project has ended, and the only defence is a real game that must ship on it.
