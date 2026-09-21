# Forge — Decision Ledger

Three sections: **Inherited** (already ratified elsewhere — binding here), **Ratified**
(decided for the engine), **Rejected** (do not re-propose without new information), and
**Open** (needs a decision, with the decision owner named).

A decision is not ratified until it has a date and an owner. Draft-1 items below are
marked `PROPOSED` and are not binding until signed off.

---

## 1. Foundations — binding

These were decided on the dates shown and are **not** re-openable by this project. Where
Forge appears to contradict one, Forge has a defect.

| # | Decision | Ratified |
|---|---|---|
| I-1 | Terrain is stateless: `chunk = generate(body_seed, pos) ⊕ edits[key]` | 2026-08-19 |
| I-2 | Simulation scales with **players**, never geometry; regions/bubbles are player-driven | 2026-08-19 |
| I-3 | Cells/channels are an **addressing scheme, never a server topology** | 2026-08-27 |
| I-4 | Chunk key is `(body_id u32 partition, morton u64 sort)` — **not** a single int64 | 2026-08-27 |
| I-5 | Chunk index is a **sparse hash of generated-or-edited chunks only. Never an array.** | 2026-08-19 |
| I-6 | Noise basis takes **integer/fixed-point voxel coords**, never float world position | 2026-08-19 |
| I-7 | Generator splits by **scale**: global solve once at low res, then a pure local function | 2026-08-19 |
| I-8 | **Unwarped cubic lattice** for volumetric bodies; cube-sphere projection is for heightfields only | 2026-08-19 |
| I-9 | Every position is `(frame_id, local)`. Never a single global vector. | 2026-08-27 |
| I-10 | **Kepler ephemeris `f(t)`, fixed 5 Newton steps in `double`.** Never n-body integration, never a convergence loop. | 2026-08-27 |
| I-11 | **State is shared, simulation is not** — the Layer1/Layer2 seam is the channel boundary | 2026-08-27 |
| I-12 | Exactly **one authority per entity**; damage resolved by the *target's* authority | 2026-08-19 |
| I-13 | A profile may change cost. It may **never** change outcome. | 2026-08-19 |
| I-14 | Unattended state must be expressible as `f(elapsed, state)` — no world-tick service | 2026-08-19 |
| I-15 | **Field-with-promotion** for rings, asteroids, debris; touching one makes it real | 2026-08-27 |
| I-16 | **Real astronomical distances are kept.** Bridges and fast ships are the gate. | 2026-08-27 |
| I-17 | Any body whose orbital period exceeds a human lifetime is a **constant**, not a sim entity | 2026-08-27 |
| I-18 | Interest caps (K≈50) come first — they linearise the N², the largest single win | 2026-08-19 |
| I-19 | Classify profiles by **measurement, not label**, with hysteresis | 2026-08-19 |
| I-20 | A canonical `int64` tick; a client wall clock never feeds a physics-relevant evaluation | 2026-08-27 |

*"Foundations" refers to planetary-architecture work ratified before this plan. Those
decisions are binding here and are restated in full above; the source documents are not
published.*

---

## 2. Ratified for the engine

All `PROPOSED` pending sign-off.

| # | Decision | Rationale | Status |
|---|---|---|---|
| E-1 | Language is Rust throughout: engine, gameplay, extensions, and the target of blueprint codegen | safety, determinism, and the ownership model is what makes the regionized scheduler possible | PROPOSED |
| E-2 | Licence `Apache-2.0 OR MIT`; DCO not CLA; trademark the name | Rust convention + patent grant; a CLA contradicts "no rug-pull" | PROPOSED |
| E-3 | Depend on `bevy_ecs`/`bevy_reflect`/`bevy_app`/`bevy_tasks`/`bevy_asset` as **libraries**. Do **not** depend on `bevy` or `bevy_render`. | best ECS + reflection in Rust; but `bevy_render` assumes f32 and one adapter, and both are wrong for us | PROPOSED |
| E-4 | Own the renderer, on `wgpu`, WGSL-only via `naga` | f64→camera-relative and the adapter pool are not patches onto someone else's graph | PROPOSED |
| E-5 | `avian` for physics (generic over f32/f64), `rapier` as fallback | f64 support is the deciding factor | PROPOSED |
| E-6 | **`forge-num` vendors its own transcendentals.** Platform `libm` is not bit-reproducible across OS or arch. | a silent, low-bit divergence that a hash amplifies into a different chunk — the worst class of bug | PROPOSED |
| E-7 | **One annotation, four outputs**: `#[forge_api]` emits reflect registration, blueprint node, JSON schema, and command variant | the only way four surfaces stay in sync for the life of the project | PROPOSED |
| E-8 | **Headless core + command bus. The UI has no privileged path.** | makes agent access complete by construction instead of perpetually lagging | PROPOSED |
| E-9 | **MCP is five tools and three resource roots.** Compose, do not enumerate. | an abandoned 626-tool server on a prior project; do not re-learn it | PROPOSED |
| E-10 | **Blueprints compile to Rust.** Nothing interprets a graph at runtime. | UE's BP VM is ~10× slower than C++ and "port to C++ before ship" is a known, expensive tax | PROPOSED |
| E-11 | **One IR** serves blueprint, material, generator, and PCG graphs | four graph compilers is three too many | PROPOSED |
| E-12 | Agent-authored and third-party code runs in **WASM**; native `cdylib` is a shipping path, never an authoring path | sandboxing and hot reload fall out together | PROPOSED |
| E-13 | **Regionized multithreading inside one process**, ahead of process-level projection | the Foundations name it "strictly better — and UE cannot do it." Rust can. | PROPOSED |
| E-14 | Multi-GPU in **three tiers**: heterogeneous compute (commit), LAN farm (commit), split-frame realtime (editor/offline only, spike-gated) | the honest split; over-promising realtime multi-GPU would cost credibility | PROPOSED |
| E-15 | Astrophysics is **closed-form with real statistics** (IMF, occurrence rates, Hill stability, Jeans escape, Roche, tidal locking) | O(1), deterministic, defensible as "features of the real one" | PROPOSED |
| E-16 | Climate is a **2-D energy-balance model**, not a GCM; weather is a stochastic process conditioned on it | Köppen-classifiable output for a few thousand lines; seasons and Milankovitch fall out free | PROPOSED |
| E-17 | Three `BodyRepr` impls chosen per body: unwarped lattice / cube-sphere quadtree / fluid field | the Foundations reject projection for volumetric bodies but heightfield worlds should not pay 3D cost | PROPOSED |
| E-18 | Platform backends live behind a **HAL trait boundary**; consoles implemented privately by licensed porters | the only legal shape for an open engine; Godot's answer | PROPOSED |
| E-19 | Frame tree has **arbitrary depth**; `FrameKind` is a physics hint, not a tier count | makes "is inter-galactic a fourth tier" a content decision, not an engine one | PROPOSED |
| E-20 | **The dogfood gate: the reference game must port onto Forge by end of M4**, and that port is the acceptance test | the only real defence against scope collapse | PROPOSED |
| E-21 | **Three workspace presets: 2D, 3D, Planetary.** Presets are **data** (`presets/*/workspace.ron`), set defaults only, and never gate capability (I15) | Unity's URP/HDRP fork is the anti-pattern: a project-start decision that splits the ecosystem and every tutorial | PROPOSED |
| E-22 | **3D is the degenerate case of Planetary** — one frame, one static body. **2D is the one genuinely different kind.** | no fork, no second code path; only the 2D render path and 2D solver are separate | PROPOSED |
| E-23 | **True 2D**, not 3D with an ortho camera. *(Resolves O-7.)* | Godot's most-cited draw; half-doing it forfeits the cheapest audience in the engine to serve | PROPOSED |
| E-24 | **Kernel + plugins, and the engine's own subsystems are plugins** (I16) | the only honest implementation of "any feature is modifiable" | PROPOSED |
| E-25 | **Extension points support `replace`, `remove` and `chain`, not only `add`** | append-only hooks cannot modify a built-in, which is what was actually asked for | PROPOSED |
| E-26 | **Two plugin flavours: WASM (drop-in, sandboxed) and source/cargo (native, full access).** No drop-in native ABI. | Rust has no stable ABI; the skew failure mode is a silent miscompile | PROPOSED |
| E-27 | **One capability-grant model** shared by MCP sessions, plugins, and remote sessions | one thing to audit instead of three | PROPOSED |
| E-28 | **`ProjectStore` is a trait**; LocalFs / Git / S3 / SQL are peers (I17). Assets are content-addressed blobs; project data is diffable RON; generated content is a `SeedPath` and is not stored | git is bad at binaries, and a world that is a function needs no storage at all | PROPOSED |
| E-29 | **Semantic three-way merge on the reflect tree**, plus asset locking | this is what studios buy Perforce for, and reflection makes it nearly free | PROPOSED |
| E-30 | **Three remote modes: headless / split editor / thin client.** Split editor runs panels locally and streams only the viewport. | a direct dividend of I7; a general remote-desktop tool cannot do it because it does not know the app's structure | PROPOSED |
| E-31 | **Self-hosted only** — no account, no relay, no telemetry, loopback by default | the positioning is the product; a cloud dependency contradicts it | PROPOSED |
| E-32 | **The volumetric/mineable layer is a per-world toggle, hybrid with static meshes**, and its edits go through the command bus | not a global engine mode; V-9 (bus-routed undo) is the item that is expensive to add later | PROPOSED |
| E-33 | **Windows and Linux are the only targets** (I18). **macOS is out of scope** — the Mac is an authoring machine for planning, not a development or target platform. The HAL lets someone add it later. | a platform nobody tests is a platform that is broken; carrying an unfunded target costs more than admitting it | PROPOSED |
| E-34 | **A sandbox is `baseline ⊕ deltas`** (I19) — the same equation as the terrain layer, one level up. A sandbox is a **row, not a process**. | same problem (many readers, few writers, cheap isolation), same answer; and it keeps hosting affordable | PROPOSED |
| E-35 | **Live and Pull are two subscription policies over one command stream** (I20), switchable mid-session at no cost | two systems would drift; this is I5's shape applied to collaboration | PROPOSED |
| E-36 | **Publishing is `ProjectStore::commit`.** Push/pull are the store operations, not new concepts. | Ch.33 already built it; a second history would be a second source of truth | PROPOSED |
| E-37 | **Concurrent same-property editing is NOT promised.** Prevent with scoped ownership, detect with command preconditions, resolve with a conflict UI — and say so in user-facing docs. | a CRDT over a scene graph with referential integrity is research-grade; the expectation gap is where users get hurt | PROPOSED |
| E-38 | **Identity is local accounts or bring-your-own OIDC.** Never a Forge-operated account service. Device pairing authenticates the machine; user auth authenticates the person. | consistent with E-31; two different things that are routinely conflated | PROPOSED |
| E-39 | **Roles are capability sets on the existing grant model** (Ch.22/32/34) — the fourth user of one mechanism | one thing to audit instead of four | PROPOSED |
| E-40 | **A user's plugins and scripts run with that user's capabilities, never the server's** | if a Developer's plugin executes with server authority, every role above it is decorative | PROPOSED |
| E-41 | **Published publicly with no licence for now — all rights reserved.** A permissive `Apache-2.0 OR MIT` licence is applied when the project is ready to accept use and contribution. | deliberate staging: visibility now, grant later. See the note below — it has consequences that need managing. | RATIFIED 2026-09-20 (user) |

---

## 3. Rejected — do not re-propose

| Rejected | Why | Source |
|---|---|---|
| Shrinking the solar system / fake distances | distance is the *gate* that makes bridges and fast ships worth building, and a slow transfer is a database row — architecturally **cheaper** than a short system | the Foundations, 2026-08-27 |
| N-body gravity integration | diverges between client, server and restart; no random access at arbitrary `t` | the Foundations |
| A `while (err > eps)` loop anywhere in generation | different iteration counts on different compilers and platforms | the Foundations |
| Cube-sphere projection for **volumetric** bodies | anisotropic voxels break mining volume, ore density, and collision consistency | the Foundations |
| A dense chunk index array | 134 M slots per 16.384 km cell; 8.6 GB indexing chunks that do not exist | the Foundations |
| Real-world geodata import | does not translate into generator graphs cleanly | the Foundations |
| A blueprint virtual machine | see E-10 | this plan |
| Depending on `bevy` the framework | `bevy_render` assumes f32 and one adapter | this plan |
| The Autodesk FBX SDK | poisons the licence | this plan |
| GPL for the engine | makes console ports and many commercial uses impossible | this plan |
| A CLA | contradicts the "no rug-pull" positioning, which is a real asset | this plan |
| Royalties or per-seat pricing, ever | see Unity 2023 | this plan |
| A drop-in **native** plugin ABI (`abi_stable`/`stabby` dylib loading) | no stable Rust ABI; version skew silently miscompiles rather than erroring. Drop-in = WASM; native = compiled in | this plan |
| A render-pipeline choice made at project creation | Unity's URP/HDRP fork; splits assets, docs and tutorials permanently | this plan |
| A hosted remote-editor service, relay, or account requirement | contradicts the self-hosted positioning, which is the product | this plan |
| Telemetry | — | this plan |
| 3D-with-an-ortho-camera as the 2D story | known-inferior for pixel art; forfeits the 2D audience | this plan |
| Promising live multi-user editing before M8 | a classic two-year sink; the command log keeps the door open without promising it | this plan |
| Binding a process to a region, channel, cell, or body | this is *the* recurring mistake; every mechanism added to fix it is machinery to undo it | the Foundations |

> **The meta-lesson, stated once because it recurs at every scale:** *binding compute to
> geometry* was the source of nearly every problem in the Foundations' superseded design, and it
> will be tempting again — at the cell level, the channel level, the GPU level, and the
> farm level. Every time it appears, the fix is the same: make the thing an **address**,
> and spawn compute against it on demand.

---

## 4. Open — needs a decision

| # | Question | Blocks | Owner |
|---|---|---|---|
| ~~O-1~~ | ~~The name.~~ **RESOLVED 2026-09-20 → Forge Engine.** Crate prefix `forge-*`. See the naming note below. | — | — |
| ~~O-2~~ | ~~Repo home and visibility.~~ **RESOLVED 2026-09-20 → `Loom-Forge-Studios/Forge-Engine`, public, unlicensed.** | — | — |
| O-3 | Is inter-galactic a fourth channel tier? (open in the Foundations; they lean yes) | content only, given E-19 | user |
| O-4 | Exact split of what the global terrain solve owns vs the local function (open in the Foundations) | Ch.13, M3 | architect |
| O-5 | Extraction algorithm: Transvoxel vs surface nets vs dual contouring | Ch.12, M1 | render agent, with a written comparison |
| O-6 | Editor UI: `egui` permanently, or `egui` as an M2 stopgap with a custom retained layer? | Ch.21, M2 | user + UI agent |
| ~~O-7~~ | ~~2-D pipeline?~~ **RESOLVED 2026-09-20 → true 2D (E-23), Ch.35, M4.** | — | — |
| O-8 | Minimum supported hardware. Sets the fallback-path budget for the whole renderer. | Ch.9, M1 | user |
| O-9 | Binary stars — allowed? They complicate the frame tree meaningfully. | Ch.16, M3 | architect |
| O-10 | Web export: a first-class target or best-effort? (Determines whether `f64` everywhere is affordable on wasm.) | Ch.27 | user |
| O-11 | Default store backend for a new project: `LocalFs`, or `LocalFs` + an offer to link a remote on first save? | Ch.33, M2 | user |
| O-12 | Does the first-party plugin index require signing, and who holds the key? | Ch.32, M5 | user |
| O-13 | Is the split editor's optimistic local preview allowed to diverge from the remote core for one frame, or must every change round-trip? (Latency vs. I8 strictness.) | Ch.34, M5 | architect |
| O-14 | Does the volumetric toggle default **on** in the Planetary preset? (the reference game wants it; a first-time user may not.) | Ch.31/36, M3 | user |
| ~~O-15~~ | ~~macOS scope.~~ **RESOLVED 2026-09-20 → out of scope (E-33).** | — | — |
| O-16 | Review gate on publish: can a Developer publish to baseline directly, or is Maintainer approval required by default? | Ch.37, M5 | user |
| O-17 | Default sandbox visibility — `Private` or `Team`? | Ch.37, M5 | user |
| O-18 | When does the licence land? It gates external contribution (see below). | Appendix A | **user** |


---

## 5. Two notes on the repository as published

### 5.1 Unlicensed is a real state with real consequences (E-41)

Publishing publicly with **no licence file** means **all rights reserved**. That achieves
the stated intent — nobody may legally use, fork for use, modify or redistribute the code
— but three consequences need managing rather than discovering:

1. **GitHub's Terms of Service still permit forking and viewing within GitHub** for any
   public repository. "Nobody can take the code" is true legally and only partly true
   practically. If that matters, the repository should be private until the licence lands.
2. **Contributions received before a licence exists are legally murky.** A contributor
   holds copyright in their own contribution and, with no licence and no CLA/DCO in place,
   has granted nothing. Merging outside PRs now creates a cleanup problem later.
   **Recommended: disable pull requests, or state in the README that PRs are not accepted
   until the licence lands.**
3. **No licence means no contributors, no packagers and no downstream** — which is fine and
   is the point, but it also means the community-building work in Appendix A cannot start
   until **O-18** is answered.

None of this is a reason to change course. It is a reason to write the intent down in the
README so that people read it as deliberate rather than as an oversight.

### 5.2 The name has two collisions worth knowing about

- **Minecraft Forge** is the dominant mod loader in the Minecraft ecosystem — the same
  ecosystem an adjacent project already operates in. "Forge" plus "game development" will
  be read as Minecraft modding by a meaningful fraction of the audience. *Forge Engine* in
  full, consistently, mitigates most of it.
- **`VoxelPlugin/Forge`** already exists as "Build system written in Unreal," in an
  organisation that is already accessible from this account.

Neither is a blocker and neither is a legal problem at this scale. Both are worth knowing
before the name is on a logo, a domain and a crates.io namespace. **Check `forge-*` name
availability on crates.io before M0 publishes anything** — a squatted prefix discovered at
M1 is an annoying rename.
