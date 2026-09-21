# Forge — Milestones, DoD & Gates

**Every milestone ends in something a human can launch and an agent can drive.** No
milestone may end in "the architecture is now correct."

Statuses: `settled` · `blocked(reason)` ·
`superseded(by)` · `UNMET` · `unread`. **A milestone ships at 0 UNMET and 0 unread.**
`Unbuilt(reason)` is a legitimate terminal state for a *gate row*; "nobody got to it" is
not one of the reasons (W9).

---

## M0 — Contracts & the Determinism Spine

*Nothing can be parallelised before this is done. This is the fan-out gate.*

| id | Definition of done |
|---|---|
| M0-1 | Workspace, `justfile`, `xtask`, CI on three platforms (macOS-aarch64, ubuntu-x86_64, windows-x86_64) |
| M0-2 | `forge-num` — vendored transcendentals, `newton<N>`, integer noise basis. **Spike S2 resolved.** |
| M0-3 | `test_cross_platform_hash` green on all three legs, **with its mutation positive control failing as designed** |
| M0-4 | `forge-frames` — `FramePos`, `FrameVel`, `FrameTree`, `rebase`, `Tick` |
| M0-5 | `test_rebase_roundtrip` + `test_rebase_velocity_terms` with **two distinct** failing mutants |
| M0-6 | `forge-seed` — derivation, `SeedPath` provenance, order-independence test |
| M0-7 | `forge-reflect` — `#[forge_api]`, `test_four_outputs_agree` |
| M0-8 | `forge-cmd` — envelope, txn, dry-run, undo, provenance; `test_undo_fuzz` |
| M0-9 | `forge-core` — world, region types, scheduler skeleton (not yet parallel) |
| M0-10 | `just verify` == CI's list in CI's order; `xtask gate-parity` green (I12) |
| M0-11 | `just plan-coverage` green — every directory in the tree claimed by a chapter |
| M0-12 | `cargo-deny` in `just verify` with the allow-list committed (I13) |
| M0-13 | The `no bare DVec3 position` lint (I1) with its allow-list and reasons |
| M0-14 | `forge-plugin` — `ExtensionPoint`, `Registry` with add/replace/remove/chain, `PluginId`, the capability enum |
| M0-15 | `forge-store` — the `ProjectStore` trait and `LocalFs`; blob addressing (BLAKE3) |
| M0-16 | **CI green on Windows and Linux from the first commit** (I18), plus macOS as the developer leg. **Spike S14 is prevented here or not at all.** |
| M0-17 | Asset-reference case lint — the Windows-works/Linux-breaks bug class, killed at M0 |
| M0-18 | **Chapters 1–7, 31, 32, 33 frozen.** Signature changes from here are plan amendments. |

**Exit criterion:** a headless binary that derives a seed path, evaluates an ephemeris,
rebases between two frames, applies and undoes a command, saves and reloads through two
store backends, loads one trivial plugin that *replaces* a built-in, and produces a hash
identical on three platforms. It renders nothing. That is correct.

> **Why the plugin and store contracts are in M0 and not later.** A kernel/plugin boundary
> drawn after twenty subsystems exist is drawn around whatever those subsystems happened
> to do, and every private escape hatch found later is permanent. A storage trait added
> after the editor calls `std::fs` in forty places is added around forty exceptions.
> Both are cheap now and are never cheap again — the same argument as I1 and I7.

---

## M1 — Render Spine

*A true-scale planet on screen, at the right scale, with the right sky.*

| id | Definition of done |
|---|---|
| M1-1 | `forge-gpu` — adapter **pool** (length may be 1), render graph, WGSL + `naga`, shader hot reload |
| M1-2 | **Spike S1 resolved** with a written verdict and the Tier-1 fallback stated |
| M1-3 | f64 → camera-relative f32 at the last mile; no f32 position anywhere below `forge-render` |
| M1-4 | Four-shell multi-frustum depth, reversed-Z, correct compositing |
| M1-5 | `BodyRepr` trait + `CubeSphereQuadtree` (heightfield) — the cheap one first |
| M1-6 | `UnwarpedLattice` + extraction (**O-5 decided and written down**) at one LOD |
| M1-7 | Clustered forward+, PBR metal-rough, directional + punctual lights, VSM |
| M1-8 | Bruneton atmosphere parameterised by body composition |
| M1-9 | Distant bodies as point sprites with correct position, brightness, phase |
| M1-10 | Star field and deep sky from the generated catalogue |
| M1-11 | Perf budgets named and a regression gate in CI |
| M1-12 | **Windows (DX12) and Linux (Vulkan) render identically**; `test_platform_parity` green (I18) |

**Exit criterion:** stand on a 6,371 km sphere, look up at a correctly-placed sun and moon
that move, look down at terrain with no z-fighting and no precision shimmer, at 60 fps.

---

## M2 — Editor Spine

*The editor exists, and an agent is a peer of it from the first day it does.*

| id | Definition of done |
|---|---|
| M2-1 | `forge-editor` as a **client of `forge-cmd` and nothing else** |
| M2-2 | Viewport, hierarchy, inspector **generated from reflection** (Ch.6), asset browser |
| M2-3 | Undo/redo across every editor action, via the bus |
| M2-4 | `test_command_liveness` green **with its positive control failing as designed** (I7) |
| M2-5 | Play-in-editor; headless play; deterministic replay of a recorded command stream |
| M2-6 | `forge-asset` — VFS, glTF import, hot reload, content addressing |
| M2-7 | **`forge-mcp` v1**: all five tools, all three resource roots, capability grants, audit log |
| M2-8 | An agent builds a small scene end-to-end through MCP alone, with a transcript in the repo as an example |
| M2-9 | **Spike S4 resolved** — regionized scheduler fuzzed and measured at 4/16/64 regions |
| M2-10 | **Spike S6** — two `bevy_*` upgrade cycles tracked, churn cost measured and recorded |
| M2-11 | `forge-trace` — Tracy/Perfetto, named budgets |
| M2-12 | **Workspace presets 3D and Planetary** shipping as data; `test_no_preset_gating` + `test_preset_promotion` green (I15) |
| M2-13 | **Plugin system v1** — WASM host, source plugins, manifest, capability grants, hot reload; `test_no_privileged_plugin` green with its positive control (I16) |
| M2-14 | **At least three first-party subsystems shipped as plugins**, proving I16 rather than asserting it |
| M2-15 | `forge-store` — `Git` backend (GitHub device flow + any remote), blob store, generated ignore rules, three-click new-project flow |
| M2-16 | **Split editor v1** — commands + state deltas over QUIC on LAN, local panels, locally-rendered viewport; `test_split_editor_parity` green |
| M2-17 | `forge --headless`; `test_headless_parity` green |

**Exit criterion:** a human and an agent edit the same project through the same bus, the
agent's changes appear in the human's undo stack, a recorded session replays exactly — and
the human is at a laptop driving a core on another machine, on Windows and on Linux, with
the project committing to a self-hosted git remote.

---

## M3 — The Universe  ·  **the vertical slice nothing else can do**

| id | Definition of done |
|---|---|
| M3-1 | `forge-astro` — galaxy → system → body chain from one `u64`; IMF, M-L-R, metallicity, occurrence rates |
| M3-2 | Hill-stability filter, Roche limits, tidal-locking classification, Jeans retention |
| M3-3 | Kepler ephemeris, fixed-5-Newton, with the 128-tick golden table in the determinism corpus |
| M3-4 | `forge-gen` — generator graph on the Ch.24 IR; global solve (tectonics, erosion, flow accumulation, rivers, lakes, cave trunks) |
| M3-5 | Local evaluation as a pure function of coarse fields + detail noise. **Spike S8 resolved.** |
| M3-6 | Edit-delta store, `(BodyId, morton)` keyed, **sparse index** (I-5) |
| M3-7 | `forge-climate` — EBM: insolation, obliquity, circulation cells from rotation rate, orographic precipitation, ocean diffusion |
| M3-8 | Köppen classification of a generated body, and a written sanity review by a domain agent |
| M3-9 | Weather state machine + local advection in-region |
| M3-10 | `forge-pcg` — scatter driven by the *same* climate/hydrology fields; field-with-promotion |
| M3-11 | Ocean, volumetric cloud, aerial perspective driven by Ch.15 fields |
| M3-12 | `FluidField` body repr — a star and a gas giant are enterable |
| M3-13 | **Spike S7 resolved** — physics determinism at region scale |
| M3-14 | `forge-volume` — the **mineable-world toggle**: V-1..V-5, V-9, V-12 from Ch.36 §36.3 |
| M3-15 | Brushes (sphere, box, smooth, flatten, paint) routed **through the command bus**, so an agent and a script can mine (V-9) |
| M3-16 | Toggle behaviour: off→on non-destructive, on→off lossy **with the warning in the UI, not only in the plan** |
| M3-17 | **Spike S12 resolved** — a 20 GB sample project measured on every store backend |

**Exit criterion — the demo that justifies the project:**
type a seed, get a galaxy; fly to a system, pick a planet; descend from orbit to the
ground in one unbroken shot; the desert is on the lee side of the mountains because the
wind put it there; dig a hole; ask an agent to place a village along the river it finds;
walk away, come back in a game-year, and the season has changed. Reproduce the whole
thing on another OS from the same seed, bit-for-bit.

---

## M4 — Simulation, and the Dogfood Gate

| id | Definition of done |
|---|---|
| M4-1 | Regionized scheduler in production — merge on proximity, split on load, **frame-equality as a merge precondition** |
| M4-2 | `test_region_determinism` — identical results at 1/2/4/16 regions |
| M4-3 | `forge-phys` — `avian` f64 in region-local frames, character controller, deterministic fixed step |
| M4-4 | Collision LOD rings per the Foundations; independent geometry and gameplay residency radii |
| M4-5 | Frame-crossing handoff: `rebase` + physics-world migration, the runtime's highest-risk sequence, with a property test |
| M4-6 | `forge-nav` — per-region navmesh from the density field; hierarchical long-range pathing |
| M4-7 | `forge-net` — transport, replication, **one authority per entity**, target-resolves-damage, interest caps at K≈50 |
| M4-8 | Profile system with hysteresis; `test_profile_equivalence` green (I5) |
| M4-9 | **DOGFOOD GATE (S9): the reference game ports onto Forge.** Its terrain, frames, ephemeris and edit store run on the engine. |
| M4-10 | A written list of everything the port needed that the engine did not have — this becomes M5's backlog |
| M4-11 | **`forge-2d` and the 2D preset** — the Ch.35 §35.2 list, complete, and nothing beyond it (**Spike S13**) |
| M4-12 | The 2D sample game ships: tilemaps, cutout animation, 2D lights, gamepad. This is 2D's acceptance test. |
| M4-13 | `test_2d_has_no_planetary_cost` green — a 2D project links none of the universe crates |
| M4-14 | Volumetric V-6, V-7, V-8, V-10, V-11 complete (navmesh, scatter, multiplayer sync, import, material paint) |
| M4-15 | **Spike S10 resolved** — WASM overhead measured at a generator node and a PCG rule |

**Exit criterion:** the dogfood gate. If the reference game cannot port, the engine is wrong and M5
does not start.

> **2D is parallelisable from M2.** It touches the core, the GPU device layer and the asset
> system, and nothing in the universe layer — so it can run as an independent agent stream
> without contending for the same files. It is listed under M4 because that is when it must
> be *done*, not when it may start.

---

## M5 — Authoring

| id | Definition of done |
|---|---|
| M5-1 | `forge-script` — WASM component host, capability-scoped, hot reload |
| M5-2 | Native `cdylib` shipping path from the same source crate |
| M5-3 | `test_sandbox_boundary` (I9) with its positive control |
| M5-4 | `forge-graph` — graph model, typed IR, Rust codegen, `test_no_interpreter` (I10) |
| M5-5 | **Spike S3 resolved** — compile latency measured, fallback implemented if needed |
| M5-6 | Nodes generated from `#[forge_api]`; graph↔text round trip for the mappable subset |
| M5-7 | Material graph and PCG graph moved onto the same IR (E-11) |
| M5-8 | `forge-play` — abilities, input remapping, save/load via reflection, localisation |
| M5-9 | Scene composition: nesting + inheritance (Godot's model) |
| M5-10 | An agent authors a gameplay system as a graph, compiles it, runs it, and asserts on the result — all through MCP |
| M5-11 | **Thin-client mode** — hardware-encoded viewport (NVENC/VA-API/AMF/QuickSync), AV1 with H.264 fallback, WebRTC for NAT traversal |
| M5-12 | **Spike S11 resolved** — latency budget measured on a real LAN pair, the number published, the fallback documented |
| M5-13 | Remote security: device pairing, TLS/DTLS, capability-scoped sessions on the Ch.22 model, audit log, loopback default |
| M5-14 | **Plugin index v1** — signed manifests, self-hostable, `forge add <id>` |
| M5-15 | `forge-identity` — local accounts, optional OIDC, device pairing vs user auth kept distinct |
| M5-16 | Roles as capability sets on the existing grant model; `test_role_enforcement` green with its escalation positive control |
| M5-17 | **Create Team → Add Member UI** — three clicks, invite by email or join code, role + optional path scoping, revocable pending invites |
| M5-18 | `forge-collab` — sandboxes as `baseline ⊕ deltas`; `test_sandbox_isolation` green (I19) |
| M5-19 | **Pull/Push mode** — publish is `ProjectStore::commit`; pull selects a revision range |
| M5-20 | Sandboxed play-in-editor — each user simulates against their own view |
| M5-21 | `forge-server` — the multi-user host: baseline + N sandboxes + N sessions, authz **per command**, rate limits, audit log |
| M5-22 | `test_plugin_runs_as_user` green — a sandbox plugin cannot reach a capability its owner lacks |
| M5-23 | **Spike S16 resolved** — 50 idle sandboxes measured; a sandbox is a row or the team-size cap is published |

---

## M6 — Scale-out

| id | Definition of done |
|---|---|
| M6-1 | `forge-jobs` Tier 0 — heterogeneous compute across every adapter, throughput-scheduled, fixed-order merge |
| M6-2 | Measured speedup on a real bake with a mismatched second GPU, documented with numbers |
| M6-3 | Tier 2 — `forge-farm` LAN daemon, same job interface, discovery, failure handling |
| M6-4 | Tier 1 — split-frame for editor viewports and offline renders **only**, with the limitation documented in user-facing docs |
| M6-5 | Virtualised geometry — meshlet DAG, GPU cluster cull + LOD select. **Spike S5 resolved.** |
| M6-6 | Dynamic GI (surfel/probe); hardware RT where present |
| M6-7 | Distributed CI running the determinism matrix on the farm |
| M6-8 | `forge-store` S3 and SQL backends; `test_store_backend_parity` across all four |
| M6-9 | Asset locking and semantic three-way merge on the reflect tree |
| M6-10 | **Live mode** — auto-advance on publish; `test_live_pull_equivalence` green (I20) |
| M6-11 | Command preconditions on every `EditorCommand`; `test_rebase_preconditions` detects a synthetic conflict rather than swallowing it |
| M6-12 | Scoped ownership — claim a subtree, others get live read-only |
| M6-13 | Conflict resolution panel; presence indicators; publish/review queue |
| M6-14 | **Spike S15 resolved** — real conflict rate measured; if too high, scoping becomes mandatory in Live |
| M6-15 | The docs state plainly that concurrent same-property editing is not supported (E-37) |

---

## M7 — Parity I

Animation stack (state machines, blend spaces, per-bone masks, root motion, IK,
**retargeting** — the acquisition feature), `AnimationPlayer`-style animate-any-reflected-
property, sequencer, audio (buses, HRTF, adaptive, occlusion from engine fields), UI
toolkit for shipping games, VFX/particles on the IR, 2-D decision (O-7), profiling polish.

Motion matching is here if it is here at all. It is not a 1.0 requirement.

---

## M8 — Ship

Platform HAL complete; Windows/macOS/Linux/Web/iOS/Android exports; the console HAL
boundary documented for licensed porters; documentation; sample projects; the package
index; governance and the foundation; **1.0 is drawn at "a real game ships on it,"** not at
"the comparison table is full."

---

## Gate rows — the format

`just gate` prints one row per invariant and per pinned contract:

```
I1   no-bare-position            BOUND     tests/liveness/test_frame_liveness.rs
I2   cross-platform-determinism  BOUND     tests/determinism/test_cross_platform_hash.rs
I7   command-liveness            BOUND     tests/liveness/test_command_liveness.rs
I10  no-graph-interpreter        UNBUILT   reason: forge-graph does not exist until M5
S1   wgpu-multi-adapter          AWAITING  reason: needs a second physical adapter
```

Three terminal states and nothing else: **BOUND** (a test file asserts it, and the test
has a non-vacuous positive control), **AWAITING(reason)** (blocked on hardware, an
operator, or an input that does not exist on disk), **UNBUILT(reason)** (the subsystem
does not exist yet, and the reason says why that is correct right now).

**There is no fourth state, and "nobody got to it" is not a reason.** The three questions
to ask of any AWAITING row, each learned the hard way: *is it blocked on
hardware, on a file nobody wrote, or on an input that does not exist?*
