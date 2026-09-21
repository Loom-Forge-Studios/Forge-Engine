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
| E-56 | **A placeholder EULA is drafted in-house now, for counsel to react to rather than a blank page.** It is marked as unreviewed on every page and gates no sale. | a lawyer briefed with a concrete draft and a list of open questions costs a fraction of one briefed with an idea | **RATIFIED** |
| E-42 | **Forge is commercial and source-available, never open source.** Source is public and readable; use requires a paid licence. | owner's decision, 2026-09-20 | **RATIFIED** |
| E-43 | **Licence tiers.** **Individual — $5 one-time, perpetual, all versions**; ships commercial products; **no team/collaboration features**. **Team — a subscription: $25/year** while the project is under $100K gross, **$40/year** at or above. Includes **4 seats** (purchaser + 3); included and additional seats are **usable only on projects owned by that team**; additional seats **$5/year** each, billed with the subscription. | owner's decision; Team moved to subscription 2026-09-20 | **RATIFIED** |
| ~~E-44~~ | ~~5% product royalty.~~ **RESCINDED 2026-09-20 → E-51. There are no royalties of any kind.** | — | RESCINDED |
| E-51 | **No royalties on games, applications or any other product, at any revenue, forever.** A licensee pays once and keeps 100%. This is stated **affirmatively** in the EULA and the docs, never merely omitted. | it is the product's sharpest differentiator and an omission would read as an oversight a lawyer could later close | **RATIFIED** |
| E-52 | **The marketplace is the only ongoing revenue: 5% commission on sales through the Forge Index.** Plugins and assets sold anywhere else owe **0%** — it is a storefront fee for payments, hosting and discovery, not a royalty on creation. | the Fab model at less than half Fab's rate | **RATIFIED** |
| E-53 | **Revenue is self-declared. No audit, no reporting obligation, no instrumentation.** | the only revenue-dependent number left is a **$15** tier difference; any enforcement apparatus would cost more than it recovers and would insult everyone honest | **RATIFIED** |
| E-54 | **Tier enforcement is a local, offline check of a signed entitlement, in the editor only.** The entitlement declares tier, seat type, and for included/additional seats the team it is bound to. | preserves I21; the runtime and shipped games contain no licensing code at all | PROPOSED |
| E-57 | **On lapse the editor degrades to the Individual tier. It never locks out.** The licensee keeps the editor, the projects, and the ability to build and ship; only the Ch.37 collaboration features go dark until renewal. | it is the whole answer to "what happens if I stop paying mid-project," which is the first question any studio asks about a subscription | **RATIFIED** |
| E-58 | **A lapsed or terminated licence never affects a shipped product.** | already structurally true — no licensing code exists in `forge-runtime` (I21) — but it is stated as a term rather than left as a property | **RATIFIED** |
| E-59 | **Recommended: a perpetual fallback licence.** After **12 continuous months** of subscription the licensee keeps perpetual Team rights to the version current at their 12-month mark, renewed or not. | **it costs almost nothing because the source is public — every version is on GitHub forever and cannot be withheld.** The grant merely formalises what is already physically true, while removing the single largest objection to subscribing for multi-year game development. | PROPOSED |
| E-60 | **The Individual tier keeps "all versions" — v2 is not a second $5.** | $5 is a token price whose job is to create a licence relationship, not revenue; the Team subscription now supplies the recurring line that makes a free v2 for individuals affordable | PROPOSED |
| E-61 | **Forge operates no project hosting.** `forge-server` runs on the licensee's own hardware, so a lapsed subscription can never strand a project — there is nothing for the publisher to withhold. | closes the gap the EULA draft found in "you keep your projects": it is only true if nobody else is holding them. The self-hosted pillar (E-31) already guaranteed this; it was never written down as the reason. | **RATIFIED** |
| E-62 | **Gross Revenue is cumulative lifetime per product. The team rate never falls back** — once a project crosses $100K it renews at $40 thereafter. | resolves a contradiction the draft surfaced: §4.2.1 implied the rate could drop while a cumulative definition made that impossible. Cumulative matches Unreal's "lifetime gross" framing and a project that crossed $100K can afford $40. | PROPOSED |
| E-63 | **Renewal pricing may change with notice; a term already paid for is never repriced.** Stated explicitly and in the licensee's favour. | the draft found the seam: "we never change terms retroactively" and "we can reprice your renewal" are both true and the gap between them is exactly where a licensee feels misled. Saying it plainly costs nothing and pre-empts the complaint. | PROPOSED |
| E-64 | **Required engine credit in every shipped product**, in four places: in-product credits, executable metadata inserted automatically by the build tools, `NOTICES`, and the product's store page or documentation. **Visible and inert** — no network, no reporting, nothing that executes. | owner's decision, 2026-09-20. Conventional (Unreal requires a credit notice), it is free marketing, and it makes Forge-built products identifiable. Removable from source like any gate, so its force is contractual. | **RATIFIED** |
| E-55 | **Forking is accepted.** The repository stays public; GitHub cannot disable forking on a public repo, and a fork confers no licence. | owner's decision, 2026-09-20, after the constraint was surfaced | **RATIFIED** |
| E-45 | **Permitted:** build and ship games/apps; create and distribute plugins and assets for the editor; modify the source for your own use; redistribute `forge-runtime` **in binary form only**, embedded in a shipped product. **Prohibited:** redistributing engine source, sublicensing, distributing the editor, or producing a competing engine derived from this one. | this is the shape the owner described, written in licence-grant terms | **RATIFIED** |
| E-46 | **An account exists to buy a licence. The software never requires one to run.** Activation is at install; there is no runtime check, no phone-home, and **nothing in a shipped game** (I21). | a paid engine needs a purchase path; a DRM'd runtime would poison the product and every shipped game with it | PROPOSED |
| E-47 | **Do not hand-roll the licence.** Draft the EULA from the Unreal Engine EULA's structure, or from PolyForm Perimeter / FSL as a base, and have a lawyer review it **before any money changes hands**. | a homemade licence is how projects discover they cannot enforce anything; this is cheap now and unfixable later | PROPOSED |
| ~~E-48~~ | ~~Recommended plugin boundary.~~ **ADOPTED 2026-09-20 → E-52.** | — | ADOPTED |
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
| ~~O-19~~ | Plugin 5% — commission or royalty? | **Storefront commission on Forge Index sales only; 0% elsewhere** (E-52) | 2026-09-20 |
| ~~O-20~~ | Royalty threshold? | **No royalties at all** (E-51). The only revenue-dependent number is the $25/$40 team tier, measured **per project**, self-declared. | 2026-09-20 |
| ~~O-21~~ | Per-seat or per-person? | **Per person for Individual; per seat for Team**, with included and additional seats bound to team-owned projects (E-43). | 2026-09-20 |
| O-22 | EULA: placeholder drafted in-house (E-56); **counsel review still gates the first sale (S17)**. | **needs owner — engage counsel** | open |
| **O-23** | Does a Team licence transfer on acquisition of the company? | needs counsel | open |
| **O-24** | Is the $100K team threshold gross revenue *per project* or *per team across all projects*? Drafted as per-project; confirm. | needs owner | open |
| **O-25** | Does an Individual licensee get read-only participation in a team project, or no access at all? Affects Ch.37's Viewer role. | needs owner | open |
| ~~O-26~~ | Additional seats one-time or annual? | **$5/year per seat**, billed with the subscription. Closes the one-subscription-plus-fifty-one-time-seats loophole. | 2026-09-20 |
| **O-27** | Grace period length after lapse, and whether the 12-month fallback clock (E-59) resets on a lapse-and-resume. | needs owner | open |
| **O-28** | **Does the project take a position on AI training on the source?** Epic added one. Silence will be misread given agent editing is a headline feature. | **needs owner** | open |
| **O-29** | Do CI and build agents consume a seat? Drafted as no; confirm. | needs owner | open |
| **O-30** | Do **purchased additional seats** vest under the fallback licence alongside the included four? The draft is silent and silence here is expensive. | **needs owner** | open |
| **O-31** | Does the fallback version advance annually or continuously? Must be computable offline. | needs owner | open |
| **O-32** | Auto-renewal disclosure, renewal reminders and cancellation flow (UK, EU, California ARL). **These are checkout-and-email requirements, not EULA text** — a compliant agreement attached to a non-compliant checkout is the usual failure. | **needs owner — product work, not legal text** | open |
| **O-34** | **Is the default splash screen removable?** Recommending yes: a required credit is universally accepted, a forced splash was one of Unity Personal's most resented terms and Unity made theirs optional in Unity 6. | **needs owner** | open |
| **O-35** | **Online licence check at editor boot** — see §8. **I21 is not changed until this is decided.** | **needs owner** | open |
| **O-33** | Does §9.7's "you keep your projects" need an export path and retention period for any case where a project is not on the licensee's own hardware? Answered by E-61 for `forge-server`; confirm no other case exists. | architect | open |


---

## 6. The commercial model, and what it does and does not fund

Settled 2026-09-20. The recommendations that used to live in this section were adopted or
rendered moot; what follows is analysis of the model as ratified.

### 6.1 It is a coherent flywheel, and each piece needs the others

No royalty and a $5 entry price **maximise adoption**. Adoption is the only thing that
creates **marketplace volume**. Marketplace volume at 5% is **the entire ongoing revenue**.
And 5% against Fab's 12% makes the Forge Index the better place for an author to sell,
which is what pulls the catalogue across.

Every piece reinforces the next. That is rare and it is worth protecting: **raising the
licence price or adding a royalty later would not add revenue, it would remove the input
the revenue depends on.**

### 6.2 The pitch is now genuinely strong

| Scenario | Forge | Unreal | Unity |
|---|---|---|---|
| Solo dev, product grosses $50K | **$5** | $0 | $0 |
| Solo dev, product grosses $2M | **$5** | **$50,000** (5% above $1M) | per-seat tiers |
| 4-person team, product grosses $500K | **$25** | $0 | per-seat tiers above $200K |
| 4-person team, product grosses $5M | **$40** | **$200,000** | per-seat tiers |
| Selling a $20 plugin through the first-party store | **5% — $1.00** | 12% on Fab — $2.40 | Asset Store cut |

*Unity's tier pricing has changed repeatedly; verify current figures before publishing this
table.* The Unreal figures follow its stated 5%-above-$1M-lifetime-gross-per-product rule.

The headline writes itself: **pay once, ship anything, keep everything.** For any product
crossing $1M, Forge is cheaper than Unreal by three to four orders of magnitude.

### 6.3 What the model funds — revised 2026-09-20 when Team became a subscription

The earlier version of this section said flatly that licence fees are not a business. **A
recurring team line changes that materially and the correction is worth making, because it
changes what M0–M5 can be planned against.**

| Teams subscribed | At ~$30/yr average | |
|---|---|---|
| 1,000 | $30K/yr | a hobby that pays for itself |
| 5,000 | $150K/yr | one full-time person |
| 20,000 | $600K/yr | a small team |

Compare the one-time model it replaced: ten thousand licences at $5–$40 was $50K–$100K
**once**, and then nothing. Recurring revenue is not a marginal improvement over that — it
is the difference between a side project and something that can pay for its own
development.

So the realistic shape is now: **team subscriptions are the near-term funding line, and the
marketplace is the long-term upside.** Both still require adoption, and adoption is what
M0–M5 buys. Three rules survive the revision unchanged:

1. **Do not plan any milestone as funded by licence revenue.** Recurring or not, it arrives
   after the engine is good, not before.
2. **Do not let revenue pressure distort the engine** — no paid-only engine features, no
   store lock-in, no curation gate favouring revenue over quality.
3. **Do not raise prices when revenue disappoints.** §6.1 still explains why that makes it
   worse, and it applies with more force to a subscription, where a price rise is visible
   every single year.

### 6.4 Tier enforcement is a compliance mechanism, not a technical barrier

The Individual tier is gated out of team features by a signed local entitlement (E-54).
**Anyone who compiles from source can remove that check in an afternoon** — the source is
public; that is inherent to source-available and it is not a flaw to be engineered away.

The gate exists so honest licensees know what they bought. It is worth exactly what a
cheap, correct, offline check costs and **not one hour more**. Any proposal to harden it —
obfuscation, server checks, integrity verification, a binary-only editor build — is
rejected in advance: it would cost real engineering, fail anyway, and violate I21 and the
A.7 commitments that are the reason anyone would trust a paid engine in the first place.


---

## 7. The fallback licence undermines the subscription — a decision, not a discovery

Raised by the EULA draft (counsel question 41) against E-59, which is **my recommendation,
so it deserves a straight answer rather than a defence.**

**The hole is real.** A rational team subscribes for 12 months, vests perpetual Team rights
under the fallback, cancels, and keeps collaboration features forever for $25. Written that
way it looks fatal.

**Why it is probably still correct, for now:**

1. **The vested version freezes.** No fixes, no features, no platform updates. For an engine
   in its first years — where month 24 will be dramatically better than month 12 — a frozen
   build is a bad deal, so the churn is largely theoretical during exactly the period the
   model depends on.
2. **The source is public.** Someone determined to stop paying can already run any version
   they like. The fallback does not create a leak; it legitimises a frozen one.
3. **The amount at stake is $25/year.** The fallback's job is to remove the *"I am afraid to
   start a three-year project on a subscription"* objection — and per §6.1, adoption is the
   input every other part of the model depends on. Trading a little churn for that is the
   same trade the whole pricing model already makes.

**Where it inverts:** once the engine matures and a 12-month-old build is nearly as good as
current, argument 1 evaporates and the hole opens properly. **Revisit at 1.0, and again
whenever release cadence slows.** Options then, in increasing severity: lengthen vesting to
24 or 36 months; vest the *tier* but not the collaboration server; drop it.

**Recommendation: keep it, vest at 12 months, and record that the trade is deliberate.**
What must not happen is meeting it for the first time in the first renewal cohort.


---

## 8. Proposal under consideration: an online licence check at editor boot

Proposed by the owner 2026-09-20: the editor must be online to boot and verifies an active
licence under the user's account; once booted it may run offline. **Recorded for decision.
Nothing in I21, the EULA or the public README has been changed on its account.**

What it would and would not touch: it applies to the **editor** only. Shipped games are
unaffected either way — `forge-runtime` still contains no licensing code (I21's strongest
clause survives regardless of this decision).

### The case against a check at *every* boot

1. **It cannot stop anyone who would pirate, because the source is public.** Deleting the
   check and rebuilding takes an afternoon — the same reason the tier gate is accepted as a
   compliance mechanism (§6.4). A boot check therefore lands **only on paying users.**
2. **An outage stops every licensee on Earth.** When the licence server is down, nobody can
   open the editor. The publisher is signing up to run a 24/7 critical service, at a
   studio's scale, where every minute of downtime is every customer's lost working time.
3. **It makes "perpetual" depend on the company surviving.** If the server is ever switched
   off — acquisition, insolvency, a decision to stop — every licence stops working, which
   contradicts the perpetual grant in the EULA. This is the argument counsel will press
   hardest, and it is the one that has sunk the most always-online products.
4. **It breaks cases the plan was built for:** headless CI and build farms (Ch.26, Ch.34),
   often firewalled or air-gapped; **console development, where dev kits commonly live on
   isolated networks**; and developers on poor connections — the cheap-hardware audience
   this plan names overlaps heavily with that one.
5. **It reverses a live public commitment.** The README currently promises "no runtime
   licence check… the editor and build tools work offline indefinitely." Nothing has been
   sold, so changing it is not retroactive — but it is the specific line written to answer
   Unity's 2023 episode, and it would go.

### The alternative that gets most of the value

**A periodic check, never a boot gate:**

- The editor verifies online **at most every 30 days**, in the background, never blocking.
- If it cannot reach the server it keeps working through a **30-day grace period**.
- On final failure it **degrades to Individual** (E-57) — it never locks out.
- `--headless`, CI and farm nodes are **exempt**; they never check.
- **A written end-of-life commitment:** if the licence server is ever permanently shut
  down, a final update removes the check. That single sentence is what keeps "perpetual"
  true against company death, and it costs nothing to promise.

That catches honest users whose subscription lapsed — which is the only group any check can
catch — without making every outage everyone's outage, and without making the licence
mortal. It is the JetBrains model, and it is widely accepted.

**Recommendation: the periodic alternative, or nothing.** The local signed-entitlement
expiry (E-57) already tells an honest user their subscription lapsed, with no network at
all; any online check adds operational risk in exchange for catching the same people.
