# Forge — Master Implementation Plan

> Codename placeholder. Apache-2.0 OR MIT.

**Status:** draft 1, 2026-09-20. Nothing below is ratified until it appears in
`forge-decision-ledger.md` with a ratification date.

---

## Who this document is for

A contributor, or a team of contributors, building the engine. It is also the contract that keeps
those contributors from disagreeing with each other. When two chapters conflict, **Chapter 1
wins**; when Chapter 1 conflicts with the Foundations architecture notes, **the notes win and
Chapter 1 has a defect** — file it, do not paper over it.

---

## What is being built

An engine and editor for large worlds, in 2D and 3D.

- The editor is a headless core with a UI *client*.
- Everything is Rust. Gameplay is Rust. Blueprints **compile to** Rust. Extensions are
  Rust compiled to WASM.
- It runs on one machine and it runs on four cheap used ones.

---

## Reality check — read before planning anything

This section exists so that nobody quietly discovers it halfway through M4 and stalls.

**1. Feature parity with Unreal is not a goal and cannot be one.** UE is roughly
twenty years and low-thousands of engineer-years. Unity similar. Godot is ~14 years and
hundreds of contributors. The attached feature comparison lists, conservatively, 400+
discrete subsystems. Treating that list as a backlog is how this project dies at 30%
completion with nothing anyone can ship.

**2. The list is still the right list — as a *destination*, ordered by a different rule.**
The ordering rule is: *does this exist because the engine is built for large worlds edited
through one command bus, or does it exist because every engine has one?* The first category is built
first and built well. The second is adopted from the Rust ecosystem, deferred, or
accepted as a gap with a written reason. See *What we are not building*.

**3. What is actually winnable.** Not "a better Unreal." This:

> The engine for large worlds where every edit is a command on one bus, every result is deterministic across platforms, and every subsystem is a replaceable plugin.

Nobody else is trying to be that. Unity has no story at all above ~10 km. Godot is the right
*shape* but has no scale layer. Bevy has the architecture and no editor. The gap is real, it
is large, and it is reachable.

**3a. The moat is technical, and it is narrower than an open-source one would have been.**
Forge is commercial and source-available (Appendix A). That is a deliberate owner decision
and the plan implements it — but it must be planned around honestly, because it removes a
competitive asset the earlier draft of this plan leaned on. "Free forever, no rug-pull
risk, community governance" was worth something real against Unity's 2023 runtime-fee
episode, and it is gone.

What replaces it has to be stated rather than assumed:

| Was going to be the argument | Is the argument now |
|---|---|
| free and open source | **$5 once. No royalties, ever, at any revenue.** |
| no rug-pull risk, because MIT | no rug-pull risk, because a purchased licence is irrevocable for the versions it covers — in the EULA, not a blog post |
| community-governed | owner-governed, with the source readable so nothing is a black box |
| cheaper than Unity and Unreal | **for any product over $1M, cheaper than Unreal by three to four orders of magnitude**, and the store takes 5% against Fab's 12% |

**Pay once, ship anything, keep everything.** A product grossing $2M pays Unreal $50,000
and pays Forge $5. That is a stronger commercial argument than the open-source one it
replaced, and it is available immediately rather than after a community forms.

**4. The specific payoff, in the user's own words.** `the Foundations`
§10 states that Folia-style *regionized multithreading inside one process* is "**strictly
better than projection — and UE cannot do it.**" Rust can. Region-partitioned mutable
world access is precisely what an ownership-and-borrow model is for. The single thing
that architecture names as better-but-unavailable becomes available by building in Rust.
Alongside it:

| the Foundations pain | Cause | Gone in Forge because |
|---|---|---|
| Multi-UWorld packing to dodge 2.5 GB baseline | UE process baseline | a `World` is kilobytes; sparse players stop needing a trick |
| Replication quadratic pinned to one game thread | UE game thread | regionized scheduler parallelises it |
| `double` world pos → `float` at the last mile | UE render transform | the renderer is ours; f64→camera-relative is the *only* path |
| Projection needed at 400+ crowd | no in-process regions | regions first, projection only if regions run out |
| BP VM ~10× slower than C++ | interpreted graph | graphs compile to Rust |

**5. Timescale, stated honestly.** A **self-hosting engine that the Foundations ships on**
is M4–M5. **Broad parity** is a multi-year community project and should be planned as one,
with the 1.0 line drawn at "the Foundations ships on it" — not at "the comparison table is
full."

**6. This is a marathon that must produce runnable things monthly.** No milestone is allowed
to end in "the architecture is now correct."

---

## Working agreements (binding)

Adopted from a previous project process model, which earned each of these the hard way.

**W1. A guard can only fail on a question somebody thought to ask.** Every invariant in
this plan has a named test file. A rule with no test is a wish.

**W2. Every guard needs a non-vacuous positive control.** Prove the guard fails when you
break the thing it guards, in CI, every run. a previous project shipped a liveness test
whose positive control named a module that had never existed.

**W3. A name match is not a reference.** Liveness checks resolve through the import graph,
follow re-exports, and normalise spelling. Two complete modules passed a name-matching
caller-liveness test while being entirely dead.

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

**W10. Interfaces are frozen before fan-out.** The binding constraint on parallel
work is interface ambiguity, not headcount. Contracts freeze at M0-20; parallel work
starts after. Role assignments and handoff contracts live in the roster document.

---

## What we are *not* building

Each of these is a decision, not an oversight. Re-proposing one requires new information.

| Not building | Instead |
|---|---|
| A shading language | WGSL, compiled by `naga` |
| A physics solver (v1) | `avian` (generic over f32/f64) with `rapier` as fallback |
| An FBX parser | `ufbx` (MIT). **Never** the Autodesk SDK — it poisons the licence |
| Image/texture codecs | `image`, `ktx2`, `basis-universal`, `texpresso` |
| A blueprint virtual machine | graphs compile to Rust (Ch. 23) |
| Real-world geodata import | rejected in the Foundations: does not translate into generator graphs cleanly |
| Console platform backends in the public export | first-party backends kept in the private source and handed only to platform-approved licensees (E-71) |
| ~~A MetaHuman equivalent~~ | superseded 2026-09-27: a character creator is M10-1 (E-67, E-71) |
| ~~An asset marketplace~~ | superseded 2026-09-27: the Forge Index becomes a storefront, M10-9 (E-71) |
| Our own ECS | `bevy_ecs` (see Ch. 5 for why we take the crate and not the framework) |
| A drop-in **native** plugin ABI | Rust has no stable ABI; drop-in is WASM, native is compiled in (Ch. 32) |
| A render-pipeline choice that forks the ecosystem | one pipeline; presets change defaults, never capability (Ch. 31) |
| A hosted remote-editor service or relay | self-hosted only; point at your own TURN if you need one (Ch. 34). An account exists to **buy**, never to run (E-46) |
| Telemetry of any kind | — |
| DRM, runtime licence checks, or anything in a customer's shipped game | activation once at install (I21, Ch.38) |
| Calling the project "open source" | it is **source-available and commercial**; the term has a specific meaning (Appendix A) |
| An LFS-style service dependency | engine-native content-addressed blobs that work on every backend (Ch. 33) |
| ~~Live multi-user editing before M8~~ | superseded 2026-09-27: live co-editing is M10-8 (E-71 amends E-37) |

---

## The fourteen invariants

These are the engine. Everything else is implementation. Each names its guard.

| # | Invariant | Guard |
|---|---|---|
| **I1** | **No global position type exists.** Every position is `(FrameId, DVec3)`. | `tests/liveness/test_frame_liveness.rs` |
| **I2** | Generation is a pure function of `(body_seed, integer position)` and is **byte-identical on every supported platform**. | `tests/determinism/test_cross_platform_hash.rs` |
| **I3** | Seeds derive. They are never accumulated, stored, or passed as mutable state. | `tests/determinism/test_seed_algebra.rs` |
| **I4** | Thread and process count scales with **players and viewers** — never with geometry, extent, body count, or channel count. | `tests/e2e/test_empty_world_cost.rs` |
| **I5** | A profile may change cost. It may never change outcome. | `tests/e2e/test_profile_equivalence.rs` |
| **I6** | Exactly one authority per entity. Ghost replicas resolve nothing. | `tests/net/test_authority_uniqueness.rs` |
| **I7** | **Every mutation of project state is a command on the bus. The UI has no privileged path.** | `tests/liveness/test_command_liveness.rs` |
| **I8** | Every command is dry-runnable, undoable, and provenance-tagged. | `tests/liveness/test_command_contract.rs` |
| **I9** | Third-party code runs in the WASM sandbox. Native `cdylib` is a *shipping* path, never an *authoring* path. | `tests/script/test_sandbox_boundary.rs` |
| **I10** | Blueprint graphs compile. Nothing interprets a graph at runtime. | `tests/graph/test_no_interpreter.rs` |
| **I11** | A subsystem with no production caller **reached through a real import path** is not done. | `tests/liveness/test_caller_liveness.rs` |
| **I12** | The local gate is a **superset** of the remote gate. | `xtask/src/gate_parity.rs` |
| **I13** | **Permissive dependencies only** — no copyleft of any strength, no NDA-encumbered code — and a generated `NOTICES` file in every distribution. | `xtask/src/licence_audit.rs` |
| **I15** | **A workspace preset sets defaults. It never gates capability.** Every project is promotable to every preset. | `tests/preset/test_no_preset_gating.rs` |
| **I16** | **No first-party subsystem uses a capability a plugin cannot use.** The engine is a kernel plus plugins. | `tests/plugin/test_no_privileged_plugin.rs` |
| **I17** | `ProjectStore` is a trait. The local filesystem is one implementation, not a privileged one. | `tests/store/test_store_backend_parity.rs` |
| **I18** | **Windows and Linux are co-primary.** A behavioural difference between them is a defect, never a caveat. macOS is planned but parked until the owner asks (E-68). | `tests/platform/test_platform_parity.rs` |
| **I19** | **`project_view = baseline ⊕ sandbox_deltas`.** A sandbox never mutates the baseline in place, and an idle sandbox is a row, not a process. | `tests/collab/test_sandbox_isolation.rs` |
| **I20** | **Live and Pull are subscription policies over one command stream, not two systems.** The same publish sequence yields the same final state under either. | `tests/collab/test_live_pull_equivalence.rs` |
| **I21** | **The software never phones home to function.** Licence activation happens once at install. There is no runtime check, no telemetry, and **nothing whatsoever in a customer's shipped game.** | `tests/licence/test_no_runtime_phone_home.rs` |

> **I1 and I7 are the two that are cheap today and brutal to retrofit.** the Foundations learned this
> about `frame_id` and about the one-authority rule. Adopt both before anything exists to
> retrofit.

> **I16 and I17 join them.** A kernel/plugin boundary drawn after the engine has twenty
> subsystems is drawn around whatever those subsystems happened to do, and every private
> escape hatch found later is a permanent one. The same is true of a storage trait added
> after the editor has learned to call `std::fs` directly in forty places.

---

## Document map

| Ch | Title | Depth | Crate(s) |
|---|---|---|---|
| **1** | **System Contracts** | **FULL · FROZEN** | — |
| **2** | **Coordinates, Frames & Time** | **FULL · FROZEN** | `forge-frames` |
| **3** | **Determinism & the Math Contract** | **FULL · FROZEN** | `forge-num` |
| **4** | **Seed Algebra** | **FULL · FROZEN** | `forge-seed` |
| **5** | **ECS Core & the Regionized Scheduler** | **FULL · FROZEN** | `forge-core` |
| 6 | Reflection, Annotation & the Four Outputs | FULL · FROZEN | `forge-reflect` |
| 7 | The Command Bus, Undo & Provenance | FULL · FROZEN | `forge-cmd` |
| 8 | Asset System, VFS & Import | FULL | `forge-asset` |
| 9 | GPU Device Layer & Render Graph | FULL | `forge-gpu` |
| 10 | Renderer — Scene, Materials, Lighting, GI | FULL (§10.1–10.9); GI, material graph, post BRIEF | `forge-render` |
| 14 | PCG Framework | BRIEF | `forge-pcg` |
| 17 | Physics & Simulation Bubbles | BRIEF | `forge-phys` |
| 18 | Navigation & AI | BRIEF | `forge-nav` |
| 19 | Animation | BRIEF | `forge-anim` |
| 20 | Audio | BRIEF | `forge-audio` |
| 21 | UI Toolkit & Editor Shell | FULL | `forge-ui`, `forge-editor` |
| 23 | Scripting — Rust, WASM, Hot Reload | BRIEF | `forge-script` |
| 24 | Blueprints — Graph, IR, Codegen | FULL | `forge-graph` |
| 25 | Networking, Replication & Authority | BRIEF | `forge-net` |
| 26 | Heterogeneous Compute & the Distributed Farm | FULL | `forge-jobs` |
| 27 | Platform HAL & Export | BRIEF | `forge-hal` |
| 28 | Gameplay Framework | BRIEF (FULL for §28.1–28.8, scene composition; §28.9–28.18, the input runtime) | `forge-play`, `forge-scene`, `forge-input` |
| 29 | Profiling, Debugging, Observability | BRIEF | `forge-trace` |
| 30 | Testing, Gates & CI | FULL | `xtask` |
| **31** | **Workspace Presets: 2D and 3D** | **FULL · FROZEN** | `forge-editor`, `presets/` |
| **32** | **Plugin System & Extension Points** | **FULL · FROZEN** | `forge-plugin` |
| **33** | **Project Storage, Version Control & Collaboration** | **FULL · FROZEN** | `forge-store` |
| **34** | **Remote & Headless Operation** | **FULL** | `forge-remote` |
| **35** | **The 2D Pipeline** | **FULL** | `forge-2d` |
| **37** | **Teams, Sandboxes & Multi-User Sessions** | **FULL** | `forge-collab`, `forge-identity` |
| **38** | **Licensing, Entitlement & Royalty Reporting** | **FULL** | `forge-licence` |
| A | Licensing, Governance & Community | FULL | — |
| B | Risk Register & Spikes | FULL | — |

**FULL** = written to implementation depth here. **BRIEF** = contract, decisions, DoD and
an expansion brief; expanding it is a task, and the first task of whoever owns it.
**FROZEN** = contracts frozen at M0-20 (W10, WP-07, ADR 0014): the chapter's signatures, as
written here and as the crate's public API implements them, change only by **plan
amendment** (an `Amended in WP-NN (ADR NNNN)` note in the chapter plus the ADR), never by a
silent refactor. Additive changes (a new method, variant or extension point that breaks no
caller) are ordinary work and need only the chapter's implementation section updated.
Chapter 33's frozen part is the `ProjectStore` trait and its M0 backends (§33.1, §33.6);
collaboration (§33.3–33.5) freezes with Ch.37.

> **Chapter numbers are identities, not an order.** Chapters 31–36 were added after 1–30
> and are numbered by allocation, not by where they read. Renumbering would break every
> DoD id, gate row and handoff that cites one — the same trap as a DoD item id, which is a
> *position* and not an identity unless you refuse to renumber. Read in this order:
> **1–7** (contracts) → **31, 32** (shape and extensibility) → **8–21** (subsystems, with
> **35** beside 10) → **22–30** → **33, 34, 37** (storage, remoting and
> collaboration — one arc; read them together).

---

## Repo tree — coverage checklist

Every directory must be fully specified by some chapter. `just plan-coverage` fails on an
unclaimed directory.

```
forge/
├── crates/
│   ├── forge-num/        Ch.3   deterministic math, vendored transcendentals, noise basis
│   ├── forge-frames/     Ch.2   FrameId, FramePos, FrameVel, Tick; the one world frame and the FrameResolver seam
│   ├── forge-seed/       Ch.4   seed algebra and derivation
│   ├── forge-core/       Ch.5   world, regionized scheduler, profiles
│   ├── forge-reflect/    Ch.6   #[forge_api], type registry, schema emit
│   ├── forge-cmd/        Ch.7   EditorCommand, txn, undo, provenance, bus
│   ├── forge-asset/      Ch.8   vfs, importers, cache, hot reload
│   ├── forge-gpu/        Ch.9   adapter pool, transfers, render graph
│   ├── forge-render/     Ch.10-11  scene, materials, lighting, depth passes
│   ├── forge-sky/        Ch.10  the sky: the atmosphere derived from its composition
│   ├── forge-pcg/        Ch.14  scatter, rules, biome binding
│   ├── forge-phys/       Ch.17  bubbles, collision, character, vehicles
│   ├── forge-nav/        Ch.18  navmesh, pathing, steering
│   ├── forge-anim/       Ch.19  skeleton, state machine, blend, IK, retarget
│   ├── forge-audio/      Ch.20  buses, spatial, adaptive
│   ├── forge-ui/         Ch.21  widget layer used by editor AND games
│   ├── forge-editor/     Ch.21  the shell — a client of forge-cmd, nothing more
│   ├── forge-script/     Ch.23  wasm host, dylib host, reload
│   ├── forge-graph/      Ch.24  graph model, typed IR, Rust codegen
│   ├── forge-net/        Ch.25  transport, replication, authority, prediction
│   ├── forge-jobs/       Ch.26  work-stealing, heterogeneous dispatch, farm client
│   ├── forge-hal/        Ch.27  platform traits — no platform code, only traits
│   ├── forge-play/       Ch.28  abilities, save, localisation
│   ├── forge-input/      Ch.28  the input runtime: devices (winit, gilrs), the compiled action map, deadzones, curves, triggers, rebinding, local multiplayer, touch, gyro, haptics, on-screen controls, glyphs (M7-12, WP-65)
│   ├── forge-scene/      Ch.28  scene composition: nesting + inheritance, override diffs, cycle refusal, reference-only files (M5-9, WP-U20)
│   ├── forge-trace/      Ch.29  tracy/perfetto, counters, budgets
│   ├── forge-sim/        Ch.34  the play core: simulation world forked from the edit world, fixed 60 Hz steps, replay files (M2-5, ADR 0030)
│   ├── forge-plugin/     Ch.32  kernel boundary, extension points, wasm+source loaders
│   ├── forge-wasm/       Ch.32  the WASM component plugin host (wasmtime); apart from forge-plugin so only hosts link wasmtime (M2-13, ADR 0033)
│   ├── forge-store/      Ch.33  ProjectStore trait; LocalFs/Git/S3/Sql backends, blobs
│   ├── forge-project/    Ch.33, Ch.31, Ch.38  project files, the core's store host (create/open/save/push/pull), packager trait (WP-U7, ADR 0032)
│   ├── forge-remote/     Ch.34  split editor, thin client, encode, transport, pairing
│   ├── forge-2d/         Ch.35  sprites, tilemaps, 2D lights, its own 2D solver (ADR 0047), cutout rigs
│   ├── forge-identity/   Ch.37  users, teams, roles, auth — its own security boundary
│   ├── forge-collab/     Ch.37  baseline, sandboxes, live/pull policy, rebase, conflicts
│   ├── forge-licence/    Ch.38  entitlement, activation, NOTICES — NOT linked by forge-runtime
│   └── forge-runtime/    Ch.28  the shipping runtime binary
├── tools/
│   ├── forge-cli/        Ch.30
│   ├── forge-editor-bin/ Ch.21  the `forge-editor` binary: shell + first-party panel plugins + winit runner (ADR 0018)
│   ├── forge-perf-gate/  Ch.29  the named budgets and their regression gate (M1-11): the gate, the shared measurements, the base gate
│   ├── docker/           Ch.30  linux-verify/: the local ubuntu-x86_64 leg in Docker, `just verify-linux` (ADR 0044)
│   ├── forge-bake/       Ch.26
│   ├── forge-farm/       Ch.26  the LAN daemon
│   └── forge-server/     Ch.37  the multi-user host: baseline + N sandboxes + N sessions
├── samples/               Ch.35  shipped sample games (workspace members, D-6), each exported like a customer game
│   └── 2d-game/          Ch.35  the 2D sample game (M4-12, S13 acceptance): platformer, menus and credits on forge-runtime, PIE backend, exported build (WP-U16)
├── tests/
│   ├── determinism/       Ch.3, Ch.30
│   ├── liveness/          Ch.30
│   ├── perf/              Ch.29, Ch.30
│   ├── platform/          Ch.30   Windows/Linux parity (I18), asset-reference case lint (M0-17)
│   ├── collab/            Ch.30   sandbox isolation, rebase, role enforcement
│   ├── licence/          Ch.30   no runtime phone-home (I21), NOTICES freshness
│   ├── plugin/  store/  preset/   Ch.30
│   └── e2e/               Ch.30   serial leg — NOT part of `just check`
├── xtask/                 Ch.30   gate, dod, plan-coverage, licence-audit, gate-parity, fp-rules, layering, allocators
├── presets/               Ch.31  2d/ 3d/ — DATA, not code (a plugin adds its own presets)
├── plugins/               Ch.32  first-party plugins; each one proves I16
├── docs/
│   ├── plan/              —      this plan: master-plan, decisions, milestones, dod-status.ron
│   ├── defects.md         W8 — the one allocator
│   ├── error-codes.md     Ch.1.2 — the one error-code allocator
│   ├── evidence/          Ch.30  committed observations; linux/: the Docker ubuntu leg, one file per row (ADR 0044)
│   ├── guides/            Ch.21  how-tos for plugin and tool authors: building editor tools on forge-ui (WP-U12)
│   ├── adr/               Ch.30
│   └── local-notes/       WP-lanes  per-lane working notes (lives alongside the plan, updated per work package)
├── .cargo/                Ch.3.3  config.toml — the pinned FP build rules
├── .config/               Ch.30   nextest.toml — wall-clock budget tests never overlap disk-heavy perf tests
├── .github/               Ch.30   workflows/ci.yml — the remote gate; == `just verify` (I12)
└── justfile               Ch.30
```

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
   └──────────────────┬──────────────────────────┘
                      ↑
        forge-gpu → forge-render
                      ↑
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
pub struct Error(Box<ErrorInner>);   // one pointer: Result<(), Error> is 8 bytes (D-10)

#[non_exhaustive]
pub struct ErrorInner {              // read through Error's Deref: e.code, e.ctx, e.source
    pub code: ErrorCode,          // stable, greppable, never reused
    pub ctx: SmallVec<[Ctx; 4]>,  // breadcrumbs, cheap
    pub source: Option<Box<dyn StdError + Send + Sync>>,
}
```

*Amended in WP-04 (D-10, ADR 0007):* the three fields are unchanged; they moved behind a
`Box` so a fallible hot path returns a register instead of copying ~200 bytes, and the
error allocates only when it is actually built. `size_of::<Result<(), Error>>() ==
size_of::<usize>()` is a compile-time assert.

Rules: no `unwrap` outside tests and `main`; no `panic!` reachable from a command handler
(a panicking command must be caught at the bus boundary and turned into a `Rejection`, or
one bad call takes down the editor); `#[deny(clippy::unwrap_used)]` crate-wide
outside `#[cfg(test)]`.

## 1.3 The four cross-cutting types

Every crate in the tree uses these and defines no alternative.

```rust
// forge-frames
pub struct FrameId(pub u32);
pub struct Tick(pub i64);            // canonical time, monotonic from epoch

// forge-seed
pub struct Seed(u64);

// forge-core
pub struct EntityId(/* bevy_ecs Entity */);
```

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

In the base edition there is **one world frame**. Every position is a `FramePos` in it: an
`f64` offset from the world origin, which is precise to well under a millimetre across a
world tens of kilometres wide, and the renderer draws camera-relative (§2.4). There is
still no global position type (I1): a position always names the frame it is in.

## 2.2 The types

```rust
/// The ONLY legal way to express a position anywhere in the engine.
/// There is no `WorldPos`, no `GlobalPos`, and no bare DVec3 in any public signature.
#[derive(Clone, Copy, Reflect)]
pub struct FramePos { pub frame: FrameId, pub local: DVec3 }

#[derive(Clone, Copy, Reflect)]
pub struct FrameVel { pub frame: FrameId, pub local: DVec3 }
```

## 2.4 Rendering — the last mile

The renderer resolves everything into **camera frame in f64**, then emits **f32
camera-relative**. There is no other path to the GPU. `forge-render` is the only crate
permitted to hold an `f32` position, and it holds only camera-relative ones.

## 2.5 Canonical time

One authoritative `Tick` (`i64`, from epoch) owned by the global-services layer. Clients
estimate it with offset + drift.

**A client's wall clock never feeds a physics-relevant evaluation.** That seam is trivially
exploitable.

## 2.9 The world frame and the resolver seam (WP-43)

Every consumer of positions turns a `FramePos` into another frame through one trait,
`forge_frames::FrameResolver`: `resolve`, `rotation_to_root`, `enclosing` and `view_depth`.
The renderer's last mile, the editor camera, picking, gizmos, measuring and camera flights take
a `&dyn FrameResolver` and never assume how many frames exist.

- **`WorldFrame`** is the resolver of a single-frame world: `FrameId::WORLD` (0) and nothing
  else. A position is an `f64` offset from the world origin — resolved to a few nanometres
  10,000 km out, far beyond a world tens of kilometres wide — and the renderer subtracts the
  camera in `f64` before it narrows (§2.4).
- **Depth.** `ViewDepth::WORLD` is one reversed-Z pass from 1 cm to 1e4 km: a 32-bit float
  depth buffer under a reversed projection keeps ~1e-7 relative precision across it.
  `forge-render` draws the passes the resolver names, back to front, each with its own depth
  clear (`forge_render::depth`).
- **Errors.** `FRAMES-0001` (a frame the resolver does not know) and `FRAMES-0002` (no path
  between two frames); a resolver's own errors arrive as `FrameError::Extension`, each with
  its own registered code.
- **Tests** of the consumers use `forge_frames::testing::FixedFrames`, a resolver double
  with fixed frames placed in the world, so the seam's non-trivial paths are exercised in
  every edition.

---

# Chapter 3 — Determinism & the Math Contract

## 3.1 The three failure modes, and why each is not obvious

**1. `f32` mantissa.** 24 bits. Any `f32` path — GPU compute, a material graph, SIMD with
fast-math — *will* diverge from the CPU. The Foundations already states the fix:

> **Do the noise basis in integer / fixed-point voxel coordinates, never float world
> position.** Hash integer voxel coords → derive the float you feed the noise.

**2. Convergence loops.** `while err > eps` takes a different iteration count on a different
compiler, platform, or optimisation level. Generalise it: *no iteration count in the
generation pipeline may depend on a computed value.*

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
/// Amended (ADR 0003): the seed is the raw `u64` — forge-num is the leaf of Ch.1.1 and
/// cannot name forge-seed's `Seed`, which wraps and derives it and hands its raw value down.
/// `IVec3` is forge-num's own `{ x, y, z: i64 }`.
pub fn hash3(x: i64, y: i64, z: i64, seed: u64) -> u64;
pub fn value_noise_i(p: IVec3, seed: u64) -> f64;
pub fn simplex_i(p: IVec3, scale_log2: i8, seed: u64) -> f64;
/// Added (ADR 0003): the octave sum the §3.4 positive control perturbs, and §3.3's reduction.
pub fn fbm_i(p: IVec3, base_scale_log2: i8, octaves: u32, seed: u64) -> f64;
pub fn tree_sum(xs: &[f64]) -> f64;
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

2. Hash each with BLAKE3.
3. Compare against `tests/determinism/golden.txt`, committed.
4. **Positive control (W2):** a `#[cfg(feature = "mutate-det")]` build perturbs one
   noise octave by 1 ulp and the job asserts the comparison **fails**. A determinism gate
   that cannot demonstrate failure is decoration.

A golden change requires a plan amendment and a one-line reason in `docs/adr/`.

> **Superseded wording (E-33, D-2):** the macOS-aarch64 leg is dropped; the legs are
> ubuntu-x86_64 and windows-x86_64.

## 3.5 Client/server divergence is *detected*, never assumed away

The Foundations' rule, adopted verbatim: for an unedited chunk the server sends `"baseline, hash =
X"`; the client generates locally and verifies. Bandwidth is ~zero in the common case
**and float-divergence surfaces as a mismatch rather than as a wallhack-adjacent bug.**
Keep this even after §3.4 is green. The gate proves the platforms we test; the hash check
covers the ones a player brings.

## 3.6 Implementation (WP-01, ADR 0003)

What is built, so the next chapter can rely on it without reading the code:

- **`forge_num::det`** — `sin cos exp ln pow atan2` vendored, `sqrt` forwarded. Built only
  from IEEE `+ - * /`, `sqrt`, integer ops and bit manipulation: no FMA (Dekker products
  instead), no platform `floor`/`round`, no data-dependent loop counts. **Faithfully
  rounded**: worst measured error 0.778 ulp over 3.1 M samples against an independent
  ~2^-90 reference (`crates/forge-num/tests/accuracy.rs`, table in ADR 0003). Every NaN
  returned is the canonical `0x7FF8_0000_0000_0000` (hardware default NaNs differ in sign
  between x86-64 and aarch64). Every embedded constant (2/pi bits, pi/2 and ln 2 pieces,
  atan(j/8)) is re-derived from first principles by a test-only bignum on every run.
- **`newton::<N>`** — exactly `N` steps, no convergence test.
- **Noise basis** — `hash3` (SplitMix64 finaliser chain), `value_noise_i`, `simplex_i`,
  `fbm_i`. `simplex_i` is exact integer arithmetic up to the kernel weights (3D simplex's
  skew factors are exactly 1/3 and 1/6, so a fixed-point denominator of `6·2^scale` makes
  skew, cell, ordering, corners and the support test integers): identical bits everywhere,
  equal detail at the origin and 2^50 units out. `scale_log2` is clamped to [-24, 56].
- **`tree_sum`** — the §3.3 fixed-order reduction; `tree_split(n)` exposes the split so a
  parallel reduction brackets identically.
- **Guards** — I2 (`tests/determinism/test_cross_platform_hash.rs`, 276 golden rows, the
  `mutate-det` control spawned as a real feature build and required to fail on exactly the
  256 chunk rows); forge-num's `clippy.toml` bans `f64` transcendentals and `mul_add`;
  `crates/forge-num/tests/test_no_platform_libm.rs` scans the source and requires an empty
  `[dependencies]`. Gate rows: I2 BOUND, S2 BOUND, `C-determinism-linux-leg` AWAITING.
- **Cost** — 2.3–3.5x std for sin/cos/exp/ln, 4.0x for pow, 3.4x for atan2, 49 ns per
  simplex sample (ADR 0003 has the table). WP-02 (ADR 0004) made `ln_dd` table-driven
  (pow 10x -> 4x), and removed two of atan2's five divisions and replaced its general
  double-double adds with ordered ones (5.6x -> 3.4x); no golden hash moved and the
  accuracy table is unchanged.
- **Spike S2: resolved, fallback not needed** (Windows observed; the ubuntu leg is
  observed locally in Docker, WP-30: the corpus digest equals Windows').

---

# Chapter 4 — Seed Algebra

## 4.1 The rule

```rust
impl Seed {
    /// Deterministic, associative-free, collision-resistant descent.
    /// `tag` is a compile-time string so the derivation path is greppable.
    pub fn child(self, tag: &'static str, index: u64) -> Seed {
        Seed(splitmix64(self.0 ^ fnv1a(tag).rotate_left(17) ^ splitmix64(index)))
    }
}
```

## 4.2 Why not a PRNG stream

Because a stream has *position*, and position is state. Two workers generating the same
chunk must agree without communicating; a stream forces them to agree on how many numbers
they have drawn. Derivation has no such requirement — which is exactly what makes the Foundations'
Layer 1 stateless.

**Corollary, and it is a strong one:** any generator that consumes randomness *in a
loop whose length depends on data* has smuggled a stream back in. Guard:
`test_seed_algebra` asserts that every registered generator produces identical output
when its inputs are evaluated in a different order.

## 4.4 Implementation (WP-02, ADR 0004)

- **`forge_seed::Seed`** — `Copy`, no `&mut` method, no "next". `Seed::root(u64)`,
  `child(&'static str, u64)` exactly as §4.1 (`splitmix64` is SplitMix64's output for
  state `x`; `fnv1a` is 64-bit FNV-1a; both pinned to published test vectors), `raw()` to
  hand the value to forge-num's noise basis. Equality and hashing use the value only.
- **Provenance** — `Seed::path() -> Option<SeedPath>`: recorded in debug builds by an
  interning recorder (each distinct step stored once, capped at 2^20 steps, never read by a
  value), `None` in release. Errors `SEED-0001..0002`.
- **Registry** — a generator is a `Generator { name, eval: fn(Seed, [i64; 3]) -> u64 }` in a
  `&[Generator]` list its crate exports (`forge_seed::BASIS`: the forge-num noise basis
  and position-keyed derivation). Generator crates append their list to
  `test_seed_algebra::registry()`.
- **Guard (I3)** — `tests/determinism/test_seed_algebra.rs`: the formula and test vectors,
  derivation order/thread independence, every registered generator evaluated backwards,
  shuffled, on threads and interleaved; positive control a generator with a call counter.
  Gate row I3 BOUND.

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

The Foundations on Folia:

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
- **Two regions may only merge if they share a frame** (the Foundations correction to invariant 6).
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
  (W7). a previous project DEFECT 172 was exactly this: a `VoxelMap` integrated from an
  async loop and queried from a dispatch thread, with a "serialized" claim that meant
  "against each other," not "against the event loop."

## 5.5 Implementation (WP-03, ADR 0005)

What is built (M0-9), and what is not yet:

- **`bevy_ecs` as a library** — pinned `=0.19.1`, `default-features = false, features =
  ["std"]` (no `bevy_reflect` until Ch.6 decides, no executor, no `multi_threaded`), once, in
  `[workspace.dependencies]`; `forge_core::bevy_ecs` re-exports it so every crate derives
  against the same pin. S6 baseline in ADR 0005.
- **Error contract (§1.2)** — `forge_core::{Error, ErrorCode, Ctx, CodedError}` with the
  pinned fields. A crate above forge-core implements `CodedError` and gets
  `From<ItsError> for Error`; forge-core implements it for `FrameError`/`SeedPathError` by
  parsing their `code()`. Codes `CORE-0001..0013`.
- **Types** — `EntityId` (wraps `Entity`, ordered by `(index, generation)`), `World`
  (bevy world + region table + `Tick`; no `&mut` escape hatch), `RegionId`, `Region`
  (frame, profile selector, sorted entity set), `Regions` (create / assign / transfer /
  release / merge (same frame only, `CORE-0004`) / split (atomic) / remove),
  `RegionAccess` (declared reads/writes), `RegionView` (hands out `&`/`&mut` only to owned
  entities and declared components — `CORE-0005`/`CORE-0006`, never a panic), `Outbox`
  (effect / transfer / spawn / despawn, applied at the barrier in (region, system, send)
  order, stale messages rejected with codes), `RegionSystem` exactly as §5.3.
- **Scheduler skeleton** — plan (consecutive non-conflicting systems form a stage; a stage
  over every region is a wave), prove (jobs real; same-region jobs in a wave do not
  conflict; entity sets disjoint — incrementally, by replaying the ownership journal),
  run (single-threaded, jobs of a wave in order), barrier (once per tick). A wave is exactly
  what M4-1 will hand to threads.
- **Profiles (§1.4)** — `ProfileSelector`: stub classifier over the four measurements plus
  `fluid_fraction`, value bands and a dwell of 4. Thresholds are provisional until M4.
- **Guards** — `crates/forge-core/tests/test_region_disjointness.rs` (proptest over fuzzed
  spawn/assign/transfer/merge/split/remove/tick sequences; control: the
  `mutate-disjointness` build fails it and the scheduler refuses the overlap, `CORE-0007`);
  `profile.rs::positive_control_no_hysteresis_thrashes`. Gate rows
  `C-region-disjointness`, `C-profile-hysteresis` BOUND.
- **Not built here** — interaction-graph region derivation and the merge-on-proximity /
  split-on-load *policies* (the primitives exist); parallel execution (M4-1);
  `test_region_determinism` and `test_thread_boundary` (they need threads to mean anything).

**Spike S4 resolved (WP-17, ADR 0038; M2-9).**

- **Correctness under load** — `crates/forge-core/tests/test_scheduler_s4.rs` (gate
  `C-scheduler-s4`): at 4, 16 and 64 regions, two seeds each, 120 ticks of 2,048 entities under
  three systems (integrate; a per-entity write plus a barrier effect into the next region from
  every region; ~2 % of entities handed to another region per tick), with 1–3 merges or splits
  between every pair of ticks, a refused cross-frame merge, spawns and despawns. After every
  tick the plan and the ownership table verify from scratch, every live entity is owned exactly
  once, no barrier message is rejected; across region counts the integrated state is identical
  (the layout is a scheduling decision, never an outcome). 4,400–12,500 barrier messages and
  270–350 merges and splits per run; 4.8–5.3 ms/tick in a debug build. Control: the
  `mutate-disjointness` build fails it on an S4 violation.
- **Contention** — `cargo run --release -p forge-core --example s4_contention` (100,000
  entities, ~60 ns per entity-step, 8 worker threads, workers spawned once):

  | Regions | forge-core serial tick | owned + barrier: 1 → 8 threads | barrier | lock wait | shared queue: lock wait | region locks: lock wait |
  |---|---|---|---|---|---|---|
  | 4 | 6.59 ms | 2.25 → 0.78 ms (×2.9) | 0.086 ms | 0 | 0.43 ms | 0.22 ms |
  | 16 | 6.78 ms | 2.27 → 0.46 ms (×4.9) | 0.114 ms | 0 | 0.83 ms | 0.43 ms |
  | 64 | 7.21 ms | 2.31 → 0.40 ms (×5.7) | 0.140 ms | 0 | 0.42 ms | 0.73 ms |

  The design as built (a job owns its region and its outbox; everything cross-region waits for
  the serial barrier) has **no lock contended during a wave**, by construction; the serial
  barrier costs ~0.1 ms per ~2,000 messages. Both Appendix B fallbacks are worse: one shared
  message queue spends 0.4–0.8 ms/tick waiting on its lock, and per-region locks (effects
  applied at once, no barrier) wait 0.2–0.7 ms/tick — growing with region count, since a job's
  effect waits for its neighbour's whole job — and give up the barrier's fixed order. Speed-up
  is bounded by regions per thread: 4 regions keep 4 of 8 threads busy; ≥ 16 regions on 8
  threads balance. The serial scheduler's ~4.5 ms over the prototype's per-thread work is the
  per-access `RegionView` check and the bevy lookups (ADR 0007's ~25 ns/entity-access),
  which M4-1's proven-disjoint slices remove. **Decision: keep per-job outboxes and the serial
  barrier; neither fallback is needed; M4-1 splits for ≥ 2 regions per worker.**

**Spike S6 resolved (WP-17, ADR 0038; M2-10).** Two `bevy_*` upgrade cycles, each measured
on a clean export of the tree with only the workspace pins changed, checking and testing every
crate that names `bevy_ecs` or `bevy_reflect` (forge-core, forge-frames, forge-reflect,
forge-cmd, forge-sim, forge-editor, forge-tests):

| Cycle | Source changes | Tests | Unique deps (`bevy_ecs` / `bevy_reflect`) |
|---|---|---|---|
| 0.18.1 → 0.19.1 (taken; measured backwards) | 3 files, 4 lines (`bevy_reflect::enums` / `structs` became public modules) | green, the four-outputs and command-contract guards included | 53 / 41 → 56 / 42 |
| 0.19.1 → 0.20.0-rc.1 (the newest published) | none | green, the determinism and replay goldens and the four-outputs guard included | 56 / 42 → 59 / 45 |

Churn is far inside budget: no vendoring; the pins stay exact and the next upgrade waits for
0.20.0 final. The fallback's trigger is written down in ADR 0038 (a cycle costing more than a
working day, or breaking a determinism golden).

---

# Chapter 6 — Reflection, Annotation & the Four Outputs

**This is the elegance keystone. One annotation, four outputs.**

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
| A **command variant** if it mutates | the bus, undo, provenance | 7 |

**Rules:**
- Anything a game can call is `#[forge_api]`. A public item that is not is an internal
  detail and the API-surface test will say so.

**Guard:** `test_four_outputs_agree` — for every `#[forge_api]` item, assert that the
node descriptor, the schema fragment, and the reflect registration describe the same
arity and the same types. A drift here is silent and catastrophic.

## 6.1 Implementation (WP-04, ADR 0008)

`forge-reflect` (runtime) and `forge-reflect-macros` (the attribute, at
`crates/forge-reflect/macros`). `bevy_reflect` is taken as a library at `=0.19.1` (the
`bevy_ecs` pin, S6) with `std` + `reflect_documentation` — this is the decision §5.5
deferred to this chapter.

`#[forge_api]` goes on a **free fn**, a **struct with named fields**, or an **enum** with
unit or named-field variants (generics, methods, tuple structs/variants: compile errors,
follow-ups). A doc comment is mandatory. The macro parses once and emits the outputs as
separate token lists:

| Output | For a fn | For a struct / enum |
|---|---|---|
| 1 · reflect | `<T as Typed>::type_info()` of each by-value parameter and the success return type; types registered in the `TypeRegistry` | `#[derive(Reflect)]`; its `TypeInfo` (field names, order, docs) |
| 2 · node | `NodeDesc`: inputs = by-value params, output = return value, context = reference params, purity, fallible | make node (inputs = fields) / select node (variants) |
| 3 · schema | object of the inputs (`required` in order), `x-forge-purity`, `x-forge-returns`, `x-forge-command`, `$id: schema://forge/<path>` | the type's definition under `$defs` (enum: externally tagged `oneOf`) |
| 4 · command | `CommandDesc` (`ApplyImpulse`, fields = inputs) iff purity is `Mutate` | — |

**Purity is inferred**: a `&mut` reference parameter → `Mutate` (a command, I7); `&` →
`Read`; none → `Pure`. Reference parameters are the execution context, never pins. `pure` /
`reads` / `mutates` in the attribute assert the inference (mismatch = compile error);
`destructive` marks a mutating fn that needs a capability grant . A fn returning `Result<T,
_>` is fallible with output `T`.

Registration is explicit — `ForgeRegistry::register::<T>()` (a fn is named by its identifier)
— and **refuses** an item whose annotations do not fit its pin kinds (`REFLECT-0002/0003`) or
whose four outputs disagree (`REFLECT-0006`). `ForgeRegistry::schema_document()` is the
`schema://` document (every type under `$defs`, every fn under `x-forge-functions`).

## 6.2 Inspector metadata attributes (WP-U6 generates the inspector from these)

On a field, variant field or fn parameter: `#[forge(...)]`. For a fn's return value:
`#[forge_api(returns(...))]`. On the item: `#[forge_api(name = "...", category = "...")]`.

| Attribute | Meaning | Node pin | Schema keyword | Inspector |
|---|---|---|---|---|
| *(doc comment)* | tooltip (fields); `doc = "..."` for fn params | `meta.doc` | `description` | tooltip |
| `name = "..."` | label; default = identifier humanised (`max_speed` → "Max Speed") | `meta.display_name` | `title` | row label |
| `category = "..."` | group | `meta.category` | `x-forge-category` | collapsible group, first-seen order |
| `units = "N·s"` | physical unit (grammar below); compile error if unknown | `unit` (parsed) | `x-forge-unit`, `x-forge-dimension` | value suffix |
| `min = a`, `max = b`, `range = a..=b` | inclusive bounds (scalars only) | `meta.min/max` | `minimum` / `maximum` | clamp, slider ends |
| `step = s` | drag/spin increment (> 0) | `meta.step` | `x-forge-step` | increment |
| `read_only` | shown, never edited (no edit command) | `meta.read_only` | `readOnly` | disabled editor |
| `hidden` | not shown (still a pin and serialised) | `meta.hidden` | `x-forge-hidden` | omitted |
| `widget = "..."` | editor hint (`slider`, `angle`, `color`, ...); unknown → default for the kind | `meta.widget` | `x-forge-widget` | editor choice |
| `entity` | an entity reference (`EntityId` only) | `meta.entity` | `x-forge-entity` | picker / drag-drop |

`units` is legal on `Int`, `Float`, `Vector` pins; `min/max/step` on `Int`, `Float`;
`entity` on `Entity` — anything else is `REFLECT-0002` at registration.
`ForgeRegistry::inspector(type_path)` returns the resolved rows (`InspectorDesc`).

## 6.3 Units

One grammar (`crates/forge-reflect/src/units_core.rs`), compiled into both the macro and the
runtime. Factors joined by `·`, `*` or space, at most one `/` (everything after it is the
denominator; parentheses optional), exponents `^n` or superscripts. `connect(from, to)`:
identical types (`REFLECT-0007`), and if both pins carry units, equal dimensions
(`REFLECT-0008`), returning the conversion scale. A pin with no unit is unchecked.

## 6.4 Guard

`tests/liveness/test_four_outputs_agree.rs` (gate row `C-four-outputs-agree`, **bound**).
`check_agreement` compares, for every item: arity, names and order (reflect / node / schema
`required` / command), type identity (`TypeInfo::type_path` / pin / `x-forge-type` / command),
value shape (reflect kind / pin kind / resolved schema), every annotation, docs (reflect
`docs()` for types / node / schema / command), purity ⇔ command, fallibility, the return pin.
Positive controls: 16 kinds of drift applied to registered items must each be refused, and a
`mutate-drift` build of the macro (drops the last node pin/field/variant) must fail the guard
with `REFLECT-0006`. The guard lives in `forge-tests`, which does not depend on `bevy_reflect`,
so it also proves the macro works from a crate that depends on forge-reflect alone. Every
crate that lands `#[forge_api]` items registers them in its `engine_registry()`.

**Not built here**: the API-surface test (rule 1 above), generics, methods, tuple shapes,
`Vec`/`Option` pins.

---

# Chapter 7 — The Command Bus, Undo & Provenance

## 7.1 Shape

The editor is a **headless core** plus clients. The UI is a client. Scripts, tests, CI and
the CLI are clients. There is exactly one way to change project state.

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

## 7.4 Implementation (WP-05, ADR 0009)

`crates/forge-cmd`. The `Bus` is the in-process `CommandSink` and **owns the `Project`**
(entities keyed by a stable, never-reused `EntityKey`, a hierarchy, reflect-path properties
of `Value`, project settings); `Project`'s mutators are crate-private, so outside the crate
the only way to change it is a command. `CommandSink` gains `redo(txn)` beside the three
pinned methods.

| Piece | What it is |
|---|---|
| `EditorCommand` | `Spawn`, `Despawn` (subtree), `Rename`, `Reparent`, `SetProperty`, `RemoveProperty`, `SetSetting`, `Invoke { target, args }`; `Reflect` + serde |
| `Diff` / `Change` | `Created`, `Removed`, `Renamed`, `Reparented`, `Property`, `Setting`, each with `before` and `after` — invertible by construction |
| `DiffBuilder` | how every command (built-in or `Invoke` handler) describes its effect over `&Project`; reads see the command's own earlier changes |
| `apply` | atomic: each change checks its `before`; a failure rolls back (`CMD-0008`); a panic while planning is `CMD-0009`, never an unwind |
| transactions | fresh `TxnId` = one undo step; `begin` / `commit` / `cancel` for gestures; value changes merge (earliest `before`, latest `after`, a no-op drops out); `undo(open)` = cancel |
| undo / redo | inverse / forward diff with precondition checks: selective undo works when changes still apply, a conflict is refused; `undo_target` / `redo_target` for Ctrl+Z / Ctrl+Y; bounded history |
| `CommandPolicy` | `human_only`, `undoable`, `redoable` — the primitive for "undo and redo never grant" (Ch.21.18) |
| provenance | `Issuer` on every envelope, transaction, `Applied` event and `AuditEntry`; every refusal is audited; `undo_as` / `redo_as` attribute the undo to the asker |
| `Applied` stream | `Bus::subscribe()`: `Arc<Applied>` per event (`Command`, `Commit`, `Cancel`, `Undo`, `Redo`), in `seq` order |
| memory | bounded in a long session: only live transactions keep a record (retired ones leave a tombstone, then a watermark), a record's commands are a `CommandSpan`, duplicate refusal uses a 4096-id replay window, gesture frames fold into one audit entry per target, the undrained audit is capped with a counted overflow; `Bus::footprint()`, pinned by `crates/forge-cmd/tests/test_bus_memory.rs` |

**Guards.** `tests/liveness/test_command_contract.rs` (I8, bound): `forge_cmd::contract::
check_contract` over every variant listed by `EditorCommand`'s reflected `TypeInfo` — dry run
does not change the state hash and equals `apply`'s diff, one undo entry, undo and redo restore
the hashes, the `Applied` issuer is the envelope's, serde (wire) and `FromReflect` round-trip;
positive control: seven sinks that each break one clause, a sample list missing a variant, a
vacuous and an invalid sample. `crates/forge-cmd/tests/test_undo_fuzz.rs` (`C-undo-fuzz`,
bound): fixed-seed random sequences of commands, gestures, cancels, undos, redos, selective
undos, invalid and panicking commands; undo all ⇒ start hash, redo all ⇒ end hash; positive
control: the `mutate-undo` build (merge keeps the newest `before`) must fail.
`tests/liveness/test_command_liveness.rs` (I7): resolves every path in every forge-editor item
through the import graph to its definition and refuses references to project-state writers
(`forge_core::World`, `std::fs` writes) outside `tests/liveness/i7_allow.txt`; positive
controls: ten bypass spellings, and the plan's mutant handler injected into forge-editor
(`C-command-liveness-resolver`, bound). I7 itself stays `Unbuilt` until forge-editor exists
(WP-U4); the check then runs on it automatically.

**Not built here**: `RemoteBus` (M2-16), the shell's `ProjectMirror` / `CommandEmitter` /
`Gesture` (WP-U4), audit-log persistence (the store drains it), sibling order (children are
in key order), the security-class commands themselves (WP-U9).

**Amended in WP-06 (ADR 0010).** `apply`, `commit`, `cancel`, `undo` and `redo` return
`Arc<Applied>` (one allocation shared with every subscriber). Each subscription is bounded
(`DEFAULT_SUBSCRIPTION_CAPACITY`, `subscribe_with_capacity`); a stalled subscriber's queue
collapses into one `Gap { first_missed, last_missed, missed }` and it resyncs from
`Bus::project()` + `Bus::next_seq()`. Audit folding happens only while the gesture's own
lines are the newest in the trail (never across another entry); `AuditEntry::last_at_ms`
spans the folded frames; `drain_audit` keeps capacity and `drain_audit_into` reuses the
caller's buffer. `Change::Property::path` is an `Arc<str>` interned through the project's
property map, so a drag frame allocates no path (3-property gizmo frame: 3 590 → 2 270 ns).
I8 gains a totality clause (`check_totality`): a panicking `Invoke` must be `CMD-0009` from
both `dry_run` and `apply`.

**Amended in WP-17 (ADR 0037), additive — the split editor's wire and prediction.**
`Applied`, `AppliedKind`, `Rejection`, `CmdError`, `TxnState` and `Gap` are serde types
(the remote wire carries them). `Project` has a **canonical wire form** (entities in key order
with their properties, settings, the key allocator); deserializing rebuilds the project through
the same checked `apply_all` a command goes through, after the builder's name / path / value
checks, so a malformed snapshot is refused (a missing parent, a cycle, a bad name) and equal
projects serialize to equal bytes. **`Replica`** is a client's copy that follows the `Applied`
stream and carries **predictions**: `predict` applies a planned diff on top, `reconcile` rolls
the predictions back (inverses, newest first; a predicted spawn restores the key allocator),
applies the authoritative events, forgets the settled ones and re-applies the rest (one that no
longer applies is dropped), `authoritative` rolls back on a copy. **`Bus::plan_for`** plans a
command from an issuer against another project with this bus's planners, policies and reserved
settings. Nothing outside a bus can reach a bus's project: a `Replica` is not a sink.

---

# Chapter 8 — Asset System, VFS & Import

**Contract:** content-addressed store; stable `AssetId` independent of path; async load
with typed handles; hot reload on file change; importers are `#[forge_api]` plugins.

**Decisions:** glTF 2.0 and USD are first-class; FBX via `ufbx` (MIT) only. Textures
transcode to KTX2/Basis at import. `.forge` scene format is RON-over-reflect, diffable,
and merge-friendly — because "my scene file is an unmergeable binary blob" is a real
reason teams leave Unity.

## 8.1 Implementation (WP-12, ADR 0015)

`crates/forge-asset`, above `forge-store` / `forge-plugin` / `forge-cmd` / `forge-seed`
(and `forge-num` for the deterministic sRGB tables). "`#[forge_api]` plugins" is realised as
extension points (`Importer`, `Exporter`, `AssetType`, Ch.32.2) registered by source plugins;
the kernel's asset types are `forge.asset`, and the first-party importers and exporter are the plugin
`forge.importers` (`plugins/forge-importers`, WP-15), both loaded through the ordinary loader (I16).

| Piece | What it is |
|---|---|
| `AssetId` | 128 bits, 32 hex digits. `mint(path, content)` once at first import; `child(label)` for sub-assets; `generated(generator, seed_path)` for generated assets |
| sidecar | `<source>.meta.ron`: `AssetMeta(id, importer, settings: {k: v}, import: Some(ImportRecord(importer, importer_version, source, source_size, settings, deps, artefacts)))`; committed with the source; the record is a cache the server regenerates |
| artefact | kind-tagged bytes stored with `blob_put` (BLAKE3). `mesh` = `FMSH` LE binary (GPU-layout streams, `u32` indices); `texture` = KTX2 RGBA8 with a full mip chain; `material`, `scene` = RON |
| `Vfs` | the only storage path: a `ProjectStore` (I17) with a cached listing and **exact** reference resolution (M0-17) |
| `AssetServer` | the database: `open`, `import`, `reimport`, `rename`, `remove`, `poll` (hot reload), `load::<T>`, `thumbnail`, `export`, `generated`, `assets` / `assets_of_kind` / `info` (what the asset browser lists) |
| `Handle<T>` | typed (checked against the kind's registered `TypeId`), async (`wait`, `ready()` future, `state()`), shared per asset, swapped in place on reload |
| commands | `forge.asset.import` / `remove` / `rename` (undoable) and `adopt` (not undoable); `AssetServer::sync(&Project)` follows the project |

## 8.2 Importers

| Importer | Extensions | Output |
|---|---|---|
| `gltf` | `.gltf`, `.glb` | main `scene` (default scene, parents before children, TRS in `f64`); `mesh:<name>` per mesh (all primitives; POSITION, NORMAL, TANGENT, TEXCOORD_n, COLOR_n, JOINTS_n, WEIGHTS_n; fans/loops rewritten); `material:<name>` (metallic-roughness, alpha mode, textures by id); `texture:<name>` per image, plus `@linear` when an image is also used as data (normal, metallic-roughness, occlusion) |
| `image` | `.png`, `.jpg`, `.jpeg` | one `texture`; settings `srgb` (default `true`), `mips` (default `true`) |
| `ktx2` | `.ktx2` | one `texture`, the file byte for byte after validation |

Buffers and images come from the GLB chunk, a base64 `data:` URI, or a relative `uri` read
through `ImportCx::dependency` — never the filesystem. Labels use unique names, so
reordering in the authoring tool keeps ids. Unbuilt (gate row `C-asset-usd-fbx`): USD and
FBX (`ufbx`) importers — both are peers on the same point. Exporter: `gltf` writes a
self-contained `.glb` (streams byte for byte, factors narrowed exactly, textures as PNG).

## 8.3 The dependency graph and reimport invalidation

A record is **fresh** iff: same importer id and version, same source hash, same settings
hash, every recorded dependency still exists (exact spelling) with the same hash, and every
artefact blob is present. Fresh ⇒ the importer does not run. Otherwise it runs and the new
artefact addresses are compared with the old: only artefacts whose address changed reload
under live handles (and a source's thumbnails re-render). The server keeps the reverse map
dependency → sources, so a changed `.bin` or texture reimports every glTF that read it.
Bumping `Importer::version` makes all of that importer's records stale on the next open.

## 8.4 Hot reload

`AssetServer::poll` diffs `ProjectStore::stamps()` (additive in WP-12, see §33.6) against
the previous poll: changed sources and dependents reimport (content-checked — a touch is a
no-op); a source renamed without its sidecar is re-attached by content — new files are
compared by size (`ProjectStore::size`, metadata only) before any is read, and each is
hashed at most once per poll (a pair rename is
recognised by id); a hand-edited sidecar's settings reimport; a deleted source keeps its
last value and its sidecar (its return restores the id); a deleted sidecar removes the
asset; a new importable file is reported `Discovered` (the editor may issue the import
command). A quiet poll is one `stamps()` call; after acting on changes the poll re-stamps only
the files it wrote itself (`stamps_of`), so it never walks the project twice and never
swallows an edit made to another file meanwhile. `poll_interval()` tells the editor's timer
when to poll next: 20× the last `stamps()` cost (≤ ~5 % of one core), clamped to 250 ms–2 s.

**Idle cost on a real folder, and the plan past it (ADR 0017).** `LocalFs::stamps` is one
directory walk that reads no file: ~3 µs per file on NTFS (release; 10 000 files ≈ 16–30 ms,
100 000 ≈ 375 ms). Budget, asserted in release by `forge-store/tests/test_local_stamps.rs`:
≤ 60 ms per 10 000 files. That keeps projects up to a few 10k files well inside 5 % of a core.
Past that, the next step is a change feed behind the `ProjectStore` trait (additive
`ProjectStore::changes()`; `LocalFs` backed by `ReadDirectoryChangesW` / inotify, with the
walk kept as the resync after an overflow), which makes idle cost independent of project
size. A per-directory mtime short-circuit was rejected: a directory's mtime changes when
entries are added or removed, not when a file's content changes, so it cannot skip a file.

## 8.5 Generated assets

`register_generator(name, kind, fn(&SeedPath) -> bytes)` and `generated(name, seed)` give an
asset whose id is `AssetId::generated(name, seed)`: listed, loaded through the same typed
handles, thumbnailed and exported like a file-backed one, but never stored — it is
regenerated from its seed path on the workers (Ch.33.2's third row).

## 8.7 Thumbnails (for WP-U5's asset browser)

`thumbnail(id, size)` → `Handle<Thumbnail>` (square RGBA8): texture = best mip box-filtered,
aspect kept; mesh / scene = isometric flat-shaded software raster framed to the bounds;
material = lit swatch in the base colour × the texture's 1×1 mip. Deterministic, rendered on
the workers, refreshed on reload, never written to the project.

## 8.8 Guards

| Test | Gate row | Positive control |
|---|---|---|
| `crates/forge-asset/tests/test_gltf_roundtrip.rs` | `C-asset-gltf-roundtrip` | a chained exporter that drops normals fails the import → export → import comparison |
| `crates/forge-asset/tests/test_asset_hot_reload.rs` (memory + `LocalFs`) | `C-asset-hot-reload` | a store with length-only stamps misses the same-size `.bin` edit |
| `crates/forge-asset/tests/test_asset_id_stable.rs` | `C-asset-id-stable` | moving a file and losing its sidecar with the editor closed mints a new id, and the check says so |
| `crates/forge-asset/tests/test_asset_ref_exact.rs` | `C-asset-ref-exact-case` | a mutant glTF importer that resolves `uri`s by asking the store (on an NTFS-like case-insensitive store) imports the wrong-case reference, and the same `ASSET-0002` check fails |
| `crates/forge-asset/tests/test_asset_store_only.rs` | `C-asset-store-only` (I17) | every `std::fs` / `File::open` / `OpenOptions` / `read_dir` spelling in synthetic sources is caught; look-alikes are not |

Also: the asset points in `tests/plugin/test_extension_point_replaceable.rs`, the I8 contract
over the asset commands, and `.gltf` in the M0-17 lint.

## 8.9 Measured (`test_import_perf.rs`, MemoryStore, 12-thread dev box)

| Scene | Build | Cold import | Fresh reimport | Load 500 meshes | 128 px thumbnail | Reopen |
|---|---|---|---|---|---|---|
| 500 objects × 25 verts, 8 materials, 2 textures | release | 7.4 ms | 1.5 ms | 2.3 ms | 3.9 ms | 2.5 ms |
| 500 objects × 1089 verts (28.3 MiB `.bin`) | release | 51.9 ms | 8.2 ms | 4.3 ms | 69 ms | 9.6 ms |
| 500 objects × 1089 verts | dev (`opt-level = 2` for forge-asset) | 70.5 ms | 12.7 ms | 5.9 ms | 78 ms | 14.9 ms |

A quiet poll costs 1–5 µs on `MemoryStore` (its stamps are stored hashes). On `LocalFs` it is one
directory walk: 10 000 files + the 500-object scene 16–34 ms, 100 000 files ≈ 375 ms (release,
NTFS; `test_import_perf.rs`, `forge-store/tests/test_local_stamps.rs`) — the old per-path
`stat` took 0.5–0.8 s at 10 000 files. A just-written 28 MiB file is hashed at most twice, then
never again. Reopening a fresh project runs no importer.

**Not built here:** block compression (BC7/ASTC via Basis, `C-asset-block-compression`,
with forge-gpu); USD/FBX importers (`C-asset-usd-fbx`); the `.forge` scene format (Ch.8
decision; it lands with the scene/prefab work — `SceneAsset` is the imported-hierarchy
artefact, not the editable scene); read-only VFS mounts for engine/plugin content; parallel
import of many sources at open (imports run on the calling thread; loads and thumbnails on
the pool); the `AssetIndex` adapter for forge-ui's picker (WP-U5, `C-ui-asset-index`).

---

# Chapter 9 — GPU Device Layer & Render Graph

**Contract:** `wgpu` is the only graphics API surface. An **adapter pool**, not an
adapter — `Vec<Device>` from day one, even when the length is 1, because retrofitting
plurality is the expensive version.

**Decisions:** WGSL only, compiled by `naga`; bindless where available with a bound
fallback; a declarative render graph with automatic barrier/alias computation; every pass
declares reads/writes (same discipline as `RegionSystem`, deliberately).

**Expansion brief** (answered below, WP-08): resource lifetime and aliasing; transient
allocator; cross-adapter transfer primitives (see Ch.26 and **spike S1**); shader hot
reload; a `wgpu-hal` passthrough escape hatch for Vulkan external memory, isolated behind a
feature so the portable path never depends on it.

**DoD seed:** the same frame renders identically on 1 adapter and on 2, with the second
adapter doing only compute. *(Bound on the real cross-device path by `test_multi_adapter`;
on a one-GPU machine the second member is the software rasteriser, so the
two-physical-GPU case is `S1` AWAITING, §9.6.)*

## 9.1 Implementation (WP-08, ADR 0019)

`crates/forge-gpu`, above `forge-core` (error contract) and `forge-store` (shader sources
through `ProjectStore`, I17). It is one of the three crates allowed `unsafe` (Ch.1.5); the
only `unsafe` is in `probe.rs` (two reads through `wgpu-hal`, §9.2), every block carries
`// SAFETY:`, and `clippy::undocumented_unsafe_blocks` is denied in the crate. It
re-exports `wgpu`, so the workspace names one copy (`forge_gpu::wgpu`).

| Module | What it is |
|---|---|
| `floor` | the O-8 floor as a pure function of `AdapterFacts`; the user-facing `GPU-0002` message |
| `probe` | `AdapterFacts` from the driver: Vulkan `apiVersion`, D3D12 feature-level probe, compute flag |
| `pool` | `AdapterPool`, `GpuDevice`, `PoolOptions`, `SoftwarePolicy`, `plan_selection` (pure), the per-adapter report |
| `transfer` | `upload_buffer`, `read_buffer`, `read_texture`, `write_texture`, `copy_buffer_across`, `copy_texture_across` |
| `graph` | `RenderGraph`, versioned `Handle`s, `PassBuilder`, `compile` → `GraphPlan` (order, culling, lifetimes, aliasing slots, barriers), `verify_plan` |
| `exec` | `CompiledGraph::execute`, `PassContext` (declared-access check), `Imports`, `TransientPool` |
| `shader` | `validate_wgsl`, `compile_wgsl`, `ShaderLibrary` (hot reload) |

Error codes `GPU-0001`..`GPU-0010` (`docs/error-codes.md`). No wgpu error panics the
process: each pool device installs an uncaptured-error sink
(`GpuDevice::take_uncaptured_errors`), and graph execution and shader creation run inside
validation error scopes and return `GPU-0010` / `GPU-0004`.

## 9.2 The adapter pool and the O-8 floor

`AdapterPool::new(&PoolOptions)`:

1. **Enumerate every adapter** on Vulkan, Direct3D 12 and Metal (`WGPU_BACKEND` overrides).
   OpenGL is not enumerated by default — it is below the floor, and creating a GL context on
   Windows opens a hidden window. **In GPU mode Single, Vulkan first** (WP-48, ADR 0065):
   with the default software policy (`FallbackOnly` or `Exclude`) the pool enumerates Vulkan
   alone and skips the other backends when `vulkan_suffices` — a Vulkan discrete adapter above
   the floor, the adapter the full enumeration would make primary. Direct3D 12 enumeration
   costs 0.4-1 s on the dev machine and there only adds the same card again (a duplicate) and
   WARP (left out by default). Vulkan with no qualifying discrete adapter, a Vulkan device
   that fails to open, `Include`/`Only`, and GPU mode Multi all enumerate every backend, so
   the Direct3D 12 and WARP fallbacks are unchanged. `AdapterPool::skipped_backends()` names
   what was not enumerated. (Vulkan and Direct3D 12 are never brought up in parallel: that
   deadlocked about once in 25-30 launches.)
2. **Probe each** (`probe::facts`): backend, device type, PCI ids, compute
   (`DownlevelFlags::COMPUTE_SHADERS`) and the API level the floor is stated in. wgpu does
   not expose either number portably, so:
   - **Vulkan** — `VkPhysicalDeviceProperties::apiVersion` through `Adapter::as_hal::<Vulkan>()`;
   - **Direct3D 12** — `D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_12_0, NULL)`: with a null
     output it only answers whether the level is supported and creates nothing.
3. **Apply the floor** (O-8): Vulkan ≥ 1.2 or D3D12 FL ≥ 12_0, and compute. Refused, each
   with a reason phrased for the user: FL 11_x, Vulkan 1.0/1.1, OpenGL, Metal (E-33), an
   unreadable API level, no compute.
4. **Count one physical GPU once.** Adapters with the same vendor id, device id and type
   (the name only when the ids are 0) are one part on several backends. The backend with the
   most qualifying adapters in the group wins — two identical cards stay two — and ties go to
   **Vulkan, then D3D12, then Metal** (ADR 0019: one backend's behaviour on both CI legs, and
   the only backend with an external-memory path for S1).
5. **Software rasterisers** (WARP, lavapipe/llvmpipe; `DeviceType::Cpu`) follow
   `SoftwarePolicy`: `FallbackOnly` (default — a GPU-less CI runner still renders),
   `Exclude`, `Include` (tests that need a second, mismatched adapter), `Only` (reproducible
   pixels).
6. **Order**: discrete, integrated, virtual, other, software; enumeration order within a
   type. Index 0 is the **primary**. `max_devices` caps the pool.
   **GPU mode** (ADR 0054, E-65): `PoolOptions::mode`, default `GpuMode::Single`, opens a
   device on the primary only (the next in line if its device fails to open) and reports
   every other qualifying adapter `LeftOutByGpuMode` ("GPU mode: Single"); `GpuMode::Multi`
   opens every selected adapter. The editor reads its Graphics preference, and an exported
   game its `forge.json` (`graphics.gpu_mode`), before the pool is built: a change applies
   at the next start. Multi is experimental until S1 measures two physical GPUs.
7. **A device and queue per selected adapter**, with the adapter's full limits and the
   intersection of `wanted_features` with what it has (features are never required).

Nothing qualifying → `GPU-0002` with the message the user sees: the floor in plain words
(roughly a GTX 1060 / RX 580 / Arc / recent iGPU), every adapter found with why it was
refused, and "update the graphics driver first". No adapter at all → `GPU-0001`.
`AdapterPool::report()` lists every enumerated adapter with its verdict (`Selected(i)`,
`Duplicate{of}`, `BelowFloor(reasons)`, `NotRequested`, `Capped`, `LeftOutByGpuMode`,
`DeviceFailed`), each with `AdapterVerdict::reason()` in words;
`cargo run -p forge-gpu --example adapters` prints it (and the skipped backends).
`test_adapter_pool`'s skip guard checks, on this machine against the full GPU-mode-Multi
enumeration, that every enumerated adapter is reported and that the skip happens exactly when
a qualifying Vulkan adapter exists; its positive controls are a skip on an iGPU-only machine,
a missed skip, a lost adapter and a skipped backend's row, and forcing the full enumeration on
the dev machine fails it ("enumerated anyway").

On the dev machine: RTX 3080 on Vulkan 1.4 (selected, primary), the same card on D3D12 FL
12_0+ (duplicate), WARP on D3D12 (selected with `Include`, left out by default). In GPU mode
Single (the default) Direct3D 12 and Metal are skipped and the report holds the RTX 3080 on
Vulkan alone. Editor cold start to interactive (`ui_startup_budget`, medians of three
launches): 1246-1305 ms before the skip, 1009-1116 ms after (six sets; one set of 2.1 s ran
under another build's load), budget 1500 ms unchanged.

**The 4 GB VRAM part of O-8 is not enforced** (no portable query; Vulkan heaps and DXGI
budgets disagree on what "VRAM" is). It is a documented requirement, not a gate.

## 9.3 Transfers

`transfer::*` are synchronous helpers (they wait for the GPU): uploads via
`Queue::write_buffer` / `write_texture` (lengths padded to `COPY_BUFFER_ALIGNMENT`),
readbacks via a `MAP_READ` staging buffer (texture rows de-padded from
`COPY_BYTES_PER_ROW_ALIGNMENT`). **Cross-adapter copies are host-staged**: the source is
copied into a mappable staging buffer, mapped, and the mapped range is written straight into
the destination queue — one host copy, no intermediate `Vec`. Same device → a plain GPU
copy. Each returns `TransferStats` (bytes, elapsed, GB/s). The external-memory arm is §9.6.

## 9.4 The render graph

**Declaration.** Resources are transient (`create_texture` / `create_buffer`, owned by the
graph for one frame) or imported (`import_texture` / `import_buffer`, owned by the caller:
a swapchain image, a history buffer). A pass is declared with `add_pass(name)` and joins
the graph when its builder drops or `run(body)` is called:

- `read(h, access)` — reads version `h`;
- `write(h, access) -> h'` — contents discarded, returns the next version;
- `modify(h, access) -> h'` — load-then-store / read-write storage;
- `side_effect()` — never culled.

`Access` is `Sampled`, `StorageRead`, `StorageWrite`, `ColorAttachment`, `DepthRead`,
`DepthWrite`, `CopySrc`, `CopyDst`, `Uniform`, `Vertex`, `Index`, `Indirect`; each maps to
exactly one wgpu usage. **Versioned handles** make order a computed property: only the
latest version may be written (a resource never forks), and a reader names the version it
wants. Misuse is recorded and reported by `compile` (`GPU-0006`): reading a transient before
any write, writing a stale version, two uses of one resource in a pass, an access invalid for
the resource kind or for the verb, a handle from another graph.

**Compile** (`compile_with(CompileOptions { aliasing, cull })`) produces a `GraphPlan`,
plain data:

1. **Culling.** Roots are side-effect passes and writers of imports; a live pass keeps the
   producers of every version it reads. Everything else is culled.
2. **Order.** Edges among live passes: RAW (producer → reader), WAR (readers of version n →
   the resource's next *live* writer, i.e. the lowest version above n whose producer was not
   culled), WAW (each live writer → the next live writer). Going through a culled writer
   would order nothing, and a reader of n could then run after the writer of n+2 and see
   the wrong contents; the independent plan verifier applies the same rule. Kahn's algorithm with a min-heap on
   declaration index: deterministic, and declaration order wherever hazards allow. A pass
   declared late that reads an old version runs before the pass that overwrote it; two passes
   that each need the other's input intact are a **cycle** error naming exactly the passes
   on the cycle — its strongly connected component, not passes merely stuck behind it
   ("copy the resource so both versions exist").
3. **Lifetimes and usages.** Each resource's first/last position among live passes and the
   union of its accesses' usages.
4. **Aliasing.** Transients in first-use order, greedy interval assignment: a texture reuses
   a slot with an identical description whose last resident ended earlier; a buffer reuses
   the free buffer slot that fits best (the slot grows to the largest resident). The slot's
   usage is the union of its residents'. `GraphPlan::transient_bytes()` reports declared vs
   allocated bytes. (wgpu has no placed resources, so "aliasing" is reuse of one physical
   resource by successive virtual ones — the saving is the same for same-description
   transients, which is the common case: ping-pong targets, per-pass scratch.)
5. **Barriers.** wgpu inserts the API barriers itself from resource usage. The graph (a)
   creates every resource with exactly the union of its declared usages, so no transition is
   ever undeclared, and (b) emits the transition plan: one `Barrier { before, res, from, to,
   aliased_from }` wherever a resource's access changes, before its first use (`from: None`),
   and between two storage writes (a UAV hazard in the same state). `aliased_from` names the
   previous resident of the physical slot. The plan drives load ops:
   `PassContext::color_load` is `Clear` for a `write` (contents discarded — required for an
   aliased slot) and `Load` for a `modify`.

**`verify_plan(&plan)`** re-derives every property from the declared uses and the order
alone — hazards ordered, roots live, residents of a slot disjoint and compatible, slot usage
sufficient, the barrier set exactly the transitions — independent of the compiler.

**Execute** (`CompiledGraph::execute(&GpuDevice, &mut TransientPool, &Imports)`): physical
slots come from the `TransientPool` (keyed by description and usage, reused across frames,
released after `KEEP_FRAMES` = 3 unused frames — **a steady-state frame allocates nothing**);
imports are checked for size, format and declared usage before any pass runs; passes run in
order on one command encoder, one submit, inside a validation error scope. A pass body sees a
`PassContext` whose `texture` / `view` / `buffer` refuse any resource the pass did not
declare (`GPU-0006`) — the `RegionSystem` discipline at run time.

**Per-pass timing** (WP-09): `CompiledGraph::execute_timed(.., &mut GpuTimer)` records each
pass body's CPU recording time and, where the device has `TIMESTAMP_QUERY_INSIDE_ENCODERS`
(`GpuTimer::FEATURES`, requested through `PoolOptions::wanted_features`), GPU time from
timestamps written on the encoder before and after the pass, resolved and read back after
the submit (the call waits for the frame). Without the feature, `gpu_ns` is `None` — never a
fabricated number. forge-render's per-pass table is Ch.10 §10.9.

**Not yet:** passes are single-device (cross-device work goes through §9.3 and Ch.26's
`GpuJob`); async-compute queues (wgpu exposes one queue per device); bindless (the bound
path is the only path until `forge-render` needs bindless, Ch.10); a compiled-plan cache
(compile is ~1.1 ms for 257 passes in a debug build — rebuild per frame).

## 9.5 Shaders: WGSL only, hot reload

`validate_wgsl(path, src)` parses with naga's WGSL front end and validates with all
capabilities; errors carry `path:line:col:` on their first line plus naga's annotated
excerpt. `create_module` hands the validated IR to wgpu (`ShaderSource::Naga`, the `naga-ir`
feature) so the text is parsed once, inside a validation error scope (a capability the
device lacks is still an error, not a panic). **Every shader module in the workspace is
created through `forge_gpu::shader`** — forge-ui's three shaders included — and no GLSL,
HLSL, SPIR-V or MSL front end or file exists (`test_wgsl_only`).

`ShaderLibrary` holds project shaders by `StorePath`:

- `load(store, device, path)` — stamp, read, validate, create; invalid at load is an error
  (there is no last good version yet);
- `poll(store, device)` — **one `stamps_of` call** for all held paths (on `LocalFs`: one
  directory listing per shader directory, no reads); changed files are re-read, validated
  and swapped; generation increments;
- a failure — syntax error, validation error, half-saved file, deletion — produces one
  `ShaderEvent::Failed` with the diagnostic, **keeps the last good module in service**, and
  is not re-reported until the file changes again;
- consumers rebuild pipelines when `generation(id)` moves.

Polling rather than an OS file-watch thread (ADR 0019): the stamp call is what forge-asset's
hot reload already pays (ADR 0017), it works on every `ProjectStore` backend, and it adds no
thread and no new dependency.

## 9.6 Spike S1 — multi-adapter (ADR 0020)

Measured on the dev machine (RTX 3080, PCIe 4.0; release build; 64 MiB): host → GPU upload
8.2 GB/s, GPU → host readback 5.4 GB/s (warm staging), cross-device host-staged to WARP
0.01 GB/s (WARP's own rasteriser speed — not a physical-GPU figure). A host-staged transfer
between two physical GPUs is bounded by readback + upload: ~3.3 GB/s serial, ~5 GB/s
pipelined, i.e. ~12–20 ms for a 4K colour+depth frame (≈66 MB).

**Verdict (provisional; AWAITING a second physical adapter for the two-GPU number):**
host-staged transfer is the committed cross-adapter path. It is sufficient for **Tier 0**
(heterogeneous compute: results are small relative to the work) and **Tier 2** (LAN farm).
**Tier-1 fallback: split-frame rendering is limited to editor viewports, offline/cinematic
renders and multi-display**, where a frame of latency is free — exactly Ch.26's ruling; it
is not offered on the player-facing hot path. The Vulkan external-memory passthrough
(`wgpu-hal`, behind a feature) is **not built**: it is only worth building if the two-GPU
measurement shows host staging cannot feed a 60 Hz editor viewport. Gate row `S1` stays
AWAITING(reason: second physical adapter).

## 9.7 Guards

| Guard | File | Positive control |
|---|---|---|
| render graph: order, culling, aliasing, barriers, declared access (`C-render-graph`) | `crates/forge-gpu/tests/test_render_graph.rs` | `positive_control_the_verifier_catches_every_broken_plan`, `positive_control_an_undeclared_access_and_an_unbound_import_are_refused` |
| shader hot reload keeps the last good (`C-shader-hot-reload`) | `crates/forge-gpu/tests/test_shader_hot_reload.rs` | `positive_control_the_broken_shaders_are_really_broken` |
| adapter floor and selection (`C-adapter-floor`) | `crates/forge-gpu/tests/test_adapter_pool.rs` | `positive_control_every_excluded_class_is_refused` |
| WGSL only, through forge-gpu (`C-wgsl-only`) | `crates/forge-gpu/tests/test_wgsl_only.rs` | `positive_control_each_rule_flags_its_violation` |
| DoD seed: 1 vs 2 adapters render identically (`C-multi-adapter-frame`) | `crates/forge-gpu/tests/test_multi_adapter.rs` | `positive_control_a_different_helper_result_changes_the_frame` |
| UI on forge-gpu within D-5 budgets (`C-ui-on-forge-gpu`) | `crates/forge-ui/tests/test_ui_on_forge_gpu.rs` | `positive_control_batching_disabled_breaks_the_budget_on_the_gpu` |

## 9.8 Measured (dev box: RTX 3080, 12 threads)

- Render graph build + compile, 257 passes / 257 transients: ~1.1 ms (debug build).
- Aliasing: the four-texture test graph allocates 2,304 of 3,328 declared bytes; a 257-pass
  chain ping-pongs between 2 allocations; steady-state frames allocate 0.
- UI on the pool primary: a 1,000-button panel in 1 draw call; pixel goldens 0.0000 %
  differing on the forge-gpu device (dark, light, high-contrast).
- `AdapterPool::new` (Vulkan + D3D12 instance, enumerate 3 adapters, probe, one device): ~450 ms debug, ~200 ms release; the floor probes add under 1 ms. Enumeration dominates (driver work).
- `ui_gallery --windows 2` on the pool: RTX 3080 (Vulkan 1.4), 2 windows sharing one device, 0 frames and 0 wakeups over a 2 s idle window.
- Transfers: see §9.6.

---

# Chapter 10 — Renderer: Scene, Materials, Lighting, GI  *(FULL for §10.1–10.9: the last mile, scene, PBR, clustered lights, shadows, timing — material graph, GI stages, decals, transparency and the post stack are still BRIEF)*

**Contract:** clustered forward+ as the baseline; PBR metal-rough with a layered material
model; virtual shadow maps; reversed-Z per shell (Ch.2 §2.4).

**Decisions:** ship a *good* renderer, not a novel one. GI staged: (1) baked irradiance
volumes + SSAO, (2) screen-space GI + reflections, (3) a surfel/probe dynamic GI, (4)
hardware ray tracing where present. **Do not start at (4).** A path tracer for reference
renders is cheap once the material model is settled and pays for itself in validation.

**Open risk:** virtualised geometry (Nanite-equivalent) is **Ch.26/M6 and spike S5.**
Note that for *procedural terrain we do not need it* — analytic LOD already exists. It is
an *asset* feature.

## 10.1 Implementation (WP-09, ADR 0022, ADR 0023)

`crates/forge-render`, on `forge-gpu` (device, render graph, WGSL through naga) and
`forge-frames` (positions). `#![forbid(unsafe_code)]`. Optimised in dev builds (ADR 0022).

| Module | What it is |
|---|---|
| `last_mile` | `CameraView` (the camera resolved at a tick), `ViewOffset` (camera-relative, view axes), `FrameOffset` (camera-relative, camera-frame axes — shadow cascades), the **only** `f64 -> f32` narrowing in the engine; `mutants::naive_model_view` (W2 control only) |
| `camera` | `Camera { pos: FramePos, orientation, fov_y }`, `Camera::looking` |
| `scene` | `MeshData`/`Vertex` (object space, `f64`), `Material` (metal-rough), `Instance`, `DirectionalLight`, `PunctualLight` + `Spot`, `Scene` (ambient, EV100 exposure, background) |
| `cluster` | `ClusterGrid`, CPU light binning (`bin_into` with a kept `ClusterScratch`: one geometric pass records hits, a count + prefix sum + stable scatter fills the lists — no per-frame allocation once warm; `bin` is the allocating convenience), the shader's cluster lookup (`cluster_of`) |
| `shadow` | `CascadeSettings`, cascade `fit(settings, &Camera, aspect, toward_light, snap)` with world-anchored texel snapping (the camera's frame-local position is read inside `fit` only; cascades hold `FrameOffset`s) |
| `prepare` | `PreparedFrame`: the frame as plain camera-relative `f32` data, built on the CPU |
| `renderer` | `Renderer`, `RenderOptions`, pipelines, buffers, the per-frame render graph, `FrameReport` |
| `atmosphere` | `GpuAtmosphere` (Bruneton tables from `forge_sky::AtmosphereParams`, compute-shader precomputation, `probe`), `AtmosphereSettings` (WP-11) |

Error codes `RENDER-0001..0004` (`docs/error-codes.md`). A frame is
`Renderer::prepare(scene, camera, tree, tick)` (CPU, no device) then
`Renderer::render(device, &prepared, target)`; `render_scene` does both.

**Mesh pools (WP-U21, ADR 0056).** Meshes of equal vertex count drawn with a few index lists —
a streamed terrain's patches — go in a pool (`add_mesh_pool(vertex_count, reserve)`,
`add_pool_topology`, `add_pooled_mesh` → an ordinary `MeshId`): their vertices live in one
storage-buffer arena, each instance carries its mesh's first vertex, and every instance of the
pool drawn with one index list is one instanced draw per pass (`vs_pulled`, the vertex pulled by
`material.y + vertex_index`). Preparation groups by draw group (a mesh id, or `POOLED_DRAW` +
a pool topology). `test_mesh_pool` (C-render-mesh-pool): the pooled frame equals the per-mesh
frame to one 8-bit step, 400 patches in 10 calls against 1,999.

## 10.2 The last mile (Ch.2.4, M1-3)

Every frame: each instance's and light's `FramePos` is resolved in the camera's frame, the
camera's position is subtracted **in `f64`**, and the result is rotated into view axes
(`ViewOffset`). Only then is anything narrowed: an
instance uploads a model-view matrix whose translation is its camera-relative offset (a few
metres for anything near), so no matrix on the GPU ever carries a large translation and no
view matrix exists. Mesh vertices are object-space and narrowed at upload.

`tests/liveness/test_no_f32_below_render.rs` (gate `C-no-f32-below-render`): no `f32` in any
workspace crate in `forge-render`'s dependency closure (from `cargo metadata`) (allow-list `tests/liveness/f32_allow.txt`: forge-asset's glTF and mesh
decoders and its fixture writer, which read/write `f32` file formats and widen at once);
the scanned set is complete; inside forge-render every `as f32`/`f32::from` is in
`last_mile.rs`.

`test_last_mile` (gate `C-camera-relative-last-mile`): 2,000 camera steps of 1 mm, far
from the world origin on no axis, past a rotated cube 3 m away, through the GPU's `f32`
arithmetic — max error **2.3e-7 m**, max frame-to-frame screen deviation **2.0e-4 px**
(1080p). Naive `f32` world positions: **0.24 m** and **474 px**. On the GPU the same lit
scene near the origin and far from it gives identical pixels at six sub-millimetre camera
positions.

## 10.3 Clustered forward+ (M1-7)

16x9 screen tiles x 48 logarithmic depth slices from 1 cm to 1e7 m. Binning runs
on the CPU in `f64` view space (ADR 0022): each light's sphere is tested exactly against each
candidate cluster's box, padded by one pixel and 0.1 % of depth so GPU rounding can never put
a fragment in a cluster that lacks a light that reaches it. Candidate tiles come from the
projected bounding box of the sphere (all tiles when it reaches behind the eye). The shader
computes its cluster from the fragment's pixel and view depth exactly as `cluster_of` does.
Light falloff is inverse-square windowed to exactly zero at the range (`(1 - (d/r)^4)^2`),
which is what makes the culling exact rather than approximate.

`test_clustered_lighting` (gate `C-clustered-lights`): 400 lights, 40,000 random fragments,
no miss (control: halved radii miss); the default grid and a 1x1x1 grid (every light in one
list) render identical pixels.

## 10.4 Materials and lights (M1-7)

Metal-rough (glTF 2.0): base colour, metallic, perceptual roughness (clamped to 0.045),
reflectance (`f0 = 0.16 r^2` for dielectrics), emissive. BRDF: Lambert diffuse + GGX
distribution, height-correlated Smith visibility, Schlick Fresnel (Filament's
formulation). Lights: one directional (illuminance in lux), point and spot lights
(intensity in candela, smooth spot cone via scale/offset). Exposure is EV100
(`1 / (1.2 * 2^EV100)`); ACES filmic tone map; sRGB by the target format (or in the shader for
a non-sRGB target). Ambient is a uniform term until GI stage 1 (§10.6).

`test_pbr` (gate `C-pbr-matches-reference`): six materials x eight light setups (normal
incidence and a 57-degree view with the sun and a point light at the mirror direction), nine
pixels each, against an independent `f64` reference through ACES, sRGB and 8-bit
quantisation: worst difference **1 level** (controls: a reference without Fresnel, or with a
flat distribution, disagrees).

## 10.5 Shadows (M1-7; ADR 0023)

**Cascaded shadow maps are the documented fallback for virtual shadow maps**: wgpu has no
sparse resources, so a VSM must be emulated (page table, visibility feedback, page atlas
allocator, per-page culling and caching) — a milestone of its own. Four cascades
(`Depth32Float` array, 2048² default) split by the practical scheme to 500 m; each is the
bounding sphere of its frustum slice (constant texel size under rotation), snapped to the
texel grid in the camera frame's own `f64` coordinates (a moving camera does not make edges
crawl), reversed orthographic depth extended 1 km toward the light; normal-offset bias and
3x3 hardware PCF whose taps each compare against the receiver's own depth at that tap
(receiver-plane depth bias from the screen derivatives of the view position, WP-11, ADR
0027: one depth across the kernel drew acne rings on open ground). Casters are culled per
cascade and drawn one call per mesh. Punctual lights do not cast shadows yet.

`test_shadows` (gate `C-shadow-cascades`): a sub-texel camera move far from the origin leaves every
cascade's texel phase unchanged to **1e-8 texel** (control: unsnapped drifts 0.35 texel); a
floating box's shadow lands exactly down the sun ray (ambient-only value there, lit value in
the open, lit again when the box does not cast). DoD M1-7 stays **Blocked** on the VSM half;
gate row `C-vsm` is `Unbuilt`.

**Restated in WP-18 (ADR 0028, decision 6).** A VSM is buildable on the RTX 3080 without sparse
resources (GPU page marking with storage atomics, a GPU page allocator, per-page caster routing
with indirect draws and clip distances, static-page caching). It stays owed because on M1's
content it buys neither owner rule: the four cascades cost 0.116 ms of the 0.18 ms M1 frame at
1280x720 (perf gate), which a VSM's fixed passes alone would match, and its gains (far texel
density, cached static pages) need dense distant casters M1 does not have. `test_shadows`'
floor-shadow check is its acceptance test when a measured shadow cost or far-shadow quality
calls for it.

## 10.6 GI, material graph, decals, transparency, post  *(BRIEF — not built by WP-09)*

As the expansion brief above. The uniform ambient term in `mesh.wgsl` is the placeholder GI
stage 1 replaces; smooth metals look dark until an environment term exists.

## 10.7 The frame graph

```text
shadow.cascade0..3   depth only, one layer each of the cascade array (write, then modify)
shell.sky            write hdr: clear to background, draw sky sprites
shell.far/mid/near   modify hdr, write own depth (clear 0), reversed-Z; Mid/Near read cascades
tonemap              read hdr, write the caller's target
```

Visible instances are uploaded once per frame to a storage buffer; each pass draws index
ranges into it. Buffers persist and grow; transients come from forge-gpu's pool, so a
steady-state frame allocates no GPU memory.

## 10.8 Goldens

`test_render_goldens` (gate `C-render-goldens`): `materials` (18 spheres, sun + cascades,
a point light), `lights` (48 point + 2 spot lights) — each 480x270,
compared with committed PNGs at 3 levels / 0.2 % of pixels (never widened, W5);
`FORGE_BLESS=1` rewrites them (control: one sphere 0.15 rougher fails). Blessed on the RTX 3080; every forge-render GPU guard, the goldens included,
also passes on Microsoft WARP (`FORGE_RENDER_SOFTWARE=1` selects the software rasteriser
only) — what a GPU-less CI runner renders with.

## 10.9 Measured (dev box: RTX 3080, Vulkan; gate `C-render-pass-timings`)

`test_render_perf`, 1920x1080, 2,501 instances (cubes and spheres),
256 point lights, sun with four 2048² cascades, medians of 20 frames:

| Pass | GPU | CPU record |
|---|---|---|
| shadow.cascade0 | 0.124 ms | 0.017 ms |
| shadow.cascade1 | 0.172 ms | 0.005 ms |
| shadow.cascade2 | 0.129 ms | 0.005 ms |
| shadow.cascade3 | 0.106 ms | 0.004 ms |
| shell.sky | 0.001 ms | 0.002 ms |
| shell.near | 0.309 ms | 0.025 ms |
| tonemap | 0.018 ms | 0.009 ms |
| **sum** | **0.86 ms** | |

Preparation (last mile, culling, 4,564 cluster entries, cascade fit, packing): **0.79 ms**
CPU (dev profile, forge-render at opt-level 2; 1.09 ms before WP-10 made a steady frame
allocation-free, 2.2 ms before binning moved to reusable scratch). 10 draw calls; 0
steady-state GPU allocations; **0 steady-state heap allocations in preparation**
(`test_prepare_alloc`, counting allocator; control: a dropped frame allocates): every buffer
of a `PreparedFrame` returns to the renderer's scratch through `Renderer::recycle` (done by
`render_scene`), sorts are unstable over total orders (no merge buffer), the pass table is
static. The frame graph is **built and compiled only when its topology changes** (cascade
count, what samples the cascades): pass bodies are `run_retained` and read
the frame's draws from shared state, and `FrameReport::graph_rebuilt` is `false` on every
steady frame (asserted in `test_render_perf`).
Light binning of 400 large lights: ~13 ms unoptimised (test build), well under a millisecond
optimised. Budgets and the regression gate are DoD M1-11.

---

# Chapter 14 — PCG Framework  *(BRIEF)*

**Contract:** placement is a **pure function of position and seed**, never a stored list.
Instances are promoted to entities on proximity and demoted on departure — the Foundations'
field-with-promotion pattern , applied to vegetation, rocks, and props.

**Expansion brief:** biome→asset binding; slope/altitude/moisture rules; deterministic
position-hash instantiation; **touching an instance makes it real** (it gains an identity
and a KV row and becomes persistent, visible Layer-1 state); scatter LOD and impostors.

---

# Chapter 17 — Physics & Simulation Regions  *(BRIEF)*

**Contract:** `avian` (generic over `f32`/`f64` — this is why it is preferred over `rapier`)
inside a region whose origin is frame-local.

**Collision LOD rings** carried from the Foundations — doubling radius while doubling voxel size
holds triangle count constant, so every ring costs the same (~16 MB, 206k tris). Seven
rings reach 2 km for ~112 MB/player. **Full 0.25 m collision is affordable only within
~32 m**; pushing it to 128 m is 3.3 M triangles and ~264 MB for one player.

**Geometry residency and gameplay residency are two independent radii. Do not force them
to match.**

---

# Chapters 18–20, 25, 27–29 — Parity Subsystems  *(BRIEF)*

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

**Ch.21 UI & Editor Shell** was BRIEF here; WP-U0 expanded it to FULL — see *Chapter 21*
below and ADR 0001 (D-1 amends O-6: no `egui` stopgap).

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

*Ch.28 §28.1–28.8 as built in WP-U20 (DoD M5-9, ADR 0058) — scene composition, FULL.

**§28.1 The model: nesting and inheritance are one mechanism** (`crates/forge-scene`). A
*scene* is a reusable subtree kept in the project's **scene library** (a root entity marked
`scene.library`; each child carrying a stable `scene.id` is a scene root). An **instance**
is an authored node carrying `scene.instance = <id>` whose subtree **mirrors** the scene:
every mirrored node carries `scene.base`, the entity it mirrors (instance roots carry it
too, pointing at the scene root). A **derived** (inherited) scene is a scene whose own root
is an instance of its base, so a derivation chain of any depth, and scenes nested in scenes,
are the same links followed recursively. Every node inside a scene carries a `scene.uid`
unique in that scene (authored nodes: the next free integer; mirrored ones: a fixed integer
mix of their instance root's and base node's uids, in a separate range), the stable key
override records use. The library is excluded from the world: the viewport draws it only
when a scene is opened in isolation, and the play core (`forge_sim::EditSnapshot`) leaves it
out.

**§28.2 Overrides are reflected property diffs.** A mirrored node overrides exactly the
properties whose value differs from what it is *expected* to hold (a property the base has
and the node lacks is *cleared*), and its name if it differs; an instance root's name is
always its own. There is no separate override bookkeeping to drift: an edit on an instance
*is* the override, and Revert to base is setting the value back.

**§28.3 Instance placement.** Transforms in the editor are flat (a frame and an offset, not
relative to the parent), so a scene's layout is read relative to its root: an instance root's
pose against its scene root's pose is a rigid placement (translation, rotation, uniform
scale, frame) applied to every mirrored node's `transform.*` (`forge_scene::place`). A
mirror's expected content is its base's content placed; moving or turning an instance moves
its content and is not an override of its nodes. Nested placements compose (a lamp placed in a
street placed in the world). The arithmetic is IEEE `+ - * /` and forge-num's
bit-reproducible `sin`/`cos`/`atan2`, so placed values are identical on every platform (Ch.3).

**§28.4 Propagation: one diff, whoever sends the edit.** Amends Ch.7, additively: the bus
gains a **`CommandDeriver`** (`Bus::set_deriver`) that runs inside planning after every
command, with the command's `DiffBuilder`, and may append derived changes or refuse — so
`apply`, a dry run, a batch preview and `plan_for` see the same diff (I8), and undo/redo
replay the recorded diff and never derive again; and `Project` keeps a **reference index**
(who holds `Value::Entity(k)` at which path; `Project::referrers`, `DiffBuilder::referrers`,
derived from the properties by the only mutation path, not content, not on the wire), so a
base node's mirrors are found in O(log n + k), never by a scan. `forge_scene::SceneDeriver`
is that deriver. As a worklist over the diff (derived changes are processed too, so a change
runs down any depth of nesting and inheritance): a base node's content change is applied to
each mirror *only where the mirror still holds what it was expected to hold before* —
anything else is an override and stays; names likewise; nodes created, removed or moved in a
scene are created, removed or moved in every instance; an instance root or scene root that
moves re-places the instance's nodes; a node created inside a scene gets its uid; a node put
straight into the library becomes a scene. It refuses (only the command's own changes):
`scene.*` written outside `forge.scene.*` (`SCENE-0006`); deleting or moving a node an
instance inherits (`SCENE-0003`: edit the base, or Make Local); deleting a scene, or moving it
out of the library, while instances use it (`SCENE-0004`, naming them); and a **cycle**
(`SCENE-0002`, the loop named scene by scene: "Gamma → Alpha → Beta → Gamma"), checked by the
instance command and on every move into a scene. Backstops: a mirror's base chain longer than
`MAX_CHAIN` (64) and more than `MAX_DERIVED` derived changes are refused rather than grown.
Commands that load a whole, already composed document (`forge.project.load`, the team sync)
are exempt. Cost: a command that touches no scene pays an index lookup per change.
Measured (release, dev box, one ad-hoc run): a world property edit 1.75 µs per command
without the deriver vs 1.72 µs with it (no measurable cost); a base node edit reaching 100
instances 324 µs (3.2 µs per mirror, each property's contents and placements read once per
fan-out); moving an instance and re-placing its 20 nodes 66 µs.

**§28.6 The files store references, not copies** (`forge_scene::store`, used by
`forge_project::format::ProjectDoc::encode`/`decode`). An instance is written as its root
(the scene id, its overrides, `scene.cleared.<path>` for cleared ones) plus `scene.keys` —
`uid:id` of each mirror not stored, so every entity loads back with its id (a team's merge
matches entities by id) — and an **override record** only for a mirror that carries
something of its own (overrides, a renamed name, a stable uid of its own, authored children,
or an entity reference to it): `scene.of = <base uid>` and the overridden values, listed
under its instance root and keeping its id. `decode` expands scenes in dependency order
(Kahn), so a `ProjectDoc` in memory, and every merge, always holds every entity; the form is
**lossless** (`expand(compact(e)) == e`, ids included). A file naming a missing scene, or
scenes instancing each other, loads with nothing dropped (the instance stays unexpanded and
says so in the inspector); an override record whose node left its scene stays as a plain
child. **Measured** (`crates/forge-project/tests/test_scene_store_size.rs`, 100 placed
instances of a 21-node, 6-property scene, one in ten with an override): `project/scene.ron`
is **59,164 B** as instances vs **1,109,920 B** as 100 plain copies — **18.8x smaller**
(≈ 480 B per instance vs 11,099 B per copy); the guard requires ≥ 10x and its control (a
store that copies) fails it.

**§28.7 The editor.** Hierarchy: composition icons (library, scene/instance, inherited
node); a scene tile dropped on a row instances the scene under it (`VirtualTree::
accept_assets` → `AssetDropped`, one command, one undo step). Asset browser: the project's
scenes as tiles in the library's folder (drag payload `scene:<id>`; activate places the scene
under the selection; rename and delete act on the scene; *New scene*). Inspector: a Scene
group naming what the node is and follows, with *Open base scene* (session state: the
viewport shows only that scene, the base node selected, *Back to the world* returns), *Revert
node to base*, *Make local* with its loss named beside it (Ctrl+Z undoes it), and for a scene
*Open scene*, *Place in the world*, *New inherited scene*; a plain entity offers *Save as
scene*. Every row of an inherited node is marked *inherited* or *overridden* and an
overridden row has *Revert*; the marks follow edits by any issuer through the row's
refresher (no rebuild under a drag). Every string is a localisation key; every change is a
command (I7).

**§28.8 Guards.**

| Test | Gate row | Positive control |
|---|---|---|
| `crates/forge-scene/tests/test_scene_composition.rs` — nested propagation except overrides (values, names, nodes added / moved / removed), depth-3 inheritance, revert undoable, Make Local, structural refusals, determinism | `C-scene-composition` | `positive_control_a_deriver_ignoring_overrides_fails` (and a deriver that does not propagate) |
| same file — cycles refused, readable, by instancing and by moving | `C-scene-cycle-refused` | `positive_control_a_deriver_skipping_the_cycle_check_fails_the_move_guard` |
| same file — placement: moves and turns content, scene edits land placed, overrides stay, nested placements compose | `C-scene-placement` | `positive_control_a_deriver_that_does_not_place_fails` |
| `crates/forge-scene/tests/test_scene_store.rs` — lossless store form, unchanged mirrors not stored, missing scenes lose nothing | `C-scene-store-lossless` | `positive_control_a_lost_override_record_does_not_round_trip` |
| `crates/forge-project/tests/test_scene_store_size.rs` — 100 instances vs 100 copies through the real files, lossless decode, loaded instances still linked | `C-scene-store-references` | `positive_control_a_store_that_copies_fails_the_size_claim` |
| `plugins/forge-panels-scene/tests/test_scene_composition_ui.rs` — hierarchy drop, inspector marks / Revert / Revert node / Make Local's loss / Open base scene, viewport library and isolation | `C-scene-editor` | `positive_control_an_inspector_that_never_marks_rows_fails` |
| `crates/forge-cmd/tests/test_deriver.rs` — derived changes are one diff in apply, dry run and `plan_for`, one undo; the reference index equals a scan after a random edit/undo/redo run | (under `C-scene-composition`) | — |

**Not built here (follow-ups):** "sibling order" inside instances follows key order like the
rest of the project (Ch.7 §7.4); instances of scenes in other projects or packages (the id is
project-local); per-instance "editable children" beyond overrides and local additions; a
confirmation dialog for Make Local (it is one undo step and names its loss beside the button
instead); the expansion's warnings are returned by `ProjectDoc::decode_scene_report` but not
yet shown by the open path (what they report stays visible in the document itself).

*Ch.28 §28.9–§28.18 as built in WP-65 (DoD M7-12, ADR 0064) — the input runtime, FULL. A base
feature (both editions): `crates/forge-input`, the editor's input map and input debugger over
it, and the game's input UI in `forge-runtime`.*

**§28.9 The runtime and its frame** (`forge_input::runtime`). An `InputRuntime` owns devices,
local players, the compiled map and the per-frame state. Backends push raw events at any time
(`push`/`send`, `touch`, `motion`); `update(dt)` applies every event pushed since the last update
and evaluates each active player's enabled contexts. **Latency: none** — an event pushed before
an update shows in that update's action state (`input.latency.frames` = 0). A press and its
release inside one frame still register (per-control pressed/released edge bits), so a tap faster
than a frame is never lost. A control's value is a `[f32; 2]`; delta controls (mouse motion, the
wheel, finger motion, pinch, twist, gyro) add up within a frame and read zero the next; state
controls hold. Devices: `DeviceDesc { class, name, family, stable_key, is_virtual }`;
`disconnect` zeroes a device's controls and its player remembers the stable key.
Adapters (features): `backend::winit::WinitInput` — keys by **physical** position (the label
is learnt from the layout's key text), mouse aim from raw `DeviceEvent::MouseMotion` (the
cursor stands in until one arrives), losing focus releases every key; `backend::gilrs::GilrsBackend`
— every standard pad button and axis, triggers as `Axis1D`, sticks as `Axis2D` plus their X/Y,
gilrs' own filters off (our deadzones apply to raw values), the pad's UUID as the stable key,
force feedback from the runtime's motor commands. On Linux gilrs needs `libudev-dev` to build.

**§28.10 The map** (`forge_input::map`). Authored as project settings by the input-map panel
(M2-68), read by `InputMapDef::from_settings` over the same keys: `input.map.<m>.{name,
priority, consume}`, `.action.<a>.{name, kind, modifiers, triggers}`, `.bind.<n>` (a
`Binding`: a control, `Composite1D(neg, pos)`, `Composite2D(up, down, left, right)`),
`.bindmod.<n>` (that binding's modifiers). What does not parse is skipped and named, never
fatal. `CompiledMap::compile` flattens it: contexts ordered by priority, controls resolved to an
index in their class's table (`forge_input::control`: keyboard by physical key, mouse, the
standard gamepad by position incl. `Gyro` and `Touchpad`, touch), triggers resolved to action
indices, and an evaluation order in which a chord's or combo's other actions come first (a cycle
is named and reads the previous frame). Each player compiles its own bindings once, when its
overrides change, never per frame. Contexts are enabled per player; a context that `consume`s
hides the controls of its actuated actions from contexts of **lower priority** (never from its
own).

**§28.11 Modifiers** (`forge_input::modifier`), pure functions on a binding or on the action:
`Deadzone(Radial | Axial | UnscaledRadial, lower, upper)` — radial measures the deflection's
magnitude (a diagonal is not cut early) and rescales `lower..upper` to `0..1` keeping the
direction, so the smallest movement past the deadzone is a small value, not a jump; `Curve(Linear
| Power e | Smooth | Points x y, ...)` over the magnitude (a 2D value keeps its direction);
`Scale(x, y)`, `Negate(x|y|xy)`, `Swizzle`. Text form: `Deadzone(Radial, 0.15, 0.95) | Curve(Power, 2)`.

**§28.12 Triggers** (`forge_input::trigger`). Per frame each trigger yields None / Ongoing /
Triggered; explicit triggers combine as *any*, implicit (`Chord`) as *all*; the action's
events (Started, Triggered, Completed, Canceled) follow from the result against the previous
frame's. `Down` (default), `Pressed`, `Released`, `Hold(t)` / `Hold(t, repeat)`, `Tap(max)`,
`MultiTap(n, gap)` (fires on the completing **press**), `Pulse(interval)`, `Chord(map/action)`,
`Combo(map/a w, map/b w, ...)` (each step's action within its window of the previous; a step out
of order restarts it). Buttons actuate at 0.5, axes past their deadzone.

**§28.13 Rebinding and persistence.** `start_rebind(RebindRequest { player, action, slot, part,
devices, cancel, conflicts, timeout })` listens for the next actuated control that **fits** the
action (a stick flick never binds a button action; delta and position controls never bind);
Escape / Select cancel; a conflict in the same context is swapped (default), refused naming the
other action, or allowed; `part` rebinds one direction of a composite; the press that completed
it does not also fire the action (suppressed until released). Overrides (`BindingOverrides`:
action path → slot → binding or `None`) hold **only what the player changed**; `persist`
saves them per profile (`FileOverrideStore`: `<dir>/<profile>.input.ron`, written to a temporary
file and renamed; `MemoryOverrideStore` for tests and consoles' save-data layers). The game's
screen is `forge_runtime::input_ui::ControlsScreen`: every action with its keyboard-and-mouse
and gamepad columns as glyphs (a composite one button per direction), rebinding on press, the
outcome shown (the swap named), **saved on every change**, Reset to defaults; every visible
string a localisation key (`controls_strings`), action names from `input.<m>.<a>` keys when the
game's tables have them.

**§28.14 Local multiplayer.** `JoinPolicy::Single` (one player owns every device: the default),
`AutoJoin { max_players, button }` (a button on a device with no player seats the next player; a
keyboard and the mice are one seat; `button` can require `Gamepad/Start`; the cap holds; a
device never belongs to two players) and `Manual` (`assign`). `PlayerEvent`s: Joined,
DeviceLost (to prompt "reconnect your controller"), DeviceRegained (the device came back with its
stable key — to the **same** player), Left.

**§28.15 Touch and on-screen controls.** A touch device's fingers become Touch controls
(`forge_input::touch`): `Tap`, `DoubleTap`, `LongPress`, `Swipe{Left,Right,Up,Down}` (one-frame
pulses), `Pinch` (log-ratio of the spread per frame), `Rotate` (radians, counter-clockwise),
`TwoFingerPan`, and the first finger as `Primary`/`Position`/`Delta`; thresholds in px
(`GestureConfig::scaled` by DPI). `forge_input::onscreen::OnScreenControls` are virtual sticks
(floating by default) and buttons that **drive a virtual gamepad**, so every pad binding works by
touch; a finger is captured by the control it lands on; fingers on no control reach the
gestures. `forge_runtime::input_ui::OnScreenControlsView` draws them (the mouse stands in for a
finger on desktop) and names each control for assistive technology.

**§28.16 Gyro and haptics.** `forge_input::motion::GyroProcessor` turns motion samples (deg/s,
g) into the `Gamepad/Gyro` control, degrees of yaw and pitch this frame: continuous calibration
(the bias is learnt while the rate is steady and gravity near 1 g), `Player` space by default
(yaw about gravity, relaxed toward the pad's plane; `Local` and `World` too), tightening below a
threshold, sensitivity and inversion. Samples enter through the `MotionSource` seam; a platform
HID backend for DualShock 4 / DualSense / Switch Pro reports is **not built**
(`C-input-gyro-device` UNBUILT: gilrs exposes no motion). Haptics (`forge_input::haptics`):
`Rumble { low, high, duration, attack, release }` per player or device; effects mix per motor
(the strongest wins), only changed levels become `MotorCommand`s (the gilrs adapter plays them
as a strong and a weak effect), and every rumble ends with a zero command.

**§28.17 Glyphs** (`forge_input::glyph`). `PadFamily::from_ids(vendor, name)` (Microsoft,
Sony, Nintendo, else by name, else Generic); `GlyphContext::control` gives a label and an icon
key (`pad.ps.south`, `key.w`): ✕/○/□/△ and L1/R2 on PlayStation, A/B/X/Y and LB/RT on Xbox,
Nintendo's swapped face letters and ZL/ZR, the layout's own character for a key
(`KeyLabels::learn`), Cmd/Win/Super and Option/Alt per platform. `forge_runtime::input_ui::InputPadSource`
feeds the menus (`forge_ui::game::GamepadSource`) from a player's real pads with their family's
glyph set.

**§28.18 The input debugger, budgets and guards.** `InputRuntime::debug_snapshot` (devices and
their live controls, players with devices, contexts and every action's phase and value, the
last raw events when the log is on — off costs nothing). The editor's **Input debugger** panel
(`forge.input_debugger`, `forge-panels-domain`) shows it against the project's map through
`InputActions::debug_snapshot`: opening reads no device, **Refresh** reads once, **Live**
refreshes 20 times a second only while on and visible (with it off the editor sleeps: D-5). The
editor's backend is `forge_editor::domain::input::DeviceInput` — a runtime with gamepads through
gilrs, started on the first capture or debugger refresh, plus a test's virtual devices; the input
map's press-to-bind captures pad controls through it. Budgets live in
`crates/forge-input/budgets.ron` (declared into the profiler as `input.*`):

| Row | Allowance | Measured (dev box, test profile) |
|---|---|---|
| `input.update` (4 players, 3 contexts, 48 actions, 8 devices, 40 events) | 0.1 ms | 0.058 ms |
| `input.update.idle` (nothing fed) | 100 ns | 28 ns |
| `input.latency.frames` | 0 | 0 |
| `input.update.alloc_bytes` (steady frame) | 0 | 0 |

| Test | Gate row | Positive control |
|---|---|---|
| `plugins/forge-panels-domain/tests/test_input_backend.rs` — press-to-bind a pad button through the runtime; authored triggers/modifiers play in a runtime compiled from the project | `C-input-backend` | `positive_control_a_backend_that_never_pumps_binds_nothing` |
| same file — the debugger's rows, Refresh, Live at 20 Hz, idle with Live off | `C-input-debugger` | `positive_control_a_live_toggle_that_keeps_its_timer_fails_the_idle_check` |
| `crates/forge-input/tests/test_triggers_curves_deadzones.rs` — radial deadzone rescale, curves, WASD normalisation | `C-input-deadzone-curve` | `positive_control_a_deadzone_that_does_not_rescale_fails` (and per-axis radial, linear curves) |
| same file — every trigger kind, same-frame taps, consumption | `C-input-triggers` | `positive_control_a_hold_that_ignores_its_time_fails` (and one per trigger, lost taps, no consumption) |
| `crates/forge-input/tests/test_rebinding_persists.rs` — rebind, conflicts, cancel, timeout, composite part, save, restart | `C-input-rebinding-persists` | `positive_control_a_runtime_that_ignores_loaded_overrides_fails` (and any-shape) |
| `crates/forge-input/tests/test_local_multiplayer.rs` — auto-join, seats, isolation, per-player contexts, loss and regain | `C-input-device-assignment` | `positive_control_no_re_pairing_on_reconnect_fails` (and joining owned devices) |
| `crates/forge-input/tests/test_input_budgets.rs` (timed alone) | `C-input-latency-budget`, `C-input-alloc-free-frame`, `C-input-idle-cost`, `C-input-update-budget` | `positive_control_events_a_frame_late_fail`, `positive_control_a_scratch_buffer_per_frame_fails`, `positive_control_evaluating_with_no_player_exceeds_the_idle_budget`, `positive_control_name_lookups_per_read_exceed_the_budget` |
| `crates/forge-input/tests/test_touch_gyro_haptics.rs` | `C-input-touch-gestures`, `C-input-onscreen-controls`, `C-input-gyro`, `C-input-haptics` | `positive_control_taps_that_ignore_movement_fail`, `positive_control_on_screen_controls_without_capture_fail`, `positive_control_an_uncalibrated_gyro_fails`, `positive_control_a_rumble_that_never_stops_fails` |
| `crates/forge-runtime/tests/test_controls_screen.rs` — the rebinding screen saves and survives a restart, swaps are named, composites per direction, pseudo-locale; the on-screen view; the menus' pad source | `C-input-game-ui` | `positive_control_a_screen_that_does_not_save_loses_the_rebinding` |

**Not built here (follow-ups):** motion samples from real pads (`C-input-gyro-device`: a HID
backend behind `MotionSource`); adaptive-trigger and HD haptics (DualSense) beyond two motors;
the runtime binary's game loop that pumps the winit and gilrs adapters every frame (the runtime
has no game loop until M7-13; the adapters are built and tested); keyboard and mouse from the
editor's game view into play-in-editor (the editor's runtime reads pads and virtual devices).

**Ch.29 Observability.** Tracy + Perfetto; named budgets per subsystem; **a perf gate
that fails CI on regression**, because a budget nobody enforces is a comment.

*Ch.29 as built so far (WP-11, DoD M1-11, ADR 0027) — the budgets and the gate;
Tracy/Perfetto and `forge-trace` are M2-11.* GPU rows have baselines per device class —
`rtx3080` (the dev box) and `warp` (Microsoft's software rasteriser, the GPU-less Windows CI
leg) — allowed `max(base x 1.25, base + slack[class])` (WP-18, ADR 0028; 1.5x before), the
slack being the class's timestamp quantisation and run-to-run spread (RTX 3080 0.003 ms,
WARP 0.05 ms) — a floor under the relative band, never a ceiling over it: the gate refuses a
budget file in which any row of any class would let a pass 15 % under its baseline grow 1.5x
without failing its own row; a device of any other class prints its GPU rows as NOT GATED
(lavapipe on the ubuntu leg: gate row `C-perf-gate-linux-leg` AWAITING a baseline). CPU rows
are the measured time over an in-process calibration workload (20,000 lattice fBm
evaluations, the same kind of scalar `f64` code), allowed `ratio x 1.5 + 0.002`, so one
number holds on any machine. Counters are exact. Coverage is checked both ways: a budgeted
row nobody measured and a measurement with no row both fail, so a new pass must get a named
budget. nextest gives the gate every test slot (`threads-required = "num-cpus"`,
`.config/nextest.toml`) so nothing shares the GPU while it measures; the measured table goes
to `$CARGO_TARGET_DIR/perf-gate.txt`, the source of a deliberate baseline update (never
raised to pass, W5).

*The gate as a crate (WP-44).* The gate is `tools/forge-perf-gate`: the budget file's model,
`check` (bands, slack floors, the SLACK check, coverage both ways), the calibration, the
alone-and-timestamped measuring, and the measurements every edition shares — the
atmosphere's precomputation and the 2D rows. Its own test,
`tools/forge-perf-gate/tests/test_perf_gate.rs` (gate `C-perf-gate`), gates them over
`tests/perf/budgets.ron`; its injected-regression control evaluates every 2D light 24 more
times per pixel and fails `render2d.frame.gpu.lights` against the clean run's own numbers.

*Gate rows that mix wall-clock timing with correctness (WP-30, backlog L-16 made this
explicit).* Proven by
`frame_budgets_bite_on_the_reference_device_and_correctness_everywhere` in the same file: a
25 ms frame fails only on the reference device, while a forced wait or a broken cover fails
everywhere.

*Ch.29 as built in WP-13 (M2-11, ADR 0030) — `forge-trace`.* One API over three sinks,
chosen at run time (`Tracer::enable(Sinks)`), in `crates/forge-trace` at the bottom of the
spine (no engine dependency, so every crate above may open zones):

| Piece | What it is |
|---|---|
| `zone!("sim.step")` / `Tracer::zone` | RAII zone, `&'static str` dotted name `subsystem.metric`; not `Send` (a zone opens and closes on one thread, so per-thread slices nest); captures the sinks at open. **Disabled: one relaxed atomic load and a branch** — no clock read, lock or allocation |
| `counter`, `instant`, `frame_mark` | a value (updates the budget of the same name), a named moment, the end of a frame (zone totals since the last mark become the frame's parts) |
| COUNTERS sink — the live feed | a 240-frame timeline of `FrameSample { total_ms, parts }`, the latest counters, **named budgets** (`declare_budget`, or `declare_budgets_ron` over `tests/perf/budgets.ron` for a device class) measured each frame, `over()` when over, `over_frames`/`samples`/`peak`, grouped by subsystem (`budgets_by_subsystem`); `FeedCell` (generation, live, a waker called once until consumed — `forge_ui::LiveCell`'s rules) is bumped per frame mark |
| PERFETTO sink | slices, counter samples and instants recorded per thread, capped (default 2M events; drops counted and written into the trace); `take_perfetto` encodes the native protobuf trace by hand — track descriptors (process, one per thread, one per counter) then `SLICE_BEGIN/END` properly nested per thread, `COUNTER`, `INSTANT` (frame marks, `over budget: <name>`, play controls) in time order; `perfetto::read` decodes it back |
| TRACY sink (feature `tracy`) | `tracy-client` (`enable`, `only-localhost`, `ondemand`, no `broadcast`): spans, plots, frame marks, messages; enabling it in a build without the feature is `TRACE-0003` |
| Editor binding | `forge_editor::sim_bridge::TraceCounters` is the profiler's `CounterSource` (§21.21, WP-U6) and the editor's default one (`EditorServices::profiler`, over the process tracer, `EditorServices::tracer`; the editor binary turns the COUNTERS sink on at start): a `forge_ui::LiveSource` over the feed, and `sim_bridge::profile_snapshot` maps `TraceSnapshot` to the panel's `ProfileSnapshot` — the timeline's frames (time between marks, and one part per zone that ran that frame) and the lanes copied; a budget's name, value, allowance and unit (its `peak`/`samples`/`over_frames` stay in the tracer, for `--trace` reports); counters and dropped events into `ProfileSnapshot::counters`/`dropped_events` (added for it); replication `None` with the reason (forge-net, M4-7). Producers: the play core (`sim.step` zones, `sim.bodies`, a frame mark per step batch) and the viewport's GPU host (`viewport.render_cpu`, the adapter lane, a frame mark per rendered view); the panel's own redraws mark nothing, so it cannot wake itself. `crates/forge-editor/tests/test_play_trace_backends.rs` (gate `C-profiler-trace-source`) plays through the shell's bus controls and requires the profiler to show the steps; positive control: a profiler not reading the tracer fails |

Guards: `crates/forge-trace/tests/test_trace_overhead.rs` (gate `C-trace-disabled-overhead`)
measures a disabled zone, a disabled `zone!` and a disabled counter against an empty loop
(minimum of nine 2M-iteration trials; runs alone in nextest's wall-clock set) and requires
each under 2 ns — measured 0.43 / 0.59 / 0.22 ns on the dev box — its positive control, a zone
that reads the clock while disabled, measures ~55 ns and fails; `test_perfetto_trace.rs`
(`C-perfetto-trace-well-formed`) decodes a three-thread recording and checks descriptors
before events, balanced nesting, time order, exact counter values and the instants, with a
checker that rejects an unbalanced trace. Enabled costs, for the record: ~75 ns per zone with
counters, ~70 ns with Perfetto; 900k events encode in ~0.2 s. Tracing never changes a
simulation (`crates/forge-sim/tests/test_step_cost.rs`: identical state hash with every sink
combination); one step of 10,000 moving bodies costs 0.93 ms in a release build (5.5 ms in a
dev build, where forge-core's checked `RegionView` runs unoptimised) and the disabled `sim.step`
zone is under 0.0003 % of it. Opening a trace in the Perfetto UI and connecting Tracy are an operator's check
(`C-trace-viewers`, AWAITING). `forge --headless --trace FILE` writes a run's trace and prints
each budget's peak and over-budget frames.

---

# Chapter 21 — UI Toolkit & Editor Shell

*FULL. Expanded from BRIEF by WP-U0. The decision it rests on is ADR 0001
(`docs/adr/0001-retained-ui-from-day-one.md`), which amends O-6.*

**There is one UI layer. The editor and shipped games both use it, and the editor has been
built on it since the editor's first commit.** Godot is built with its own UI toolkit, and
that is why building tools in Godot is plausible for its users. Forge has the same
property. The editor shell is a **client of `forge-cmd` and nothing more** (I7). Every
panel it shows is a plugin (I16), and every preset can open every panel (I15).

## 21.1 The decision, and what it replaces

The BRIEF version of this chapter and O-6 planned an `egui` editor for M2–M4 and a
retained layer at M5–M7, with the editor moved across afterwards. **D-1 / ADR 0001 drops
the stopgap.** `forge-ui` is retained from the start:

- **Built on:** `taffy` for layout, `cosmic-text` for shaping, `AccessKit` for
  accessibility, `wgpu` for rendering, `winit` for windows and input.
- **Why:** an immediate-mode editor redraws every frame while anything moves, and it would
  have been rewritten panel by panel at M5. A retained, damage-tracked editor draws nothing
  while idle, and no panel is written twice.
- **Where the full argument lives:** ADR 0001.

`egui` is not a dependency of any crate at any milestone. `test_ui_wgpu_confined` checks
this against the resolved graph (§21.23).

## 21.2 Crates, layering and dependencies

| Crate | Is | Depends on (forge) | Linked by |
|---|---|---|---|
| `forge-ui` | the widget layer: tree, bindings, layout, text, style, input, a11y, damage, renderer | `forge-frames` (only for the `FramePos` editor widget's value type) | `forge-editor`, panel plugins, `forge-runtime`, games |
| `forge-editor` | the shell: window host, dock host, action registry, bus client, `ProjectMirror` | `forge-ui`, `forge-cmd`, `forge-reflect`, `forge-plugin`, `forge-store` (types only: `StorePath`, `RevId`, `Rev`; the shell never calls a `ProjectStore` method, §21.18) | the `forge` editor binary |
| `plugins/forge-panels-*` | every panel in §21.21, as first-party **source plugins** | `forge-editor`'s public panel API, `forge-ui` | the editor binary, via the plugin registry |

- **Until `forge-gpu` lands (D-3),** `forge-ui` depends on `wgpu` directly, and only in
  one module (§21.13).
- **When `forge-gpu` lands,** that module takes its device from `forge-gpu`'s adapter pool
  and nothing else changes. *(Done in WP-08: `forge-ui` depends on `forge-gpu`, names `wgpu` only as `forge_gpu::wgpu` inside `render_wgpu`, and `RunOptions::gpu` lets the editor pass its own pool; §21.13.)*
- **`f32` is allowed.** `forge-ui` is above `forge-render`, so it may use `f32` for pixel
  math (Ch.1.5). **World data is never `f32` in the UI:**
  - a `FramePos` is shown and edited as `f64` text;
  - the viewport camera is `f64` and camera-relative (Ch.2 §2.4, WP-U6).
- **The whole layer is `#![forbid(unsafe_code)]`.** Surface creation from `Arc<Window>`
  is safe `wgpu` API, so no Ch.1.5 exception is needed.

**Module map of `forge-ui`.** A module does exactly the job its name says:

| Module | Job |
|---|---|
| `id` | `WidgetId`, `Key` |
| `tree` | the retained widget arena |
| `state` | `Signal`, `Memo`, `Effect`, `Bind` |
| `layout` | the `taffy` bridge and measure functions |
| `text` | `cosmic-text` buffers, the shaping cache, the glyph atlas allocator |
| `style` | tokens, themes, style resolution |
| `input` | events, hit testing, pointer capture |
| `focus` | focus scopes, traversal, spatial navigation |
| `ime` | the IME bridge |
| `clipboard` | the clipboard bridge |
| `dnd` | typed drag-and-drop |
| `a11y` | the AccessKit tree |
| `damage` | dirty flags, damage rects, frame scheduling |
| `anim` | tweens, springs, reduced motion |
| `virtual` | virtualised list, tree and table |
| `widgets` | the §21.16 catalogue |
| `render` | the display list, the batcher, the `UiRenderer` trait and a recording renderer. **No `wgpu`.** |
| `render_wgpu` | feature `wgpu`. The **only** module that names `wgpu`. |
| `platform_winit` | feature `winit`. The runner, windows, DPI, OS events. |

**Third-party dependencies.** Each is checked by `cargo-deny` against the Appendix A.5
allow-list when it is added (I13). Versions are the current releases when WP-U1 adds them,
pinned in `Cargo.lock`.

| Crate | Licence | For |
|---|---|---|
| `taffy` | MIT | flexbox + CSS grid layout |
| `cosmic-text` (+ `swash`, `rustybuzz`, `fontdb`) | MIT / Apache-2.0 | shaping, bidi, line breaking, rasterisation, font discovery |
| `accesskit`, `accesskit_winit` | MIT / Apache-2.0 | UIA (Windows) and AT-SPI (Linux) |
| `winit` | Apache-2.0 | windows, input, IME events, DPI |
| `wgpu` | MIT / Apache-2.0 | the renderer (one module) |
| `etagere` | MIT / Apache-2.0 | shelf packing for the one glyph atlas and the one icon atlas |
| `lyon_tessellation` | MIT / Apache-2.0 | paths for the curve, gradient and graph editors |
| `resvg` / `tiny-skia` | Apache-2.0 OR MIT / BSD-3-Clause | rasterising SVG icons into the icon atlas at the display DPI |
| `arboard` | MIT / Apache-2.0 | the OS clipboard |
| `slotmap` | Zlib | the widget arena |

**Fonts.** The bundled UI font is Roboto plus Roboto Mono, **both from their Apache-2.0
releases**. OFL-1.1 is not on the A.5 list, and widening it is an owner decision (ADR 0001
§7). The faces are compiled into `forge-ui` (D-7, ADR 0013), so cold start scans no
system fonts and every machine lays text out identically. Other scripts and emoji fall back
to fonts installed on the user's system: `fontdb` scans them **lazily, once**, on the first
glyph the bundle lacks, and they are never redistributed. Until an Apache-2.0 Roboto Mono
file is vendored, Droid Sans Mono (Apache-2.0) is the mono face (`crates/forge-ui/fonts/`).
Gate row `C-ui-lazy-font-fallback` (bundled faces, lazy system scan) is bound to
`test_ui_bundled_font`; `C-ui-bundled-font` (the googlefonts Roboto + Roboto Mono release
files themselves) is Awaiting the owner's approval to download them. **Icons** are Lucide (ISC). **Fuzzy
matching** (command palette, search fields) is written in-house because `nucleo` is
MPL-2.0.

## 21.3 The frame pipeline, and the zero-idle rule

One UI thread per process owns the widget tree, the signal runtime and the renderer. All
UI state is **serialised against the UI thread (W7)**. Other threads never touch it: the
bus client, asset thumbnails and the viewport renderer post messages into a queue. The UI
thread drains that queue at the start of each iteration, and the posting thread wakes the
loop with `EventLoopProxy`.

```
  OS events ─┐   bus Applied deltas ─┐   timers/animations ─┐
             ▼                       ▼                      ▼
   ┌──────────────────────────  one loop iteration  ───────────────────────────┐
   │ 1 drain queue → 2 route events → 3 run actions → 4 flush signals (batched) │
   │ 5 layout dirty subtrees (taffy) → 6 re-record dirty display-list slices    │
   │ 7 a11y TreeUpdate for changed nodes → 8 if damage ≠ ∅: batch + render      │
   └────────────────────────────────────────────────────────────────────────────┘
            ▲ ControlFlow::Wait — or WaitUntil(next timer/animation deadline)
```

**The zero-idle rule (D-5).** The loop blocks in `ControlFlow::Wait` whenever there is no
damage, no running animation and no due timer. Nothing polls, and nothing redraws "just in
case." Four places where editors usually leak redraws are closed on purpose:

- **The text caret** blinks for 5 s after the last input and then stays solid. The user
  benefit is that WCAG 2.2.2 asks for blinking longer than 5 s to be stoppable, and the
  editor becomes truly idle. The blink can be turned off in Settings.
- **Spinners and progress animations** run only while their widget is **visible** and its
  task is live. A spinner hidden in a background tab or scrolled out of view does not
  schedule frames.
- **Hover fades** are animations that end. They are not a steady state.
- **Live-data panels** (profiler, remote latency, compute throughput, presence) refresh
  only when their data changed, only while visible, at a capped rate, and never because of
  the UI's own frame counters (§21.11, "Live-data panels"). Without that rule a profiler
  showing the editor's frame time would schedule a frame for every frame it drew.

`ui_idle_zero_redraw` (§21.22) opens every M2 panel, focuses a text field, lets the caret
timeout expire, then advances an injected clock by 10 s. It runs once with every
live-data panel in the foreground (visible) and once with each hidden, with their sources
quiescent. It asserts **0 frames rendered and 0 loop wakeups**. Its positive control leaks
a spinner in a hidden tab, and the test must fail on it.

## 21.4 The widget tree and stable ids

The tree is **retained**: it is built once per panel and then changed in place by bindings
(§21.5). It is never rebuilt every frame.

```rust
/// Stable across frames AND across runs: hash64(parent WidgetId, Key).
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct WidgetId(u64);

pub enum Key { Static(&'static str), Index(u32), Id(u64), Str(Arc<str>) }

pub trait Widget: 'static {
    /// AccessKit role. Required: a widget with no role does not compile.
    fn role(&self) -> Role;
    /// Leaves only (text, images, virtualised containers); containers are pure taffy.
    fn measure(&mut self, cx: &mut MeasureCx, known: Size<Option<f32>>,
               avail: Size<AvailableSpace>) -> Size<f32> { Size::ZERO }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled;
    /// Records this widget's primitives into its own display-list slice (§21.11).
    fn paint(&self, cx: &mut PaintCx);
    fn a11y(&self, cx: &mut A11yCx, node: &mut accesskit::Node);
}
```

- **Storage.** A generational arena (`slotmap`) holds the nodes, with a `WidgetId → slot`
  index beside it. Removing a widget frees its slot, and a stale handle is detected
  instead of aliasing a new widget.
- **Keyed children.** An unkeyed static child gets `Key::Index(i)`. A child in a dynamic
  collection **must** be keyed by its model identity, for example `Key::Id(entity.to_bits())`.
  Reordering a keyed list then moves widgets instead of recreating them, so focus, scroll
  position and animations survive the reorder.
- **What stability buys.** A `WidgetId` is the same value in every run. That one property
  serves four uses:
  - It is the AccessKit `NodeId`, so a screen reader keeps its place across updates.
  - It is where focus is restored after a panel is rebuilt.
  - It is the selector UI tests use.
  - It is the key for per-widget session state (scroll offsets, expanded tree nodes,
    splitter ratios), which is saved with the layout.

## 21.5 State, bindings and reactivity

**Fine-grained signals.** A panel's build function runs **once**. Each dynamic property is
bound to a signal, and when that signal changes, only its subscribers are invalidated.
Nothing is diffed and no build function re-runs. This choice is what makes "work
proportional to change" true.

```rust
pub struct Signal<T>;   // Copy handle into the UI Runtime's arena
pub struct Memo<T>;     // derived; recomputed lazily, only if a dependency changed
pub enum Bind<T> { Const(T), Signal(Signal<T>), Memo(Memo<T>) }

impl<T: PartialEq + 'static> Signal<T> {
    pub fn get(&self, rt: &Runtime) -> T where T: Clone;
    pub fn with<R>(&self, rt: &Runtime, f: impl FnOnce(&T) -> R) -> R;
    /// Setting an equal value is a no-op: no invalidation, no damage.
    pub fn set(&self, rt: &mut Runtime, v: T);
}

// A panel builds once; the label re-shapes only when `name` changes.
let name = mirror.watch(rt, entity, path!("Name"));   // Memo<String>, fed by the bus
row![ icon(Icon::Entity), label(name), toggle(visible).on_change(SetVisible) ]
```

**How changes propagate.**

- Signal writes during an iteration are **batched**. Subscribers run once, in dependency
  order, at step 4 of §21.3.
- An invalidated subscriber marks its widget `LAYOUT` or `PAINT` dirty (§21.11), and
  nothing else.
- **Structure** changes through keyed `For` over a signal of keys, and through the
  virtualised containers (§21.12), which ask their data source for rows on demand.

**Actions.** Widgets do not mutate anything outside themselves. They raise **typed
actions**: `on_press(|cx| cx.action(DeleteSelected))`. Actions bubble to the nearest
handler. In the editor, handlers are panel code, and panel code changes project state only
through the command emitter (§21.18). In a game, handlers are game systems. `forge-ui`
itself does not depend on `forge-cmd`, and never will, so it can be used from
`forge-runtime` (§21.20).

## 21.6 Layout (`taffy`)

- **One taffy node per widget.** A widget's style resolves to a `taffy::Style` (flex,
  grid, block, absolute).
- **Leaves** (text, images, virtualised containers) supply measure functions:
  - A text measure returns the size from the cached shaped buffer (§21.7). It never
    shapes again just to measure.
  - A virtualised container measures as its viewport. Its rows are laid out by the
    virtualiser, not by taffy (§21.12).
- **Incremental layout.** A `LAYOUT`-dirty widget marks its taffy node dirty.
  `compute_layout` runs only from the highest dirty ancestor whose size could change, and
  taffy's own cache handles the rest. A change inside a fixed-size panel therefore never
  lays out its siblings.
- **Units.** Layout runs in **logical pixels**. Edges snap to physical pixels only at
  paint time (§21.14). Snapping during layout would make layout depend on DPI.

## 21.7 Text (`cosmic-text`)

**Shaping and wrapping.**

- `cosmic-text` does bidi, script itemisation, shaping (`rustybuzz`), line breaking and
  font fallback.
- Every text-bearing widget owns a `TextLayout` handle into the **shaping cache**. The
  cache key is `(text hash, attrs, font size × scale factor, wrap width bucket)`.
- A label re-shapes only when its text, attributes or scale change, or when its wrap width
  crosses a bucket. Buckets are 1 logical px, and only wrapping text has a width in its
  key.
- Unchanged text **never re-shapes**. `ui_text_shaping_cached` counts shape calls to
  prove it.

**One glyph atlas (D-5), in two planes.**

- There is **one** glyph atlas: one object, one key space, one bind group. It has two
  planes, because almost every glyph is a coverage mask and only emoji carry colour:

  | Plane | Format | Holds | Starts | Grows to (cap) | VRAM at cap |
  |---|---|---|---|---|---|
  | mask | `R8Unorm` | coverage of every outline glyph | 2048² (4 MB) | 4096² | 16 MB |
  | colour | `Rgba8UnormSrgb` | colour glyphs (emoji, COLR/bitmap fonts) | 512² (1 MB) | 2048² | 16 MB |

- **Why two planes and not one RGBA8 texture** (owner rule 2). A single RGBA8 atlas
  stores four bytes per texel for glyphs that need one. Grown 2048² → 4096² → 8192², it
  costs 16 / 64 / 256 MB. The two planes cost 5 MB at start and **32 MB at most**, which
  matters on the O-8 floor of 4 GB VRAM, where the viewport and the world need that memory
  far more than text does.
- **It is still one bind.** Both planes sit in the same bind group as the icon atlas and
  the image slot table (§21.13). Each glyph instance carries a one-bit plane flag, and the
  uber-shader samples `mask.r` × the text colour, or `colour.rgba`. A plane switch never
  breaks a batch.
- Each plane is shelf-packed with `etagere`. Glyphs are keyed `(font id, glyph id, size px,
  subpixel x bin of 4)`, and the key decides the plane.
- A plane grows by reallocating and copying, up to its cap. The atlas **never opens a
  second texture per plane**.
- Glyphs unused for 600 frames that were actually drawn are evicted when a plane runs
  short. Idle time does not count toward those 600 frames, because idle draws no frames.
- **At the cap, after eviction.** A 4096² mask plane holds about 25,000 glyphs at 28 px
  (14 px text at 200% scale), which is more distinct glyphs than a 4K screen of CJK text
  shows at once. If one frame's working set still does not fit, the frame is drawn in
  **atlas epochs**: the batcher draws what is resident, the plane is recycled, and the
  rest is drawn in a second pass. Each epoch costs one extra draw call. `FrameStats`
  counts epochs, and the frame is never drawn with missing glyphs.

**Rendering quality.** Text is greyscale anti-aliased with gamma-correct blending and
4-bin horizontal subpixel positioning. LCD subpixel AA is deliberately not used: it is
wrong on rotated, transparent or scaled surfaces, and on the OLED/RGBW panels many
laptops now ship.

**Editing** is built on `cosmic-text`'s `Editor`, which supplies grapheme-correct cursor
movement, word and line selection, and bidi-aware caret placement. The widget adds an
undo stack for the field and IME (§21.9).

## 21.8 Style, themes and tokens

**Tokens.** Styling goes through **design tokens**, which are named values a theme
supplies:

- colour roles: `bg.base`, `bg.raised`, `fg.primary`, `fg.muted`, `accent`, `danger`,
  `warning`, `success`, `focus.ring`, `selection`;
- spacing steps, radii, border widths;
- the type scale;
- elevation shadows;
- motion durations and easings.

**Widgets name tokens and never raw colours.** A lint in `forge-ui`'s tests rejects a
colour literal anywhere under `widgets/`.

**Themes are data.** A theme is a RON file of token values.

| Theme | Contract |
|---|---|
| `forge.dark` (default) | text on its background ≥ 4.5:1, UI boundaries ≥ 3:1 (WCAG 2.1 AA) |
| `forge.light` | same as dark |
| `forge.high-contrast` | text ≥ 7:1 (AAA), focus ring ≥ 3:1 against every adjacent background, no information carried by colour alone |

`test_ui_contrast` computes every `(fg, bg)` pair the widgets actually use, for all three
themes, and fails on any pair below its theme's floor. The positive control is a theme
with one low-contrast token, and it must fail.

**Style resolution** goes widget type → variant (`primary`, `ghost`, `danger`) → state
(`hover`, `pressed`, `focused`, `disabled`) → local override. It is resolved once when the
state changes, never per frame.

**Users can change themes** by copying a theme file. `Theme` is an extension point
(§21.19), so a plugin can add or replace one. Games use the same mechanism for brand
styling (§21.20).

## 21.9 Input, focus, keyboard navigation, IME, clipboard, drag-and-drop

**Pointer input.**

- Hit testing walks the painted rects front to back, honouring clips and the
  `hit_test: false` style. A drag captures the pointer.
- Hover is tracked per widget. Enter and leave events fire only on change.

**Focus.**

- Every interactive widget is focusable.
- **Tab / Shift+Tab** traverse in visual order within a focus scope. Panels, dialogs and
  popovers are scopes. A modal traps focus.
- Arrow keys move inside composite widgets (lists, trees, tables, menus, tabs, radio
  groups) following the WAI-ARIA authoring practices.
- **Spatial navigation** is the fallback for gamepads (§21.20). It picks the nearest
  focusable widget in the pressed direction, weighted by overlap.
- `F6` cycles focus between panels.
- The focus ring is always drawn when focus arrived by keyboard, a 2 px gap outside the
  widget so its only neighbour is the background it sits on (ADR 0012).
- F6 skips panels with nothing focusable and restores each panel's last focus.
- `test_ui_focus_traversal` walks every gallery widget by keyboard alone.

**IME.**

- `winit` `Ime::Preedit` and `Ime::Commit` events drive the focused text widget.
- Pre-edit text is drawn in place with an underline and its own cursor. It is never
  committed to the model before `Commit`.
- The caret rect is reported every frame the caret moves, through
  `Window::set_ime_cursor_area`, so the candidate window follows the caret.
- IME is enabled only while a text widget has focus.
- `test_ui_ime_composition` drives a synthetic Japanese composition (pre-edit, candidate
  change, commit, cancel). It asserts that the model sees exactly the committed text, once.

**Keyboard shortcuts** are resolved by the keymap (§21.17). Resolution runs against the
focus path, so a key bound in a text field (for example Ctrl+Z as field undo) is handled
there before it reaches the global binding (the project undo).

**Clipboard.**

- Text goes through `arboard` behind the `Clipboard` trait (`OsClipboard`). Its Windows
  backend is BSL-1.0, admitted to A.5 by D-8 (ADR 0006). The in-process D-4 clipboard
  serves headless and wherever no platform clipboard exists, and says so. Gate row
  `C-ui-os-clipboard` is bound to `test_ui_os_clipboard`.
- Structured editor content (entities, components, graph nodes) is copied as
  **RON over reflect** under a Forge MIME type, with a plain-text fallback.
- **Copy is a read. Paste is a command** (§21.18).

**Drag-and-drop.**

- Drags carry typed in-process payloads: `Entities`, `Asset`, `Panel`, `GraphNodes`,
  `Files`. A drop target declares the payload types it accepts, and the preview shows
  "accepted", "refused (reason)" or an insertion line.
- OS file drops arrive as `winit` `DroppedFile` and become import commands.
- **Dragging out to the OS is not supported.** `winit` has no API for it. This is
  recorded as a known gap, not hidden.

## 21.10 Accessibility (`AccessKit`)

**Every widget contributes an AccessKit node:**

- its role (required by the `Widget` trait);
- its name, which is its label or its `aria-label` equivalent;
- its value, and range for sliders and numeric fields;
- its states: disabled, checked, expanded, selected, busy;
- the actions it supports: click, focus, set value, increment/decrement, expand/collapse,
  scroll into view.

**Updates are incremental.** Step 7 of §21.3 emits a `TreeUpdate` containing only the
nodes whose accessible properties changed. The adapter is **activated lazily**: no tree is
built until an assistive technology connects (`accesskit_winit`'s activation handler), so
the cost is zero for users who do not use one.

**Virtualised containers** expose `row_count` and `row_index` for the full model and
realise nodes only for live rows. A screen reader announces "row 51,203 of 100,000"
without 100,000 nodes existing.

**Platform coverage.** UIA on Windows and AT-SPI on Linux, co-primary (I18). AT-SPI is
observed in the Linux verify container (WP-U12): `crates/forge-ui/examples/atspi_probe.rs`, a
screen-reader-side client over D-Bus, walks `ui_gallery`'s tree on the accessibility bus
(`tools/docker/linux-verify/atspi-probe.sh`; evidence `docs/evidence/linux/M2-22.md`).

`test_ui_a11y_tree` asserts:

- every interactive widget in the gallery and in every M2 panel has a role and a non-empty
  name;
- every icon-only button has a label;
- the tree's focus matches UI focus.

The positive control is an unlabelled icon button, and it must fail.

## 21.11 Damage tracking and zero-idle redraw

**Dirty flags.** Each widget carries `LAYOUT`, `PAINT` and `A11Y` flags.

- A signal write sets flags on its subscribers only.
- `LAYOUT` implies `PAINT` for the widget and for any sibling whose rect moved.

**The display list** is retained and partitioned into **one slice per widget**. Painting
re-records only `PAINT`-dirty slices. Everything else is reused byte for byte.

**Damage** is the union of the old and new rects of every re-recorded slice, plus shadow
outsets, collected per window.

**Rendering.**

- **Empty damage:** nothing is rendered, submitted or presented.
- **Non-empty damage:** the renderer re-rasterises only the damaged rects, with a scissor,
  into a persistent per-window UI target texture, then composites that target and any
  viewport textures (§21.13) into the swapchain image.
- Typing one character therefore rasterises a field-sized rect, not the window.
- `wgpu` swapchains have no portable partial present, so the composite copies the whole
  surface. It is a single textured quad, and it happens only on frames that have damage.

**Timers** (tooltips after 500 ms, key repeat, caret blink until its 5 s timeout, toast
expiry) are deadlines in one timer heap. The loop sleeps in `WaitUntil(earliest)`, and
fires nothing early.

**Live-data panels.** Some panels show data that changes by itself: the profiler (engine
and UI counters), the remote latency indicator, compute and farm throughput, and presence.
They could easily defeat the zero-idle rule. Every such panel reads its data through one
mechanism, and no panel polls on its own:

```rust
pub trait LiveSource: Send + Sync {
    /// Bumped by the producer whenever the data changes. Unchanged ⇒ nothing to draw.
    fn generation(&self) -> u64;
    /// True while the producer is running (engine playing, session connected, job live).
    fn is_live(&self) -> bool;
    /// Rule 1: the shell registers a waker only while the panel is visible (never for a
    /// `self_ui` feed); the producer calls it on a bump, at most once until `consumed`.
    fn set_waker(&self, waker: Option<Arc<dyn UiWaker>>);
    fn consumed(&self);
}
pub struct LiveFeed { pub source: Arc<dyn LiveSource>, pub max_hz: u8, pub self_ui: bool }
```

The shell applies four rules to every `LiveFeed`:

1. **Push, not poll.** The producer bumps `generation()`. If a waker is registered, it
   wakes the loop through `EventLoopProxy`, at most once until the UI has consumed the
   bump. A quiescent source arms no timer and causes no wakeup.
2. **Only while visible.** The shell registers a feed's waker only while its panel is
   visible. A panel is not visible in a background tab, a collapsed section, a minimised
   window, or a window `winit` reports `Occluded`. A hidden feed's producer bumps its
   generation silently, so a playing engine behind a hidden profiler costs no wakeups.
   On becoming visible, the panel compares generations and refreshes once if they differ.
3. **Capped, and only on a visible change.** A visible feed refreshes at most `max_hz`
   times a second. The defaults are profiler 10 Hz, remote latency 2 Hz, compute
   throughput 2 Hz, and presence 10 Hz, with presence events coalesced. The panel's
   signals hold the **displayed** values, such as latency rounded to 1 ms. Setting an
   equal value is a no-op (§21.5), so a change the user could not see causes no damage.
4. **The UI's own counters never schedule a frame.** UI-frame counters (frames rendered,
   UI CPU time, draw calls, damage area) are written into a `ui.self` namespace whose feed
   has `self_ui: true`. A `self_ui` bump never wakes the loop and never marks anything
   dirty. The profiler shows the `ui.self` values as of the last frame that was drawn for
   some other reason. Drawing a frame therefore can never cause the next frame.

As a result, an editor with the profiler open and the engine stopped draws nothing. With
the engine playing, it redraws the profiler's rect at ≤ 10 Hz, and only while the profiler
is visible. `ui_live_panel_refresh_bounded` (§21.22) proves all four rules.

## 21.12 Virtualisation

`VirtualList`, `VirtualTree` and `VirtualTable` render only the rows that intersect the
viewport, plus an **overscan of 8 rows** on each side.

- **The row index is a counted B-tree, not a flat array.** Rows are held in a `CountedSeq`:
  a B-tree with fanout 32 whose leaves are rows and whose internal nodes store two sums
  over their subtree, the **row count** and the **height sum**. It supports, in
  O(log₃₂ n) node visits each:
  - row index → row, and row index → pixel offset (descend, subtracting the sums);
  - pixel offset → row (the scroll position → first visible row);
  - insert or remove a row, or a whole run of rows as one subtree;
  - change one row's height (update one leaf and the sums on its path).

  A Fenwick tree is deliberately not used: its positions are fixed, so inserting or
  removing rows in the middle means rebuilding it in O(n).
- **Row heights.** Uniform heights are the fast path: the offset is `index × h`, and the
  height sums are not kept at all. Variable heights keep the sums.
  - An unmeasured row counts at a **fixed estimate per row kind**, which its `RowSource`
    declares. It is never a running median: a moving estimate would change every
    unmeasured row's contribution at once, which is O(n) per change.
  - Measuring a row replaces its estimate with the measured height: one leaf update.
  - Scrolling to row *k* is exact once row *k* has been measured.
  - **Scroll anchoring.** When measurements above the viewport replace estimates, the
    first visible row keeps its screen position, so content never jumps under the user.
- **Data source.** A row is fetched through a `RowSource` trait:

  ```rust
  pub trait RowSource {
      fn len(&self) -> usize;
      fn key(&self, i: usize) -> Key;
      fn build(&self, i: usize, cx: &mut BuildCx) -> WidgetId;
  }
  ```

  A row widget scrolled out is **recycled** into a pool keyed by row kind. It is re-bound,
  not re-created.
- **Trees are never flattened.** The visible-row index of a tree is the model tree
  itself, counted:
  - Every node *x* keeps its children in its own `CountedSeq`, and keeps
    `inner(x)` = the number of rows its children show if *x* is expanded (the sum of
    `rows(c)` over its children). `rows(x)` = 1 + (`inner(x)` if *x* is expanded,
    otherwise 0).
  - `inner(x)` is maintained **whether or not *x* is expanded**, so re-expanding a node
    does not have to count its subtree again.
  - **Row index → node.** Descend from the root: in each node's `CountedSeq`, find the
    child whose row range contains the index, and subtract the rows before it. The cost
    is O(*d* · log₃₂ *b*), for tree depth *d* and largest fanout *b*.
  - **Expand or collapse *x*.** `rows(x)` changes by ±`inner(x)`. That delta is added to
    the sums on the path from *x*'s leaf up through each ancestor's `CountedSeq`, which
    is O(*d* · log₃₂ *b*) node updates **whatever the number of rows *k* that appear or
    disappear**. Expanding a 50,000-row subtree touches a few dozen index nodes, not
    50,000 rows.
  - **Insert, remove or move a model node.** One `CountedSeq` insert or remove in the
    parent, plus the same delta propagation up the ancestors: O(*d* · log₃₂ *b*).
  - With variable heights, the height sums propagate on the same paths.
  - Nothing is re-flattened on expand, on edit, or per frame. `ui_virtual_tree_100k`
    counts index-node updates per expand and gates them at (*d* + 1) · (⌈log₃₂ *b*⌉ + 1)
    on every leg.
- **Flat lists** are one `CountedSeq` (*d* = 1). A uniform-height list keeps only the
  counts.
- **Tables** add sortable, resizable columns. Sorting happens in the `RowSource` (model
  side). The table only re-binds live rows.
- **The budget (D-5).** 100,000 rows: ≤ 2 ms of layout+paint per frame while scrolling.
  Live row widgets stay ≤ ⌈viewport ÷ row height⌉ + 16 whatever the row count
  (`ui_virtual_list_100k`, `ui_virtual_tree_100k`, `ui_virtual_table_100k`, §21.22).
- **Implementation (WP-U2, ADR 0013).** Rows are painted by the view from its model
  instead of being built as recycled row widgets: `RowSource` is realised as the view's
  model API (`VirtualTree::insert`, `move_node`, `set_label`, ...; `TableModel` for
  tables), and only the live range gets accessibility nodes. The counted index, the
  overscan, the live-row bound and the budgets are as specified above.

## 21.13 The renderer: `UiRenderer`, batching, and the `wgpu` implementation

```rust
pub enum Primitive {
    Quad { rect: Rect, radii: Corners, fill: Fill, border: Border, shadow: Option<Shadow> },
    Glyphs { run: GlyphRunId, origin: Point, color: Color },     // atlas-resident
    Image { tex: TextureId, uv: Rect, rect: Rect, tint: Color }, // icons, thumbnails
    Viewport { tex: ExternalTexture, rect: Rect },               // forge-render output
    Mesh { verts: MeshId },                                      // lyon-tessellated paths
    PushClip { rect: Rect, radii: Corners }, PopClip,
}

pub trait UiRenderer {
    fn upload(&mut self, tex: TextureId, update: TextureUpdate) -> Result<(), UiError>;
    fn resize(&mut self, target: TargetId, size: PhysicalSize) -> Result<(), UiError>;
    /// Renders only `damage`; returns counters the budget tests read.
    fn render(&mut self, target: TargetId, batches: &[Batch], damage: &[PxRect])
        -> Result<FrameStats, UiError>;
}
```

**Batching belongs to `forge-ui::render`, not to a backend.**

- `batch::build(&DisplayList) -> Vec<Batch>` is renderer-independent, so the draw-call
  budget is tested headless.
- All sampled textures share one bind group: the glyph atlas's two planes (§21.7), the
  icon atlas, and a small slot table of images and thumbnails. The icon atlas is `R8Unorm`
  too, because Lucide icons are single-colour and are tinted when drawn.
- A batch therefore breaks only on a **clip change** or a **viewport composite**. It never
  breaks on a texture change.
- Quads (rounded, bordered, shadowed with an analytic SDF blur), glyphs and images are all
  instanced through **one uber-pipeline**. Meshes use a second pipeline.
- **The budget:** a 1,000-button panel draws in ≤ 4 draw calls (`ui_draw_calls_batched`).

**Implementations.**

| Implementation | Where | Use |
|---|---|---|
| `WgpuRenderer` | `render_wgpu` (feature `wgpu`) | real windows, and offscreen targets for pixel goldens |
| `RecordingRenderer` | `render` | headless tests. It records batches and `FrameStats`. The budget, damage and display-list golden tests use it, and they run on every CI leg with no GPU. |

**Goldens come in two kinds.**

- **Display-list goldens** are exact hashes of the display list. They are
  platform-independent and gate every leg.
- **Pixel goldens** run on `WgpuRenderer` wherever an adapter exists. That includes the
  software adapters: WARP on Windows, lavapipe on Linux when installed. They compare with a
  fixed perceptual threshold that is written at the constant and never widened (W5). Where
  no adapter exists the test reports `AWAITING(no adapter)`. It never passes silently (W9).

**Moving onto `forge-gpu` (D-3).** `WgpuRenderer::new` takes a `wgpu::Device` and
`wgpu::Queue`. Today the editor creates them. After M1-1 they come from `forge-gpu`'s pool,
always the primary adapter, because UI is latency-critical and not Tier-0 work (Ch.26).
*Done in WP-08 (Ch.9 §9.1):* `GpuContext` wraps a pool member (`from_pool`, `for_surface`,
`headless`); `WgpuRenderer::for_window_in(pool, ..)` renders on the first member that can
present (the primary), `for_window` builds a default pool; `new` returns `Result` because
its shaders now compile through `forge_gpu::shader`. Budgets on the GPU path:
`test_ui_on_forge_gpu`.

**Viewport composition.** `forge-render` (or WP-U6's placeholder renderer before it
exists) renders the 3D viewport into its own texture on its own schedule. The UI
composites it as a `Viewport` primitive. A new viewport frame damages only the viewport's
rect.

## 21.14 Windowing, DPI and multi-window (`winit`)

- **The runner.** `platform_winit::run(app)` implements `winit`'s `ApplicationHandler`.
  The loop starts in `ControlFlow::Wait`.
- **Multi-window.** One `UiWindow` exists per OS window: the main window, floating dock
  windows (§21.17), and detached viewports.
- **Shared resources.** All windows share one device, one glyph atlas, one icon atlas and
  one shaping cache. Each window has its own surface, UI target and damage set.
- **DPI.**
  - Every coordinate above `render` is logical pixels.
  - Each window has its own scale factor from `winit` (per-monitor DPI on Windows,
    per-output scale on Wayland/X11). A user **UI scale** setting (50–300%) multiplies it.
  - A scale change re-shapes text at the new size and re-rasterises the icon atlas.
    Layout does not change, because layout is logical.
  - Rect edges snap to physical pixels at paint time, so 1-px borders stay crisp at 125%
    and 150%.
- **Surfaces** use `PresentMode::Fifo` (vsync), which gives no tearing and no busy
  presenting. Frames are requested only on damage (§21.11).

## 21.15 Animation and transitions

- **What animates.** Tweens and critically-damped springs, on style properties only:
  opacity, colour, offset, size, and the scroll position during smooth scroll.
- **The clock.** A monotonic presentation clock, `std::time::Instant` via the runner. It
  is **not** the canonical `Tick` (Ch.2 §2.5): UI animation is presentation, and no
  simulation reads it.
- **Scheduling.** An active animation requests frames until it settles, then stops.
  Infinite animations (spinners) follow the visibility rule in §21.3.
- **Reduced motion.** The OS preference is honoured where the platform exposes it, and
  the Settings toggle always is. Reduced motion makes durations 0 and replaces motion with
  a cross-fade no longer than 80 ms.
- **Durations and easings** are theme tokens (§21.8).

## 21.16 The widget catalogue

Every widget meets four requirements:

- full keyboard operation;
- an AccessKit role and name;
- theming through tokens only;
- input and a11y tests.

All widgets appear in the `ui_gallery` example (WP-U1 starts it, WP-U2 completes it).

| Group | Widgets |
|---|---|
| Text & display | label, rich text (spans, links, inline icons), icon, image, separator, badge, keyboard-shortcut hint |
| Buttons & toggles | button (primary / secondary / ghost / danger; sizes), icon button, toggle button, checkbox (tri-state), radio group, switch, segmented control |
| Numeric | slider (linear/log), **numeric drag field with units** (Ch.6 unit annotations: shows `N·s`, converts on entry, refuses incompatible units with a reason), spin box, range slider |
| Text entry | text field, password field, **multiline editor** (selection, field-local undo, IME, line numbers option), search field (fuzzy, debounced), path field |
| Choice | combo / dropdown with filter, multi-select chips, autocomplete |
| Collections | **virtualised list, tree and table** (§21.12): multi-select, drag-drop reorder/reparent, rename in place, column sort/resize, type-ahead |
| Containers | scroll area (kinetic where the platform does it), splitter, tabs (closable, reorderable), collapsible section, card, group box, grid |
| Menus & overlays | menu bar, context menu, submenu, tooltip, popover, **modal dialog** (focus-trapped; confirm dialogs name the loss for lossy actions), toast / notification |
| Feedback | progress bar (determinate/indeterminate), spinner (visibility-gated, §21.3), skeleton placeholder, empty state |
| Editors | colour picker (HDR-aware: linear/sRGB, exposure, alpha), **vector / quaternion editors** (Euler view with gimbal warning), **`FramePos` editor** (frame selector + `f64` components, never `f32`, I1), curve editor, gradient editor, asset reference picker (with drag-drop), **property grid** (reflection-driven rows, §21.21 inspector) |
| Navigation | breadcrumb, status bar, **command palette** (§21.17), toolbar, dock tab strip |
| Graph | node canvas (pan/zoom, pins, wires, minimap), used by the graph editor (WP-U8) |

**Implementation (WP-U2, ADR 0013).** Every group is built in `crates/forge-ui/src/widgets/`
and shown on the `ui_gallery` catalogue tabs; the node canvas came with the graph editor
(WP-U8, ADR 0034: `widgets/node_canvas.rs`, the gallery's Graph page) and the dock tab strip
with docking (WP-U3). Composite editors (colour picker,
vector / quaternion / `FramePos` editors, property grid) are trees of catalogue widgets
built through `widgets::Build`, so each part is its own tab stop and AccessKit node.
`test_ui_widgets_catalogue` checks the gallery shows the whole catalogue, that every page is
named and Tab-reachable and idles at zero frames; `test_ui_widgets_*` hold each widget's
input and accessibility tests.

## 21.17 Docking, layouts as data, keybindings and the command palette

**The docking model.**

```rust
pub enum DockNode {
    Split { axis: Axis, ratios: Vec<f32>, children: Vec<DockNode> },
    Tabs  { panels: Vec<PanelId>, active: usize },
}
pub struct Layout {
    version: u32,
    main: DockNode,
    floating: Vec<FloatingWindow>,   // { rect, monitor hint, root: DockNode }
    maximised: Option<PanelId>,
}
```

- `PanelId` is the panel's stable extension-point id string, for example
  `"forge.hierarchy"` (§21.21).
- **Interactions:**
  - drag a tab to dock it, with drop previews for left/right/top/bottom/centre and window
    edges;
  - tear a tab off into a floating OS window, and put it back;
  - maximise the focused panel (Shift+Space);
  - close and reopen panels (the Window menu, or the palette);
  - drag panels across monitors.
- **Layouts are RON data.**
  - Each preset ships a default at `presets/<preset>/layout.ron`, named from its
    `workspace.ron` (Ch.31 §31.3).
  - A user's layouts are saved in the per-user config directory.
  - **The layout is not project state.** "Save layout as project default" is the one
    exception, and it writes project settings **through a command** (§21.18).
- **An unknown panel id is kept**, for example when its plugin is disabled. It shows as a
  placeholder naming the missing plugin, so disabling a plugin never destroys a layout
  (`test_layout_unknown_panel_kept`).
- **Autosave is crash-safe.** Layouts are written by write-then-rename, debounced 2 s
  after a change, and again on exit. Round trips are tested (`test_layout_roundtrip`).

**Keybindings.**

- A `KeyMap` maps chords (including two-stroke chords such as `Ctrl+K Ctrl+S`) to
  `ActionId`s per **context**: global → window → panel type → focused widget role.
- Resolution walks the focus path from innermost outwards.
- **Conflict detection:** two bindings of the same chord in overlapping contexts are
  reported with both action names when a binding is made. They are never silently
  last-wins (`test_keybinding_conflicts`).
- Defaults are data, overridable per preset and per user.
- The keymap is **user config**, not project state.

**The action registry and the command palette.**

- Every user-invokable thing is an `Action` with an `ActionId`, title, category, default
  chords, an enabled predicate, and a kind:
  - **`Command`**: it emits an `EditorCommand` (§21.18);
  - **`Session`**: it changes session state or user config (open a panel, change theme,
    move the camera).
- The palette (Ctrl+Shift+P) fuzzy-searches actions, panels ("Open: Profiler"), settings,
  and every **`EditorCommand` variant constructible with no arguments or with the current
  selection**. Every registered command is therefore reachable by keyboard (I7's surface,
  seen from the UI).
- Results show chords, and the last-used actions rank first.
- Actions come from the same registry as menus and toolbar buttons, so the palette cannot
  drift from the menus.

## 21.18 The editor shell: a client of `forge-cmd` (I7)

**The structural rule.** Panel code receives exactly two handles into the project:

```rust
pub struct PanelCx<'a> {
    pub mirror: &'a ProjectMirror,        // read-only view of project state
    pub cmd:    CommandEmitter<'a>,       // the only way to change it
    pub session: &'a mut SessionState,    // selection, camera, scroll — NOT project state
    pub ui: &'a mut forge_ui::BuildCx,
}

impl CommandEmitter<'_> {
    /// One command, one transaction, one undo entry. Issuer is set by the shell.
    pub fn emit(&mut self, cmd: EditorCommand) -> Ticket;
    /// A continuous gesture (slider drag, gizmo drag): one txn,
    /// at most one command per frame, one undo entry; Esc cancels (undoes the txn).
    pub fn gesture(&mut self, label: &str) -> Gesture<'_>;
    /// dry_run — validation, hover previews, "what would this delete?" dialogs.
    pub fn preview(&self, cmd: &EditorCommand) -> Result<Diff, Rejection>;
}
```

- **There is no `&mut World` or `&mut dyn ProjectStore` anywhere a panel can reach.**
  This is enforced by types first. `test_command_liveness` then enforces it through the
  import graph (§21.23).
- **Panels cannot forge an issuer.** The shell stamps `Issuer::Human { user }` on every
  envelope it sends.

**Three classes of state, and only one is on the bus.**

| Class | Examples | Where it lives | On the bus? |
|---|---|---|---|
| **Session state** | selection, editor camera, expanded tree nodes, scroll offsets, which panel is focused | the editor process | no. Presence (Ch.37) publishes selection read-only to teammates |
| **User config** | theme, UI scale, keymap, layouts, reduced motion | per-user config dir | no. It is not project state and is never in the repository |

The liveness guard's definition of "mutation" is **project state**. The session and
user-config writers are listed in its allow-list, each with its reason (W3's discipline:
the list is explicit, never implied by naming).

**Security-relevant state is classified explicitly.** Capabilities use one grant model for
plugins, remote sessions and team roles (Ch.32 §32.4, Ch.34 §34.5, Ch.37 §37.6). Each item
that model touches in the UI is classified here, so none of them is a panel-local write:

| Item | Panel | Class | How it changes | Why |
|---|---|---|---|---|
| The project's plugin set: add (`forge add`), remove, enable, disable | Plugin manager | **project state** | a command (`Plugin::Add / Remove / SetEnabled`), undoable, audited | it changes what code the project loads, for every teammate and every build |
| Plugin capability grants and revokes | Plugin manager | **project state** | a command (`Plugin::Grant / Revoke`), audited, issuer must be `Human` | grants ship with the project so every teammate gets the same sandbox (Ch.32 §32.4); a plugin must never grant itself |
| The downloaded plugin cache (fetched bytes, before a project adds the plugin) | Plugin manager | **user config** (allow-listed writer `PluginCache`) | a direct write, recorded in the local log | fetching bytes changes nothing a project loads until a `Plugin::Add` command does; the cache is shared by every project on the machine |
| Team membership and roles | Team | **project state** | a command, audited | Ch.37 §37.6 |
| Scoped ownership claims and releases | Ownership claims | **project state** | a command (`Collab::Claim / Release`), audited; refused for a role without claim rights | a claim makes a subtree read-only for every teammate (Ch.37 §37.4) |

Two rules apply to every security command:

- **Only a human issues it.** Commands in the security class reject any issuer other than
  `Issuer::Human`, with a stable `ErrorCode`.
- **Undo and redo never grant.** Undoing a grant revokes it. Undoing a revoke, or redoing
  an undone grant, is refused, so a capability is only ever granted by a fresh human
  command. If `forge-cmd` (WP-05)
  has no way to mark a command non-redoable or non-undoable, WP-U9 adds one there, as §21.25 prescribes
  for any missing command.
- **Security state is written only by security commands** (added by WP-U9). The settings
  that hold grants and the plugin set (`security.*`, `plugins.*`) are reserved on the bus to
  their commands (`Bus::reserve_settings`): a plain setting edit, any other handler or a
  plugin's command that touches them is refused with `CMD-0014`, whoever sends it. The project
  load is the one other writer; the core performs it and it is never undone or redone.

**State flows back only through the `Applied` stream.**

```
 panel action ──emit──▶ CommandEmitter ──▶ BusClient ──▶ CommandSink::apply (core)
                                                             │
 widgets ◀── signals ◀── ProjectMirror::apply ◀── Applied ◀──┘   (every issuer:
                                                                  script, teammate)
```

- `ProjectMirror` is a read model keyed by `(EntityId, reflect path)`. Its signals are
  created lazily, only for paths some widget watches.
- An `Applied` delta is a reflect-tree diff (Ch.33). It updates exactly the signals it
  touches. There is no "refresh the inspector" pass.
- **Every issuer's changes reach the UI by the same path.** A script's or a teammate's
  change appears in the hierarchy and inspector in the next frame, and in the undo history
  tagged with its issuer. It is true by construction.
- **`ProjectMirror::apply` is private to the shell's stream pump.** A panel cannot call
  it, so a panel cannot fake a state change. It can only request one.

**Gestures, and one undo entry per user intent.**

- A slider drag or gizmo drag opens a transaction. It sends at most one command per frame
  inside that transaction, and commits on release.
- **Undo reverts the whole transaction** (`CommandSink::undo(txn)`). One drag is one undo
  step, and the viewport showed real state during the drag (`test_gesture_single_undo`).
- Esc during a gesture undoes the transaction, which is a cancel.

**Optimistic preview in the split editor (Ch.34, O-13).**

- `BusClient` is a trait. `LocalBus` runs in-process; `RemoteBus` sends commands and
  state deltas over QUIC (M2-16).
- With a remote core, the mirror applies the `dry_run` diff as a **pending overlay**
  tagged with the ticket. When the authoritative `Applied` arrives, the overlay is dropped
  and the real delta applied. A divergence is corrected in the next frame, the same way
  network prediction corrects (`test_optimistic_reconciliation`, O-13).
- **Panels cannot tell local from remote.** That is what makes the split editor a
  transport swap rather than a rewrite (Ch.34 §34.2).

**Undo and redo UI.**

- Ctrl+Z / Ctrl+Y (Ctrl+Shift+Z on Linux keymaps too) issue undo and redo through the
  bus, with whatever redo primitive `forge-cmd` provides (WP-05).
- The **Undo history** panel lists transactions with their issuer (human / script /
  teammate), a label and a time.
- Clicking an entry undoes back to it, as one bus call per transaction.

**Errors.**

- A `Rejection` from the bus becomes a toast naming the command and its stable
  `ErrorCode` (Ch.1.2), with "Details" opening the console at the entry.
- Nothing is silently dropped.
- A panicking command is already caught at the bus boundary (Ch.1.2), so a bad command
  cannot take the editor down.

**Implementation (WP-U4, ADR 0018)** — how this section is built:

| Piece | Where | What it is |
|---|---|---|
| Headless core | `crates/forge-editor/src/core.rs` | `EditorCore` owns the `Bus` behind `Arc<Mutex<_>>`; `LocalBus` is the in-process `BusClient` (one per client, fixed `Issuer`, own subscription); a change by one client wakes the others; the only module naming `Bus`/`CommandSink`, and it names no `forge_ui` |
| `BusClient` | `client.rs` | requests return a `Ticket`; outcomes arrive only through `pump()`: `Applied` events of every issuer, a `Gap`, this client's `Refused` requests; `snapshot()`/`history()` resync; `preview()` is the dry run |
| `ProjectMirror` | `mirror.rs` | entities, hierarchy, properties, settings and the undo history (issuer, label, time, state); revisions (whole / structure / settings / history, per-path only for watched paths); `apply` and `resync` crate-private; `plan_travel` for the history panel |
| `CommandEmitter`, `Gesture` | `emitter.rs` | `emit`, `emit_all` (one txn), `gesture` (≤ 1 command per loop turn, the rest held to the next turn or commit; drop = cancel), `preview`, `undo`/`redo`/`travel` |
| Panel runtime | `panel_rt.rs`, `panels.rs` | `PanelCx` carries `ShellHandles` (mirror read-only, emitter, session); `add_live` steps get a `PanelBuilder`: handlers for widget actions (routed per window from the source widget up), sync steps run only when the mirror or session revision moved, named session ops |
| Session and user config | `session.rs`, `settings.rs`, `keybind.rs`, `notify.rs` | selection, notifications (bounded, codes, actions), editor settings (reflected `EditorSettings`), keymap with reset/user layer; settings pages generated from `#[forge_api]` inspector metadata |
| Shell | `shell.rs` | main window (keymap host ⊃ menu bar from the action registry, toolbar, dock, status bar), palette host, toasts, per-window stream pump + sync + settings application, debounced crash-safe saves of layout / editor settings / keymap through `UiApp::next_deadline`, flush on `exiting`; `assemble` loads `forge.editor` actions and the plugins through the ordinary loader |
| Headless | `headless.rs`, `tools/forge-editor-bin` | `forge-editor --headless [--script FILE]`: JSON `EditorCommand` lines, `undo`, `redo`, as `Issuer::Script`; prints the state hash |
| Core panels | `plugins/forge-panels-core` | undo history, notifications, settings, keybindings (chord capture widget), command palette panel — a source plugin; the other §21.21 panels stay the labelled stand-ins |
| Test loop | `crates/forge-editor/src/testing.rs` | `Rig`: the runner's loop over injected time, counting frames and wakeups, waking for UI deadlines, shell deadlines and wakers |

Measured (dev box, RTX 3080, release build, Vulkan): first frame 0.70–0.72 s after launch;
idle window 20 s: 0 frames, 0 wakeups; process CPU over 10 s of idle: 15.6 ms (one
Windows scheduler tick, ≈0.16 % of one core). Headless: `ui_editor_idle_zero_redraw` is 0/0 over 10 s
with every core panel open and a focused field (spinner control sees frames); a 60-frame
drag is 62 commands in 62 loop turns, one undo entry.

Guards: `test_command_liveness` (I7) checks `forge-editor`, the binary and every
`plugins/forge-panels-*` crate, with the split rule (bus only in `core`, no UI in `core`;
`C-command-liveness-split`) and the store mutant in `forge-panels-core`
(`C-command-liveness-panels`); `test_gesture_single_undo` (`C-gesture-single-undo`);
`test_undo_every_editor_action` (M2-3, `C-undo-every-editor-action`);
`test_shell_idle` (`C-editor-idle`). `test_optimistic_reconciliation` (M2-16, WP-17, ADR 0037;
`crates/forge-remote/tests`, gate `C-optimistic-reconciliation`): the shell runs as a remote
client over QUIC with a 300 ms simulated link; a concurrent spawn from another client, a refused despawn and a
20-frame drag are each shown the turn they are emitted (the overlay) and equal the host's project
one loop turn after the answer arrives; control: `MirrorFaults::keep_overlays` fails it.
**Optimistic overlay, as built:** `Pumped` gains `predicted` (a remote client's dry runs of
the requests it sent since the last pump) and `settled` (tickets whose outcome arrived); the
mirror lays each prediction on as an overlay the pump it is made, and in the pump an answer
arrives in it lifts every overlay (inverses, newest first), applies the core's events, drops the
settled ones and re-lays the rest, each only if its preconditions still hold on the
authoritative state. A local client predicts nothing (its answers arrive in the same turn).

## 21.19 Extension points: panels, inspector widgets, themes, tools (I16), and presets (I15)

`forge-editor` registers these with `forge-plugin` (Ch.32 §32.2). Each supports `add`,
`replace`, `remove` and `chain`:

| Extension point | `ID` | Item | Used by |
|---|---|---|---|
| `EditorPanel` | `forge.editor.panel` | `PanelDescriptor { id, title, icon, default_dock, build: fn(&mut PanelCx) }` | every panel in §21.21 |
| `EditorOverlay` | `forge.editor.overlay` | palette, toasts, presence badges, drag previews | §21.17, Ch.37 |
| `InspectorWidget` | `forge.editor.inspector_widget` | a custom editor for a reflected type or `#[forge(...)]` attribute | the inspector (§21.21) |
| `ViewportTool` | `forge.editor.viewport_tool` | gizmos, brushes, measurement, selection modes | WP-U6 |
| `Theme` | `forge.ui.theme` | a token set (§21.8); the point is defined in `forge-editor` (ADR 0029) | §21.8 |
| `Action` | `forge.editor.action` | an entry in the action registry (§21.17) | menus, toolbar, palette |

**I16: first-party panels have no privileged path.** Every first-party panel lives in a
`plugins/forge-panels-*` **source plugin**. It registers through the same `EditorPanel`
registry a third-party plugin uses and reaches only `forge-editor`'s public API.
`test_no_privileged_plugin` covers these crates, and `test_panel_extension_replaceable`
(inside `test_extension_point_replaceable`) replaces the hierarchy panel from a test
plugin, chains the inspector, and removes the console. This also discharges part of M2-14:
the panel sets are first-party subsystems shipped as plugins.

**WASM plugins declare their panels.** A WASM component cannot hold widgets across the
sandbox boundary. It returns a **`ViewSpec`**, a serialisable tree of catalogue widgets
bound to reflect paths and actions. The host builds it with the same widgets, and the
plugin's actions become commands under the plugin's capability grant (Ch.32 §32.4).
Custom-painted widgets are source-plugin only. That matches Ch.32's table (source plugins
reach every extension point), so no first-party panel uses anything a source plugin
cannot.

**I15: presets choose defaults and never gate panels.** A preset's `workspace.ron` names a
default layout and which panels are open. It never removes a panel from the registry.
`test_every_panel_under_every_preset` opens every registered panel under 2D and 3D. The positive control is a panel descriptor gated on a preset, and the test must
fail. A panel whose subject a project lacks shows an explanatory empty state and one-click
enablement, never an absence.

**Implementation (WP-U3, ADR 0016)** — how §21.17 and §21.19 are built:

- **Where.** Docking is `forge_ui::dock` (model, geometry, the `DockArea` widget, and the
  multi-window `DockController`); panels come from the application through `PanelHost`.
  `crates/forge-editor` holds the rest: `keymap` + `keys` (the window key sink), `actions`
  (the `Action` point), `palette`, `panels` (`PanelCx`, `EditorPanelHost`, `open_panel`,
  `check_panels_under_presets`), `presets` (the three built-ins compiled in from
  `presets/`), `layout_store` (named user layouts, crash-safe debounced autosave) and
  `user_config` (the per-user directory; the only direct file writer, I7-allow-listed).
- **Flat keyed frames.** A panel's frame is a direct child of the dock area keyed
  `panel:<id>`, positioned by percent insets and pixel margins from the tree's geometry;
  re-docking restyles and shows/hides, and never rebuilds a panel. Drop zones: a tab strip
  (insert at the pointer), a group body's outer quarter (split that side) or centre
  (tab), and the area's 24 px edge band (dock along the whole side); a drop onto the
  panel's own place is refused, so it is never taken for a tear-off. Releasing over no dock
  area tears the panel off at that desktop point; the controller docks it instead if the
  point is over another window's dock.
- **Keymap.** Contexts overlap as: identical, global/window with anything, a panel with a
  widget role. An inner binding overrides an outer one only with `shadows: "<action>"`;
  a two-stroke chord conflicts with its first stroke. Data: `crates/forge-editor/data/
  keymap.ron`, then `presets/<p>/keymap.ron`, then the user's file.
- **Palette.** Entries: every action (with its first chord), "Open: <panel>" for every
  registered panel, every `EditorCommand` variant (`palette::VARIANTS`: built with no
  arguments, from the selection, or through an argument prompt — reflection-checked), and
  every `Invoke` handler. Fuzzy score plus recency (last 32); unavailable entries last.
- **Guards** (gate rows): `C-dock-drag-to-dock`, `C-dock-redock-keeps-panels`,
  `C-layout-roundtrip`, `C-layout-unknown-panel-kept`, `C-keybinding-conflicts`,
  `C-panel-extension-replaceable` (the editor-level replace/chain/remove/add test; the
  registry-level `test_panel_extension_replaceable` stays in `tests/plugin/`),
  `C-every-panel-under-every-preset`, `C-palette-every-command`. Every catalogue panel is
  built by a `plugins/forge-panels-*` crate; the labelled stand-in plugin
  `forge_editor::stand_in` serves none and is not loaded (`C-first-party-panels`,
  `test_first_party_panels_are_real`, WP-U12).

## 21.20 Game UI: the same layer in `forge-runtime`

`forge-runtime` links `forge-ui` with features `wgpu` and `winit`. It links **no** editor
crate: not `forge-editor`, not any `forge-panels-*`, not `forge-licence`
(`test_game_ui_links_no_editor` checks the resolved graph; I21's guard already excludes
`forge-licence`).

**What games get:**

- the whole catalogue, themes as brand styling, and localisation (Ch.28);
- **gamepad navigation** through spatial focus (§21.9), with focus-visible styling and
  per-platform button glyph mapping behind the HAL (Ch.27);
- **resolution-independent scaling**: a reference resolution plus a scale mode (fit /
  fill / integer for pixel-art, Ch.35);
- safe-area insets;
- the same zero-idle behaviour for menus. A pause menu over a paused game costs nothing.

**How game UI wires to the game.** Game UI binds to game state through the same `Signal` /
`Memo` model. Actions go to game systems as ECS events. There is no `forge-cmd` in a
shipped game's UI path, because game UI changes *game* state, not project state.

**The credits entry (E-64).** `forge-ui` provides `CreditsScreen` with a pre-populated
"Made with Forge Engine" entry. It is static text and executes nothing, which keeps
attribution inert (`test_attribution_is_inert`). The packager ensures the entry exists
(Ch.38 §38.7).

**Deferred to M7:** world-space UI (rendered to a texture on a mesh), and HUD templates.
The toolkit itself exists from M2 (DoD M2-29), which is what D-1 changed about M7.

**Implementation (WP-U11, ADR 0040)** — game UI on `forge-ui` in `forge-runtime`:

| Piece | Where | What it is |
|---|---|---|
| Game layer | `crates/forge-ui/src/game.rs` | `GameScaling` (reference resolution; `Fit`, `Fill`, `Integer`) and `placement`; `SafeArea` (uniform, TV title-safe, as root padding); `GamepadNav` (d-pad and left stick → arrow keys with hysteresis and deadline-driven hold-to-repeat — `next_deadline` is `None` when nothing is held; South → Enter, East → Escape while a popup is open else `PadAction::Back`, Start → `Menu`, shoulders → tabs); the `GamepadSource` seam with the labelled in-memory `ScriptedPad` (the HAL's pad backends are `UNBUILT`, row `C-gamepad-hal-backend`); `ButtonGlyphs` per `GlyphSet` (Xbox, PlayStation, Nintendo, generic); `Credits` whose first section is always `ENGINE_CREDIT` ("Made with Forge Engine"), built as static labels — nothing in it executes |
| Navigation | `crates/forge-ui/src/ui.rs`, `focus.rs` | `Ui::game_nav`: Up/Down always move focus between rows (a slider or segmented control keeps Left/Right), inert while a popup is open; spatial navigation prefers candidates in the beam (overlapping on the cross axis), nearest along the axis first |
| Localisation | `crates/forge-ui/src/l10n.rs` | `{name}` templates, `format`, placeholder mismatch; `pseudo_localise` (Latin-1/Ext-A look-alikes every bundled font draws, ~40 % longer, bracketed, placeholders intact) and `is_pseudo`; the runtime `Localiser` (current → source → `⟦key⟧`, the pseudo-locale `qps-ploc` generated from the source); `LocalisedTexts` (labels bound to keys; a language switch re-sets only the signals whose text changed); RFC 4180 CSV |
| Sample | `crates/forge-runtime/src/game_ui.rs` | `GameMenu`: main menu, settings (volume sliders, fullscreen and subtitles switches, language), credits, HUD (health, score, pad prompt with the pad's glyphs), pause menu over the HUD; English, French and the pseudo-locale; a language switch rebuilds the controls whose accessible names are build-time text; `GameEvent`s go to game systems as `bevy_ecs` messages (`Messages<GameEvent>`) — no `forge-cmd` on this path |
| Binary | `crates/forge-runtime/src/main.rs` | `forge-runtime --menu [--locale] [--glyphs] [--exit-after]` runs the sample on the winit runner; the UI scale follows the window through `UiApp::user_scale` |

Measured (dev box): `forge-runtime --menu --exit-after 3`: 1 frame, then 0 idle frames and 0
wakeups; first frame 1.26 s including adapter start-up (RTX 3080, Vulkan). Headless
(`crates/forge-runtime/tests/test_game_menu.rs`): a 10 s idle menu draws 0 frames with 0 wakeups;
an unchanged HUD value draws nothing.

Guards (gate rows): `C-game-ui-no-editor` (`tests/liveness/test_game_ui_links_no_editor.rs`:
the cargo-resolved runtime closure contains `forge-ui` with `wgpu` and `winit` and no
`forge-editor`, `forge-panels-*` (by name and by location) or `forge-licence`; controls:
synthetic graphs and a real-cargo scratch runtime depending on the real `forge-editor`),
`C-game-ui-sample` (the pad walks every screen, the pseudo-locale transforms every visible
menu string; control: a hard-coded label is caught). I21's registry scan now matches socket
types as code tokens, not text (ADR 0040).

## 21.21 The editor panel inventory

Every panel the plan implies, drawn from every chapter:

- **Id** is the `EditorPanel` extension-point id.
- **WP** is the work package that builds it (§21.24). §21.24 mirrors the orchestrator
  backlog (`backlog.json`), so a panel is given to a WP only when that WP's backlog scope
  names it. A panel that no backlog scope names is `UNSCHEDULED` and is listed in the
  §21.24 gap table with the backlog change it needs. It is never silently given to a WP
  whose implementer would not know to build it (W9).
- **Backlog clause** quotes, verbatim, the words in that WP's backlog scope that cover the
  panel. The clause must **name the panel**: of all the panels in this table, this panel's
  name must match the clause best (most shared name words, ties broken by the earliest
  match). `test_ui_panel_inventory` checks the naming rule on every run and checks each
  quote against `backlog.json` when it is available (§21.23).
- **DoD** is its row in `milestones.md` (M2, appended by WP-U0).
- **Backend** names the trait the panel talks to. Where the real backend is not built yet,
  the panel runs against a labelled in-memory implementation (D-4). The real one shows as
  `UNBUILT(reason)` in `just gate`, never as green.
- **Crate** is the plugin that registers the panel.

`test_ui_panel_inventory` (bound) parses this table and asserts:

- every chapter named exists in the document map;
- every WP exists in §21.24 and claims the panel's DoD id there, or the WP is
  `UNSCHEDULED` and the §21.24 gap table claims the DoD id, never both;
- every scheduled panel's backlog clause names that panel (the naming rule above);
- when the backlog is available (`FORGE_BACKLOG` names `backlog.json`), every WP named in
  §21.24 exists in the backlog and every backlog clause appears in that WP's backlog title
  or scope. `backlog.json` is the orchestrator's file and is not in the repository, so
  neither `just verify` nor CI sets `FORGE_BACKLOG`. That half of the check is therefore
  its own gate row, `C-ui-panel-backlog`, which `just gate` shows as
  `AWAITING(reason)`. It never shows as a pass. `just backlog-check <path>` runs it;
  the orchestrator runs it before it accepts a change to §21.21 or §21.24;
- every DoD id exists in `milestones.md` and has a status row;
- every panel the WP-U0 scope names is present;
- every UI DoD id (M2-18 onward) is cited by this chapter.

| Panel | Id | Chapters | WP | DoD | Crate | Backend (D-4) | Backlog clause |
|---|---|---|---|---|---|---|---|
| Viewport | `forge.viewport` | Ch.2, Ch.10, Ch.21 | WP-U6 | M2-32 | `forge-panels-scene` | placeholder grid/axes/bounds renderer until `forge-render` (M1) | "Viewport panel hosting a render surface" |
| Play controls | `forge.play_controls` | Ch.7, Ch.37 | WP-U6 | M2-33 | `forge-panels-scene` | headless play via the bus (M2-5) | "play/pause/step/stop toolbar for PIE" |
| Scene hierarchy | `forge.hierarchy` | Ch.5, Ch.7, Ch.21 | WP-U5 | M2-35 | `forge-panels-scene` | `ProjectMirror` | "Scene hierarchy panel" |
| Inspector | `forge.inspector` | Ch.6, Ch.4 | WP-U5 | M2-37 | `forge-panels-scene` | reflect registry (`forge-reflect`) + mirror | "Inspector generated from reflection metadata" |
| Asset browser | `forge.assets` | Ch.8, Ch.33 | WP-U5 | M2-38 | `forge-panels-assets` | `AssetCatalog` trait over `forge-asset`'s `AssetServer` (`ServerCatalog`); an in-memory store until the launcher opens project folders (WP-U7) | "Asset browser" |
| Console | `forge.console` | Ch.29, Ch.4, Ch.1 | WP-U5 | M2-39 | `forge-panels-core` | `ConsoleLog` (the editor log, fed from any thread by `LogSender`); `forge-trace` (M2-11) feeds it when built | "Console/log panel" |
| Profiler | `forge.profiler` | Ch.29, Ch.26, Ch.25 | WP-U6 | M2-40 | `forge-panels-core` | `CounterSource` trait over `forge-trace` (M2-11) | "Profiler panel reading forge-trace style counters" |
| Undo history | `forge.undo_history` | Ch.7 | WP-U4 | M2-41 | `forge-panels-core` | the bus's transaction log | "undo/redo UI with history panel" |
| Notifications | `forge.notifications` | Ch.21, Ch.1 | WP-U4 | M2-42 | `forge-panels-core` | shell notification queue | "notification centre" |
| Settings | `forge.settings` | Ch.6, Ch.31 | WP-U4 | M2-43 | `forge-panels-core` | reflect registry; project settings via commands | "settings window (editor + project settings generated from reflection)" |
| Keybindings | `forge.keybindings` | Ch.21 | WP-U4 | M2-44 | `forge-panels-core` | user-config keymap | "keybindings editor" |
| Command palette | `forge.command_palette` | Ch.7, Ch.21 | WP-U3 | M2-45 | `forge-panels-core` | action registry | "command palette over every registered command with fuzzy search" |
| Graph editor | `forge.graph` | Ch.24, Ch.10, Ch.14 | WP-U8 | M2-46 | `forge-panels-assets` | `GraphIr` trait; `forge-graph` since WP-23 (the S3 wasm/native build is M5-4) | "A general node-graph widget + editor panel" |
| Project launcher & new project | `forge.launcher` | Ch.33, Ch.31 | WP-U7 | M2-47 | `forge-panels-project` | requested through the bus client; the core calls `ProjectStore` (`LocalFs`, M0-15) | "Project launcher (recent projects, open, create)" |
| Preset & promotion | `forge.presets` | Ch.31 | WP-U7 | M2-48 | `forge-panels-project` | preset manifests (`presets/`) + commands | "preset switcher and promotion dialog" |
| Revision history & store | `forge.history` | Ch.33 | WP-U7 | M2-50 | `forge-panels-project` | requested through the bus client; the core calls `ProjectStore::history` (the `Git` backend: Ch.33 §33.7, WP-16) | "revision history (semantic, commit/push/pull as commands)" |
| Build & export | `forge.export` | Ch.27, Ch.38 | WP-U7 | M2-51 | `forge-panels-project` | `Packager` trait; in-memory until the HAL exporters (M8) | "build & export (targets, E-64 attribution, NOTICES preview)" |
| Plugin manager | `forge.plugins` | Ch.32, Ch.38 | WP-U9 | M2-52 | `forge-panels-connect` | `forge-plugin` registry; `PluginIndex` trait in-memory until the index (M5-14) | "Plugin manager (installed plugins, capabilities requested vs granted" |
| Audit log | `forge.audit_log` | Ch.34, Ch.37 | WP-U9 | M2-54 | `forge-panels-connect` | `AuditSource` trait over the core's audit book; teammates in-memory until `forge-collab`/`forge-server` | "audit log viewer greppable by session" |
| Remote connect | `forge.remote` | Ch.34 | WP-U9 | M2-55 | `forge-panels-connect` | `RemoteTransport` trait; `forge_remote::QuicTransport` (M2-16, WP-17) in the editor, the in-memory loopback for scripted tests | "split-editor connect/pair dialog" |
| Compute & farm | `forge.compute` | Ch.26 | WP-U9 | M2-56 | `forge-panels-connect` | `ComputePool` trait; in-memory until `forge-jobs` / `forge-farm` (M6-1, M6-3) | "compute & farm panel (adapters, throughput, farm nodes) over the ComputePool trait" |
| Team & members | `forge.team` | Ch.37 | WP-U10 | M2-57 | `forge-panels-collab` | `IdentityBackend` trait; in-memory until `forge-identity` (M5-15) | "Create Team -> Add Member in three clicks" |
| Sandbox & Live/Pull | `forge.sandbox` | Ch.37, Ch.33 | WP-U10 | M2-58 | `forge-panels-collab` | `CollabBackend` trait; in-memory until `forge-collab` (M5-18, M6-10) | "per-user Live/Pull toggle (one click)" |
| Presence | `forge.presence` | Ch.37 | WP-U10 | M2-59 | `forge-panels-collab` | `CollabBackend` presence stream (in-memory) | "presence indicators in hierarchy/inspector/viewport" |
| Ownership claims | `forge.ownership` | Ch.37, Ch.33 | WP-U10 | M2-69 | `forge-panels-collab` | `CollabBackend` claims (in-memory until `forge-collab`, M5-18) | "scoped ownership claims" |
| Publish & review queue | `forge.publish_queue` | Ch.37, Ch.33 | WP-U10 | M2-60 | `forge-panels-collab` | `CollabBackend` (in-memory) | "publish/review queue" |
| Conflict resolution | `forge.conflicts` | Ch.37, Ch.33 | WP-U10 | M2-61 | `forge-panels-collab` | `CollabBackend` + semantic three-way diff (in-memory until M6-9) | "conflict resolution panel" |
| Licence status | `forge.licence` | Ch.38 | WP-U10 | M2-62 | `forge-panels-collab` | `EntitlementSource` trait; in-memory until `forge-licence` (M5-25) | "licence/entitlement status" |
| Sequencer & timeline | `forge.sequencer` | Ch.19, Ch.6 | WP-U11 | M2-63 | `forge-panels-authoring` | `AnimSource` trait; in-memory until `forge-anim` | "Timeline that keys any reflected property" |
| Animation state machine | `forge.anim_graph` | Ch.19 | WP-U11 | M2-64 | `forge-panels-authoring` | `AnimSource` (in-memory) | "animation state machine + blend space editor UI" |
| Localisation | `forge.localisation` | Ch.28 | WP-U11 | M2-65 | `forge-panels-authoring` | `StringTables` trait; in-memory until `forge-play` (M5-8) | "localisation string table editor" |
| 2D editors (tile palette, sprite sheet, 2D rig) | `forge.editors_2d` | Ch.35 | WP-U13 | M2-66 | `forge-panels-domain` | `Scene2d` trait; `forge-2d` (`Forge2dScene`, M4-11, WP-U15) | "2D tile palette / sprite sheet / 2D rig (Ch.35)" |
| Audio mixer | `forge.audio_mixer` | Ch.20 | WP-U13 | M2-67 | `forge-panels-domain` | `AudioBuses` trait; in-memory until `forge-audio` | "audio mixer (Ch.20)" |
| Input action map | `forge.input_map` | Ch.28, Ch.27 | WP-U13 | M2-68 | `forge-panels-domain` | `InputActions` trait; `forge-input` (`DeviceInput`, WP-65, §28.18) | "input action map (Ch.28)" |
| Input debugger | `forge.input_debugger` | Ch.28 | WP-65 | M7-12 | `forge-panels-domain` | `InputActions::debug_snapshot`; the `forge-input` runtime (§28.18) | "input debugger panel in the editor" |

**What each panel must do.** Its DoD row in `milestones.md` is the acceptance test. These
are the requirements that are not obvious:

- **Viewport.**
  - An `f64`, camera-relative camera (Ch.2 §2.4) with log-scaled fly speed.
  - Orbit, fly and pan navigation.
  - Transform gizmos in local, world or frame space, with snapping. **Gizmo drags are
    gestures (§21.18).**
  - Picking.
  - Overlays: stats, current frame, `SeedPath` under the cursor, and debug overlays for
    navmesh (Ch.18), physics (Ch.17) and regions (Ch.5).
  - Multiple viewports.
  - The viewport redraws only when its scene or camera changes, or while playing (§21.13).
- **Play controls.**
  - Play, pause, step and stop, issued as commands.
  - Play-in-editor runs against the user's **own sandbox view** (Ch.37 §37.5).
- **Hierarchy.**
  - A virtualised tree: 100k entities within the D-5 budget.
  - Drag to reparent, multi-select, rename in place, search/filter, visibility and lock
    toggles.
  - **Every edit is a command.**
  - Presence badges from Ch.37.
- **Inspector.**
  - Generated from reflection metadata (Ch.6), with units and ranges from `#[forge(...)]`.
  - Multi-object editing with mixed-value display.
  - Add and remove components.
  - `InspectorWidget` extension point (§21.19).
  - Generated content shows its `SeedPath` and "touch to promote" (Ch.14).
- **Asset browser.**
  - Grid and list views, async thumbnails, folders, search, drag-drop import.
  - Rename and move through commands.
  - Lock indicators (Ch.33 §33.3).
- **Console.**
  - Level filters, search, grouping of repeated lines.
  - Click-through to source: to a `SeedPath` (Ch.4 §4.3), to the offending node for
    shader errors, and to the command for rejections.
- **Profiler.**
  - A frame timeline, named budgets that turn red when over (Ch.29), adapter pool lanes
    (Ch.26), and replication statistics (Ch.25).
  - It is a live-data panel (§21.11): it refreshes at ≤ 10 Hz, only while visible and
    only when a source's generation changed. The editor's own UI counters (`ui.self`) are
    shown but never schedule a frame, so the profiler cannot keep the editor awake by
    measuring it.
- **Undo history.**
  - Transactions with their issuer (human / script / teammate).
  - Click to undo back to an entry.
- **Notifications.**
  - Toasts with stable `ErrorCode`s, a history drawer, and actions ("Open console",
    "Retry").
- **Settings.**
  - Editor settings are user config. Project settings are reflection-generated and edited
    through commands.
  - UI scale, theme, reduced motion, caret blink.
- **Keybindings.**
  - A searchable list, rebind by pressing the chord, and conflicts shown with both action
    names.
  - Reset per binding or all.
- **Command palette.**
  - Every action and every no-argument `EditorCommand` (§21.17).
  - Fully usable by keyboard alone.
- **Graph editor.**
  - One canvas for the blueprint, material, generator and PCG graphs (E-11).
  - Pins typed with units. An incompatible connection is **refused with a reason**
    (Ch.6).
  - Node search from the `#[forge_api]` registry, comments/groups, reroutes, minimap,
    copy/paste.
  - A compile button with errors mapped back to nodes, and a graph↔text side-by-side view
    (Ch.24).
  - Every edit is a command.
  - Budget: 2,000 nodes pan and zoom within `ui_graph_2k_nodes`.
- **Launcher and new project.**
  - Recent projects.
  - A **three-click** new-project flow (Ch.33 §33.5) with the templates 2D and 3D.
  - The "link a remote" offer comes **on first save, not at creation** (O-11).
  - Creating and opening a project are requests to the headless core through the bus
    client. The launcher performs no `ProjectStore` write itself (I7, I17).
  - **Project settings (WP-U7's "project settings").** A **Project** page in WP-U4's
    settings window: project name, linked remote and store backend, and the active
    preset. The preset row is read-only there and opens the preset switcher, so each lossy
    change keeps its one confirmation path. Every edit is a command. The page is a reflected
    `ProjectLifecycleSettings` type that the settings window renders like any other
    project settings; WP-U7 owns the type and the page's DoD (M2-47), and WP-U4 owns the
    window (M2-43).
- **Preset and promotion.**
  - Every arrow in Ch.31 §31.4. Lossy arrows **state what is lost and require
    confirmation in the UI** (`test_lossy_warnings_present`).
- **Revision history.**
  - Semantic history read from command envelopes ("ada placed 37 trees", Ch.33
    §33.4).
  - Commit, push and pull are requested as commands. The core performs the store
    operations (E-36); the panel never calls `ProjectStore`.
  - Link-a-remote.
- **Build and export.**
  - Targets (Windows, Linux; Web and Android best-effort, Ch.27).
  - Shows the attribution the packager will insert (E-64), plus a `NOTICES` preview.
- **Plugin manager.**
  - Installed plugins, with capabilities requested vs granted.
  - Enable and disable. Adding, removing, enabling and disabling a plugin, and granting
    or revoking its capabilities, are commands; grants are human-only and audited
    (§21.18, security-relevant state). Only the downloaded-bytes cache is a direct write.
  - `replaces` conflicts shown with **both names** (Ch.32 §32.4).
  - Browse the index, and an equivalent of `forge add`.
- **Audit log.**
  - Every remote and teammate envelope, greppable by session, user and sandbox.
  - Every security command (plugin grants and revokes, plugin set changes) and
    every device pairing and unpairing (§21.18).
- **Remote connect.**
  - Pair a device, list sessions, show a latency indicator.
  - Pairing is user config, written by the allow-listed `RemotePairingStore`, audited, and
    needing a human on both devices (§21.18). The latency indicator is a live feed at
    ≤ 2 Hz (§21.11).
  - Loopback by default. LAN exposure is an explicit choice (Ch.34 §34.5).
- **Compute and farm.**
  - Adapters and their measured throughput, as a live feed at ≤ 2 Hz (§21.11).
  - LAN farm nodes: discovery, status, and failures (Ch.26).
- **Team and members.**
  - **Create Team → Add Member in three clicks**: invite by email or join code, pick a
    role, optionally scope paths, and see revocable pending invites (Ch.37 §37.7).
  - Membership changes are commands.
- **Sandbox and Live/Pull.**
  - Sandbox status and visibility (default `Team`, O-17).
  - A **one-click, reversible** Live/Pull toggle (I20).
  - A plain statement that concurrent same-property editing is not supported (E-37).
- **Presence.**
  - Who is looking at what, as badges in the hierarchy, inspector and viewport.
  - A live feed with events coalesced to ≤ 10 Hz (§21.11).
- **Publish and review queue.**
  - Publishing is a **command** (`Collab::Publish`) issued through the bus client. The
    core performs the `ProjectStore::commit` (E-36); the panel never calls
    `ProjectStore` (§21.2, I7), like revision history.
  - Optional maintainer approval, and per-path overrides (O-16).
- **Ownership claims.**
  - Claim a subtree (a scene, a prefab, a body, an asset) from the hierarchy or the asset
    browser's context menu, or from this panel. Release it
    here (Ch.37 §37.4).
  - Claims and releases are **commands** (`Collab::Claim / Release`), audited. A role
    without claim rights (Ch.37 §37.6: Reviewer, Viewer) is refused with a reason.
  - While someone else holds a claim, its subtree is **live read-only** for everyone
    else: the inspector's fields are disabled, the hierarchy shows the holder's badge,
    and an edit command on it is rejected by the core. The UI does not just hide it.
  - The panel lists the claims you hold and the team's claims by path, and the member
    list shows what each member has claimed (Ch.37 §37.7).
  - When S15's fallback makes scoped ownership mandatory in Live, the Live/Pull toggle
    asks for a claim before it turns an edit on. The rule comes from the backend, so the
    panel does not change.
- **Conflicts.**
  - A three-way view on the reflect tree. Command preconditions that failed are surfaced
    and **never dropped** (Ch.37 §37.4).
- **Licence status.**
  - Shows tier, seat, bound team and expiry.
  - **On lapse: a banner and a renew button, collaboration panels greyed, nothing locked**
    (E-57). No network call anywhere in the panel's path (I21, E-54).
- **Sequencer.**
  - Keys any reflected property on any entity (Ch.19, animate-any-property).
  - Curve editing and tracks.
- **Animation state machine.**
  - States, transitions, 1D/2D blend spaces, and per-bone masks (Ch.19).
- **Localisation.**
  - String-table editing and a pseudo-locale preview (Ch.28).
- **2D editors.**
  - Tile palette with autotiling, sprite-sheet slicing, and a `Skeleton2D` cutout-rig
    editor (Ch.35 §35.2). Open in the 2D preset's default layout; available in every
    preset (I15).
- **Audio mixer.**
  - Buses, sends, meters, and spatial preview (Ch.20).
- **Input action map.**
  - Game input actions and rebinding defaults (Ch.28). This is distinct from the editor
    keymap.

**Requirements on every panel (WP-U12, M2-70).** WP-U12's backlog scope asks for
"onboarding/first-run tips, consistent empty states and error messages". They apply to
every row above, so they are one DoD row, not a panel:

- **Empty states.** Every panel declares an empty state with a short explanation and at
  most one primary action ("Import an asset", "Create a team",
  per §21.19). It is built from the catalogue's empty-state widget (§21.16), so every
  empty state looks alike. A panel that shows a blank area has no empty state and fails
  `test_panel_empty_states`.
- **Error messages.** Every user-visible error is a toast or an inline message carrying
  its stable `ErrorCode` (M2-42), what happened, and what to do next. A rejected
  command shows its `Rejection` reason, never a generic "failed".
- **First-run tips.** A short, dismissible tip per panel on first open. Tips are
  localisation keys. "Seen" is user config, not project state. A tip never takes focus,
  never blocks input and never schedules a frame after it is dismissed (the zero-idle
  rule, §21.3). Settings has "Reset tips" and "Never show tips".

**Implementation (WP-U12, ADR 0046).** `PanelCx::first_run_tip` (a tip bar above the panel,
`crate::tips`; shown only when the editor has a user config directory to remember a
dismissal in; `tips_seen` and **First-run tips** in the editor settings, **Reset tips** in
Settings) and `PanelCx::never_empty` for panels that always have content; `EmptyState`
wraps and takes a bound message. A warning or an error is a `notify::Problem` (code, what,
why, next step; `EditorError::next_step`, `next_step_for_rejection`, `EDITOR-0014` refusals,
`EDITOR-0015` counted incomplete problems). Every UI string is a key: `forge_ui::tr!` /
`trf!` / `l10n::tr_str` (the source text is the key; a per-thread UI locale; the
pseudo-locale). The Forge icon set (`forge_ui::icons`, 53 vector icons, MIT OR Apache-2.0)
draws the icon buttons. Audits over every panel: `tools/forge-editor-bin/tests/test_editor_panels_audit.rs`.
Guide: `docs/guides/editor-tools-with-forge-ui.md`.

**Implementation (WP-U5, ADR 0024)** — the hierarchy, inspector, asset
browser and console:

| Piece | Where | What it is |
|---|---|---|
| Change log | `crates/forge-editor/src/mirror.rs` | `ProjectMirror::changes_since(seq)`: created / removed / renamed / reparented / property, bounded (`CHANGE_LOG_CAP` 8192), cleared on resync (`None`: rebuild) |
| Components | `crates/forge-editor/src/inspect.rs` | a registered `#[forge_api]` struct under a key; field `f` is property `key.f`, nested structs extend the path, an enum holds its variant name with its data variant's fields beside it, `FramePos`/`FrameVel` are `path.frame` + `path.local`; defaults read by reflection from `Default`; hidden fields stored, not shown; `editor_for` is total over `PinKind`; `field_state` (mixed values), `set_commands` (+ touch to promote: `gen.promoted`), `add_commands` / `remove_commands` / `variant_commands`; built-ins `Transform`, `Light`, `Tag` |
| Hierarchy | `plugins/forge-panels-scene/src/hierarchy.rs` | `VirtualTree` over the mirror, incremental from the change log **with or without a filter** (a change re-tests only the entity it names and walks its ancestors, adding or pruning dimmed context rows); the filter is a `ContainsQuery` folded once (allocation-free matching) and is applied as a diff in `FILTER_SLICE` (4 ms) slices per loop turn — a narrowing query re-tests only the rows shown, a broadening one removes nothing — with a spinner in the bar while it runs, the panel asking the shell for another turn (`want_turn`) until done; rename, drag-reparent, delete, add, search (matches + dimmed ancestors), visibility / lock row badges (`editor.hidden`, `editor.locked` properties), locked entities refuse edits; multi-row edits one transaction |
| Inspector | `plugins/forge-panels-scene/src/inspector.rs` | a group per component on the selection, rows from reflection with units, ranges, tooltips, categories; editors: tri-state toggle, number field, **exact integer fields** for integer / id / time rows (`IntegerField` over `i128`: the Rust type's range narrowed by `#[forge(range)]`, typed values outside it refused with the reason, a `u64` stored as its bit pattern so all 64 bits round-trip, time as fixed-point seconds), slider (one gesture across the selection, `Gesture::update_all`), text, entity picker, frame + vector, variant choice; "mixed" per row; add / remove component; `InspectorWidget` by `widget` hint, by type, or for a whole component; generated content banner (seed path, reveal, promote); rebuild only on a shape change, refresh only touched rows |
| Asset browser | `plugins/forge-panels-assets/src/browser.rs`, `crates/forge-editor/src/assets.rs`, `crates/forge-ui/src/widgets/grid.rs` | `VirtualGrid` (grid / list) with async thumbnails (`ThumbProvider` → `AssetServer::thumbnail` → a waker posts to the window; capped, evicted from the atlas); folders from source paths, breadcrumbs, search; rename / move / remove as `forge.asset.*` commands, a folder rename moves its contents in one transaction; OS drops staged then imported (one transaction); lock badges from store locks; `ServerCatalog` follows the mirror's import intents (`AssetServer::sync_intents`); `CatalogIndex` is the picker's `AssetIndex` |
| Console | `plugins/forge-panels-core/src/console.rs`, `crates/forge-editor/src/console.rs` | follows the log from a cursor: appends only entries past it, trims dropped entries from the front, relabels the newest row on a repeat — a line at the 10,000-entry cap writes two rows (`ConsoleLog::entries_from`, O(log n) to the start); level filters, search (message, subsystem, link), grouped repeats, entries from any thread (`LogSender`, one wake per drain); click-through: seed path → navigator, entity → selection, transaction → undo history, graph node → `forge.graph`; refusals logged, their toast's "Details" reveals the entry (`NoticeAction::Reveal`, `SessionState::reveal`) |
| forge-ui additions | `crates/forge-ui/src/widgets/` | `RowBadge` / `RowBadgeClicked` on tree rows, `VirtualGrid`, `SignalRelay` (acting on signal-bound editors without effects), `Label::tooltip`, `ImageAtlas::remove`, `Build::poster`, `NodeStyle::indent`, `IntegerField`, `fuzzy::ContainsQuery`; `VirtualTree::remove` is O(subtree) (it no longer sweeps every row), `rows_written` (the guards' cost counter), `key_order_position`, `child_at` |

**WP-U13 follow-ups to WP-U5.** The console's search is a `ContainsQuery` folded once, and
each entry's link text is rendered when it is logged (`LogEntry::link`), so re-filtering
10,000 entries allocates nothing (`test_console_search_alloc`, gate row
`C-console-search-alloc-free`). The hierarchy's filter job walks everything by cursor: the
match scan over the rows shown (`TreeWalk`, a pre-order walk resuming by key), the prune
over every row, and the merge over the mirror's roots and each parent's children
(`ProjectMirror::child_after`) — no phase change snapshots O(n) rows, so no step between two
slice checks touches more than one item and its ancestors (`test_hierarchy_filter_bounded`,
gate row `C-ui-hierarchy-filter-slices`, positive control: the snapshots restored).

**Implementation (WP-U13, ADR 0026)** — the 2D editors, the audio mixer and the input
action map, in `plugins/forge-panels-domain`:

| Piece | Where | What it is |
|---|---|---|
| Models | `crates/forge-editor/src/domain/` | project settings under `d2.`, `audio.`, `input.`, one value per field, read by total parsers (a malformed value is skipped and reported); every edit is `SetSetting`, several fields one transaction; `ProjectMirror::settings_under` reads a namespace in O(log n + matches) |
| 2D model | `crates/forge-editor/src/domain/scene2d.rs` | tile sets with autotile terrains (rules per blob-47 canonical mask, `canonical`, `blob47`), tile maps with layers in 16 × 16 chunks of cells (empty, `t<tile>`, `a<terrain>`: terrains are stored, tiles solved when shown), paint `Stroke`s (only touched chunks; each step sends only the chunks it dirtied — WP-U7 follow-up), sprite sheets (`SheetSlice`, frame animations), `Skeleton2D` rigs (bones with parent, offset, rotation, length, sprite, draw order; loop checks) |
| Audio model | `crates/forge-editor/src/domain/audio.rs` | buses routed to `master` with volume, pan, mute, solo and pre/post sends; loop detection (`would_loop`, `route_loop`); spatial settings (min/max distance, rolloff) |
| Input model | `crates/forge-editor/src/domain/input.rs` | action maps, actions (`Button`, `Axis1D`, `Axis2D`), default bindings (`Keyboard/Space`, `Composite1D(…)`, `Composite2D(…)`) with fit checks and conflicts naming both actions; project state, never the editor keymap |
| Backends (D-4) | same files | `Scene2d` (`Forge2dScene` since WP-U15: forge-2d's autotile solve, grid slicing and forward kinematics; image sizes from PNG / Aseprite headers), `AudioBuses` (`MemoryAudio`: a power-sum mix graph with solo-in-place and equal-power pan, test tones, a meter `LiveCell` bumped only on change, rolloff curves), `InputActions` (`MemoryInput`: standard keyboard, mouse and gamepad; key and mouse capture; `inject` stands in for a pad press); services `scene2d`, `audio`, `input` |
| 2D editors | `plugins/forge-panels-domain/src/editors_2d.rs` | tabs Tile palette / Sprite sheet / 2D rig; tile sets and terrains, rules list ("Assign to rule", "Blob template"), maps and layers, the canvas (a stroke is one gesture), the palette; slicing fields, the sheet preview, animations built by picking frames; rigs, the bone tree (rename, drag-reparent with loop refusal, Delete removes the subtree), exact bone fields, the pose view (a tip drag is one gesture) |
| Mixer | `plugins/forge-panels-domain/src/mixer.rs` | routing tree (drag to re-route, loops refused with the reason), strip (volume and pan faders as gestures, mute, solo, test tone), sends (only loop-free targets offered; level fader; pre-fader), meter bridge on the backend's live feed (≤ 10 Hz, only while visible), spatial preview (source position is session state) |
| Input map | `plugins/forge-panels-domain/src/input_map.rs` | maps and actions (rename in place, remove), press to bind (key, mouse, or a gamepad polled only while capturing), composites from 2 or 4 presses, pick any control, conflicts listed and refused naming both actions |
| Widgets | `plugins/forge-panels-domain/src/widgets.rs` | `TileCanvas` (virtualised: visits only the cells in view; keyboard cursor, Space paints, Delete erases), `TilePalette`, `SheetPreview`, `RigView`, `MeterBridge` (`Role::Meter` children), `SpatialPad`, `BindCapture`; each raises actions, never edits the project |
| Shell addition | `crates/forge-editor/src/panel_rt.rs` | `PanelBuilder::want_turn`: a panel filling its lists in its sync step shows them right after it is built |

Measured (dev build, RTX 3080 box): the canvas over a 512 × 512 map (262,144 painted cells)
visits 472 cells per paint (its view); a 40-cell stroke is 1 undo entry; a stroke step on
that map (terrain cells, 1,024 chunks) decodes or copies 1 chunk and its loop turns take
0.96 ms median against 0.91 ms on a 128 × 128 map (a full re-read per step: 1,024 chunks,
97 ms — the mirror's setting log, `ProjectMirror::setting_changes_since`, lets
`Doc2d::apply_setting` apply a changed chunk key in place, and layers are `Arc`-shared
with the canvas instead of copied); a steady test tone
costs 0 frames and 0 wakeups over 10 s once the meters settle; a finished bind capture
leaves no timer (0 frames, 0 wakeups). With the cursor-walked filter job the hierarchy's
largest unchecked step is 2 items over 20,000 entities (was the row count at every phase
change); over 100,000 entities a keystroke's turn is 4.2 ms p95 and a filter applies within
43 turns.

Guards (gate rows): `C-tilemap-canvas-virtual`, `C-tile-stroke-single-undo`,
`C-tile-stroke-bounded`,
`C-mixer-meters-idle`, `C-domain-editors-a11y`, `C-ui-hierarchy-filter-slices`,
`C-console-search-alloc-free` (bound, each with its positive control);
`C-audio-backend`, `C-input-backend` (UNBUILT, D-4; `C-editors-2d-backend` is bound since WP-U15, §35.5). Every domain
panel's edits go through the emitter, which `test_command_liveness` checks over
`plugins/forge-panels-*`. Not built here: tile and sprite pixels (the in-memory backend
draws numbered swatches until `forge-2d` renders), sound on a device, reading real
gamepads (the other §35.2 items are forge-2d's, §35.5).

**WP-U7 follow-ups to WP-U13.** A paint stroke's step sends only the chunks that step
dirtied (`Stroke::unsent`): a step replacing one the gesture still holds back (same frame)
keeps the held one's chunks, so nothing is lost, and a step after a sent one carries just
its own — a 40-cell stroke over three chunks sends 40 chunk commands, not 72 (O(n²) over a
long stroke before). Every other `d2.` key is applied in place too
(`Doc2d::apply_setting`): a map's or a layer's own field is re-read by exact lookup (its
chunks are kept), a layer or map that appears is read from its own keys, one whose keys are
all gone is dropped, and a tile set, sheet or rig is re-read alone — a layer rename on a
1,024-chunk map decodes 0 chunks (was 1,024). Only a non-canonical layer or chunk spelling
still re-reads the namespace. `test_tile_stroke_bounded` bounds all three (gate rows
`C-tile-stroke-bounded`, `C-tile-stroke-deltas`).

Measured, the same stroke on the same 512 × 512 map (terrain cells, 1,024 chunks), median
loop turn per step (the step and the mirror's echo):

| Build | 512 × 512 | 128 × 128 | Full re-read per step (the control) |
|---|---|---|---|
| dev | 0.93 ms | 0.90 ms | 92.5 ms |
| release | 0.65 ms | 0.65 ms | 39.3 ms |

**Implementation (WP-U6, ADR 0029)** — the viewport, play controls, profiler, and
the `EditorOverlay`, `ViewportTool` and `Theme` points:

| Piece | Where | What it is |
|---|---|---|
| Camera | `crates/forge-editor/src/viewport/camera.rs` | `EditorCamera`: a `FramePos` and a `DQuat` (view axes → frame axes, `forge_render::Camera`'s convention), `f64` throughout; orbit (yaw about the frame's up, pitch clamped at ~89°), look, pan (the pivot-depth point stays under the pointer), dolly by a constant factor per wheel notch (0.85), fly speed = distance to the pivot × √2 per user step (log-scaled); `frame_sphere` and `move_to_frame` move the camera into a target's frame; `offset_of` resolves into the camera's frame and subtracts in `f64` — the only thing ever projected |
| Scene, picking, lines | `viewport/scene.rs` | `ViewportScene`: drawables (every entity with `transform.position`, not hidden) kept from the mirror's change log (one entity re-read per change); OBB picking camera-relative; grid (a power of ten for the camera's height, never finer than `editor.grid_size`), frame axes, bounding boxes (selected first, then nearest, capped at 2,000) as projected segments — the placeholder renderer when no GPU host is attached; volumes are drawn by their field, not as boxes |
| Gizmos | `viewport/gizmo.rs` | translate (axes, planes, view-plane centre), rotate (rings; an edge-on ring falls back to the screen angle), scale (uniform: `Transform` has one scale) in local / world / frame space; snapping from the project's `editor.snap_translate`, `editor.grid_size`, `editor.snap_rotate`; multi-entity drags about the first selected; `commands()` emits only the `Transform` fields that changed |
| Tools | `viewport/tool.rs`, `viewport/controller.rs` | the `ViewportTool` point (`forge.editor.viewport_tool`; item: title, category, key hint, a factory of `ToolBehavior`s); select, move / rotate / scale (`TransformTool`, one gesture per drag, Esc cancels), measure (two `FramePos`, the distance in `f64` across frames); `ViewportController`: secondary drag looks (W/A/S/D/Q/E fly while held, the wheel sets the speed), middle pans, Alt+primary orbits, the wheel dollies, a primary press goes to the tool first and otherwise navigates by the Orbit / Fly / Pan mode or clicks to select, F frames the selection |
| Render surfaces | `viewport/surface.rs`, `tools/forge-editor-bin/src/viewport_gpu.rs`, `forge_ui::render_wgpu::ExternalCx` | a cell requests a frame (camera, physical size, drawables, selection) only when what it shows changed; the runner's `UiApp::render_external` (called for a damaged window frame before it is drawn) lets the binary's `ViewportRenderHost` render each request with `forge_render::Renderer` (public API only: a cube per entity, a selection material, a ground plane, a sun) on the UI's pool device and register the texture composited under the overlay lines |
| Viewport panel | `plugins/forge-panels-scene/src/viewport.rs` | toolbar (tool, space, snap — a project-setting command —, navigation mode, 1 / 2 / 4 cells, frame selection), four cells (perspective, top, front, side cameras; hidden cells cost nothing); overlays: cell, mode, entity count, the camera's frame and distance from its origin, pivot, fly speed, renderer (or "placeholder"), the play sandbox's tick, the `SeedPath` under the pointer, the tool's status; Q / W / E / R pick tools; repaints only on a camera, scene, selection, tool or play change or while a flight moves the cell |
| Play | `crates/forge-editor/src/play.rs`, `plugins/forge-panels-scene/src/play.rs`, `overlay.rs` | play / pause / step / stop are `forge.play.control`, a **session command** the core registers (validated, audited, no project change, never an undo step) that every client issues — panel, menu, chord, script — and the core hands to the clients that follow session commands (`SessionCommand`, in stream order; the shell's play core runs it, and so does a headless run, Ch.34.4); the `PlayBackend` trait (fork from the edit world read-only; each control a `PlayCommand` recorded with its tick in the session log; advance on the editor clock in fixed 60 Hz ticks; simulated transforms); the backend is WP-13's play core, `sim_bridge::SimPlay` (Ch.34.4; it replaced the in-memory `MemoryPlay`); the Play menu and panel; the play banner overlay; the shell wakes at the display rate only while playing |
| Profiler | `crates/forge-editor/src/profile.rs`, `plugins/forge-panels-core/src/profiler.rs` | the `CounterSource` trait (a `LiveSource` with a snapshot: frame timeline, named budgets, adapter lanes, replication); the editor's source is `forge-trace` (`sim_bridge::TraceCounters`, Ch.29 as built), with the perf gate's budgets from `tests/perf/budgets.ron` (dev-box allowances), the play core's steps and the viewport host's render time and adapter lane; `MemoryCounters` (D-4) remains a source a test pushes by hand; the panel owns a ≤ 10 Hz feed and a `ui.self` self-UI feed (`SelfUiCounters`, updated every loop turn by the shell, never waking it); a budget over its allowance is drawn red |
| Overlay and Theme points | `crates/forge-editor/src/overlay.rs`, `shell.rs` | `EditorOverlay` (`OverlayDescriptor`: title, order, a `PanelCx` build), built once into a pointer-transparent layer over the main window — the toasts (following the notification centre by id) and the play banner are first-party overlays; `Theme` (`forge.ui.theme`, item `forge_ui::Theme`, defined in `forge-editor` so `forge-ui` stays free of the plugin kernel): the three built-ins register through it, a plugin theme gets a View / palette action and is remembered as `EditorSettings::theme_override`; the shell resolves the window's theme through the registry |

Measured (dev box, RTX 3080, Vulkan): `forge-editor` (release) first frame 520 ms with the
viewport hosting `forge-render`, 4 s idle: 0 frames, 0 wakeups; headless, 10 s idle with
the viewport open: 0 frames, 0 wakeups (the control, a render-loop viewport, draws every
display frame); camera precision far from the world
origin: a 1 mm camera step moves an object 3 m away by the pinhole value to within
0.0018 px (the `f32` control jumps by hundreds of pixels); the profiler with a 100 Hz producer: ≤ 10 refreshes a second visible, 0 frames
and 0 wakeups hidden.

Guards (gate rows): `C-viewport-camera-precision`, `C-gizmo-gesture`, `C-viewport-idle`,
`C-viewport-forge-render`, `C-play-sandbox`,
`C-profiler-live-panel`, `C-overlay-theme-points` (bound, each with its positive control);
the three points joined `C-extension-point-replaceable`'s kits; `C-play-core` and `C-profiler-trace-source` were bound
by WP-13 (Ch.29, Ch.34.4). Not built here: a per-axis scale, box selection, the
debug overlays, and releasing a closed viewport's render surface.

**Implementation (WP-U7, ADR 0032)** — the project lifecycle: launcher and new project,
presets and promotion, the Project page, revision history, build and
export, in `plugins/forge-panels-project` over `crates/forge-project`:

| Piece | Where | What it is |
|---|---|---|
| Project files | `crates/forge-project/src/format.rs` | `forge-project.ron` (format, name, preset, template, engine), `project/settings.ron`, `project/scene.ron` (entities by file id, parent, properties) — RON; `forge.project.load` replaces the project in one diff (clear, settings, entities parents-first, entity references remapped), atomic: a document the project refuses changes nothing |
| Store host | `crates/forge-project/src/host.rs` | `ProjectHost`, the core's only store holder (I7 lists it as a project-state writer): create (files written, not committed), open, clone, save (commit with the envelopes of committed transactions since the last save — Ch.33 §33.4 — or nothing when nothing changed), push, pull, build; unsaved-changes tracking; `ProjectStatus` (open project, 100 newest revisions with summaries, outcomes, last build) with a generation clients compare and an epoch bumped per load; `file:` (`LocalFs` through the `StoreBackend` registration) and `memory:<name>` (in-process, D-4) |
| Push, pull, clone | `crates/forge-project/src/sync.rs` | fast-forwards through the `ProjectStore` trait alone: each missing revision's tree becomes the working files, then `commit` with its message and commands, and the replayed id must equal the original; histories that do not line up are refused (PROJECT-0007), never merged; any store is a remote (a folder remote works today; Git, S3, SQL URLs are refused at link time as UNBUILT) |
| Semantic history | `crates/forge-project/src/summary.rs` | per issuer: "human:ada: placed 37 “Tree”", "human:ada: edited 1 property on 1 entity" (a gesture's frames count once) |
| Packager | `crates/forge-project/src/packager.rs` | `Packager` trait; targets (Windows, Linux supported; Web, Android best-effort); the E-64 attribution (credits entry, executable metadata and its carrier per target, `forge.json`, the `NOTICES` line, the store-page part, the default splash); `NOTICES` preview; `MemoryPackager` (D-4): the attribution, `forge.json`, credits and `NOTICES` are produced, the executable is listed UNBUILT |
| Launcher | `plugins/forge-panels-project/src/launcher.rs` | recent projects (open, remove from list), open by location, clone from a remote, the three-click new project (New project…, a template, Create; name and folder filled in), an unsaved-changes question (save first, discard, cancel) before any create, open or clone |
| Presets | `plugins/forge-panels-project/src/presets.rs` | the three presets, the current one marked; picking one shows the Ch.31 §31.4 cost and this project's losses; lossless is one click, lossy asks with the loss named |
| Revision history | `plugins/forge-panels-project/src/history.rs` | revisions with their per-issuer summaries, save with a message, link a remote (Enter or Link), push, pull, the first-save offer with "Not now" |
| Build and export | `plugins/forge-panels-project/src/export.rs` | targets, the attribution preview, a 40-line `NOTICES` preview, build through the core, the build's files |

**Implementation (WP-U8, ADR 0034)** — the graph editor: one node canvas for blueprint,
material, generator and PCG graphs, over the Ch.24 IR contract:

| Piece | Where | What it is |
|---|---|---|
| Node canvas | `crates/forge-ui/src/widgets/node_canvas.rs` | the §21.16 Graph widget: nodes with typed pins, wires, reroutes, comment frames, errors; pan (middle/right/Alt drag, arrows), zoom about the pointer (wheel, `+ - 0`), `F` frames, marquee and click selection, Ctrl+A/C/X/V/D, Delete, Ctrl+arrows nudge, `C` comments the selection, `[`/`]` step node by node, Space or a double click opens the node search; typed actions for every edit; each candidate wire checked while dragged (`ConnectCheck`) with the refusal's reason drawn beside the pin and as its tooltip; only in-view nodes and wires recorded, one quad per node below `DETAIL_ZOOM`, wires one polyline mesh per colour (`MeshData::stroke_polyline`); the minimap is a child widget (every node's dot) that a pan never invalidates; accessibility: a list box whose on-screen nodes are options, bounds refreshed 150 ms after the view settles |
| Graph model | `crates/forge-editor/src/graph/mod.rs` | graphs as `graph.<g>.` project settings (name, kind; `node.<n>.op/x/y/in.<pin>/lit.<pin>`; `comment.<c>.x/y/w/h/text`); a wire is the field of the input it feeds; command builders (new/delete graph, add/move/delete nodes, connect/disconnect, literals, reroute insertion, comments, copy/paste fragments); `check_connect` (types and unit dimensions through `forge_reflect::connect`, reroutes resolved, cycles refused, a sentence saying why); `Graphs::follow` applies the setting change log node by node |
| Node library | `crates/forge-editor/src/graph/library.rs` | every item of the given `#[forge_api]` registries is a node (pins, units, docs, purity); the editor's built-in nodes are real `#[forge_api]` functions (math, logic, physics, material, generator, PCG) plus the component catalogue's types; category decides the graph kinds; a mutating item is blueprint-only |
| IR contract | `crates/forge-editor/src/graph/ir.rs` | `GraphIr` (compile: ops in dependency order, generated Rust, diagnostics `GRAPH-0001..0009` mapped to nodes; `parse_text`); `ForgeGraphIr` compiles through `forge-graph` (Ch.24 §24.4; WP-23 replaced the D-4 `StubIr`), and a generator graph also lowers to its global-solve plan (`lower_generator`; Compile says how many cells the solve runs on); `apply_text` turns edited text into commands |
| Panel | `plugins/forge-panels-assets/src/graph.rs` | `forge.graph`: graph list (rename in place), new graph of a kind, delete; the canvas; node search filtered by graph kind (and, for a dropped wire, to nodes with a compatible pin, auto-wired); compile (errors on nodes, in a list that frames them, and in the console with a `GraphNode` click-through that selects and frames the node); graph ↔ text view with Apply text; every edit a command, a multi-setting edit one transaction |

Measured (dev build, forge-ui at opt-level 2, RTX 3080 box): `ui_graph_2k_nodes` (2,000
nodes, ~3,900 wires, 240 frames of pan with a zoom every 12th, zoom 0.49–2.04) p95 2.6 ms
per frame (budget 4.0), at most 169 nodes recorded, the minimap untouched by pans; before
wires were polylines and accessibility bounds were deferred, 4.2–4.4 ms.

| Piece | Where | What it is |
|---|---|---|
| `forge-ui` | `crates/forge-ui/src/ui.rs` | `set_hidden` resyncs live feeds at once: a panel the dock shows mid-frame refreshes now, not at the next unrelated wake (`FeedFaults::visibility_on_next_frame` is the control) |

**Implementation (WP-U10, ADR 0039)** — teams, sandboxes and the collaboration panels (Ch.37,
Ch.38 §38.2) against labelled in-memory identity and collaboration servers (D-4) over a real
baseline store.

| Piece | Where | What it is |
|---|---|---|
| Server traits | `crates/forge-project/src/collab.rs` | `IdentityView`/`IdentityBackend` + `MemoryIdentity` (accounts, teams, roles, per-path scopes, email and join-code invites, never leaving a team without an Owner); `CollabView`/`CollabBackend` + `MemoryCollab` over any `ProjectStore` (Git tested): publish = `ProjectStore::commit` as a fast-forward, the review queue (pending, approved, rejected, stale), sandbox rows with `Private` withheld, Live/Pull policy, presence, claims. The `*View` halves read and set session state only; the write traits are named only by the core (I7 mutators) |
| Merge | `crates/forge-project/src/merge.rs` | `merge3` on the reflect tree (per setting, name, parent, property, existence; orphans and loops are conflicts; colliding new keys re-key mine), the key-preserving precondition patch (`forge.collab.sync`, `DiffBuilder::spawn_at`), content paths and globs |
| Guard | `crates/forge-cmd/src/bus.rs`, `crates/forge-editor/src/collab/guard.rs` | `CommandGuard` / `Bus::set_guard` (additive: apply, dry run, batch preview, undo, redo); `CollabGuard` reads the issuer's role per command: rank-bounded team management, Viewer/Reviewer read-only, claims enforced for every path, collaboration refused on a lapsed licence and nothing else (E-57) |
| Core | `crates/forge-editor/src/core/collab_host.rs` | the core as one person's sandbox: deltas, pull (merge → patch, conflicts kept with choices), Live following on pump, publish (scopes, review rules from the baseline), team, claim and review commands performed on the backends and written to the audit book; `EditorCore::attach_collab`, `collab_status`, `collab_follow` |
| Commands | `crates/forge-editor/src/collab/mod.rs` | `forge.team.create / bind / invite / revoke_invite / accept / set_role / set_paths / remove`, `forge.collab.claim / release / publish / approve / reject / pull / resolve / review_policy / sync`; `team.*` and `collab.*` settings reserved; `CollabServices` (feeds, presence publishing, claims and viewers for the scene panels); `licence` (`EntitlementSource`, `MemoryEntitlement`, the four rules, a 14-day grace until O-27) |
| Panels | `plugins/forge-panels-collab` | `forge.team` (three clicks: Create team, Add member…, Send invite; pending invites with Revoke; members with role, online, sandbox, claims; role and scope changes; join by code or accept an email invite), `forge.sandbox` (base and unpublished changes, one-click Live/Pull, visibility, incoming revisions with Pull everything / up to the selected, teammates' sandboxes, the E-37 statement), `forge.presence`, `forge.ownership`, `forge.publish_queue` (publish, approve / reject with a reason, review rules), `forge.conflicts` (base / mine / theirs, keep mine / take theirs / all, Apply), `forge.licence` (tier, seat, team, expiry; lapse banner and Renew). Live feeds at ≤ 10 Hz |
| Scene panels | `plugins/forge-panels-scene` | presence and claims as `RowItem::note` in the hierarchy, "Also here" and a read-only inspector under another person's claim, a teammates line in the viewport overlay and its accessible label |
| Binary | `tools/forge-editor-bin` | attaches the in-memory team server and a Team stand-in entitlement; the shell publishes this editor's presence |

Measured (dev box): with every collaboration panel, the hierarchy, the inspector and the
viewport open on a team with a teammate online, 10 s idle: 0 frames, 0 wakeups. A teammate
moving their selection at 100 Hz refreshes the presence panel at most 11 times a second. A
pull's merge and patch (release): 10,000 entities 13 ms + 4 ms, 100,000 entities 145 ms + 48 ms.
A sandbox holding 20,000 unpublished edits (release): an idle client pump 0.33 µs (129 µs when
the deltas were walked per pump), a drag frame 8.2 µs (140 µs), a status rebuild 8.4 µs (7.7 ms).

Guards (gate rows): `C-team-roles-per-command`, `C-ownership-claims-enforced`,
`C-sandbox-live-pull-in-memory`, `C-conflicts-surfaced`, `C-lapse-degrades-never-locks`
(`crates/forge-editor/tests/test_team_collab.rs`; controls: a core without its guard, pulls by
reload, a pull that swallows conflicts, a lapse that locks edits), `C-presence-live` (control:
an uncapped feed), `C-collab-panels-idle` (control: panels that poll), `C-team-settings-reserved`
(`team.*` and `collab.*` refused to every plain `SetSetting`, whoever sends it, Owner included;
control: a core without that reservation), `C-join-codes-withheld` (a join code is a bearer
credential: the identity view and the Team panel show its text only to someone who could have
minted it, `may_assign`; the invite outcome in the audit names no code; control: a view that
hands out every code), `C-sandbox-deltas-bounded` (gesture frames fold to one delta per
written key and transaction; the unpublished count and the summary are kept as deltas arrive,
`forge_project::summary::Summary`; the sandbox row is sent per transaction, undo or redo, never
per frame; controls: no folding, a row per change), `C-hierarchy-notes-no-damage` (the hierarchy
touches its tree only when a note differs; control: notes rewritten every sync);
`C-identity-backend`,
`C-collab-backend`, `C-licence-backend` UNBUILT (D-4). I19 and I20 stay UNBUILT for the real
server. Membership and roles live in the identity database (the server's), not the project;
the command that changes them is still on the bus, human-only and audited (§21.18). Live/Pull,
sandbox visibility and presence are session state. Not built here: asset-path claims (store locks
cover binaries), spikes S15 and S16.

**Implementation (WP-U11, ADR 0040)** — the sequencer, the animation state machine and the
localisation editor (models as project settings, the domain editors' conventions, ADR 0026).

| Piece | Where | What it is |
|---|---|---|
| Timeline model | `crates/forge-editor/src/authoring/timeline.rs` | `seq.` settings: clips (length, snapping rate, loop) of property tracks (an entity and **any reflect path** it has) and event tracks; keys with constant / linear / smooth interpolation, evaluated with the curve editor's own `Curve::eval` per float or vector component (integers round; bools, text, entities step); key, move (snapped, collision-refused), delete, interpolation, curve apply (one transaction), mute; `TimelineDoc::apply_changes` re-reads only the clips or tracks the mirror's change log names |
| Preview | `crates/forge-editor/src/authoring/preview.rs` | scrub = sample, write the values over the animated entities' edit-world rows, fork `forge_sim::SimWorld` from those rows only; play advances in the play core's 60 Hz steps (`tick_of_step`) and collects crossed event keys; session state (`EditorServices::anim_preview`), never the project; the viewport draws its overrides while no play session runs (`plugins/forge-panels-scene/src/viewport.rs`) |
| State machine model | `crates/forge-editor/src/authoring/anim.rs` | `anim.` settings: machines on a skeleton, float / bool / trigger parameters, states (a motion, or a 1D / 2D blend space of samples), transitions (from a state or *any*, textual conditions `speed > 0.5`, `grounded`, `!grounded`, a trigger; exit time; cross-fade), bone masks (per-bone weights); `validate` (missing states, unknown or mistyped parameters, blend spaces short of samples or with coincident ones, unknown motions, skeleton mismatches, unreachable states); `MachineSim` (any-state first, triggers consumed, cross-faded weights); 1D linear and 2D gradient-band blend weights; `AnimSource` + the labelled `MemoryAnim` (a humanoid skeleton, a locomotion set) |
| Strings model | `crates/forge-editor/src/authoring/strings.rs` | `loc.` settings: the source locale, locales by BCP-47 tag, `table.key` strings with text per locale, a note and the room in characters; issues (missing, a dropped or invented `{placeholder}`, too long — the pseudo text included); coverage; compiles to `forge_ui::l10n::Localiser` (the game's lookup); CSV export and a planned import (changes counted, unknown locales and bad keys named, blanks change nothing); `StringTables` + the labelled `MemoryStrings` |
| Panels | `plugins/forge-panels-authoring` | `forge.sequencer` (clips; the selected entity's properties, Enter adds a track; the `TimelineView` — ruler, tracks, keys, playhead; scrub, `[`/`]` select keys, K keys at the playhead, key drags one gesture, Ctrl+arrows nudge, Space plays; the curve editor on the selected track and component; interpolation; loop, length, frame rate; mute, delete), `forge.anim_graph` (the node canvas of ADR 0034: states as nodes, one input pin per incoming transition labelled with its conditions, the *Any state* node, wiring adds a transition with the typed conditions and refuses a bad one with its reason; parameters with preview values; motions add states or blend samples; the `BlendSpaceView` with draggable samples and a preview point; bone masks by subtree; the preview's state, weights and transitions; problems on their nodes), `forge.localisation` (virtualised string list, locales with coverage, source / translation / note / room, pseudo view, the game's view, problems that select their string, CSV) |

Measured (dev box, release): a clip of 200 tracks × 50 keys reads in 10.1 ms from scratch and
0.05 ms incrementally for a one-key change (the path a key-drag frame takes); a scrub forking
the play core from 200 animated entities 0.37 ms; 10,000 strings read in 13.4 ms, their issues
in 1.1 ms, compiled to the runtime lookup in 4.1 ms (string edits are discrete commits). The
three editors open with content, idle 10 s: 0 frames, 0 wakeups; a paused preview: none either.

Guards (gate rows): `C-timeline-edits-undoable` (a 30-frame key drag is one undo entry and
undo restores it; control: a drag without a gesture), `C-timeline-preview-sandboxed` (scrub and
play leave the project's state hash unchanged; control: a preview that writes properties),
`C-authoring-panels-idle` (control: a timeline animating while stopped),
`C-authoring-editors-a11y` (control: an unnamed widget). `C-anim-backend` (`forge-anim`) and
`C-string-tables-backend` (`forge-play`, M5-8) are UNBUILT (D-4). Not built here: playing clips
inside a PIE session (the play core gains that with `forge-anim`), retargeting, IK, root motion
and per-bone blending in a pose (they need skeleton data that `forge-anim` owns); plural rules
(CLDR) and right-to-left shaping checks in the string editor.

**Follow-ups (WP-U14, ADR 0041)** — per-change work and sandbox upkeep. A sandbox row waits
only for the transaction that moved its summary (`C-sandbox-row-follows-its-txn`; control:
waiting for every open transaction); membership is memoised on the backends' generations, so an
idle pump allocates nothing (`C-collab-pump-no-alloc`; control: recomputed every pump);
transaction counts follow the bus's `Applied` stream, whatever path undid, redid or cancelled
(`C-sandbox-txn-hook`; control: call-site upkeep); the folded story is metadata and a publish
commits the sandbox document (`C-sandbox-story-metadata`; control: a replayed story). The
hierarchy edits its tree only for changes a row shows (`C-hierarchy-hidden-edits-no-damage`);
the sequencer re-shows only the tracks the change log re-read and the timeline paints only
visible keys (`C-sequencer-drag-cost`); `C-authoring-panels-idle` plays, then pauses (control: a
pause that keeps playing). The runtime sample advances its message buffers every turn
(`C-game-messages-bounded`), quits through the runner (`UiApp::exit_requested`, the run report
prints), localises the credits' headings and audits every screen under the pseudo-locale.

## 21.22 UI performance budgets (D-5) as named tests

Two kinds of assertion are used:

- **Counters** (frames, wakeups, shape calls, draw calls, live rows) are deterministic and
  machine-independent. They gate **every** leg.
- **Millisecond budgets** gate on the reference machine and, once Ch.29's perf gate exists
  (M1-11), against its stated regression band. Where neither applies, the test prints the
  measured value and reports `AWAITING(reference machine)`. It never passes silently (W9)
  and its tolerance is never widened (W5).
- **The reference machine** is the development workstation: RTX 3080, 12 logical CPUs,
  held idle per W6.

Every test below lives in `tests/perf/test_ui_budgets.rs` unless noted, and has a
positive control that must fail:

| Test | Budget | Positive control (must fail) | Gate row | DoD |
|---|---|---|---|---|
| `ui_idle_zero_redraw` | 0 frames and 0 loop wakeups over 10 s of injected-clock idle, every M2 panel open, a text field focused, caret timeout expired; run with every live-data panel visible and again with each hidden, sources quiescent | a spinner animating inside a hidden tab | `C-ui-idle` | M2-20 |
| `ui_live_panel_refresh_bounded` | profiler visible, engine stopped: 0 frames after one settle frame; engine playing: profiler refreshes ≤ 10 Hz and damage ⊆ the profiler rect; profiler in a hidden tab with the engine playing: 0 frames and 0 wakeups; `ui.self` counter bumps alone: 0 frames; an unchanged displayed value (latency equal after rounding): 0 damage | a profiler whose `ui.self` feed can wake the loop (it then redraws at vsync forever); a feed that keeps its waker while hidden | `C-ui-idle` | M2-20 |
| `ui_idle_no_busy_loop` | the runner is in `Wait`/`WaitUntil` whenever damage, animations and due timers are all empty; editor idle CPU recorded on the reference machine | a runner using `ControlFlow::Poll` | `C-ui-idle` | M2-20 |
| `ui_damage_bounded` | hovering one button re-records exactly one display-list slice, and damage ⊆ its rect + shadow outset | hover marks the root dirty | `C-ui-idle` | M2-20 |
| `ui_virtual_list_100k` | 100k rows: ≤ 2.0 ms layout+paint p95 over 240 scrolled frames; live rows ≤ ⌈viewport/row⌉ + 16 | virtualisation disabled (all rows realised) | `C-ui-virtual-budget` | M2-25 |
| `ui_virtual_tree_100k` | as above, plus expand/collapse of a 50k subtree ≤ 2.0 ms; index-node updates per expand/collapse ≤ (*d* + 1) · (⌈log₃₂ *b*⌉ + 1) whatever the subtree size (§21.12) | full re-flatten on expand; a row-by-row O(*k*) splice of the expanded run | `C-ui-virtual-budget` | M2-25 |
| `ui_virtual_table_100k` | as the list, 8 columns, sort + resize | columns realised for off-screen rows | `C-ui-virtual-budget` | M2-25 |
| `ui_hierarchy_100k` (WP-U5, `plugins/forge-panels-scene/tests`) | the real hierarchy panel over 100k mirrored entities meets the list budget | hierarchy bypasses `VirtualTree` | `C-ui-hierarchy-100k` | M2-35 |
| `ui_hierarchy_100k` — filtered edits | with a filter open over the same 100k entities, a rename by another issuer (leaving, joining or staying in the filter) costs ≤ 2.0 ms p95 over 60 edits, and the incremental rows equal a fresh filter row for row | the hierarchy rebuilding every row per change under a filter | `C-ui-hierarchy-filtered` | M2-35 |
| `ui_hierarchy_100k` — filter typing | typing, deleting and retyping a filter over 100k entities: the keystroke's turn and each turn finishing it ≤ one frame (16.7 ms) at p95 | the filter applied in one turn instead of `FILTER_SLICE` slices | `C-ui-hierarchy-filter-typing` | M2-35 |
| `ui_typing_latency_one_frame` | a key event delivered before frame N appears in frame N's display list and is presented at N; damage ⊆ the field rect | a field that defers its update by one frame | `C-ui-typing-latency` | M2-21 |
| `ui_text_shaping_cached` | redraw of 1,000 unchanged labels = 0 shape calls; changing one label = 1 | shaping cache disabled | `C-ui-shaping-cache` | M2-19 |
| `ui_single_glyph_atlas` | gallery in all three themes at scale 1.0 and 2.0, plus an emoji and CJK page → exactly 1 glyph atlas (1 `R8Unorm` mask plane + 1 `Rgba8UnormSrgb` colour plane, one bind group), atlas VRAM ≤ 32 MB, and a frame whose glyph working set exceeds the capped mask plane draws every glyph with one extra draw call per atlas epoch (§21.7) | one atlas per font; an RGBA8 mask plane | `C-ui-glyph-atlas` | M2-19 |
| `ui_draw_calls_batched` | 1,000-button panel ≤ 4 draw calls; a full editor frame ≤ clip changes + viewport composites + 1 | batching disabled (one draw per primitive) | `C-ui-draw-batching` | M2-19 |
| `ui_graph_2k_nodes` (WP-U8) | 2,000-node graph: pan/zoom frame ≤ 4.0 ms p95 on the reference machine; only on-screen nodes re-record on pan | all nodes re-recorded per pan frame | `C-ui-graph-budget` | M2-46 |
| `ui_startup_budget` (WP-U12, `tools/forge-editor-bin/tests`) | the real `forge-editor` binary, spawned, to its first interactive frame (`UiApp::settled`: every window drawn, nothing left to do) ≤ 1.5 s on the reference machine: one warm-up launch, the median of three, timed alone; measured 1.20 s (RTX 3080) | an injected 2 s stall in startup (`FORGE_STARTUP_STALL_MS`, a test build's `controls` only) | `C-ui-startup` | M2-30 |
| `ui_idle_zero_redraw_with_every_panel_open` (WP-U12, `tools/forge-editor-bin/tests`) | the editor with every first-party panel open — all visible side by side, and again as tabs (one visible) — a text field focused and its caret expired: 0 frames and 0 wakeups over 10 s | a panel that always animates | `C-ui-idle-every-panel` | M2-20 |

## 21.23 Guards

| Guard | Asserts | Positive control (must fail) | Where |
|---|---|---|---|
| `test_ui_panel_inventory`, backlog half | with `FORGE_BACKLOG` set, every §21.24 WP exists in `backlog.json` and every backlog clause is in its WP's backlog scope | a backlog clause the backlog scope does not contain; a WP the backlog does not define (both run on every `cargo test` against a backlog built from the plan) | gate row `C-ui-panel-backlog`: **AWAITING**, because `backlog.json` is outside the repository and `just verify` and CI do not set `FORGE_BACKLOG`; run with `just backlog-check <path>` |
| `test_command_liveness` (I7) | every UI handler in `forge-editor` and `plugins/forge-panels-*` that reaches a project-state mutation ends in `CommandEmitter` → `CommandSink::apply`, resolved through the import graph (W3); session/user-config writers only via the listed allow-list | a panel handler calling `ProjectStore::write` or a `&mut World` API directly | `tests/liveness/test_command_liveness.rs` (WP-U4) |
| `test_ui_wgpu_confined` (D-3, D-1) | no `wgpu` path outside `forge-ui/src/render_wgpu/`; `egui` is absent from the resolved graph | `use wgpu` added under `forge-ui/src/layout/` | `crates/forge-ui/tests` (WP-U1) |
| `test_game_ui_links_no_editor` | `forge-runtime`'s closure contains no `forge-editor`, no `forge-panels-*`, no `forge-licence` | a synthetic runtime graph depending on `forge-editor` | `tests/liveness/test_game_ui_links_no_editor.rs` (WP-U11) |
| `test_every_panel_under_every_preset` (I15) | every registered panel opens under 2D and 3D | a preset-gated panel descriptor | `tests/preset/` (WP-U3) |
| `test_panel_extension_replaceable` (I16) | `EditorPanel`, `InspectorWidget`, `ViewportTool`, `Theme`, `Action` support add/replace/remove/chain, exercised from a test plugin | a registry that ignores `replace` | `tests/plugin/` (WP-U3) |
| `test_ui_a11y_tree` | roles and names on every interactive widget; a11y focus = UI focus | an unlabelled icon button | `crates/forge-ui/tests` (WP-U1, audited WP-U12) |
| `test_ui_contrast` | every used token pair meets its theme's floor (§21.8) | a theme with one low-contrast pair | `crates/forge-ui/tests` (WP-U1) |
| `test_ui_focus_traversal`, `test_ui_ime_composition` | §21.9 | a widget skipped by Tab; pre-edit text committed early | `crates/forge-ui/tests` (WP-U1) |
| `test_gesture_single_undo` | a 60-frame slider drag is one undo entry; Esc cancels | one command per frame without a txn | `crates/forge-editor/tests` (WP-U4) |
| `test_optimistic_reconciliation` (O-13) | a forced divergence converges in one frame | overlay never dropped | `crates/forge-remote/tests` (WP-17 / M2-16) |
| `test_layout_roundtrip`, `test_layout_unknown_panel_kept` | layouts round-trip; unknown panels survive | a serializer dropping unknown ids | `crates/forge-editor/tests` (WP-U3) |
| `test_keybinding_conflicts` | overlapping chords are reported with both names | last-wins map | `crates/forge-editor/tests` (WP-U3) |
| `test_lossy_warnings_present` | every lossy promotion arrow (Ch.31 §31.4) shows a confirmation naming the loss | the warning removed from one path | `plugins/forge-panels-project/tests` (WP-U7) |
| `test_pseudo_locale_no_hardcoded_strings` | under the pseudo-locale every visible string in every panel and in the chrome (read from the AccessKit tree: label, value, description, placeholder, of every visible widget and of the virtual rows a tree, list or grid sends) was looked up, in two passes: an empty project, and a populated one in the editor as the binary wires it (entities with every built-in component, a selection, an imported asset, a team, every first-run tip, a refusal's toast and notification); data (numbers, identifiers, unit symbols, a short reasoned list of project values, an error's own message filled into a looked-up template) a short reasoned per-panel list (the in-memory animation library's motion names in `forge.anim_graph`) and text a build or a record carries verbatim (`Label::verbatim`, `RowItem::verbatim`: an audit record's greppable line) are exempt | one literal string in a panel; one literal row in a virtualised list (`C-pseudo-locale-rows`) | `tools/forge-editor-bin/tests/test_editor_panels_audit.rs` (WP-U12: the real first-party plugin set, ADR 0046 §6, §7) |
| `test_ui_strings_are_keys` (WP-U12) | every worded string literal of the editor's UI code (first-party panel plugins, the shell's UI modules, forge-ui's widgets and dock views) and of forge-editor's presentational model methods is a `tr!`/`trf!`/`tr_str`/`tr_key!` key or carries an `// l10n:` note naming why it is data: the code paths the pseudo-locale passes do not reach (refusals, warnings, history labels) | one hard-coded label, a formatted template, a one-word template | `tools/forge-editor-bin/tests/test_ui_strings_are_keys.rs` (`C-ui-strings-source`) |
| `test_panel_empty_states` | every registered panel, opened on an empty project, shows its declared empty state (text + at most one action) built from the catalogue widget, or declared it always has content (`PanelCx::never_empty`) and shows some; a dismissed first-run tip schedules no frame (the tips check) | a panel that renders a blank area when empty; a never-empty panel showing nothing; a tip that keeps a timer after dismissal | `tools/forge-editor-bin/tests/test_editor_panels_audit.rs` (WP-U12) |
| editor-wide a11y, contrast, keyboard audits (WP-U12) | every focusable widget of every panel has a role, a name and Focus in the AccessKit tree; every `(fg, bg)` pair the panels paint meets its theme floor in all three themes; Tab reaches every focusable widget of every panel, each stop named (recorded in `docs/evidence/keyboard-walkthrough.md`); each audit runs on the empty project and on the populated one (with the first-run tips), and on the populated one every panel has at least one Tab stop | an unlabelled icon button; a low-contrast token; a focus trap | `tools/forge-editor-bin/tests/test_editor_panels_audit.rs` (`C-editor-a11y`, `C-editor-contrast`, `C-keyboard-walkthrough`) |
| display-list goldens | the gallery's display lists hash to committed goldens on every leg | a one-pixel padding change | `crates/forge-ui/tests` (WP-U1) |

The D-5 rows are listed in §21.22. Each is a `C-ui-*` row in `tests/gates.ron`, and
`test_ui_panel_inventory` fails if one is missing.

## 21.24 Work packages

This table mirrors the orchestrator backlog (`backlog.json`) and names no WP the backlog
does not define. A WP claims a panel's DoD id only when its backlog scope names the panel
(the inventory's backlog clause, §21.21).

| WP | Scope | DoD |
|---|---|---|
| WP-U0 | ADR 0001; this chapter; the UI DoD table; `test_ui_panel_inventory` | M2-18 |
| WP-U1 | `forge-ui` core §21.3–§21.15: tree, ids, signals, taffy, text + two-plane atlas, themes, input/focus/IME/clipboard, AccessKit, damage and live feeds, `UiRenderer` + `wgpu` + recording renderer, `winit` runner, DPI; `ui_gallery` | M2-19..M2-23 |
| WP-U2 | the §21.16 catalogue, virtualisation §21.12 | M2-24, M2-25 |
| WP-U3 | docking, layouts as data, the `KeyMap` (rebindable, conflict detection, per-context), command palette, extension points §21.17, §21.19 | M2-26, M2-27, M2-45 |
| WP-U4 | the shell §21.18: bus client, mirror, gestures, undo history, notifications, settings window and theme switch, keybindings editor (on U3's `KeyMap`); `test_command_liveness` | M2-28, M2-41..M2-44 |
| WP-U5 | hierarchy, inspector, asset browser, console | M2-35, M2-37..M2-39 |
| WP-U6 | viewport, play controls, profiler | M2-32, M2-33, M2-40 |
| WP-U7 | launcher/new project, presets and promotion, the Project page of the settings window (project settings), revision history, build & export | M2-47, M2-48, M2-50, M2-51 |
| WP-U8 | graph editor | M2-46 |
| WP-U9 | plugin manager, audit log, remote connect; the security-class commands of §21.18, including the default automation capability policy row it adds to the settings window; compute & farm | M2-52, M2-54..M2-56 |
| WP-U10 | team, sandbox and Live/Pull, presence, scoped ownership claims, publish queue, conflicts, licence status | M2-57..M2-62, M2-69 |
| WP-U11 | sequencer, animation state machine, localisation, game UI in `forge-runtime` | M2-29, M2-63..M2-65 |
| WP-U13 | domain editors: 2D tile palette / sprite sheet / 2D rig, audio mixer, input action map | M2-66..M2-68 |
| WP-65 | the input runtime `forge-input` (Ch.28 §28.9–§28.18) and its input debugger panel in the editor | M7-12 |
| WP-U12 | a11y and contrast audit, pseudo-locale audit, the UI perf gate in `just verify`, onboarding/first-run tips, consistent empty states and error messages, icon set, docs for building tools on `forge-ui` | M2-30, M2-31, M2-70 |

**Ownership decisions.** Where the backlog splits one feature across two WPs, the split is
stated so neither implementer skips it:

- **Keybindings.** WP-U3 builds the `KeyMap` engine and `test_keybinding_conflicts`.
  WP-U4 builds the keybindings editor panel (M2-44), as the backlog says.
- **Settings and theme switch.** WP-U1 builds the themes (M2-23). WP-U4 builds the
  settings window and the theme switch in it (M2-43).
- **Command palette.** WP-U3 builds it (M2-45). WP-U4 hosts it in the main window.
- **Project settings.** Both WP-U4 ("settings window (editor + project settings generated
  from reflection)") and WP-U7 ("project settings") name them. WP-U4 builds the settings
  window, its editor page, and the generic rendering of reflected project settings with
  edits as commands (M2-43). WP-U7 builds the **Project** page's content, the reflected
  `ProjectLifecycleSettings` type (name, linked remote, store backend, and a read-only
  preset row that opens its dialog), and its tests (M2-47).
- **Scoped ownership claims.** WP-U10 builds the claims panel and the claim/release
  commands' UI (M2-69). The hierarchy, asset browser and viewport show the holder's
  badge through the presence/claims stream that WP-U10 provides; WP-U5 and WP-U6 do not
  build claim logic.
- **Empty states, errors and first-run tips.** WP-U2 builds the empty-state widget
  (M2-24). Each panel's WP declares that panel's empty state. WP-U12 audits every panel
  against the rules and owns `test_panel_empty_states` and the tips system (M2-70).

**Backlog gaps.** None. A panel added later that no backlog scope names is `UNSCHEDULED` and
gets a gap row here with the backlog change it needs; M2 cannot close while any gap row
remains (W9).

| Gap | Panels | DoD | Proposed backlog change |
|---|---|---|---|

**Order.** U1 → U2 → U3 → U4 → {U5, U6, U7, U8, U9, U10, U11} in any order → U12.
U12 comes last because it audits everything. WP-U13 (added for gap G-5) runs after U4
and before U12.

## 21.25 What is deliberately not built

| Not building | Why | Instead |
|---|---|---|
| an `egui` stopgap | ADR 0001 | `forge-ui` from the first commit |
| an immediate-mode API | redraws and re-runs UI code every frame (D-5) | signals + retained tree |
| a webview editor | memory, weak native IME/a11y on Linux, cannot be game UI | native `forge-ui` |
| LCD subpixel text AA | wrong on OLED/RGBW panels, transparency and transforms | greyscale AA with subpixel positioning |
| drag-out to the OS | `winit` has no API | in-app DnD and OS drop-in; revisit when `winit` gains it |
| concurrent same-property editing UI | not promised (E-37) | presence, scoped ownership, the conflicts panel |
| a UI that bypasses the bus "just for this one panel" | I7 | `CommandEmitter`; a panel with no command for its edit has found a missing command, which is added to `forge-cmd` |

---

# Chapter 24 — Blueprints: Graph, IR, Codegen

## 24.1 Blueprints *are* Rust

**Ruling: a graph compiles. Nothing interprets a graph at runtime (I10).**

```
   visual graph  ─┐
   Rust source   ─┼─→  typed IR  ─→  Rust codegen  ─→  wasm (iterate) │ native (ship)
```

Three authoring surfaces, **one semantics**. Why this and not a VM:

- UE's Blueprint VM is roughly an order of magnitude slower than C++, and "port your
  Blueprints to C++ before shipping" is a real, well-known, expensive migration. Designing
  that tax in from the start would be choosing to inherit a known defect.
- A VM means two sets of semantics that must agree forever. They will not.
- The generated Rust is **readable and editable**, and for the subset that maps back
  cleanly, graph↔text is a round trip. That is the feature Blueprint users actually want
  and have never had.
- The same IR is the material graph (Ch.10) and the PCG graph (Ch.14). **One graph
  compiler, every user.**

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

## 24.4 Implementation: `forge-graph` (WP-23)

| Module | What it is |
|---|---|
| `model` | `Graph` (name, `GraphKind` — blueprint / material / generator / PCG, nodes by id), `Node` (op, input pin → wire `Source { node, pin }`, input pin → literal used while unwired), `REROUTE` |
| `ir` | `compile(graph, &dyn NodeSource) -> Compiled`: every wire type- and unit-checked through `forge_reflect::connect`, reroutes resolved, cycles, unknown ops, kind rules (a mutating op is blueprint-only), unconnected inputs; `IrOp`s in dependency order (ties by node id); readable Rust that calls the nodes' functions; `Diagnostic { code, node, pin, message }` with `GRAPH-0001..0008`. `parse_text` reads the mappable subset of that Rust back (graph ↔ text) |
| `library` | `Signatures`: a `NodeSource` over `#[forge_api]` registries for authors other than the editor (the editor's `NodeLibrary`, with UI metadata, implements `NodeSource` itself) |

The editor's `forge_editor::graph::ir::ForgeGraphIr` converts its document to a `Graph` and
compiles through this crate; the D-4 `StubIr` is gone. **Not built yet** (Spike S3, M5-4):
building a blueprint's generated Rust into wasm (iterate) or native (ship) and loading it —
gate rows `C-graph-ir-backend` and I10 stay UNBUILT for exactly that.

---

# Chapter 26 — Heterogeneous Compute & the Distributed Farm

> "Many devs are using cheaper used gear and being able to combine them would be amazing
> for all of us." This chapter is that. It is split into three tiers because **they have
> wildly different difficulty and payoff, and conflating them is how the feature dies.**

## Tier 0 — Heterogeneous compute offload  ·  *real, large, do it first*

Dispatch **batch GPU work across every adapter in the machine**, including the iGPU and a
five-year-old card in the second slot.

Eligible work — all of it embarrassingly parallel and none of it latency-critical:
PCG density evaluation, lightmap and probe bake, texture transcode, mesh simplification and
meshlet building, path-traced reference renders.

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
| `just verify-linux` | `just verify` (or a given command) on the ubuntu-x86_64 leg, locally, in the pinned Docker image (§30.6, ADR 0044) | observes the Linux leg; the remote run still waits on the push |
| `just gate` | invariant rows: bound / awaiting / unbuilt(reason) | a row is a test file |
| `just dod` | DoD items: settled / blocked / superseded / **UNMET** | 0 UNMET, 0 unread to ship a milestone |
| `just plan-coverage` | every directory claimed by a chapter | fails on an orphan |

**W4 is load-bearing here.** A local gate that is a subset of the remote gate produces
confidence exactly proportional to what it skips. a previous project had three CI-red
causes living in precisely that diff — a formatter check that ran remotely and not
locally, a stale lint cache that made the local check *lie*, and an e2e leg that
`just check` never ran. `xtask gate-parity` (I12) asserts the local list is a superset.

`tests/e2e` is a **serial leg**, run alone. A background e2e run competing with another
suite fails scenarios for load reasons, which then get misdiagnosed as engine bugs.

## 30.2 The liveness family

Four guards, each answering a question the others do not. a previous project shipped
defects that each of the first three missed in turn, so each carries its own hard-won
refinement:

| Guard | Asks | Refinement it needed |
|---|---|---|
| `test_caller_liveness` | does this module have a production caller? | **must resolve through the import graph, following re-exports and normalising spelling — a name match is not a reference (W3).** Non-Python… non-Rust entry points read from `[[bin]]`, `main.rs`, and shell scripts |
| `test_seam_liveness` | is this seam actually *reached*, or does `None`/`Default` satisfy every question? | a seam satisfied by a default value is dead while looking alive |
| `test_config_liveness` | does any code read this config key? | same import-graph rule |
| `test_command_liveness` | does every UI mutation go through the bus? (I7) | new to this project; see Ch.7 |

**And the question a previous project added last, which is the subtlest:** a module can
fail liveness not because it lacks a *caller* but because it lacks **an input that exists
on disk**. When triaging a liveness failure, the standing question is now: *is this
blocked on hardware, on a file nobody wrote, or on an input that does not exist?*

## 30.3 Determinism, perf, and the positive-control rule

- Determinism: Ch.3 §3.4, three-platform matrix, golden hashes, mutation control.
- Perf: named budgets per subsystem (Ch.29), CI fails on regression beyond a stated band.
- **Every guard in this section has a non-vacuous positive control that runs in CI (W2).**
  A guard whose positive control names something that does not exist is worse than no
  guard, because it reports green.

## 30.4 A note on flakiness

**Never widen a tolerance or add a retry to make a scenario pass (W5).** Find the cause.
Flakiness is triggered by load and never caused by it — and a "control run" that held the
commit fixed while letting machine load vary is not a control (W6). A harness's patience
with the OS scheduler may be raised, with the reasoning written at the constant; an
assertion about the *engine* may not.

## 30.5 The M0 exit binary and the M0 platform guards (WP-07, ADR 0014)

**`tests/platform/test_asset_ref_case.rs`** (M0-17, `C-asset-ref-case`): every string literal
in Rust source (macro arguments included) and every quoted string in TOML/RON/JSON/YAML is
resolved against its file's directory, crate root and the workspace root by listing
directories and comparing names exactly; a reference that resolves only case-insensitively,
or only with `\` separators, fails on every platform, as do two directory entries that differ
only by case. (`xtask gate` accepted a wrong-case gate-row path on Windows; this lint does not.)

**`tests/licence/test_no_runtime_phone_home.rs`** (I21, M0-19): `forge-runtime`'s closure from
`cargo metadata`, with the features `cargo tree -p forge-runtime` enables (the runtime's own
build, not the workspace union), must contain no licensing crate, no network or telemetry
crate, no networking feature (`tokio/net`, Win32 networking), no registry source naming a
socket type, and no local source reaching `std::net` sockets (resolved through `use` trees
and aliases), except transports allow-listed with a reason in `tests/licence/net_allow.txt`
(empty at M0). `forge-num`'s `mutate-det` can never ship: the graph check flags it, and
`forge-runtime` fails to compile with it (`C-runtime-no-mutate-det`). Positive controls: every
rule on synthetic graphs, and real scratch workspaces (the runtime plus `forge-licence`, or
plus a socket-opening crate) resolved by cargo.

## 30.6 The Linux leg observed locally: `just verify-linux` (WP-30, ADR 0044)

There is no Linux machine; Docker Desktop (WSL2 backend) is. `just verify-linux [command…]`
runs `just verify` — or the given command — in the pinned image
`tools/docker/linux-verify/Dockerfile` (`rust:1.98.1-bookworm@sha256:93ce27a8…`, plus
lavapipe, Xvfb, the libraries winit dlopens and the Windows box's tool versions) on a
snapshot of the current worktree: the tree `git add -A` would commit, with **LF** endings as
CI's checkout has them. The host's crate cache is mounted (never re-downloaded; the run is
offline after a fetch that can only add crates the host never had, and prints the cache
count before and after); `CARGO_TARGET_DIR`, the snapshot and `CARGO_HOME` live in one
persistent volume, `forge-linux-target`, so rebuilds are incremental. `CI=true` is set, as on
GitHub's runners, so a GPU guard without an adapter fails instead of reporting AWAITING. The
container is capped at 6 CPUs / 12 GB so the Windows lanes keep half the machine; its
timed-alone lock does not cross the VM boundary, so no Windows wall-clock gate runs while it
builds. Before each run the toolchain's bytes are checked against the image build (the VM's
page cache once served a corrupted `librustc_driver.so`).

What it observes and what it does not: the Linux build, every test and gate on
x86_64-unknown-linux-gnu, and GPU work on **lavapipe (software Vulkan)** — not a hardware
Linux GPU. Wall-clock budgets run for correctness there and are not gated or tuned for a VM
sharing the dev box; they are gated on the reference machine. It is a local observation: the
remote CI run (M0-1, M0-16) still waits on the owner's push. Evidence of each observed row
is committed under `docs/evidence/linux/` (image digest, command, date, output, hashes).

---

# Chapter 31 — Workspace Presets: 2D and 3D

## 31.1 The anti-pattern this must avoid

Unity's URP/HDRP choice is made at project creation, is painful to reverse, splits the
asset ecosystem, splits the documentation, and invalidates half of every tutorial. It is
the single most-complained-about structural decision in that engine.

> **I15: A preset sets defaults. It never gates capability.**
> Every `#[forge_api]` item is reachable under every preset. Any project can be promoted
> to any preset. A preset is **data, not code**.

## 31.2 The presets, and the honest shape of the difference

**2D is the one genuinely different kind**, because it has its own render path and its own
physics solver. That is a real cost and Chapter 35 pays it deliberately.

## 31.3 What a preset actually is

`presets/3d/workspace.ron` — a manifest naming: the default plugin set, the default
panel layout, default project settings, the new-scene template, and which extension-point
defaults are installed. Nothing else. **Users author their own presets by copying one**,
which is the "any feature modifiable" principle applied to the editor's own shape.

## 31.4 Promotion

| From → To | Cost |
|---|---|
| 2D → 3D | additive: the 2D content keeps working as a 2D layer in a 3D project |
| 3D → 2D | narrowing, lossy, and the UI says so |

**Guards:** `test_no_preset_gating` (I15) asserts every `#[forge_api]` item resolves under
every preset — with a positive control that adds a preset-gated item and must fail.
`test_preset_promotion` round-trips a project through each arrow and asserts no loss where
none is expected.

## 31.5 The 2D tax must be zero

The reason unified engines lose 2D developers is that 2D projects pay for 3D machinery
they never use — build size, load time, editor complexity, irrelevant settings.

## 31.6 Implementation (WP-16, ADR 0035)

M2-12 as built. The three presets are the `forge.presets` plugin's data (`presets/<dir>/`,
compiled in, ADR 0033):

| File | Holds |
|---|---|
| `layout.ron`, `keymap.ron` | the default dock layout and keymap layer |

The default plugin set is what a project *loads by default*, never a filter: every installed
plugin loads under every preset (the I15 positive control loads only the list and loses the 2D
editors under 3D).

**A project's own presets.** The host reads them at every load (`OpenProjectInfo::presets`);
`forge_editor::presets::PresetSet::with_project` puts each loadable copy of the right family
in place of the built-in one and names any other (`problems`; the built-in stays — a copy
can never take a capability away). The core's promote planner (`promote::promote_handler`,
over the core's shared defaults table) and the preset switcher plan with the copies: a
switch applies the copy's defaults, including keys no built-in preset has, and keeps every
value a user set. Not applied yet: a copy's layout and keymap when the project opens.

**Guards.**

| Test | Gate row | Positive control |
|---|---|---|
| `tests/preset/test_no_preset_gating.rs` | `I15` | `positive_control_a_loader_that_loads_only_the_preset_plugins_fails`; `positive_control_the_real_brush_gated_in_2d_before_its_argument_check_fails`; `positive_control_the_real_brush_gated_in_2d_after_its_argument_check_fails` |
| `tests/preset/test_preset_promotion.rs` | `C-preset-promotion` | `positive_control_a_promotion_that_loses_data_is_caught`; `positive_control_a_lossy_switch_without_confirmation_is_caught` |
| `plugins/forge-panels-project/tests/test_project_presets.rs` | `C-project-presets-from-folder` | `positive_control_a_core_that_ignores_the_projects_presets_fails` |

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
engine subsystems have no privileged path to the engine.

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
importers · exporters · render passes · material nodes · graph nodes · PCG rules · physics backends · navmesh builders ·
inspector widgets · editor panels · **commands** · asset types · **store backends** ·
**presets** · script hosts · platform backends · localisers · input devices ·
profilers.

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
| Right for | marketplace, untrusted, tools, gameplay | engine subsystems, perf-critical, low-level |

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

Capabilities use **one grant mechanism** for plugins and remote sessions, so there is one
thing to audit rather than two.
`net`, `fs(write)`, `process`, and `command(destructive)` are never default-granted.

Conflicting `replaces` between two plugins is a **load error with both names printed**,
never a silent last-wins.

## 32.5 The registry

An open **index**, not a storefront: a signed manifest list, self-hostable, with
`forge add <id>` resolving versions. No revenue share, no curation gate, no account.

## 32.6 Implementation (WP-06, ADR 0011)

`crates/forge-plugin`. Amendments to §32.2's sketch, made before the M0-20 freeze:

- `ExtensionPoint` also carries `const NAME` (the manifest spelling, `EditorPanel`).
- The acting plugin is explicit on every operation, so provenance and conflicts are exact:
  `add(owner, key, item, order) -> Result<ItemId>`, `replace(by, target, item) ->
  Result<Replaced>`, `remove(by, target) -> Result<Removed>`, `chain(by, target, wrap:
  impl FnOnce(Item) -> Item) -> Result<()>`. `Order` is `First | Last | Before(key) |
  After(key)`, resolved at insertion.
- `ItemId` is `(point id, key)`; `PluginId` is lowercase dot-separated (`com.example.rivers`;
  first-party ids start `forge.`). `Registry::provenance(key)` reports owner, replacer and
  chain.
- A second plugin replacing an already-replaced item, or removing another plugin's
  replacement, is `PLUGIN-0006` naming both. A panicking `chain` wrapper is caught
  (`PLUGIN-0013`) and its entry removed.
- `Extensions` holds one registry per point; an id or name claimed by two item types is
  `PLUGIN-0012`.
- Manifests add optional `removes: [...]` and `chains: [...]`. The loader (`loader::load`)
  checks ids, the `engine` requirement against `KERNEL_VERSION` (0.1.0) and point names,
  then finds every conflict **from the manifests alone** (two providers `PLUGIN-0003`; two
  replacers, replace vs remove, remove vs chain `PLUGIN-0006`; two removers agree), WASM
  manifests included, before any plugin code runs. Installs queue declared operations only
  (`PLUGIN-0011` otherwise), applied by phase (add, replace, remove, chain) then plugin id, so
  the result never depends on discovery order. WASM plugins given to `load` without a host are
  reported not installed (`PLUGIN-0014`); `load_hosted` installs them (§32.7).
- Capabilities (E-27): `Capability::{Fs(ProjectRead|ProjectWrite|UserRead|UserWrite),
  Gpu(Compute|Render), Net(Outbound|Listen), Process, Command(Ordinary|Destructive)}`,
  `Principal::{Plugin, Remote, Role}`, one `Grants` table, nothing granted by default,
  and a `DefaultPolicy` that refuses the never-default set. The load report lists requested
  vs granted per plugin.
- Points defined here (the rest keep their catalog ids in `points::SEED_POINTS` until their
  crates land): `Command` (an `Invoke` handler; `install_commands` registers the resolved
  registry on a `Bus`), `Preset`, and `EditorPanel<Cx>` / `InspectorWidget<Cx>`, generic over
  the editor's panel context so `forge-editor` keeps full typing. `StoreBackend` is defined in
  `forge-store`.

**Guards.** `tests/plugin/test_extension_point_replaceable.rs` (`C-extension-point-
replaceable`, bound): `conformance::check_replaceable` on every defined point, observing
items by using them (panels built, commands dry-run on a real bus, backends opened), plus
Ch.21.19's `test_panel_extension_replaceable` through the loader; positive control: six
broken registries, each caught by its clause on every point, and a faithful model passes.
`crates/forge-plugin/tests/test_loader.rs` (`C-plugin-conflicts`, bound). I16 is bound by WP-15
(§32.7).

## 32.7 Implementation: plugin system v1 (WP-15, ADR 0033)

M2-13 and M2-14 as built.

**The WASM host is `crates/forge-wasm`**, not a module of `forge-plugin`: every crate links
`forge-plugin`, and only hosts (the editor, `forge`) should compile wasmtime (Apache-2.0 WITH
LLVM-exception; no default features: Cranelift, the component model, `wat` text). It joins
the loader through an additive trait: `forge_plugin::HostedPlugin` (a `kind: Wasm` manifest
and an `install` over the same `InstallCx`) and `loader::load_hosted(ext, sources, hosted,
wasm, grants)` — one manifest check, one conflict check across source and WASM plugins, one
install order. `PLUGIN-0016` carries a host's refusal (`WASM-*`).

- **The world** (`crates/forge-wasm/wit/forge-plugin.wit`, `forge:plugin@0.1.0`): a plugin
  exports one function, `call(point, key, input: list<u8>) -> result<list<u8>, string>`,
  serving every item its manifest declares; it imports only `host` (`log`, no capability)
  and `project-read` (`read`, `Fs(ProjectRead)`, through the host's `ProjectStore` reader,
  I17). No WASI is linked: a component importing anything else does not load (`WASM-0003`).
- **Capabilities, twice (E-27).** At load, an import links only if the manifest requests its
  capability (`WASM-0002`). At each call, the host checks the **shared** grant table,
  `forge_plugin::SharedGrants` (an `Arc<RwLock<Grants>>` handle; a poisoned table denies): a
  grant or revoke by any holder takes effect on the next call, and every denial is a
  `HostEvent::Denied`.
- **Adapters** turn guest items into point items (`WasmHost::adapt` / `adapt_chain`).
  Built in: `Command` — the guest *plans* (`{"args", "chained"}` in, `{"ops": [spawn,
  despawn, rename, reparent, set_property, remove_property, set_setting]}` out, `"$N"` names
  the plan's N-th spawn) and the host replays the ops onto the command's `DiffBuilder`, so a
  WASM command is undoable, dry-runnable and provenance-tagged (I7, I8); every plan needs
  `Command(Ordinary)`, a plan with a despawn, a property removal or a cleared setting also
  `Command(Destructive)`, checked when planned (`CMD-0014` otherwise) — and `Preset` (RON,
  read at install). A point with no adapter is a named load error (`WASM-0007`).
- **Bounded.** Fuel per call (default 2·10⁹) and a memory cap per instance (256 MiB). A trap
  (`WASM-0005`) discards the instance; the next call instantiates afresh.
- **Hot reload.** `WasmPlugin::reload` compiles, checks and instantiates new code outside
  the lock, then swaps it under every installed item (`GuestItem`s share the slot).
  `PluginWatcher::poll` stats `plugin.ron` and `plugin.wasm`/`plugin.wat` (no thread, no
  read while quiet), reloads changed code, keeps the old code on a failed compile, ignores a
  rewrite of identical bytes, and answers a changed manifest with `WASM-0011` (reload the
  set through the loader: declarations never change under the conflict check). Source
  plugins reload by an incremental rebuild and restart; the dev-dylib path of §32.3 is
  UNBUILT (`C-source-plugin-hot-reload`: unsafe FFI across an unstable ABI).
- **Authoring in text.** `forge_wasm::wat::component` wraps a core module in the world's
  component boilerplate (canonical-ABI lifts and lowers, a bump allocator reset by
  `post-return`), so a plugin — and every WASM fixture — is one readable core module.

**First-party subsystems moved into `plugins/` (M2-14),** each on the kernel's public API
only and loaded through the ordinary loader:

| Plugin | Id | Provides | Moved out of |
|---|---|---|---|
| `plugins/forge-importers` | `forge.importers` | `Importer("gltf")`, `Importer("image")`, `Importer("ktx2")`, `Exporter("gltf")` | `forge-asset` (the glTF, PNG/JPEG and KTX2 code and its `gltf` / `base64` / JPEG dependencies); `forge-asset` keeps the points, the asset types (`forge.asset`) and `texture_from_rgba8`, which importers share |
| `plugins/forge-presets` | `forge.presets` | `Preset("forge.preset.2d")`, `.3d`, each carrying its `presets/` files (`PresetDescriptor::files`) | `forge-editor`'s compiled-in preset table; the editor builds presets from the `Preset` registry (`presets_from_registry`) |
| `plugins/forge-store-backends` | `forge.store` | `StoreBackend("file")`, `StoreBackend("memory")` (and since WP-16 `git`, `https`, `http`, `git-file`: Ch.33 §33.7) | `forge-store`'s `FirstPartyStores`; `forge-store` keeps the point, `open_store`, and the `ProjectStore` implementations other kernel code uses as values |

`AssetServer::default_extensions()` now holds the asset types only;
`forge_importers::asset_extensions()` is the standard set, and `AssetServer::open_memory`
takes the extensions explicitly. The editor library's convenience constructors load the
importer and preset plugins through the loader (the dependency points from host to plugin).

**Guards.**

| Test | Gate row | Positive control |
|---|---|---|
| `tests/plugin/test_no_privileged_plugin.rs` | `I16` | `positive_control_each_back_door_is_caught`: a real plugin's source with a `#[doc(hidden)]` kernel item, a `pub(crate)` item, a private module reached through an alias, `#[path]` into kernel source, `include!` of kernel source, a minted `PluginId`, `registry_mut`, and a privileged kernel feature injected one at a time — each must be named |
| (same file) | `C-plugin-no-privilege-by-id` | `positive_control_a_plugin_privileged_by_its_id_is_caught`: every first-party plugin installs the same registry under a third-party id; a plugin that installs more as `forge.*` fails |
| `crates/forge-wasm/tests/test_wasm_capabilities.rs` | `C-wasm-capability-scoped` | `positive_control_a_host_that_skips_the_grant_check_is_caught` (and every denial is paired with the same call succeeding once granted) |
| `crates/forge-wasm/tests/test_wasm_hot_reload.rs` | `C-wasm-hot-reload` | `positive_control_without_a_poll_the_old_code_answers` |

The I16 scan resolves every path a `plugins/*` crate names into a kernel crate against that
crate's public surface (walked from its root through `pub` modules, `pub use` re-exports and
globs, variants and inherent methods; ~3,500 references resolved) and allows one reasoned
exception: the panel sets' W2 fault switches (`forge_editor::services::PanelFaults`, `pub`
but hidden from docs), which only degrade a panel. Field access and method calls are not
type-resolved: a `#[doc(hidden)]` method is caught by name when no public method shares it.

**S10, early numbers** (`crates/forge-wasm/tests/test_s10_call_overhead.rs`, release, dev
box): an empty call 228 ns; a call making one capability-checked host import 500 ns; a
4096-sample generator-node kernel (hashed noise into an `f64` buffer) 14.1 µs in WASM vs 4.8
µs native, **2.96×** with fuel metering on — the top of §32.3's 1.5–3× — and bit-identical
to the native output.

## 32.8 Implementation: plugin system v1.1 — WASM plugins in the editor (WP-21, ADR 0045)

**The editor hosts WASM plugins** (`crates/forge-editor/src/hosting.rs`). It looks in
`<user config>/plugins/<name>/` (drop-ins), the plugin cache `<user config>/plugins/<id>/
<version>/` for each plugin the open project's set enables at that version (the Plugin
manager's *Add*), and `<project folder>/plugins/<name>/`; a plugin the set disables loads from
nowhere. At start `shell::assemble_hosted` compiles them on a host over the core's one grant
table and loads them **with** the source plugins through `load_hosted` (one check, one install
order); a refused WASM plugin is left out with the reason and never stops the editor. The
editor's default set (`forge.editor`, `forge.asset`, `forge.importers`, `forge.presets`) is
loaded by `assemble` itself. A plugin **dropped in while the editor runs** installs on the
next loop turn the editor takes anyway (at most one poll a second; no thread, timer or watcher:
an idle editor makes zero wakeups): commands on the core and in the palette, presets in the
new-project flow, importers in the asset database, panels in the dock and the Window actions.
One that replaces, removes or chains waits for the next start (the loader's single install
order). `PluginWatcher` hot-reloads changed code in place; the Plugin manager shows the
generation.

**Presets come from the real registry.** `presets::PresetCatalog` (shared by the launcher and
the core as `SharedPresets`) lists every item of the editor's `Preset` registry; the launcher
shows the built-in templates, then each plugin preset; `Template::Preset(key)` (id
`preset:<key>`) creates a project from it. A defaults-only descriptor is its family's
built-in preset with its label and defaults on top. The built-ins are built once per process.

**Adapters.** `Importer` (`forge_wasm::importer`: describe once, binary-framed import,
dependencies by `needs` with `Fs(ProjectRead)`, version = declared version ⊕ FNV-1a of the
code) and `EditorPanel` (`forge_editor::viewspec`, §21.19's ViewSpec: headings, text, live
setting readouts, rows, buttons; bounded; a button runs only a command the same plugin
provides, so a panel can do nothing its plugin was not granted). `project-read` is not served
to editor-hosted plugins yet (UNBUILT: the store is the core's, whose lock a plugin command's
plan already holds); the call says so.

**The host assembles plugins** (`cargo xtask layering`, in `just verify` and CI). L1: a
`crates/*` crate names a `plugins/*` crate in `[dependencies]` or `[build-dependencies]`
(any target) only as a listed host edge — forge-editor → forge-importers, forge-presets (its
default set); forge-project → forge-store-backends (`ProjectHost::first_party`). L2:
dev-dependencies are free (tests assemble plugins as hosts do). L3: a plugin never depends on
`tools/*`. L4: a stale allowance is an error. `tools/*`, `xtask` and `tests` may depend on
anything.

**Guards.**

| Test | Gate row | Positive control |
|---|---|---|
| `crates/forge-editor/tests/test_wasm_plugins_in_editor.rs` | `C-wasm-plugins-in-editor` | `positive_control_an_editor_that_never_polls_installs_nothing` |
| (same file) | `C-wasm-plugins-no-idle-wakeups` | `positive_control_a_timer_poll_wakes_the_idle_editor` |
| `crates/forge-wasm/tests/test_wasm_importer.rs` | `C-wasm-importer` | `positive_control_an_ungranted_dependency_read_fails` |
| `plugins/forge-panels-project/tests/test_plugin_presets.rs` | `C-plugin-presets-new-project` | `positive_control_a_launcher_of_builtin_templates_misses_the_plugin_preset` |
| `xtask/src/layering.rs` | `C-layering-host-assembles-plugins` | `positive_control_each_layering_break_is_caught` |

**Project trust (WP-34, ADR 0045 Amendment 1).** A project's own plugins (its `plugins/`
folders and the cached plugins its set names) load, and its plugin grants take effect, only
once the person **trusts** it (`forge_editor::trust`). The answer is user config (`<user
config>/trusted-projects.ron`, keyed by the canonical project folder), never project state,
and never a bus command, so no script or remote client can give it. Until then the core's
load holds every plugin grant the project carries (against an empty project, as a proposal
marked `untrusted`; the files keep them on save), the hosting leaves the plugins out (*not
loaded: the project is not trusted* in the Plugin manager), and the shell asks once — a
notice that opens the Plugin manager's *Project trust* group (*Trust this project* / *Don't
trust*). Trusting accepts what a person's open held through the human-only
`forge.security.accept_held`; what a script opened stays held for its own look. A project a
person creates is trusted from the start; with no project open, nothing needs trust.
Headless trusts nothing unless launched with `--trust-project`. **A live install is all or
nothing**: every item staged and checked against the live registries and the bus, then
committed (presets, importers, then the commands, which the core takes all or none under its
lock), rolled back on any failure; its panels are staged against the shell's panel host too.

**Trust keyed by content (WP-35, ADR 0045 Amendment 2).** A trusted project that now carries
something new or changed (a `git pull`, another repository cloned into the folder, a team
pull) is *changed*: what the person trusted keeps running, a new or changed plugin is not
loaded (a loaded one's hot reload waits), a new grant is held against what was trusted, and
the person is asked again, shown what changed. Unchanged content, or less, never asks; the
person's own grant, policy or plugin-set change in the editor moves the answer with it.
`assemble_hosted` applies the same gate to a project already open when the editor assembles
(the start-up trust gate).

**What was vetted is what runs (WP-36, ADR 0045 Amendment 3).** A project plugin is compiled
only from the one read of its files (`forge_wasm::PluginFiles`) whose digest the gate approved,
and the hot-reload watcher shows the gate the exact bytes it would swap in: a pull landing
between the gate's look and the load cannot run unvetted code. The hosting keeps the gate's
answers against the core's trust version, so an idle poll takes no core lock per plugin, and
drops the digests of removed folders. A team pull into a trusted project folder is gated like a
`git pull` (now tested). The `--remote-host` console answers trust with `trust ID` /
`distrust ID` through the Plugin manager's audited call.

| Test | Gate row | Positive control |
|---|---|---|
| `crates/forge-editor/tests/test_project_trust.rs` | `C-project-trust` | `positive_control_both_run_the_projects_code_with_its_grants` (and one per layer) |
| (same file) | `C-project-trust-content` | `positive_control_trust_by_folder_runs_what_a_pull_brought` |
| (same file) | `C-project-trust-cached` | `positive_control_a_hosting_that_ignores_trust_runs_the_cached_plugin` |
| (same file) | `C-project-trust-startup` | `positive_control_trust_by_folder_at_startup_loads_changed_code` |
| (same file) | `C-project-trust-team-pull` | `positive_control_trust_by_folder_takes_what_a_team_pull_brought` |
| (same file) | `C-project-trust-read-once` | `positive_control_reading_again_after_the_check_runs_the_later_write` |
| (same file) | `C-project-trust-check-at-read` | `positive_control_unchecked_reads_run_the_swapped_code` |
| (same file) | `C-project-trust-idle-vets` | `positive_control_without_the_cache_every_poll_asks` |
| `crates/forge-wasm/tests/test_wasm_hot_reload.rs` | `C-wasm-vetted-reload` | `positive_control_a_watcher_that_reads_again_runs_the_later_write` |
| `crates/forge-remote/src/console.rs` | `C-console-trust` | `positive_control_a_console_that_trusts_by_the_book_fails` |
| `crates/forge-editor/tests/test_install_live_atomic.rs` | `C-install-live-atomic` | `positive_control_committing_as_it_goes_leaves_commands_installed` |

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
| **Generated content** — terrain, scatter | **not stored at all** — it has a `SeedPath` (Ch.4) | this is where generation from seeds pays off in the VCS layer: a world is a seed, not petabytes |

That third row is worth dwelling on. The only thing a repository ever holds is the seed, the
generator graph, and the **edit deltas** — which for 10,000 players over a year is tens of
gigabytes. **An engine whose worlds are functions has a version control story that an engine
whose worlds are files cannot have.**

## 33.3 Two things the big engines charge for

**Semantic merge.** Because project data is a reflect tree, a three-way merge happens on
the *tree*, not on text lines — "A moved the light, B changed its colour" merges cleanly
instead of conflicting on line 4,102. Unity and UE both struggle here and it is a common
reason studios buy Perforce.

**Asset locking.** For genuinely un-mergeable binaries, `lock()` is Perforce's actual
value proposition and costs almost nothing once a store exists. Expensive to retrofit.

## 33.4 The command log is the real history

The bus already records semantic, provenance-tagged edits (Ch.7). `commit()` takes the
envelopes, so history reads *"ada placed 37 trees along river_2"* rather than
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

## 33.6 Implementation (WP-06, ADR 0011)

`crates/forge-store`. The trait as built (a superset of §33.1's sketch, before the M0-20
freeze): `backend()`, `identity()`, `read`, `write` (atomic), `delete`, `list` (sorted),
`blob_get` (verified against its address), `blob_put`, `blob_has`, `commit(msg,
envelopes)`, `head`, `history(RevRange { to, stop_at, limit })`, `tree(rev)`,
`commands(rev)`, `lock(path)` (for the store's identity), `unlock`, `locks`. `Bytes` is
`bytes::Bytes`.

- **Revisions are content-addressed.** A commit snapshots the working files as blobs, stores
  the tree (`<hex> <path>` lines) and the command log (JSON lines) as blobs, and writes a
  RON record; `RevId` = BLAKE3 of (parent, tree, log, message) — not the time, not the
  backend — so the same project has the same revision ids everywhere.
- **Paths are portable.** `StorePath` refuses empty/`.`/`..` segments, `\ : * ? " < > |`,
  control characters, trailing dot or space, Windows device names, over-long paths and
  `.forge/`; a path differing from an existing file or directory only by case is
  `STORE-0007` on every backend.
- **Locks** (Ch.33.3): while held, only the owner writes or deletes the path; on `LocalFs`
  the lock file is created exclusively, so two editors on one folder cannot both take it.
- **Backends.** `LocalFs` (the project is a plain folder; store data in `.forge/`: blobs,
  revs, HEAD, locks, tmp; atomic writes; a size+mtime hash cache that distrusts same-tick
  rewrites) and `MemoryStore` (in-memory, labelled per D-4). Both are registered on the
  `StoreBackend` point by the first-party source plugin `forge.store` through the ordinary
  loader; `open_store(ext, "file:<dir>" | "memory:<name>", identity)` opens by scheme. S3 and
  SQL are `Unbuilt` (gate row `C-store-git-s3-sql`); the Git backend is §33.7 (WP-16).

**Guard.** `tests/store/test_store_backend_parity.rs` (I17, bound): one script (text and binary
files with CR/LF and NUL, two commits carrying real bus envelopes, a lock, deletes, an
uncommitted change) through both backends gives identical snapshots (files, revision ids,
trees, logs, locks); `parity::copy_project` round-trips memory → LocalFs → memory → LocalFs
through the trait alone with every revision id intact, under different clocks. Positive
control: CRLF translation, a dropped command log, an unsorted listing and a lock that does not
hold are each caught.

**Additive in WP-12 (ADR 0015):** `stamps()` — every working file with a cheap change token
(`Stamp`, opaque, per store instance), sorted. Default: the content hash (correct for any
backend). `MemoryStore`: its stored hash. `LocalFs`: size + mtime from the directory entries of
one walk (no per-file open or `stat` on Windows), and the content hash for a file still inside
its racy window (100 ms on sub-second filesystems, 2 s on whole-second ones; a coarse write
clock cannot hide a same-size rewrite), hashed at most until a read begins after the window
(ADR 0017). `stamps_of(paths)` — the same stamps for just those paths (default: filter
`stamps()`; `LocalFs`: one listing per parent directory). `size(path)` — a file's size (default:
read it; `LocalFs`: one `stat`; `MemoryStore`: the stored length). The asset system's hot reload polls
them (Ch.8 §8.4); no existing signature changed.

## 33.7 Implementation: the Git store (WP-16, ADR 0035)

M2-15 as built, in `plugins/forge-store-backends/src/git` (gitoxide for objects and refs,
MIT/Apache-2.0, pure Rust; our own smart-HTTP client, since gix has no push; `ureq` on rustls
with the operating system's trust store):

| Piece | What |
|---|---|
| Layout (`layout.rs`) | one Forge revision = one commit on `refs/heads/main`, a pure function of the revision (author and time from its record), so every replica has the same Git history. The tree: every text file (UTF-8, no NUL, ≤ 1 MiB, a path Git hosts accept) at its path — diffable in a PR — plus `.forge/revision.ron`, `.forge/tree`, `.forge/commands.jsonl`, and a generated `.gitignore` (`/.forge/` and every binary path; a project's own `.gitignore` wins) |
| Engine-native blobs | binary files live on `refs/forge/blobs` (one commit per revision that brought new ones, a tree of `<blake3>` entries): a plain `git clone` and PR diffs never carry them; a Forge clone fetches both refs. `blob_put` artefacts no revision holds stay local (derivable, never pushed). `forge-index` maps BLAKE3 → Git id (append-only, rebuilt from history when lost); every read is verified |
| `git:<folder>` | a project folder whose history is `<folder>/.forge/git` (bare): working files through `LocalFs`; a commit hashes only files whose stamps changed and writes only objects Git lacks |
| Remote views (`https:`, `http:`, `git-file:`) | a `ProjectStore` over a mirror in the per-user cache: opening fetches; working files are the remote head's, read lazily; commits go to the mirror; `publish` pushes with compare-and-swap on both refs (`atomic` when the server offers it) — a moved remote is `STORE-0011`, never overwritten |
| Transports | smart HTTP v0 (`smart.rs`: discovery, fetch streamed through side-band into gix-pack's indexer, push with `report-status`); a repository on a path (`git-file:`, objects copied, no `git` binary; an empty folder becomes a bare repository). SSH URLs are refused with their HTTPS form |
| GitHub (`github.rs`) | the OAuth device flow (one non-blocking poll per call) and private repository creation; credentials by host, in memory, never in a URL, a setting, the bus or a message |

forge-store gained, additively: the public kit (`forge_store::kit`: `RevRecord`,
`build_commit`, `walk_history`, the log codec, `CaseIndex`), so a backend plugin builds
identical revision ids (I16); `ProjectStore::commit_at` (a replay keeps each revision's time)
and `ProjectStore::publish` (a remote view pushes); `STORE-0010..0012`. `forge_project::sync`
replays with `commit_at` and publishes after a push; `check_remote` accepts Git URLs; the
lifecycle gained `forge.project.sign_in` and `forge.project.create_remote` (session commands)
and `git:` project locations. The history panel signs in (the code and where to enter it, then
Continue), creates the repository on GitHub and links it; the launcher clones any Git URL.

**Guards.** `tests/store/test_store_backend_parity.rs` (I17) runs LocalFs, memory, a `git:`
folder and a Git remote view through the same script, and round-trips memory → LocalFs → Git
→ LocalFs with ids intact. `plugins/forge-store-backends/tests/test_git_store.rs`
(`C-store-git-remote`: a racing push loses no revision; positive control
`positive_control_a_forced_push_loses_a_revision_and_is_caught`; the layout checked with
`git fsck --strict` and `git ls-tree`; the same Git ids on the host and in the project
folder). `test_git_smart_http.rs`: push, clone and pull over smart HTTP against real `git
upload-pack` / `receive-pack` behind a small HTTP front, through a pack real Git deltified,
sign-in required, a racing push refused. `test_github_device_flow.rs`: the device flow,
repository creation and a signed push against a mock GitHub. `C-github-oauth-app` is
UNBUILT: Forge's OAuth app is not registered (the owner's decision);
`FORGE_GITHUB_CLIENT_ID` names one.

Not built here: SSH remotes; deltified push packs; tokens in the OS credential store; importing
commits made outside Forge (refused, named); fetching blobs lazily; S3 and SQL
(`C-store-git-s3-sql`).

---

# Chapter 34 — Remote & Headless Operation

## 34.1 Three modes, named so they are never conflated

| Mode | UI runs | Core runs | For |
|---|---|---|---|
| **Headless** | nowhere | local | CI, bakes, dedicated servers, farm nodes |
| **Split editor** | **locally, native** | remote | **the laptop + workstation case** |
| **Thin client** | streamed pixels | remote | tablets, weak laptops, WAN |

## 34.2 Split editor is a free consequence of I7 — and it beats Parsec

The UI is already a client of the command bus (Ch.7). Making that bus a network transport
is a **protocol, not a rearchitecture**.

The difference from a general remote-desktop tool is structural, not incremental:

| | Parsec / RDP and other remote desktops | **Split editor** |
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

`forge --headless` runs the core with no UI at all: CI, bakes, farm nodes (Ch.26),
and dedicated game servers.

**Guard: `test_headless_parity`** — headless and GUI runs of the same scene produce
identical simulation hashes. This is a remote-mode test that doubles as a determinism
test, which is the best kind.

**Implementation (WP-13, ADR 0030; M2-5 core, M2-17).**

- **`forge --headless`** (`tools/forge-cli`, ADR 0014's binary) runs `forge_editor::headless`,
  the same code as `forge-editor --headless`: a script of editor commands (JSON
  `EditorCommand` lines, `undo`, `redo`, issued as `Script` through a `BusClient`) and play
  lines from `--script FILE` or stdin. **Play controls are the bus's `forge.play.control`
  session command** (§21.21), the one the GUI's panel, menu and chords send: the runner follows session commands and runs each one the bus delivers on
  `forge_editor::sim_bridge::SimPlay` — the play core the shell runs them on — forked from the
  core's project; `play`, `pause`, `stop` and `step N` are sugar that emits that command (the
  JSON `Invoke` form works the same). `run N` moves a simulated editor clock exactly as far
  as N fixed steps take and hands it to `PlayBackend::advance` in catch-up-sized batches (the
  shell's per-frame call); `input {SimInput}` is sugar for the `forge.play.input` session command
  (WP-19, ADR 0042: every client's simulation input is that one bus command, gate
  `C-sim-input-command`) and `sim-hash` reads the play core. `--record FILE` writes the
  last play session's replay file, `--replay FILE` replays one (against the script's project,
  checked by edit hash, or against the scene the file embeds) and fails on the first
  divergence, `--trace FILE` writes a Perfetto trace and the budgets' peaks. It prints the
  project state hash and the simulation hash.
- **The play core** is `crates/forge-sim`: `EditSnapshot` (the edit world from the core's
  project or a client's mirror, content-hashed); `SimWorld` (forked onto forge-core's
  regionized scheduler, one region per frame, plan proven once; semi-implicit Euler for
  `motion.velocity`/`motion.acceleration`, spin about the frame's up axis from
  `motion.spin_dps`, `det` trig, `f64`); `PlaySession` (Play / Pause / Step(n) / Stop as the
  editor's toolbar has them — Step while playing pauses first, both recorded — step `n` at
  tick `floor(n * 10^6 / 60)`, `advance(now)` turning the editor clock into at most 15 fixed
  60 Hz steps per call, `input` for set-velocity / impulse / set-acceleration / set-spin at the
  next step boundary). A session is handed a snapshot, never a command sink: playing cannot
  change the project. `PlayCommand`, `PlayState` and `PlayLogEntry` are the editor's play
  types too (`forge_editor::play` re-exports them). **Play-in-editor** is
  `sim_bridge::SimPlay`, WP-U6's `PlayBackend` over a `PlaySession`: the editor's default
  play backend (`EditorServices::play`, the same object as `EditorServices::play_core`, which
  exposes the recording and inputs), forking from the mirror through
  `sim_bridge::edit_snapshot`, surfacing a refused control or step through
  `PlayBackend::take_error` (added) into the console. The in-memory `MemoryPlay` is gone.
- **Replay files** (`.forgereplay`, JSON Lines v1): a header (rate, checkpoint cadence, edit
  hash, the edit world) and one line per control, input, checkpoint hash (every
  `check_every` steps) and end hash, each stamped with its simulation step, never wall time.
  `forge_sim::replay` re-issues controls and inputs at their steps and requires its own
  recording to equal the file event for event (`SIM-0007` names the first difference;
  `SIM-0006` a different scene). `crates/forge-sim/tests/test_replay.rs` (gate
  `C-replay-bit-exact`): a session played on a jittered editor clock with a stall, inputs
  between frames and a pause with single steps replays **byte-identical**; a one-ulp input
  change is caught at the next hash after it; a crash-truncated file replays to where it
  ends; the committed `tests/data/golden_session.forgereplay` replays on every build (the
  Linux leg is `C-replay-linux-leg`, AWAITING); positive control: a GUI step using the
  frame's measured time diverges.
- **`test_headless_parity`** (`tools/forge-cli/tests/test_headless_parity.rs`, gate
  `C-headless-parity`, and `C-play-core`): the GUI half builds a 16-body scene through the
  shell's command emitter in a headless `Rig`, plays it with the shell's own controls
  (`forge.play.control` on the bus, run on the shell's `SimPlay`, which forks from the
  **mirror**), and the editor loop's jittered clock advances it with inputs, a pause, single
  steps and a 480 ms stall; the headless
  half is the built `forge` binary building the scene from its script (the **project**) and
  replaying the GUI's recording. The edit hash, every checkpoint, the final simulation hash
  and the project state hash agree. Positive controls: the variable-timestep GUI step fails
  with `SIM-0007`; a GUI fork that leaves out a body hidden in the hierarchy fails with
  `SIM-0006`.

## 34.5 Self-hosted, and that is the whole point

**No account. No relay service. No telemetry. No cloud dependency.** The remote host binds
to loopback by default; LAN exposure is an explicit choice; internet exposure requires a
written acknowledgement in the config. If someone needs TURN for NAT traversal, they point
at their own TURN server; the engine ships the config, not the service.

**Guard: `test_split_editor_parity`** — the same command sequence produces byte-identical
project state whether issued locally or over the wire.

**Implementation (WP-17, ADR 0037; M2-16).**

- **`crates/forge-remote`**, above `forge-editor`. The **host** (`QuicTransport`) serves an
  `EditorCore`: each remote session is a client of the core through the core's own `LocalBus`
  (the core has no remote code path). The **client** (`RemoteBus`) is a `BusClient`, so the
  shell, every panel and the viewport (it renders the mirror) run on it unchanged — "local
  panels, locally-rendered viewport". The editor binary hosts the transport on every GUI start
  (`--no-remote-host` turns it off) and runs as a device with `--connect ADDR [--pair CODE]
  [--device-name NAME]`. `QuicTransport` is WP-U9's `RemoteTransport`: the Remote panel's
  pairing dialog, session list and latency indicator bind to it (one added binding line: the
  verified certificate goes into the pairing record).
- **The headless core hosts too** (§34.4): `forge --headless --remote-host [--port N] [--lan]
  [--config-dir DIR | --no-user-config] [--script FILE]` and the same flags on `forge-editor
  --headless` serve the core with no window, GPU or panels (`forge_remote::console`). stdin
  becomes the **host console**, where a person takes the Remote panel's decisions with the
  panel's own calls: `pair` shows a code, a device's request is announced as it arrives,
  `allow NAME` / `deny NAME` answer it, `grant` / `revoke ID CAP` set what a device may do,
  `devices`, `sessions`, `disconnect`, `unpair`, `exposure loopback|lan`, `state` (the
  project's SHA-256 over its canonical wire form), `quit`. The host identity and pairings live
  in the user config, so a restarted host keeps its certificate and its paired devices. The
  console blocks on one channel fed by the terminal and the transport's change feed (no
  polling). Guards: `test_headless_remote_host` (forge-cli: pair at the console, the parity
  script against the `forge` process, restart and reconnect, a wrong code refused; control: a
  lossy wire breaks parity with the host) and `test_editor_headless_host` (forge-editor-bin).
- **Wire** (§34.3): QUIC (`quinn`, TLS 1.3, `ring`), one ordered bidirectional stream per
  session, `postcard` frames (`u32` length, 64 MiB cap). Up: `Apply`, `Begin` (an alias
  until the host's `Begun`), `Commit`, `Cancel`, `Undo`, `Redo`, `Preview`,
  `FollowSession`, `Resync`. Down: `Welcome` (the canonical project snapshot, history,
  undo targets, lifecycle status, the issuer the host fixed), `Batch` (the events, their
  transactions' records, the settled tickets, refusals, session commands, status — one message
  per burst of requests, so an answer never overtakes its events), `Snapshot` (after a gap or
  on request), `Preview`, `Closed`. Nothing on the input path waits for the network; only a
  dry run of a command the client cannot plan (a plugin command installed only on the host)
  asks and waits (2 s).
- **Prediction** (O-13): `forge_editor::core::Predictor` = the editor's planners
  (`EditorCore::editor_bus`, the reserved settings, the promote planner) over a
  `forge_cmd::Replica`; `RemoteBus::apply` predicts, queues the request and hands the diff to
  the next pump; the host's answer reconciles the replica and the mirror (§21.18).
- **Pairing** (§34.5): the host shows a six-digit code (OS randomness, 120 s); the device runs
  **SPAKE2** over the code its person typed — the code never crosses the wire — and both sides
  prove the key with an HMAC over both certificate fingerprints, binding it to this TLS
  connection. The host lists a request for a person only after the exchange (an unverified one
  is refused on Allow, as the stand-in refused a wrong code) and withdraws a code after 3 failed
  exchanges. On Allow the pairing store records the device with its fingerprint; the device pins
  the host's certificate (`DeviceStore`, user config). Certificates are self-signed per machine
  (`rcgen`), kept in the user config.
- **Grants** (§34.5): a device's capabilities are in its pairing record (a per-machine
  credential, never project state); the host mirrors them into the core's one `SharedGrants`
  as `Principal::Remote(<device id>)` and revokes them on unpairing (its sessions close). A
  new pairing may read the project and send ordinary commands; the destructive class is a
  person's decision (`RemotePairingStore::set_capabilities`, audited). Sessions, denials and
  refused connections are audited.
- **Loopback by default**: `127.0.0.1`; the store's confirmed LAN exposure rebinds the
  endpoint on the same port (through a temporary socket if the stack refuses the wildcard beside
  the held address), and back; internet exposure is not offered (`test_remote_exposure`,
  `C-remote-exposure`; control: a host that ignores the store stays where it started).
- **Guards**: `test_split_editor_parity` (`C-split-editor-parity`: a 30-step script with
  lossy-looking values, a committed 40-frame drag, a cancelled drag, removals, a despawn, undo,
  redo and a refusal gives the same canonical project bytes, key allocator, undo history and
  refusals locally and over QUIC, and the client's replica is those bytes; control: floats sent as
  `f32`); `test_optimistic_reconciliation` (§21.18); `test_remote_panel_quic`
  (`C-remote-transport-backend`: the WP-U9 panel over the real transport — pair, list, latency,
  an edit from the laptop issued as the laptop, disconnect, unpair; control: a host that skips
  the code check pairs a wrong code); `test_remote_grants` (`C-remote-grants`; control: a host
  without the grant check lets a despawn through).
- **Measured** (dev box, release, loopback, `cargo run --release -p forge-remote --example
  split_measure`): pairing (TLS 1.3 handshake, SPAKE2, the allow) 5.6 ms;
  a session's welcome with a 10,000-entity project 24 ms (the canonical snapshot is 585 KiB,
  59 B per entity); a request's round trip (a property set answered with its event) median
  95 µs, p99 136 µs; a drag frame costs 106 B up and 191 B down on the wire, QUIC and TLS
  included. An idle connected editor draws 0 frames and wakes 0 times
  (`test_split_editor_idle`, `C-split-editor-idle`; control: a host heartbeat wakes it).
- **Not built here**: the thin-client viewport stream and its budget (Spike S11, M5;
  `C-split-editor-viewport-stream`), WebRTC for NAT traversal (`C-split-editor-webrtc`),
  streaming content-addressed blobs to the device's cache (the device has no asset files yet), a
  device-side connect dialog (the CLI flags exist) and the Remote panel's per-device capability
  control (the store API exists).

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

`FrameId` is always the root; positions are `DVec2` under the same `FramePos` discipline as
everything else.

**Scope control (spike S13):** 2D is a whole engine's worth of features and will sprawl
if it is allowed to. It is bounded by the list in §35.2 and by a shipped sample game —
a small platformer with tilemaps, cutout animation, 2D lights and a gamepad — which is the
acceptance test. Anything not in §35.2 is post-1.0.

## 35.5 Implementation (WP-U15, ADR 0047)

M4-11 as built: `crates/forge-2d` (Spike S13 resolved — the §35.2 list, complete, and the
acceptance game ships). **One amendment**: 2D physics is Forge's own deterministic 2D solver,
not `avian2d` — avian2d is a Bevy plugin (it depends on `bevy` and runs as `bevy_app`
systems), which Ch.5 §5.1 rules out; §35.2's requirement, *a 2D solver, not a 3D solver with a
frozen axis*, is kept (ADR 0047). The 2D preset's `physics.backend` is `"forge.2d"`.

**The list, item by item.**

| §35.2 item | Where | What it is |
|---|---|---|
| Sprite batcher with atlas packing | `sprite.rs`, `atlas.rs` | MaxRects (best short side fit) pages with padding and edge extrusion, deterministic order; the batcher sorts by `(layer, order, kind, texture)` and draws each texture run as **one instanced draw** |
| Tilemaps with autotiling and layers | `tilemap.rs` | 16x16 chunks, any number of layers (draw layer, collides), cells `Empty` / `Tile` / `Terrain`; blob-47 autotiling (a corner counts only with both edges; fallbacks: edges only, then isolated) — the editor's solve is this one; visible-chunk sprite generation; solid cells merged into rectangles for physics and shadows |
| Spline-based geometry | `spline.rs` | centripetal Catmull-Rom through control points, fixed samples per span; fill by ear clipping (a self-crossing shape is refused); edge strip with miter joins and arc-length UVs; collider/occluder segments |
| 2D lights, shadows, normal maps | `light.rs`, `render/` | point and spot lights with a height for `N.L`; **exact hard shadows**: each light's visibility polygon computed on the CPU in `f64` from the occluders' edges, drawn as a triangle fan — all lights one draw; sprites carry normal maps (an atlas page may have a normal page) |
| 2D physics | `physics/` | static / kinematic / dynamic bodies; circles, boxes, convex polygons, capsules, segments (convex hulls with a rounding radius, one collision family); sort-and-sweep; warm-started persistent manifolds; speculative contacts widened by closing speed; friction, restitution, revolute joints, sensors, filters, ray and box queries, contact events; fixed step, fixed iterations, id-ordered everything |
| `Skeleton2D` cutout rigging | `skeleton.rs` | bones (parents first), rest pose, keyed clips (rotation / translation / scale, microsecond keys), blending, two-bone IK, parts drawn as sprites |
| Sprite sheets and frame animation | `anim.rs` | grid slicing (offset, spacing, edge rule), per-frame durations in microseconds, loop / once / ping-pong / reverse, a player |
| Aseprite import | `aseprite.rs`, `plugin.rs` | the file format read directly (RGBA / grayscale / indexed, raw / linked / zlib cels, layers with opacity and visibility, groups, tags, palettes), bounded and never panicking; tags become animations; `forge.2d` provides `Importer("aseprite")` and `AssetType("sprite_sheet")` |
| 2D navigation | `nav.rs` | a walkable grid (from a tile map's solid cells, with clearance), A* 8-connected without corner cutting, taut paths by line of sight, deterministic ties |
| Parallax layers | `parallax.rs` | per-layer factors and repeats, applied in the last mile |
| Pixel-perfect camera | `camera.rs` | a reference size drawn at the largest whole-number scale that fits, letterboxed; the view snapped to the art's pixel grid; picking inverts it |
| 2D particles | `particles.rs` | rate and bursts, ranges, gravity, drag, size and colour over life; particle `k` is a pure function of `(seed, k)` (I3) |
| 2D inspector and the 2D preset's layout | `components.rs`; `presets/2d/` | nine `#[forge_api]` components (sprite, body, collider, light, camera, parallax, particles, tile map, rig) registered under every preset (`ComponentCatalog::editor`), offered first in a 2D project; the preset's layout, keymap, `forge.2d` in its plugin set, and a template whose entities carry the 2D components |

**The render path** (`render/`, the `render` feature): four retained render-graph passes —
`2d.sprites` (albedo + normal G-buffer at the reference size), `2d.lights`, `2d.composite`,
`2d.upscale` (whole-number scale into the output). Positions reach the GPU only through
`render/last_mile.rs`; `f32` exists nowhere else in the crate. A steady frame reuses its
compiled graph, allocates no GPU memory, and its CPU last mile (culling, parallax copies,
shadow polygons, batching, instance bytes) makes no heap allocation. `scenes.rs` generates the reference content (a
blob-47 tile set, a character, a coin, normal-mapped bricks) so goldens and perf rows need no
asset files. Positions are `FramePos2` (`forge-frames`), vectors `DVec2` (`forge-num`); a 2D
world is one frame deep (`TWOD-0002` for any other). Error codes `TWOD-0001..0005`.

**The editor** asks it through `Scene2d`: `Forge2dScene` (forge-2d's autotile, slicing and
FK; image sizes from the PNG / Aseprite headers of the project's files, read through the asset
catalogue `EditorServices::set_assets` attaches, which the editor binary does; an Aseprite
file answers the size of the sheet its import lays out. Limit: the binary's catalogue is the
labelled D-4 in-memory store until a project folder backs it, so the files it can read are the
ones staged into it this session; any other path falls back to 256x256) replaced the D-4
`MemoryScene2d`. The editor loads
`forge.2d` in its default plugin set.

**The sample game** (`samples/2d-game`, crate `forge-2d-game`; M4-12, WP-U16, ADR 0050 — the
WP-U15 acceptance game, promoted to a shipped sample): an autotiled level with stone walls, a
cutout hero (walk clip in step with its speed, IK-planted feet, mirrored), a lantern and lamps
shadowed by the terrain, normal-mapped bricks, parallax, dust, coins, 320x180 at 4x; input as
`forge_ui::game::PadInput` from a `GamepadSource` (a window maps the keyboard; tests use
`ScriptedPad`).

- **Features.** The core (`Game`, `PadState`, `demo_script`, `play_script`, `Fingerprint`)
  needs no GPU and no window — the I2 corpus links only it (row `2d/sample-game/run`).
- **Menus** (`session.rs`): `forge_runtime::game_ui::GameMenu` (WP-U11) over a game view that
  composites the `Renderer2d` texture (`UiApp::render_external`, external texture `0x2D`): main
  menu, settings, credits (E-64 engine entry first, static text), HUD (coins as the score),
  pause menu, and a level-clear banner (South returns to the main menu). Every string is a
  localisation key (`session::strings`: en, fr, pseudo-locale). One pad stream: while the level
  plays, stick / d-pad / South go to the game and Start / East / Escape to the menus. Held
  keys reach the game through `UiApp::key_first` (added to the winit runner: presses and
  releases before the UI routes them; the editor never overrides it).
- **Time.** Step `n` of a play stretch is due at `n/60` s on the UI clock; at most 15 steps a
  turn and a longer stall drops the backlog; a menu or a pause schedules nothing (D-5). The
  simulation sees only the pad state per step, so a scripted run through the menus reaches the
  core's bits at any frame rate.
- **The binary** (`forge-2d-game`): the window (`--demo` presses Play and plays the script;
  `--exit-after`, `--report-startup`); `--play-script` (headless, no GPU: Play in the menus, the
  scripted run to the banner, prints the fingerprint; exit 0 only on a win); `--probe`
  (headless startup to the first rendered frame, per phase); `--shot`.
- **Play-in-editor** (`pie.rs`): `SamplePlay` is a `forge_editor::play::PlayBackend`; the
  editor's play controls (`forge.play.control` on the bus, from the real panel) run the level —
  Play forks (resumes a pause), Pause, Step(n) (pauses a playing run, forks a stopped one), Stop
  discards; 60 Hz on the editor clock, 15 steps per advance at most. Handed the mirror read-only,
  never a command sink (I7); the level is the sample's generated level, not project entities.
  `forge-editor --play-2d-sample` installs it: while it plays, the viewports draw the level with
  `Renderer2d` (`ViewportRenderHost::take_dirty_sizes`) and the movement keys are the game's.
- **Export** (`forge_project::packager::CargoPackager`): `cargo rustc --release --locked
  --offline` of the game crate for the host target (Windows or Linux; cross-target is M8's),
  stripped, with the linker's map (`/MAP`, `-Wl,-Map`) kept beside the package; the package
  folder holds the executable named after the product, `forge.json`, `NOTICES`, `credits.txt`
  and the project's data. The executable metadata is M5-27's (listed unbuilt in the report).

**Guards.**

| Test | Gate row | Positive control |
|---|---|---|
| `crates/forge-2d/tests/test_2d_goldens.rs` (tiles, lights, shapes at 1280x720) | `C-2d-goldens`, `C-2d-goldens-linux-leg` | `positive_control_a_moved_light_fails_the_golden`; `positive_control_a_lost_normal_map_fails_the_golden` |
| `crates/forge-2d/tests/test_2d_render.rs` | `C-2d-render-steady` | `positive_control_without_batching_every_sprite_is_a_draw` |
| `crates/forge-2d/tests/test_2d_prepare_alloc.rs` | `C-2d-prepare-no-alloc` | `positive_control_a_fresh_prepared_is_counted` |
| `crates/forge-2d/tests/test_physics_2d.rs` | `C-2d-physics` | `positive_control_no_warm_start_lets_the_pyramid_sag`; `positive_control_without_speculative_contacts_it_tunnels`; `positive_control_a_step_dependent_contact_order_diverges`; `positive_control_hash_map_contact_order_makes_two_runs_disagree` (the two-run check catches `HashSet`-order nondeterminism in one process) |
| `tests/preset/test_2d_tax_is_zero.rs` (§31.5) | `C-2d-tax-zero` | `positive_control_a_2d_preset_naming_a_3d_plugin_fails`; `positive_control_a_2d_crate_linking_a_3d_crate_fails` |
| `samples/2d-game/tests/test_sample_game.rs` (S13, M4-12) | `C-2d-sample-game`, `C-2d-sample-linux-leg` | `positive_control_without_jumping_the_first_wall_stops_the_hero`; `positive_control_one_step_of_input_changes_the_bits` |
| `samples/2d-game/tests/test_sample_game.rs` `the_menus_play_the_level_to_the_clear_banner` (Play in the menus, the scripted run to the banner, the pinned `SCRIPT_WIN`, the HUD score, back to the menu; 60 Hz cadence and a free pause; credits and strings) | `C-2d-sample-menus` | `positive_control_through_the_menus_without_jumping_there_is_no_banner` |
| `samples/2d-game/tests/test_pie.rs` (the real play-controls panel runs the sample; the project hash is unchanged by Play..Stop; `Step(900)` reaches `SCRIPT_FINGERPRINT`, the export's bits) | `C-2d-sample-pie` | `positive_control_a_pie_run_without_the_script_is_caught` |
| `plugins/forge-panels-domain/tests/test_editors_2d.rs` | `C-editors-2d-backend` | `positive_control_a_backend_with_its_own_rule_is_caught` |
| `crates/forge-2d/tests/test_aseprite_import.rs` (an `.aseprite` through the asset server with `forge.2d`: texture + `sprite_sheet` checked against the authored pixels, slicing and tags) | (coverage) | `control_a_truncated_file_fails_the_import_visibly` |
| `plugins/forge-panels-domain/tests/test_editors_2d.rs` `the_shipped_wiring_reads_image_sizes_from_the_project_files` | `C-editors-2d-backend` | its control: no catalogue attached falls back to 256x256 |
| `plugins/forge-panels-scene/tests/test_inspector.rs` `a_2d_project_offers_the_2d_components_first` | (coverage) | its control: the 3D preset's order interleaves other components |
| `tests/liveness/test_no_f32_below_render.rs` rule 4 | `C-2d-f32-render-only` | `positive_control_f32_in_2d_simulation_is_flagged` |
| `tests/liveness/test_frame_liveness.rs` (I1 covers `DVec2`) | `I1` | `positive_control_a_bare_2d_position_is_flagged` |
| `tests/determinism/test_cross_platform_hash.rs` (ten `2d/*` rows) | `I2` | `positive_control_mutate_det_build_fails` |
| `tools/forge-perf-gate/tests/test_perf_gate.rs` (`render2d.*`, `phys2d.step`, `2d.cold_start`) | `C-perf-gate` | `positive_control_a_slower_2d_light_pass_fails_its_row`; `positive_control_a_1_5x_2d_pass_fails_its_own_row`; `positive_control_an_injected_2d_cpu_regression_fails_the_gate`; `positive_control_unbatched_sprites_fail_the_draw_call_row` |

**Measured** (dev box, 2026-09-24; perf frame 1920x1080: ~5,000 sprites, 2,000 of them
particles, a spline shape, 32 shadowed lights): GPU per pass on the RTX 3080 — sprites
0.071 ms, lights 0.066, composite 0.022, upscale 0.018, total 0.176; on WARP 244 / 13.8 /
6.6 / 6.6 / 271 ms; 7 draw calls; last mile ~1-2 ms CPU; one solver step with 2,000 bodies
~2.2 ms (600 bodies: 2.0 ms before the dense solver bodies, 0.8 ms after); the 2D cold start
(tile map, colliders, atlas) ~0.25-0.45 ms; a 10-row pyramid's top box moves 4.4 mm in 10 s.
The sample game (WP-U16): the exported binary 13.3 MiB on windows-x86_64 (stripped, no LTO);
headless startup to the first rendered frame 0.74-2.1 s on the RTX 3080, of which GPU device
creation is all but ~20 ms (level and menus ~2 ms, first frame 11-17 ms), 0.81 s on lavapipe;
the scripted run clears the level at step 695 with fingerprint `0xc366f9b68e66f6ac` and its 900
steps fold to `0xb051597b3a7423b3`, identical on windows-x86_64 and ubuntu-x86_64 (ADR 0050).

**Not in the list (post-1.0, S13)**: body sleeping and islands, time-of-impact CCD beyond
speculative contacts, joints other than revolute, soft shadows, polygon navmeshes, Aseprite
tilemap layers and non-Normal blend modes (reported, composited as Normal), Sprite-Shape corner
sprites, isometric and hexagonal tile maps, GPU particles.

**Not built yet (gate rows UNBUILT)**: the editor viewport drawing a 2D project (outside
play-in-editor of the sample) with
`Renderer2d` / `Camera2d` (`C-editor-viewport-2d`); the 2D editors' canvases drawing the
project's own tile and sheet pixels (`C-editors-2d-tile-pixels`); a device gamepad backend
(`C-2d-gamepad-device`, Ch.27 / M5-8); the 2D preset's binary-size budget
(`C-2d-preset-binary-size`: measured and recorded by the export guard, no budget number yet).

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
| one capability-grant model | 32, 34 | roles, with one thing to audit |

Multi-user is: **the command log becomes a shared, ordered stream**, plus a per-user layer
over it.

## 37.2 The sandbox equation

> **I19: `project_view = baseline ⊕ sandbox_deltas`.**
> A sandbox never mutates the baseline in place.

That is the same equation as the terrain layer — `chunk_state = generate(seed, pos) ⊕
edits[chunk_id]` — one level up, and not by coincidence: it is the same problem (many
readers, few writers, isolation must be cheap) and it has the same answer.

It inherits the same property, which is the one that makes hosting affordable: **a sandbox
is a row, not a process.**

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

Roles map to capability sets, **reusing the Ch.32/34 grant model** — one more user of one
mechanism, so there is one thing to audit:

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

**Team management is commands** (I7) — so it is scriptable, undoable and audited like
everything else.

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

# Chapter 38 — Licensing, Entitlement & Royalty Reporting

Forge is a commercial product (Appendix A). This chapter is how that is implemented
**without poisoning the software**, which is the only interesting engineering problem in it.

## 38.1 The invariant that governs everything else

> **I21: The software never phones home to function.**
> Licence activation happens once, at install. There is no runtime check, no telemetry,
> and **nothing whatsoever in a customer's shipped game.**

Every commercial engine eventually feels pressure to add a check, a ping, an analytics
call. Each one is defeated within a week, breaks offline and console builds, and — worst —
is inherited by every customer's shipped product, which makes *their* users your problem.
Writing I21 down now, while it costs nothing, is how it survives the pressure later.

**Structural guarantee, not a promise:** `forge-licence` is **not in `forge-runtime`'s
dependency graph, and cannot be.** The layering rule (Ch.1 §1.1) forbids it and `xtask`
enforces it. A shipped game therefore cannot contain licensing code, because the crate
that would do it is not linked.

`test_no_runtime_phone_home` walks `forge-runtime`'s full transitive dependency graph and
fails on any network-capable crate that is not an explicitly allow-listed transport, plus
on any reference to `forge-licence`. **Positive control:** a mutation build adds the
dependency and the test must fail.

## 38.2 Entitlement

| Step | Where | Online? |
|---|---|---|
| Buy a licence | web, once | yes |
| Activate a machine | editor, at install | yes, once |
| Use the editor | forever | **no** |
| Build, bake, ship | forever | **no** |

An account exists **to buy a licence, never to use the software** (E-46). Activation is a
signed entitlement file written to disk; the editor verifies the signature offline. There
is no expiry, no re-check, no seat reclamation, and no grace period to run out — a
perpetual licence that stops working is not perpetual.

### The entitlement declares three things

```rust
pub struct Entitlement {
    pub tier:    Tier,             // Individual | Team { threshold: Under100k | Over100k }
    pub seat:    SeatKind,         // Purchaser | Included | Additional
    pub bound:   Option<TeamId>,   // Some(..) for Included and Additional seats
    pub expires: Option<Date>,     // None for Individual (perpetual); Some for a Team term
    pub fallback: Option<Version>, // E-59: perpetual Team rights up to this version
    pub sig:     Signature,        // verified offline against a bundled public key
}
```

Three rules, all evaluated locally:

1. `Tier::Individual` → the Ch.37 team and collaboration features are unavailable.
2. `Tier::Team` → they are available.
3. `seat` is `Included` or `Additional` → the open project's owning team must equal `bound`.
   A seat holder who wants an unrelated personal project buys a $5 Individual licence.

4. `expires` in the past, and no `fallback` covering the running version → **degrade to
   `Individual`** (E-57). Never lock out, never refuse to open a project, never disable
   building or shipping. A warning, a renew button, and the collaboration panels go grey.

No network call evaluates any of this. The signed file is on disk and the public key is in
the binary. Renewal fetches a fresh entitlement **opportunistically, when the machine
happens to be online** — it is never a precondition for starting, opening, building or
shipping. A grace period applies before degradation (length: O-27).

The expiry is checked against the local clock, which can be set backwards. That is
accepted, for the same reason the tier gate is accepted as bypassable (Appendix A §A.7):
the source is public, so both are honour-system, and engineering against either would cost
real effort, fail anyway, and break the commitments that make a paid engine trustworthy.

## 38.3 The plugin index as a commerce surface

Ch.32 §32.5 specified an open index. It stays open — self-hostable, no curation gate — and
gains an optional commercial path:

- Publishers hold a verified identity and sign their manifests (O-12). A signed manifest is
  what makes an obligation attributable.
- Paid plugins sell through the index, which handles payment. **The 5% is taken here**,
  where the project is the payment processor and the commission collects itself.
- Free plugins pay nothing and are never second-class in discovery.
- A self-hosted index uses its own root key and takes no commission — an organisation
  running an internal index needs no blessing and owes nothing.

> **Settled (E-52): the 5% is a storefront commission on Index sales only. Plugins and
> assets sold anywhere else owe 0%** — it is a fee for payment processing, hosting and
> discovery, not a royalty on creation.

**This is the only ongoing revenue in the entire business** (`decisions.md` §6.3), which
has one consequence worth writing into the plan rather than discovering: the Index moves
from a convenience to a load-bearing component, and its milestone position should reflect
that. It also sets a standing rule — **the Index is never allowed to distort the engine.**
No paid-only engine features, no store lock-in, no curation gate that favours revenue over
quality, and free plugins are never second-class in discovery.

## 38.4 There is nothing to report

**No royalties exist** (E-51), so there is no reporting obligation, no audit apparatus, no
statement generator and no definition of gross revenue to argue about. The section that
used to be here is deleted rather than reduced.

Two numbers remain and neither needs machinery:

- **The $25 / $40 team tier** is self-declared. The difference is **$15**; any system built
  to police it would cost more to build than it could ever recover, and would insult every
  honest licensee to catch the few who are not. The upgrade is a button.
- **The 5% marketplace commission** collects itself, because the Index is the payment
  processor. Nothing is reported because nothing needs to be.

This is the clearest downstream benefit of dropping royalties: an entire subsystem, a legal
apparatus and a category of customer friction all stop existing.

## 38.5 NOTICES

`cargo-about` generates a `NOTICES` file into every distribution — the editor, the runtime,
and any product embedding the runtime. MIT and Apache-2.0 permit commercial closed-source
use **provided notices are preserved**; a distribution without them is a licence violation
of the dependencies, which is a far more immediate legal problem than anything on the
revenue side.

`just verify` fails on a stale `NOTICES`. `cargo-deny` enforces the permissive-only
allow-list (I13).

## 38.7 Engine attribution in shipped products

Every product containing `forge-runtime` credits Forge Engine in four places (E-64):

| Where | How it gets there | Who can see it |
|---|---|---|
| In-product credits / about screen | the packager adds a credits entry; the licensee places it | players |
| **Executable metadata** | **inserted by the build tools, automatically** — Windows `VERSIONINFO`, a Linux ELF `.note.forge` section, and a `forge.json` manifest beside the binary | anyone who inspects the file |
| `NOTICES` | already generated (§38.5) | anyone |
| Store page / documentation | licensee's obligation under the EULA | buyers |

Plus a **"Made with Forge Engine" splash screen, on by default** — removable or not is an
owner decision (O-34), with a recommendation of *removable*: a required credit is
universally accepted, and a *forced* splash was one of the most resented terms of Unity
Personal, which Unity made optional as of Unity 6.

**The metadata row is the one that does the work.** It makes "what engine built this?"
answerable in seconds with `readelf -n` or a file's Properties dialog, with no visual cost to
the product and nothing for a player to notice.

**What the credit detects, stated precisely so nobody over-reads it:** it identifies that a
product was **made with Forge Engine**. It says nothing about whether the maker holds a
licence; that is checked against the publisher's own records. And under E-57 a team whose
subscription lapsed **degrades to Individual, which is still licensed to ship** — so a
lapsed team continuing to update its game is *not* a violation. In practice the credit
finds products whose makers never bought a licence at all.

**It is inert, and I21 is untouched.** The credit is static text and metadata. No network,
no reporting, no identification of anyone, no licence check — nothing in it executes. A
constant string in `forge-runtime` is not licensing code.

**It is removable by anyone who builds from source**, like the tier gate (Appendix A §A.7).
Its force is contractual: removal is a breach. The build tools make compliance the default
and removal a deliberate act, which is the most a public-source product can do and all it
should try to.

## 38.6 Guards

| Guard | Asserts |
|---|---|
| `test_no_runtime_phone_home` (I21) | `forge-runtime` links no licensing crate and no un-allow-listed network crate. Positive control: adding one must fail |
| `test_offline_forever` | activate, then run the editor with the network unavailable for a simulated decade of wall-clock; it must never degrade |
| `test_notices_fresh` | `NOTICES` matches the resolved dependency graph |
| `test_licence_permissive_only` (I13) | no copyleft dependency at any depth. Positive control: adding a GPL crate must fail |
| `test_entitlement_offline_verify` | a signed entitlement verifies with no network and survives clock changes in both directions |
| `test_tier_gate` | `Tier::Individual` cannot reach Ch.37 collaboration commands; `Included`/`Additional` seats are refused on projects outside `bound`. Positive control: a forged tier must fail signature verification |
| `test_gate_is_editor_only` | no tier check exists anywhere in `forge-runtime` or in any code path a shipped product can reach |
| `test_lapse_degrades_never_locks` (E-57) | an expired Team entitlement opens every existing project, builds, and exports; only Ch.37 commands are refused. Positive control: a mutation that refuses to open a project must fail the test |
| `test_attribution_emitted` (E-64) | every packaged build carries the credit in executable metadata and `forge.json` on both Windows and Linux, and the credits entry exists. Positive control: a build with the step disabled must fail |
| `test_attribution_is_inert` | the attribution code path makes no network call, reads no entitlement, and executes nothing at runtime |
| `test_renewal_never_blocks` | with the network unavailable and an expired entitlement, start-to-export completes with no stall and no prompt that cannot be dismissed |

---

# Appendix A — Licensing, Commerce & Governance

> **Forge is commercial and source-available. It is not open source, and the documentation
> must never call it open source** — the term has a specific meaning and misusing it
> destroys trust faster than charging money ever would.

## A.1 The model

| Licence | Price | Covers |
|---|---|---|
| **Individual** | **$5 once, perpetual** | one person, all versions, ships commercial products. **No team/collaboration features.** |
| **Team** — project under $100K gross | **$25 / year** | **4 seats** (purchaser + 3 included) |
| **Team** — project at or above $100K | **$40 / year** | simply the rate matching this year's revenue |
| **Additional seat** | **$5 / year** each | same restriction as an included seat |
| **Marketplace commission** | **5%** | sales through the Forge Index. **0% everywhere else.** |

The Team tier is an **annual subscription**; Individual is a one-time perpetual purchase.
Moving Team to a subscription also deleted the awkward "$15 once on crossing the threshold"
rule it replaced — a team now just pays the rate matching its current revenue each year.

> ### There are no royalties. On anything. Ever.
> Not on games, not on applications, not on plugins, not at any revenue, not at any scale.
> A licensee pays once and keeps 100% of their product revenue forever.
>
> **This is stated affirmatively in the EULA and the documentation, never merely omitted** —
> an omission reads as an oversight that could be closed later, which is precisely the fear
> this model exists to answer.

**The included and additional Team seats are bound to the purchasing team**: they work only
on projects that team owns. Someone holding one who wants their own unrelated project buys
a $5 Individual licence. **Revenue is self-declared** — no audit, no reporting, no
instrumentation (E-53). The only revenue-dependent number in the entire model is a $15 tier
difference, and any apparatus to police that would cost more than it recovers.

The source is public and readable; a licence is required to *use* it, not to read it.
`forge-runtime` may be redistributed **in binary form only**, embedded in a shipped product.

**Permitted:** build and ship games and applications; create and distribute plugins and
assets for the editor; modify the source for your own use.

**Prohibited:** redistributing engine source; sublicensing; distributing the editor itself;
producing a competing engine derived from this one.

## A.2 Public source and a purchase fee do not gate each other

This has to be written down because it is the most common way this model is got wrong.

A public repository can be cloned by anyone, so **$5 cannot control access — it can only
buy a licence.** Enforcement is contractual and by audit, not technical. That is precisely
how Unreal works (free download, royalty enforced by contract), and it is a workable model
that has funded a AAA engine for a decade. What it is *not* is a paywall.

If the fee must gate access, the repository goes private and access is granted on purchase.
That is a different product shape — it forfeits drive-by evaluation and most of the
credibility that reading the source buys — and it should be chosen deliberately rather than
discovered.

## A.3 Honest comparison

| | Forge | Unreal | Unity | Godot |
|---|---|---|---|---|
| Cost | **$5 once (solo) · $25–40/yr (team)** | free | free tier | free |
| Product royalty | **none, ever** | 5% above $1M lifetime, per product | none (seat tiers above $200K) | none |
| Marketplace cut | **5%** | **12%** (Fab) | Asset Store cut | n/a |
| Source | readable, licensed | readable, licensed (EULA) | no | fully open, MIT |
| Governance | owner | Epic | Unity | foundation |

*Unity's tiers have changed repeatedly; verify current figures before publishing this table
anywhere public.*

Worked examples, because the abstract comparison undersells it:

| | Forge | Unreal |
|---|---|---|
| Solo dev, product grosses $2M | **$5, once** | **$50,000** |
| 4-person team, 3 years of development, product grosses $5M | **~$105 total** | **$200,000** |
| $20 plugin sold in the first-party store | **$1.00** | $2.40 |

A four-person team pays **$25–40 per year for all four seats.** Per-seat subscription
pricing at the competition is in the low thousands per seat per year, so the gap is two to
three orders of magnitude and the subscription barely dents the argument.

**Godot remains free and is the honest competitor to name.** If the technical differentiator
does not land, no pricing table saves this.

## A.4 The trust commitments, and the one real conflict in the model

Unity's 2023 episode was not about charging money. It was about **changing terms
retroactively on software people had already shipped on.** The answer, in the EULA rather
than a blog post:

> **A purchased licence is perpetual and irrevocable for the versions it covers. Changes to
> pricing or terms apply only to versions released after the change.**

### The conflict a subscription creates, and how it resolves

A.7 commits to no DRM, no telemetry, no runtime check, and indefinite offline operation. A
subscription, by definition, has to know when it lapsed. **Those are in genuine tension and
pretending otherwise would be the dishonest move.** The resolution, all of it binding:

1. **On lapse the editor degrades to the Individual tier. It never locks out** (E-57). The
   licensee keeps the editor, the projects, and the ability to build and ship. Only the
   Ch.37 collaboration features go dark until renewal. This is the answer to *"what happens
   if I stop paying halfway through a three-year project"* — which is the first question
   any studio asks about a subscription, and the one that loses the sale if it is fudged.
2. **A lapsed licence never affects a shipped product** (E-58). Structurally guaranteed
   already: `forge-runtime` contains no licensing code and cannot (I21). Stated as a term
   anyway, because a guarantee nobody has read is not reassuring.
3. **Nothing is ever being held.** Forge operates **no project hosting** (E-61):
   `forge-server` runs on the licensee's own hardware, so a lapse cannot strand a project
   because the publisher has nothing to withhold. "You keep your projects" is only a
   meaningful promise if nobody else is holding them, and the self-hosted pillar (E-31)
   already guaranteed it — it had simply never been written down as the reason.
4. **Perpetual fallback after 12 continuous months** (E-59, recommended): the licensee
   keeps perpetual Team rights to the version current at their 12-month mark, renewed or
   not. **This costs almost nothing, because the source is public** — every version is on
   GitHub permanently and cannot be withheld. The grant formalises what is already
   physically true and removes the largest objection to subscribing. **It also leaks
   revenue** — a team can vest at 12 months and cancel. That trade is analysed and
   deliberately accepted in `decisions.md` §7, and is scheduled for review at 1.0.

### What honest enforcement looks like here

The entitlement carries an expiry checked against the **local clock**, with a generous
grace period, renewed opportunistically when the machine happens to be online — **never a
blocking check**. Set the clock back and you have defeated it, exactly as stripping the
check from source defeats it. Both are true and neither is worth engineering against
(A.7). The subscription is enforced by being worth paying for, which is the only mechanism
available to a source-available product and should be understood as such before the
business is planned around it.

## A.5 Dependency policy (I13) — tighter now, not looser

Permissive only: MIT, Apache-2.0, BSD, Zlib, ISC. **BSL-1.0** (Boost) is also admitted (D-8, ADR 0006):
permissive, no copyleft, and no attribution requirement for binaries; it unblocks
`arboard`'s Windows backend (the OS clipboard, Ch.21 §21.9). **No copyleft of any strength** — a
single GPL or AGPL dependency would make the product undistributable, and an LGPL one
constrains static linking in ways that break console builds.

MIT and Apache-2.0 both permit commercial closed-source use **provided notices are
preserved**, so `cargo-about` generates a `NOTICES` file into every distribution and
`just verify` fails if it is stale. `cargo-deny` enforces the allow-list.

**`ufbx`, never the Autodesk FBX SDK.** Proprietary middleware (Wwise, FMOD, platform SDKs)
may only ever be a plugin behind a trait, never a dependency.

## A.6 Contribution — a CLA is now required

This reverses the earlier recommendation (E-2) and the reversal is not optional: **you
cannot sell a product containing a contribution you hold no grant in.** A DCO asserts the
contributor had the right to contribute; it does not grant you the right to relicense their
work into a paid product.

Therefore: a CLA with a broad, irrevocable licence grant (or copyright assignment), signed
before any contribution is merged. **Until the CLA and the EULA exist, pull requests are
not accepted** — merging one now creates a cleanup that is expensive and sometimes
impossible to unwind.

Expect few outside contributions. That is the honest cost of the model and it should be
planned for rather than hoped against.

## A.7 What is never done, regardless of commercial pressure

- **No DRM in the runtime.** It would be defeated in a week, break offline and console
  builds, and be inherited by every customer's shipped game.
- **No telemetry. Anywhere.**
- **No runtime licence check** (I21). Activation is at install, once.
- **Nothing added to a customer's shipped product** beyond the runtime they licensed **and a visible, inert "Made with Forge Engine" credit** (§38.7). Nothing hidden, nothing that runs, nothing that reports.
- **No retroactive terms changes** (A.4).
- **No lockout on lapse** (A.4). The editor degrades; it never holds a project hostage.
- **No hardening of the tier gate.** The Individual tier is gated out of team features by a
  signed local entitlement, and anyone who compiles from source can remove that check in an
  afternoon. That is inherent to source-available and is not a flaw to engineer away. The
  gate is worth what a cheap, correct, offline check costs and **not one hour more**.
  Obfuscation, server checks, integrity verification and binary-only editor builds are
  rejected in advance — each would cost real engineering, fail anyway, and break the four
  commitments above that are the entire reason a developer would trust a paid engine.

These are what keep a commercial engine trustworthy. Each one is a thing a struggling
engine eventually wants to do, which is why they are written down now, while it costs
nothing to promise them.

## A.8 Trademark

Trademark **Forge Engine**; the name is the asset that survives regardless of licence
model. Note two existing collisions — Minecraft Forge in an adjacent ecosystem, and
`VoxelPlugin/Forge` — and use the full two-word name consistently. Check `forge-*`
availability on crates.io before M0 publishes anything.

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
| **S7** | Physics precision or determinism insufficient at region scale | determinism harness on `avian` f64 with fixed step | region-local origin rebasing; if still insufficient, fork for stable contact ordering | |
| **S17** | The EULA is not drafted and reviewed before the first sale | engage a lawyer; draft from the Unreal EULA's structure or PolyForm/FSL | **none — the first sale does not happen without it.** A homemade licence is how a project discovers it cannot enforce anything | **M8, blocking** |
| **S15** | Rebase conflicts frequent enough to make Live mode unusable | synthetic two-user workload on a real scene; measure conflicts per hour | scoped ownership becomes **mandatory** in Live rather than optional; Live degrades to "live read, scoped write" | M6 |
| **S16** | A sandbox costs more than a row — per-user memory or process growth | 50 idle sandboxes on one server, measured | if a sandbox cannot be a row, Live is capped at a stated team size and that number is published | M5 |
| **S9** | Scope collapse — the project stalls chasing parity | **the dogfood gate**: the Foundations must port by end of M4 | cut parity features, not the differentiating ones. The differentiator is the product | M4 |
| **S10** | WASM plugin overhead too high at hot extension points | measure a generator node and a PCG rule in WASM vs native | that point becomes source-plugin-only and is documented as such | M4 |
| **S11** | Split-editor viewport latency budget unmet on real hardware | measure encode+transit+decode on a real LAN pair at 1080p60 | split editor still ships (panels are local and useful); thin client documented as WAN-only | M5 |
| **S12** | A realistic project makes a git repo unusable | build a 20 GB sample project, measure clone and status on every backend | blob store is the mitigation; if it is not enough, default new projects to `Sql` or `S3` | |
| **S13** | The 2D pipeline sprawls into a second engine | bound it by the Ch.35 §35.2 list and one shipped sample game | anything outside the list is post-1.0, stated in the docs | M4 — **resolved** (ADR 0047, §35.5: the list, complete; `samples/2d-game` ships, WP-U16) |
| **S14** | Windows/Linux divergence found late (case sensitivity, paths, DX12 vs Vulkan) | run the full gate on both from M0; lint asset-reference casing | none — this one is prevented, not mitigated (I18) | **M0** |

**S2 and S9 are the two that actually kill the project.** S2 because a non-deterministic
engine makes the entire premise false and the failure is silent. S9 because it is how
every ambitious engine project has ended, and the only defence is a real game that must
ship on it.
