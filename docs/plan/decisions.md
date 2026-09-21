# Forge — Decision Ledger

Three sections: **Inherited** (already ratified elsewhere — binding here), **Ratified**
(decided for the engine), **Rejected** (do not re-propose without new information), and
**Open** (needs a decision, with the decision owner named).

A decision is not ratified until it has a date and an owner. Draft-1 items below are
marked `PROPOSED` and need the user's sign-off.

---

## 1. Inherited — binding, from the Foundations architecture notes

These were decided on the dates shown and are **not** re-openable by this project. Where
Forge appears to contradict one, Forge has a defect.

| # | Decision | Source | Date |
|---|---|---|---|
| I-1 | Terrain is stateless: `chunk = generate(body_seed, pos) ⊕ edits[key]` | the Foundations | 2026-08-19 |
| I-2 | Simulation scales with **players**, never geometry; regions/bubbles are player-driven | the Foundations | 2026-08-19 |
| I-3 | Cells/channels are an **addressing scheme, never a server topology** | the Foundations, the Foundations | 2026-08-27 |
| I-4 | Chunk key is `(body_id u32 partition, morton u64 sort)` — **not** a single int64 | the Foundations corrections | 2026-08-27 |
| I-5 | Chunk index is a **sparse hash of generated-or-edited chunks only. Never an array.** | the Foundations | 2026-08-19 |
| I-6 | Noise basis takes **integer/fixed-point voxel coords**, never float world position | the Foundations | 2026-08-19 |
| I-7 | Generator splits by **scale**: global solve once at low res, then a pure local function | the Foundations | 2026-08-19 |
| I-8 | **Unwarped cubic lattice** for volumetric bodies; cube-sphere projection is for heightfields only | the Foundations | 2026-08-19 |
| I-9 | Every position is `(frame_id, local)`. Never a single global vector. | the Foundations | 2026-08-27 |
| I-10 | **Kepler ephemeris `f(t)`, fixed 5 Newton steps in `double`.** Never n-body integration, never a convergence loop. | the Foundations | 2026-08-27 |
| I-11 | **State is shared, simulation is not** — the Layer1/Layer2 seam is the channel boundary | the Foundations | 2026-08-27 |
| I-12 | Exactly **one authority per entity**; damage resolved by the *target's* authority | the Foundations | 2026-08-19 |
| I-13 | A profile may change cost. It may **never** change outcome. | the Foundations | 2026-08-19 |
| I-14 | Unattended state must be expressible as `f(elapsed, state)` — no world-tick service | the Foundations | 2026-08-19 |
| I-15 | **Field-with-promotion** for rings, asteroids, debris; touching one makes it real | the Foundations | 2026-08-27 |
| I-16 | **Real astronomical distances are kept.** Bridges and fast ships are the gate. | the Foundations header | 2026-08-27 |
| I-17 | Any body whose orbital period exceeds a human lifetime is a **constant**, not a sim entity | the Foundations | 2026-08-27 |
| I-18 | Interest caps (K≈50) come first — they linearise the N², the largest single win | the Foundations | 2026-08-19 |
| I-19 | Classify profiles by **measurement, not label**, with hysteresis | the Foundations | 2026-08-19 |
| I-20 | A canonical `int64` tick; a client wall clock never feeds a physics-relevant evaluation | the Foundations | 2026-08-27 |

*the Foundations = the Foundations. The Foundations = the Foundations.*

---

## 2. Ratified for the engine

All `PROPOSED` pending sign-off.

| # | Decision | Rationale | Status |
|---|---|---|---|
| E-1 | Language is Rust throughout: engine, gameplay, extensions, and the target of blueprint codegen | safety, determinism, and the ownership model is what makes the regionized scheduler possible | PROPOSED |
| ~~E-2~~ | ~~`Apache-2.0 OR MIT`; DCO not CLA.~~ **REVERSED 2026-09-20 → E-42.** The engine is commercial and source-available. A CLA with copyright assignment is now *required*, not discouraged: you cannot sell a product containing a contribution you hold no grant in. | — | SUPERSEDED |
| E-3 | Depend on `bevy_ecs`/`bevy_reflect`/`bevy_app`/`bevy_tasks`/`bevy_asset` as **libraries**. Do **not** depend on `bevy` or `bevy_render`. | best ECS + reflection in Rust; but `bevy_render` assumes f32 and one adapter, and both are wrong for us | PROPOSED |
| E-4 | Own the renderer, on `wgpu`, WGSL-only via `naga` | f64→camera-relative and the adapter pool are not patches onto someone else's graph | PROPOSED |
| E-5 | `avian` for physics (generic over f32/f64), `rapier` as fallback | f64 support is the deciding factor | PROPOSED |
| E-6 | **`forge-num` vendors its own transcendentals.** Platform `libm` is not bit-reproducible across OS or arch. | a silent, low-bit divergence that a hash amplifies into a different chunk — the worst class of bug | PROPOSED |
| E-7 | **One annotation, four outputs**: `#[forge_api]` emits reflect registration, blueprint node, JSON schema, and command variant | the only way four surfaces stay in sync for the life of the project | PROPOSED |
| E-8 | **Headless core + command bus. The UI has no privileged path.** | makes agent access complete by construction instead of perpetually lagging | PROPOSED |
| E-9 | **MCP is five tools and three resource roots.** Compose, do not enumerate. | the scrapped 626-tool a prior MCP server; do not re-learn it | PROPOSED |
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
| E-20 | **The dogfood gate: the Foundations must port onto Forge by end of M4**, and that port is the acceptance test | the only real defence against scope collapse | PROPOSED |
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
| E-31 | **Self-hosted only to *run*** — no relay, no telemetry, loopback by default. **Amended by E-46:** an account exists to *buy* a licence, never to use the software. | the runtime pillar survives; only the purchase path is online | AMENDED 2026-09-20 |
| E-32 | **The volumetric/mineable layer is a per-world toggle, hybrid with static meshes**, and its edits go through the command bus | not a global engine mode; V-9 (bus-routed undo) is the item that is expensive to add later | PROPOSED |
| E-33 | **Windows and Linux are the only targets** (I18). **macOS is out of scope** — the Mac is an authoring machine for planning, not a development or target platform. The HAL lets someone add it later. | a platform nobody tests is a platform that is broken; carrying an unfunded target costs more than admitting it | PROPOSED |
| E-34 | **A sandbox is `baseline ⊕ deltas`** (I19) — the same equation as the terrain layer, one level up. A sandbox is a **row, not a process**. | same problem (many readers, few writers, cheap isolation), same answer; and it keeps hosting affordable | PROPOSED |
| E-35 | **Live and Pull are two subscription policies over one command stream** (I20), switchable mid-session at no cost | two systems would drift; this is I5's shape applied to collaboration | PROPOSED |
| E-36 | **Publishing is `ProjectStore::commit`.** Push/pull are the store operations, not new concepts. | Ch.33 already built it; a second history would be a second source of truth | PROPOSED |
| E-37 | **Concurrent same-property editing is NOT promised.** Prevent with scoped ownership, detect with command preconditions, resolve with a conflict UI — and say so in user-facing docs. | a CRDT over a scene graph with referential integrity is research-grade; the expectation gap is where users get hurt | PROPOSED |
| E-38 | **Identity is local accounts or bring-your-own OIDC.** Never a Forge-operated account service. Device pairing authenticates the machine; user auth authenticates the person. | consistent with E-31; two different things that are routinely conflated | PROPOSED |
| E-39 | **Roles are capability sets on the existing grant model** (Ch.22/32/34) — the fourth user of one mechanism | one thing to audit instead of four | PROPOSED |
| E-40 | **A user's plugins and scripts run with that user's capabilities, never the server's** | if a Developer's plugin executes with server authority, every role above it is decorative | PROPOSED |
| ~~E-41~~ | ~~Unlicensed now, permissive licence later.~~ **SUPERSEDED 2026-09-20 → E-42.** The engine will not be open source. | — | SUPERSEDED |
| E-42 | **Forge is commercial and source-available, never open source.** Source is public and readable; use requires a paid licence. | owner's decision, 2026-09-20 | **RATIFIED** |
| E-43 | **$5 one-time per-developer source licence, perpetual, covering all versions.** | owner's decision | **RATIFIED** |
| E-44 | **5% royalty on gross revenue of any product or plugin built using the source, from the first dollar.** | owner's decision. See §6 for a costed recommendation to add a threshold and exempt plugins — recorded, not applied. | **RATIFIED** |
| E-45 | **Permitted:** build and ship games/apps; create and distribute plugins and assets for the editor; modify the source for your own use; redistribute `forge-runtime` **in binary form only**, embedded in a shipped product. **Prohibited:** redistributing engine source, sublicensing, distributing the editor, or producing a competing engine derived from this one. | this is the shape the owner described, written in licence-grant terms | **RATIFIED** |
| E-46 | **An account exists to buy a licence. The software never requires one to run.** Activation is at install; there is no runtime check, no phone-home, and **nothing in a shipped game** (I21). | a paid engine needs a purchase path; a DRM'd runtime would poison the product and every shipped game with it | PROPOSED |
| E-47 | **Do not hand-roll the licence.** Draft the EULA from the Unreal Engine EULA's structure, or from PolyForm Perimeter / FSL as a base, and have a lawyer review it **before any money changes hands**. | a homemade licence is how projects discover they cannot enforce anything; this is cheap now and unfixable later | PROPOSED |
| E-48 | **Recommended plugin boundary (needs owner sign-off):** charge 5% as a **commission on sales through the Forge plugin index**, 0% elsewhere; a WASM plugin built against the public API alone needs no source licence. | as the storefront you are the payment processor, so the commission collects itself; a universal royalty on plugins sold elsewhere needs visibility you will not have and rests on weaker footing. Epic takes **12%** on Fab, so 5% is a marketing line rather than an objection. | PROPOSED |
| E-49 | **Dependency policy tightens rather than relaxes:** permissive-only (MIT/Apache-2.0/BSD/Zlib), **no copyleft of any strength**, and a generated `NOTICES` file in every distribution. | MIT and Apache both permit commercial closed-source use *provided notices are preserved*; a single GPL dependency would make the whole product undistributable | **RATIFIED** |
| E-50 | **No DRM in the runtime and no telemetry anywhere.** Royalty compliance is self-reported with contractual audit rights, the Unreal model. | runtime DRM would be defeated in a week, would break offline and console builds, and would be inherited by every customer's shipped game | PROPOSED |

---

## 3. Rejected — do not re-propose

| Rejected | Why | Source |
|---|---|---|
| Shrinking the solar system / fake distances | distance is the *gate* that makes bridges and fast ships worth building, and a slow transfer is a database row — architecturally **cheaper** than a short system | the Foundations, 2026-08-27 |
| N-body gravity integration | diverges between client, server and restart; no random access at arbitrary `t` | the Foundations |
| A `while (err > eps)` loop anywhere in generation | different iteration counts on different compilers and platforms | the Foundations, the Foundations |
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
| Binding a process to a region, channel, cell, or body | this is *the* recurring mistake; every mechanism added to fix it is machinery to undo it | the Foundations, the Foundations |

> **The meta-lesson, stated once because it recurs at every scale:** *binding compute to
> geometry* was the source of nearly every problem in the Foundations' superseded design, and it
> will be tempting again — at the cell level, the channel level, the GPU level, and the
> farm level. Every time it appears, the fix is the same: make the thing an **address**,
> and spawn compute against it on demand.

---

## 4. Open — needs a decision

| # | Question | Resolution | Date |
|---|---|---|---|
| ~~O-1~~ | The name | **Forge Engine**, crate prefix `forge-*` | 2026-09-20 |
| ~~O-2~~ | Repo home and visibility | `Loom-Forge-Studios/Forge-Engine`, public, source-available | 2026-09-20 |
| ~~O-3~~ | Is inter-galactic a fourth frame tier? | **Yes — four tiers.** A Galaxy frame spanning intergalactic space puts `f64` local offsets near 1e22 m, where the ulp is ~1e6 m; a separate tier keeps every frame numerically well-conditioned. It also makes the crossing *a place*, mirroring the bridge/ship gate structure. E-19 already makes the frame tree arbitrary-depth, so this costs one `FrameKind` hint and no engine code. | 2026-09-20 |
| ~~O-4~~ | Global vs local terrain solve split | **The membership rule: if computing it correctly requires knowing about something more than one chunk away, it is global.** *Global (once, low-res, whole body):* tectonic plates and uplift, base elevation, thermal and hydraulic erosion, **flow accumulation**, river networks, priority-flood lake filling, sea level, coastline, biome assignment, cave **trunk** topology as a graph, ore provinces, and the space-view LOD pyramids. *Local (pure function per chunk):* detail noise conditioned on macro slope and flow accumulation, cave **branches** off the nearest trunk node, surface material blend, small features, and the SDF itself. Global output is a set of int16/f16 pyramids per body (~30–80 MB), treated as a derived asset keyed by seed. | 2026-09-20 |
| ~~O-5~~ | Extraction algorithm | **Dual Contouring with QEF vertex placement (manifold variant), with dual-grid seam stitching between LOD levels.** Marching Cubes cannot represent a sharp corner, and a cube dug with a box brush **must** come out as a cube — that single requirement decides it. DC also stores Hermite data and handles overhangs and caves natively. *Fallback if seam stitching proves intractable:* Transvoxel over Marching Cubes, accepting rounded mined faces as a stated, visible quality loss. | 2026-09-20 |
| ~~O-6~~ | `egui` permanent or stopgap? | **Stopgap, M2–M4.** It is excellent for tools and wrong for shipping game UI: immediate-mode, weak text shaping and IME, no real accessibility, hard to style to a brand. Ch.21 requires **one** widget layer for the editor *and* games, so a retained layer lands at M5–M7 — `taffy` for layout, `cosmic-text` for shaping, `AccessKit` for accessibility — and the editor migrates onto it. The migration cost is real and is budgeted, not discovered. | 2026-09-20 |
| ~~O-7~~ | 2-D pipeline? | **True 2D** (E-23), Ch.35, M4 | 2026-09-20 |
| ~~O-8~~ | Minimum supported hardware | **Floor: Vulkan 1.2 / DX12 feature level 12_0, 4 GB VRAM, 8 GB RAM, 4 cores** — roughly GTX 1060 / RX 580 / Arc / a recent iGPU. Compute shaders are required and non-negotiable, because generation is GPU. Bindless is an optional fast path, not a floor. **Explicitly unsupported: DX11, OpenGL, Vulkan 1.0/1.1, 32-bit.** Each fallback path doubles the renderer's test matrix, and nobody assembling used GPUs for planet-scale worlds is doing it on a GTX 660. | 2026-09-20 |
| ~~O-9~~ | Binary stars? | **Yes — hierarchical only.** P-type (circumbinary) and S-type (circumstellar) orbits are still Keplerian: each is a two-body problem about a barycentre, so the ephemeris model survives untouched. Costs one barycentre frame, which the arbitrary-depth tree already supports. Stability filtered by the Holman–Wiegert critical semi-major axis. **Rejected: non-hierarchical triples** — chaotic, require integration, violate I-10. | 2026-09-20 |
| ~~O-10~~ | Web export | **Tier-3, best-effort. 2D preset fully supported, 3D supported, Planetary not supported on web.** `f64` is *not* the blocker — wasm has it natively. The blockers are WebGPU maturity, the 4 GB wasm32 address space against a planetary working set, threads requiring cross-origin isolation, and uneven compute parity. Drawing the line at the preset boundary is honest and testable. | 2026-09-20 |
| ~~O-11~~ | Default store backend | **`LocalFs`, with a one-click "link a remote" offer on first save — not at project creation.** Project creation is the worst moment to ask; the user wants to see the editor, not configure git. First save is when they have something to lose. A project is always usable offline; a remote is additive. | 2026-09-20 |
| ~~O-12~~ | Plugin index signing | **Yes, and the project holds the root key.** With money moving through the index, provenance matters more rather than less, and signed manifests are what make a royalty obligation attributable. Self-hosted indexes use their own root — an org running an internal index needs no blessing. | 2026-09-20 |
| ~~O-13~~ | Optimistic local preview in the split editor? | **Yes, and it does not violate I8 — `dry_run` *is* the optimistic preview.** Apply the dry-run locally for instant feedback, send the command, reconcile against the authoritative response; a divergence rolls back and re-applies, visible as a one-frame correction exactly like network prediction. The alternative — round-tripping every slider drag — makes the split editor feel worse than Parsec and forfeits the feature. Guard: `test_optimistic_reconciliation` forces a divergence and asserts convergence. | 2026-09-20 |
| ~~O-14~~ | Volumetric default-on in the Planetary preset? | **Off.** It is the expensive default and most planetary projects do not need a mineable world. Turning it on is one non-destructive toggle (Ch.36 §36.2). The new-project dialog offers *Planetary* and *Planetary (Volumetric)* as two **templates** — a template difference, not a preset difference, which keeps I15 clean. | 2026-09-20 |
| ~~O-15~~ | macOS scope | **Out of scope** (E-33) | 2026-09-20 |
| ~~O-16~~ | Review gate on publish to baseline? | **Developer publishes directly by default; a per-project setting enables Maintainer approval; per-path override on top.** A mandatory gate on a two-person team is friction with no benefit, and the common case is small teams. A large team wants it on `engine-config/**` and not on `scenes/sandbox/**`. | 2026-09-20 |
| ~~O-17~~ | Default sandbox visibility | **`Team`** — readable by teammates, never writable. Private-by-default makes presence indicators show nothing and forces a ceremony before anyone can review; it also breaks Live mode's "watch me" case entirely. `Private` stays one click away. | 2026-09-20 |
| ~~O-18~~ | When does the licence land? | **Never as open source** (E-42). Source-available now; the commercial EULA is drafted and reviewed before the first sale, which gates M8. | 2026-09-20 |
| **O-19** | **Does E-48 apply** — is the 5% on plugins a *storefront commission* (index sales only) or a *universal royalty* (all plugins, anywhere)? | **needs owner sign-off** | open |
| **O-20** | Is there a royalty threshold, and is it per-product or per-company? | **needs owner sign-off** — see §6 | open |
| **O-21** | Per-developer seat or per-person licence? They differ materially for studios. | needs owner sign-off | open |
| **O-22** | Who drafts and reviews the EULA, and by when? Gates the first sale. | **needs owner** | open |


---

## 6. Recorded recommendation on the commercial terms — 2026-09-20

E-43 and E-44 are ratified and implemented as stated. This section records three costed
observations so they are available later; **none of them is applied.**

**1. Make the 5% a marketplace commission, not a universal plugin royalty.**

*Corrected 2026-09-20 after the owner pointed out Fab's 12%.* The correction matters and
sharpens the recommendation rather than removing it. What Epic actually does:

| Epic charges | On what | Rate |
|---|---|---|
| Marketplace commission | sales **through Fab**, Epic's storefront | **12%** |
| Product royalty | game gross **above $1M lifetime, per product** | 5% |
| Plugins sold anywhere else — own site, itch, Gumroad | — | **0%** |

So Epic monetises plugins through an **optional storefront commission**, not a mandatory
royalty on every plugin wherever it is sold. The two differ in enforceability more than in
principle: as the storefront you are the payment processor, so a commission collects
itself; a universal royalty on plugins sold elsewhere requires visibility you will not have
and rests on the weaker *Google v. Oracle* footing (a plugin touching only the public API
is probably not a derivative work, so the leverage is contractual and binds only people who
took the source).

**Recommendation, revised: charge 5% as a commission on sales through the Forge plugin
index, and 0% on plugins distributed elsewhere.** The revenue is the same where it is
actually collectable, enforcement becomes automatic, and **"5% against Fab's 12%" is a
marketing line rather than an objection to answer.** Undercutting the incumbent by more
than half on the thing plugin authors care about is a strong opening position.

**2. Consider a royalty threshold on products.** The 5% product royalty matches Unreal's
rate exactly; what differs is the threshold. Unreal exempts the first $1M of lifetime gross
per product, Unity was free under $200K. A **$100K per-product threshold** costs very
little — few products cross it — and removes the loudest objection a new engine will face
from the cost-sensitive audience this plan targets.

**3. The positioning argument in the Reality Check changed and the plan reflects it.**
"Free, open source, no rug-pull risk, community governance" was a genuine competitive asset
against Unity's 2023 runtime-fee episode, and it is gone. The moat is now purely technical:
true planetary scale and agent-native authoring. That is still real, and it is narrower.
Ch. *Reality check* §3 has been rewritten to argue the new position rather than the old one.

**What is not negotiable regardless of the above:** E-47 (a lawyer reviews the EULA before
the first sale), E-49 (permissive dependencies only, `NOTICES` generated) and E-50 / I21
(no runtime DRM, no telemetry, nothing in a customer's shipped game). Those three are how a
commercial engine avoids becoming untrustworthy or undistributable.
