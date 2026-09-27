# Forge — Milestones, DoD & Gates

**Every milestone ends in something a human can launch and a script can drive.** No
milestone may end in "the architecture is now correct."

Statuses, following a previous project model: `settled` · `blocked(reason)` ·
`superseded(by)` · `UNMET` · `unread`. **A milestone ships at 0 UNMET and 0 unread.**

> **Never renumber a DoD id to keep a list tidy.** An id is a position only until something
> cites it — a gate row, a handoff, a commit message, a defect — and after that it is an
> identity. **New items append at the end of their milestone**, however out of order that
> reads. This was violated once during drafting and reverted; the temptation is real,
> because inserting in place looks neater every single time.
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
| M0-18 | `cargo-deny` permissive-only allow-list + `cargo-about` NOTICES generation in `just verify` (I13) |
| M0-19 | `test_no_runtime_phone_home` green from the first commit (I21) — `forge-runtime` links no licensing or network crate |
| M0-20 | **Chapters 1–7, 31, 32, 33 frozen.** Signature changes from here are plan amendments. |

> **Why the plugin and store contracts are in M0 and not later.** A kernel/plugin boundary
> drawn after twenty subsystems exist is drawn around whatever those subsystems happened
> to do, and every private escape hatch found later is permanent. A storage trait added
> after the editor calls `std::fs` in forty places is added around forty exceptions.
> Both are cheap now and are never cheap again — the same argument as I1 and I7.

---

## M1 — Render Spine

| id | Definition of done |
|---|---|
| M1-1 | `forge-gpu` — adapter **pool** (length may be 1), render graph, WGSL + `naga`, shader hot reload |
| M1-2 | **Spike S1 resolved** with a written verdict and the Tier-1 fallback stated |
| M1-3 | f64 → camera-relative f32 at the last mile; no f32 position anywhere below `forge-render` |
| M1-7 | Clustered forward+, PBR metal-rough, directional + punctual lights, VSM |
| M1-8 | Bruneton atmosphere parameterised by body composition |
| M1-11 | Perf budgets named and a regression gate in CI |
| M1-12 | **Windows (DX12) and Linux (Vulkan) render identically**; `test_platform_parity` green (I18) |

---

## M2 — Editor Spine

*The editor exists, and it is a client of the bus from the first day.*

| id | Definition of done |
|---|---|
| M2-1 | `forge-editor` as a **client of `forge-cmd` and nothing else** |
| M2-2 | Viewport, hierarchy, inspector **generated from reflection** (Ch.6), asset browser |
| M2-3 | Undo/redo across every editor action, via the bus |
| M2-4 | `test_command_liveness` green **with its positive control failing as designed** (I7) |
| M2-5 | Play-in-editor; headless play; deterministic replay of a recorded command stream |
| M2-6 | `forge-asset` — VFS, glTF import, hot reload, content addressing |
| M2-9 | **Spike S4 resolved** — regionized scheduler fuzzed and measured at 4/16/64 regions |
| M2-10 | **Spike S6** — two `bevy_*` upgrade cycles tracked, churn cost measured and recorded |
| M2-11 | `forge-trace` — Tracy/Perfetto, named budgets |
| M2-12 | **The 3D workspace preset** shipping as data; `test_no_preset_gating` + `test_preset_promotion` green (I15) |
| M2-13 | **Plugin system v1** — WASM host, source plugins, manifest, capability grants, hot reload; `test_no_privileged_plugin` green with its positive control (I16) |
| M2-14 | **At least three first-party subsystems shipped as plugins**, proving I16 rather than asserting it |
| M2-15 | `forge-store` — `Git` backend (GitHub device flow + any remote), blob store, generated ignore rules, three-click new-project flow |
| M2-16 | **Split editor v1** — commands + state deltas over QUIC on LAN, local panels, locally-rendered viewport; `test_split_editor_parity` green |
| M2-17 | `forge --headless`; `test_headless_parity` green |

**Exit criterion:** a human and a script edit the same project through the same bus, the
script's changes appear in the human's undo stack, a recorded session replays exactly — and
the human is at a laptop driving a core on another machine, on Windows and on Linux, with
the project committing to a self-hosted git remote.

### M2 — UI definition of done  ·  *appended by WP-U0 (D-1, ADR 0001, Ch.21)*

These rows extend M2 and do not renumber it. They make the editor's UI, and the one widget
layer it shares with games, shippable at M2 (D-1).

- **Panels whose backends do not exist yet** are done at M2 when they run against the
  labelled in-memory backend Ch.21 §21.21 names (D-4). `just gate` must show the real
  backend as `UNBUILT(reason)`, never green.
- **Real-backend acceptance stays with the later milestones' rows**, for example M5-17
  (team UI on real identity) and M6-13 (conflicts on real collaboration). Those rows are
  not superseded.
- **M7's "UI toolkit for shipping games" becomes polish** (world-space UI, HUD templates),
  because M2-29 delivers the toolkit.

**Every row inherits these requirements** (Ch.21 §21.16, §21.18):

- It is fully keyboard-operable, has a correct AccessKit tree, uses theme tokens only, and
  every user-visible string is a localisation key.
- Every change to project state it makes is a command (I7).
- It is available under every preset (I15).
- It is registered through a public extension point (I16).

| id | Definition of done |
|---|---|
| M2-18 | Ch.21 expanded to FULL, ADR 0001 accepted (O-6 amended), and `test_ui_panel_inventory` **bound** with positive controls: every panel maps to a chapter, a WP and a DoD id, and every D-5 budget has a gate row |
| M2-19 | `forge-ui` core (Ch.21 §21.3–§21.15): retained tree with stable `WidgetId`s, signals/bindings, `taffy` layout, `cosmic-text` shaping cache and **one** glyph atlas (an `R8` mask plane and an RGBA colour plane, ≤ 32 MB), `UiRenderer` + `wgpu` + recording renderers, `winit` runner, per-window DPI; `ui_gallery` runs on Windows and Linux; `ui_text_shaping_cached`, `ui_single_glyph_atlas`, `ui_draw_calls_batched`, `test_ui_wgpu_confined` green with positive controls |
| M2-20 | **Idle editor = zero redraws, no busy loop**: `ui_idle_zero_redraw`, `ui_idle_no_busy_loop`, `ui_damage_bounded`, `ui_live_panel_refresh_bounded` (live-data panels refresh only on change, only while visible, capped, never from `ui.self` counters) green, each with its positive control failing as designed; idle CPU recorded |
| M2-21 | Input: pointer capture, focus scopes, Tab/arrow/F6 traversal, spatial navigation, IME pre-edit/commit, clipboard (text + RON-over-reflect), typed drag-and-drop; `test_ui_focus_traversal`, `test_ui_ime_composition`, `ui_typing_latency_one_frame` green |
| M2-22 | AccessKit tree for every widget, incremental and lazily activated; UIA and AT-SPI verified; `test_ui_a11y_tree` green with its positive control |
| M2-23 | Themes `forge.dark`, `forge.light`, `forge.high-contrast` as token data; no colour literal in `widgets/`; `test_ui_contrast` green with its positive control; UI scale 50–300% and reduced motion honoured |
| M2-24 | The full Ch.21 §21.16 widget catalogue, each widget with input and a11y tests, all shown in `ui_gallery` |
| M2-25 | Virtualised list, tree and table: `ui_virtual_list_100k`, `ui_virtual_tree_100k`, `ui_virtual_table_100k` green (≤ 2 ms layout+paint on the reference machine; bounded live rows everywhere; tree expand/collapse costs O(depth · log fanout) index updates whatever the subtree size) |
| M2-26 | Docking: tabs, splits, floating OS windows across monitors, drag-to-dock previews, maximise; layouts as RON, preset defaults under `presets/*/layout.ron`, crash-safe user-layout autosave; `test_layout_roundtrip` and `test_layout_unknown_panel_kept` green |
| M2-27 | Extension points `EditorPanel`, `EditorOverlay`, `InspectorWidget`, `ViewportTool`, `Theme`, `Action` with add/replace/remove/chain; every first-party panel lives in a `plugins/forge-panels-*` source plugin; `test_panel_extension_replaceable` (I16) and `test_every_panel_under_every_preset` (I15) green with positive controls |
| M2-28 | The shell as a bus client (Ch.21 §21.18): `PanelCx` exposes only `ProjectMirror` (read) and `CommandEmitter` (write); the mirror is fed only by the `Applied` stream, so an edit from any bus client appears in the UI and the undo history next frame; gestures are one txn / one undo entry; `test_gesture_single_undo` green; `test_command_liveness` covers `forge-editor` and every panel plugin (with M2-4) |
| M2-29 | Game UI: `forge-runtime` uses `forge-ui` for a sample menu, settings screen and HUD with gamepad navigation and the E-64 credits entry; `test_game_ui_links_no_editor` green with its positive control |
| M2-30 | The UI perf gate is in `just verify` (and CI): every Ch.21 §21.22 test runs, counters gate every leg, millisecond budgets gate on the reference machine; `ui_startup_budget` recorded; all positive controls fail as designed |
| M2-31 | Every editor string is a localisation key; `test_pseudo_locale_no_hardcoded_strings` green; keyboard-only walkthrough of every panel recorded |
| M2-32 | **Viewport** panel: `f64` camera-relative camera usable from 1 m across a large world without jitter; orbit/fly/pan; gizmos (local/world/frame, snapping) as gestures; picking; overlays; multiple viewports; redraws only on change |
| M2-33 | **Play controls**: play/pause/step/stop as commands; play-in-editor against the user's own sandbox view |
| M2-35 | **Scene hierarchy**: virtualised tree, drag-reparent, multi-select, rename, search, visibility/lock, presence badges, all edits as commands; `ui_hierarchy_100k` green |
| M2-37 | **Inspector** generated from reflection: every reflected primitive and composite type has an editor, units and ranges shown, multi-object mixed values, add/remove component, `InspectorWidget` overrides, `SeedPath` + promote for generated content |
| M2-38 | **Asset browser**: grid/list, async thumbnails, folders, search, drag-drop import, rename/move as commands, lock indicators |
| M2-39 | **Console**: level filters, search, grouping, click-through to `SeedPath`, shader node and rejected command |
| M2-40 | **Profiler**: frame timeline, named budgets red when over, adapter-pool lanes, replication stats |
| M2-41 | **Undo history**: transactions with issuer (human / script / teammate), click-to-undo-to |
| M2-42 | **Notifications**: toasts carrying stable `ErrorCode`s, history drawer, actions |
| M2-43 | **Settings**: editor settings (user config) and reflection-generated project settings (edited through commands); theme, UI scale, reduced motion, caret blink |
| M2-44 | **Keybindings editor**: search, rebind by chord, conflicts named with both actions (`test_keybinding_conflicts`), reset |
| M2-45 | **Command palette**: every action and every no-argument `EditorCommand` reachable by keyboard; fuzzy search; chords shown |
| M2-46 | **Graph editor** (blueprint / material / generator / PCG on one canvas): unit-typed pins refuse incompatible connections with a reason, node search from `#[forge_api]`, comments, reroutes, minimap, copy/paste, compile errors on nodes, graph↔text view, edits as commands; `ui_graph_2k_nodes` green |
| M2-47 | **Project launcher and new project**: recent projects; three-click creation with 2D / 3D templates (O-14); link-a-remote offered on first save (O-11); the settings window's **Project** page (name, linked remote, store backend; the preset row opens its dialog), every edit a command |
| M2-48 | **Preset switcher and promotion**: every Ch.31 §31.4 arrow, lossy arrows naming the loss and requiring confirmation (`test_lossy_warnings_present`) |
| M2-50 | **Revision history and store**: semantic history from envelopes, commit/push/pull requested as commands (the core performs the store operations, E-36), link-a-remote |
| M2-51 | **Build and export**: targets, the attribution the packager inserts (E-64), `NOTICES` preview |
| M2-52 | **Plugin manager**: capabilities requested vs granted, enable/disable (plugin set changes and grants are commands; grants human-only and audited), `replaces` conflicts with both names, index browse, `forge add` equivalent |
| M2-54 | **Audit log**: every remote and teammate envelope, greppable by session, user and sandbox |
| M2-55 | **Remote connect**: device pairing (user config via the allow-listed `RemotePairingStore`, audited), session list, latency indicator, loopback default with explicit LAN opt-in |
| M2-56 | **Compute and farm**: adapters with measured throughput, farm node discovery, status and failures |
| M2-57 | **Team and members**: Create Team → Add Member in three clicks (email or join code, role, optional path scope, revocable pending invites); membership edits as commands |
| M2-58 | **Sandbox and Live/Pull**: sandbox status and visibility (default `Team`), one-click reversible Live/Pull toggle, the E-37 statement shown |
| M2-59 | **Presence**: who is looking at what, in hierarchy, inspector and viewport |
| M2-60 | **Publish and review queue**: publishing is a command; the core performs the `ProjectStore::commit` (E-36) and the panel never calls `ProjectStore`; optional maintainer approval with per-path overrides (O-16) |
| M2-61 | **Conflict resolution**: three-way view on the reflect tree; failed preconditions surfaced, never dropped |
| M2-62 | **Licence status**: tier, seat, bound team, expiry; on lapse a banner and renew button with collaboration panels greyed and nothing locked (E-57); no network call in the panel's path (I21) |
| M2-63 | **Sequencer and timeline**: key any reflected property on any entity, curves, tracks |
| M2-64 | **Animation state machine and blend space editor**: states, transitions, 1D/2D blend spaces, bone masks |
| M2-65 | **Localisation**: string-table editor and pseudo-locale preview |
| M2-66 | **2D editors**: tile palette with autotiling, sprite-sheet slicing, `Skeleton2D` cutout-rig editor |
| M2-67 | **Audio mixer**: buses, sends, meters, spatial preview |
| M2-68 | **Input action map**: game input actions and rebinding defaults, distinct from the editor keymap |
| M2-69 | **Ownership claims**: claim and release a scene, prefab, body or asset subtree as audited commands, refused for roles without claim rights; claimed subtrees are live read-only for everyone else in the inspector, hierarchy and viewport; claims listed by holder and path (Ch.37 §37.4) |
| M2-70 | **Every panel**: a declared empty state from the catalogue widget, errors with stable `ErrorCode`s and a next step, dismissible first-run tips stored as user config that never block input or schedule a frame once dismissed; `test_panel_empty_states` green with its positive control |

---

## M4 — Simulation, and the Dogfood Gate

| id | Definition of done |
|---|---|
| M4-1 | Regionized scheduler in production — merge on proximity, split on load, **frame-equality as a merge precondition** |
| M4-2 | `test_region_determinism` — identical results at 1/2/4/16 regions |
| M4-3 | `forge-phys` — `avian` f64 in region-local frames, character controller, deterministic fixed step |
| M4-4 | Collision LOD rings per the Foundations; independent geometry and gameplay residency radii |
| M4-6 | `forge-nav` — per-region navmesh from the density field; hierarchical long-range pathing |
| M4-7 | `forge-net` — transport, replication, **one authority per entity**, target-resolves-damage, interest caps at K≈50 |
| M4-8 | Profile system with hysteresis; `test_profile_equivalence` green (I5) |
| M4-10 | A written list of everything the port needed that the engine did not have — this becomes M5's backlog |
| M4-11 | **`forge-2d` and the 2D preset** — the Ch.35 §35.2 list, complete, and nothing beyond it (**Spike S13**) |
| M4-12 | The 2D sample game ships: tilemaps, cutout animation, 2D lights, gamepad. This is 2D's acceptance test. |
| M4-13 | The 2D tax guard is green — a 2D project links only the crates the 2D preset needs |
| M4-15 | **Spike S10 resolved** — WASM overhead measured at a generator node and a PCG rule |

**Exit criterion:** the dogfood gate. If the Foundations cannot port, the engine is wrong and M5
does not start.

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
| M5-11 | **Thin-client mode** — hardware-encoded viewport (NVENC/VA-API/AMF/QuickSync), AV1 with H.264 fallback, WebRTC for NAT traversal |
| M5-12 | **Spike S11 resolved** — latency budget measured on a real LAN pair, the number published, the fallback documented |
| M5-13 | Remote security: device pairing, TLS/DTLS, capability-scoped sessions, audit log, loopback default |
| M5-14 | **Plugin index v1** — signed manifests, verified publisher identity, self-hostable, `forge add <id>`. **This is the business's only ongoing revenue (`decisions.md` §6.3); treat its milestone position accordingly.** |
| M5-15 | `forge-identity` — local accounts, optional OIDC, device pairing vs user auth kept distinct |
| M5-16 | Roles as capability sets on the existing grant model; `test_role_enforcement` green with its escalation positive control |
| M5-17 | **Create Team → Add Member UI** — three clicks, invite by email or join code, role + optional path scoping, revocable pending invites |
| M5-18 | `forge-collab` — sandboxes as `baseline ⊕ deltas`; `test_sandbox_isolation` green (I19) |
| M5-19 | **Pull/Push mode** — publish is `ProjectStore::commit`; pull selects a revision range |
| M5-20 | Sandboxed play-in-editor — each user simulates against their own view |
| M5-21 | `forge-server` — the multi-user host: baseline + N sandboxes + N sessions, authz **per command**, rate limits, audit log |
| M5-22 | `test_plugin_runs_as_user` green — a sandbox plugin cannot reach a capability its owner lacks |
| M5-23 | **Spike S16 resolved** — 50 idle sandboxes measured; a sandbox is a row or the team-size cap is published |
| M5-24 | Paid-listing path on the Index: payment processing, payout, the 5% commission. Free listings stay first-class in discovery. |
| M5-25 | Tiered entitlement (Ch.38 §38.2): Individual vs Team gating, seat binding to team-owned projects; `test_tier_gate` and `test_gate_is_editor_only` green |
| M5-26 | Subscription lifecycle: entitlement expiry, grace, opportunistic renewal, and **degrade-to-Individual on lapse**; `test_lapse_degrades_never_locks` and `test_renewal_never_blocks` green (E-57) |
| M5-27 | **Engine attribution** (Ch.38 §38.7): the packager inserts executable metadata on Windows and Linux plus `forge.json`, adds the credits entry, and ships the default splash; `test_attribution_emitted` and `test_attribution_is_inert` green (E-64) |

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

Platform HAL complete; Windows/Linux/Web/Android exports; the console HAL
boundary documented for licensed porters; documentation; sample projects; the package
index; **the commercial layer** — `forge-licence`, entitlement and offline activation (Ch.38), the paid plugin index path, and the royalty reporting form; **the EULA and CLA drafted and reviewed by a lawyer (S17 — blocking: no sale happens without it)**; **1.0 is drawn at "the Foundations ships on it,"** not at
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
to ask of any AWAITING row, learned the hard way on a previous project: *is it blocked on
hardware, on a file nobody wrote, or on an input that does not exist?*
