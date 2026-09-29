# Cloud handoff — WP-60 (C-1): forge-phys, 3D physics for both editions

Milestone M4-3 plus owner parity row M7-9 (first half). Decision record: ADR 0066. Plan:
master-plan Chapter 17 (now FULL for §17.1–17.6).

> Branch note: this session could only push to its designated branch
> `claude/gallant-dirac-fmzhg6` (the cloud harness pins the branch), not `cloud/WP-60`; the
> work is the same, carry it over from there.
>
> Rebased onto `main` at `5a7d60a` (the WP-65 input-runtime export). That export took ADR 0064
> and 0065, so this work's decision record is **ADR 0066**. Its `Cargo.lock` keeps every
> physics dependency at the version tested here, and `NOTICES` was regenerated after the
> rebase.

## Done

**`crates/forge-phys`** (base; f64; scanned by `test_no_f32_below_render`)

- `types` — bodies (static / kinematic / dynamic, damping, gravity scale, axis locks,
  sleeping, `ccd`); colliders: sphere, cuboid, capsule, cylinder, cone, convex hull generated
  from mesh vertices, concave triangle mesh, height field; materials (friction, restitution,
  density, combine rules); collision layers; triggers; the joint set (fixed, hinge, slider,
  spring, cone-twist, 6DOF); gravity / point-gravity / damping zones; ray / shape / overlap
  queries and filters; events.
- `backend` — the `forge.phys.backend` extension point (`PhysicsBackendPoint`), the
  `PhysicsBackend` trait, first-party registry helpers (`first_party_backends`,
  `copy_registry`, `create`).
- `plugin` — the `forge.phys` source plugin registering `avian3d` and `rapier3d`.
- `world` — `PhysicsWorld`: deterministic fixed step and `advance` (bounded catch-up), render
  interpolation, zones (kick-drift-kick, second order), canonical events, single / batched
  (multi-threaded) / async queries, a state hash, lazy sync of backend query structures
  (queries never change the simulation).
- `avian` — avian3d 0.7.0 in a private Bevy `World` stepped by hand (no app loop, clock,
  Transform sync, interpolation plugin or async tasks); `avian/narrow.rs` is the only `f32`
  narrowing (density, tree ray/sweep direction), allow-listed in
  `tests/liveness/f32_allow.txt`.
- `avian/six_dof.rs` — avian's generic 6DOF joint: an XPBD constraint on avian's
  custom-constraint API for the axis combinations none of its stock joints express, measured
  as rapier's `GenericJoint` measures it (linear axes along A's frame, angular axes as
  `2 atan2(q_i, q_w)` with `forge_num::det::atan2`), so no 6DOF combination is refused on
  either backend.
- `rapier` — rapier3d-f64 0.36.0; every joint a `GenericJoint`; query tree refitted after
  every step.
- `character` — the kinematic character controller (M4-3): collide and slide, walkable
  slopes, edge-aware step-up, ground snap, pushes free bodies, reports touches.
- `debug` — the debug drawing: every collider's wireframe (primitives and height fields from
  their numbers, convex hulls from the backend's hull via `PhysicsBackend::hull_edges`,
  triangle meshes by unique edge) and every joint's anchors, axis and link, as `DebugLine`s
  in the world's frame, each tagged awake / sleeping / kinematic / static / trigger / joint.
- `scenes` — the determinism corpus + script, the 2,000-body budget scene.
- Error codes `PHYS-0001..0005` (`docs/error-codes.md`).

**Play mode** — `forge-sim` (`src/phys.rs`): entities with `physics.*` properties become
bodies, one region-local physics world per frame, on the project's `physics.backend`; the edit
snapshot carries the `physics.*` settings (hashed only when present: existing recordings and
the golden replay unchanged); inputs drive physics bodies; `SIM-0010`. The editor loads
`forge.phys` in its default plugin set and hands Play the registry its load filled.

**Debug drawing in the viewport** — the play core hands the physics worlds' debug lines up per
region frame (`PlaySession::physics_debug_lines`, `PlayBackend::physics_debug`); the viewport
panel draws them while playing as `LineStyle::Physics(kind)` overlays
(`forge_editor::viewport::physics::draw`, camera-relative `f64`), under a new "Colliders"
toolbar switch (session state, on by default), at most 50,000 lines a cell; nothing is built
while stopped or switched off.

**Tests and guards** (each guard with its positive control; all green here):

| Gate row | Test | Positive control |
|---|---|---|
| C-phys-behaviour | `crates/forge-phys/tests/test_phys_behaviour.rs` (14 scenarios x 2 backends) | each scenario runs its broken configuration: continuous collision off tunnels, disjoint layers fall through, restitution 0 does not bounce, friction 0 keeps sliding, a solid instead of a trigger stops the body, unlocked axes turn, `can_sleep: false` never sleeps |
| C-phys-queries | `test_phys_features.rs` | an async ray aimed where the box will be misses before the step and hits after it; too-short and reversed rays miss |
| C-phys-joints | `test_phys_features.rs` | each joint scene without its joint (or with a free ball) breaks the constraint; the 6DOF combinations avian builds with its generic constraint (cylindrical; two limited linear axes; locked + free + limited angular axes) against the same scene with those axes free |
| C-phys-zones | `test_phys_features.rs` | `PhysFaults::ignore_zones`; no zone keeps speed; another layer's zone is ignored |
| C-phys-interpolation | `test_phys_features.rs` | `PhysFaults::no_interpolation` stutters |
| C-phys-determinism | `test_phys_determinism.rs` (goldens, two runs identical, queries never change the game) | `positive_control_a_one_ulp_nudge_changes_every_hash` |
| C-phys-step-no-alloc | `test_phys_step_alloc.rs` | `positive_control_a_rebuilt_moving_list_is_counted` |
| C-phys-budget | `tools/forge-perf-gate/tests/test_perf_gate.rs` | `positive_control_an_injected_3d_physics_regression_fails_the_gate` (16x steps) |
| C-phys-character | `test_phys_character.rs` (6 scenarios x 2 backends) | a 0.6 m step, a 60-degree ramp, `snap` 0, no wall |
| C-phys-play | `crates/forge-sim/tests/test_play_physics.rs` | `positive_control_without_physics_the_box_falls_through` |
| C-phys-play-in-editor | `crates/forge-editor/tests/test_play_physics_in_editor.rs` | `positive_control_a_play_core_without_the_registry_bypasses_plugins` |
| C-phys-debug-draw | `crates/forge-phys/tests/test_phys_debug.rs` (4 scenarios x 2 backends) | an octahedron hull drawn from its bounds instead of its hull fails the hull check |
| C-phys-debug-draw-viewport | `plugins/forge-panels-scene/tests/test_viewport_physics.rs` | `positive_control_a_scene_without_physics_draws_no_physics_lines` |
| (C-extension-point-replaceable) | `tests/plugin/test_extension_point_replaceable.rs` gains the `forge.phys.backend` kit | the existing broken-registry control |

Also: `tests/perf/budgets.ron` rows, `tests/gates.ron` rows, `docs/plan/dod-status.ron`
comments on M4-3 / M7-9, `NOTICES` regenerated (`cargo xtask licence-audit --write`;
cargo-deny: bans, licences and sources ok), `[profile.dev.package.forge-phys] opt-level = 2`.

Checks run here: `cargo xtask fmt --check`, `premium-boundary`, `plan-coverage`, `layering`,
`allocators`, `timed-gates`, `fp-rules`, `gate`, `dod`; `cargo deny check licenses bans
sources`; nextest for forge-phys (78) and forge-sim (101 together), forge-editor (all),
forge-tests (all 135 repo guards, with their mutant positive controls), forge-perf-gate's CPU
controls; clippy `--all-targets -D warnings` on forge-phys, forge-sim, forge-editor,
forge-perf-gate and forge-tests, and on forge-phys's lib with each backend feature alone and
with none.
After the rebase onto `5a7d60a` all of these ran again: 551 tests across forge-phys,
forge-sim, forge-editor, forge-cli, forge-panels-scene and forge-tests passed, plus the
perf gate's physics control. The xtask checks and clippy are also clean.
With the avian 6DOF joint and the debug drawing: 415 tests across forge-phys, forge-sim,
forge-editor and forge-panels-scene, and all 135 repo guards with their mutant controls,
pass; clippy, fmt and every xtask check are clean.

**CI on this PR.** The Linux leg was fully green on `a689fef`. On `854204a` its one failure
was `forge-input::test_input_budgets the_reference_frame_fits_its_budget`, which fails the
same way on `main` (`5a7d60a`, run 36516276178): WP-65's wall-clock `input.update` budget on
a hosted runner, nothing this PR touches. `main`'s Windows leg fails only
`forge-perf-gate the_named_budgets_hold`, one of the two known hosted-Windows failures.

## Remaining (in scope, not done)

- **M4-3 / M7-9 rows stay `Unread`**: M4-3 waits only on the Windows leg confirming the
  goldens; M7-9's second half (vehicles, cloth) is WP-61.
- **Contact points** are not in the debug drawing (colliders and joints are); a backend
  method for them would be the next step.
- The determinism corpus is crate-local (`crates/forge-phys/tests/goldens`), not rows of the
  I2 corpus in `tests/determinism/golden.txt`; moving it there is a small follow-up if wanted.

## Verify on the GPU machine / Windows

- **Determinism on Windows**: run `cargo nextest run -p forge-phys --test
  test_phys_determinism`; it must pass unchanged against
  `crates/forge-phys/tests/goldens/phys_state_hashes.txt` (gate
  `C-phys-determinism-windows-leg`). Linux (ubuntu-x86_64) hashes:
  - `avian3d/corpus/60` `6cb410ccbb11b9fe5dda7f38eb88f1e1fb17a9908d64c387641f9c4dd3c90d93`
  - `avian3d/corpus/120` `ad935a53ffd91a0f80438f4e6329f3f9765319423d3091647c24a6917f906a23`
  - `avian3d/corpus/240` `ef2a476d2c1b0447609dfce9ebcc69c22d0bbf8012e3cb1175c3e4a7afc3adc8`
  - `rapier3d/corpus/60` `d6170d9916b82444e296a010b65e100026828f9e6b83445435b16e8884954cb0`
  - `rapier3d/corpus/120` `cfb27c0c8e02a418057c38a09b420a654dca0831be97bee1d0595efc88ca98b8`
  - `rapier3d/corpus/240` `2d853f813dc6c1277f745f4ada77604002de5bb8c17cd01c439d6fd18121c26d`

  (The corpus chain's 6DOF link is a combination only avian3d's generic 6DOF constraint
  builds, so the goldens cover it; they changed with that, before any Windows run.) The PR's
  CI runs this test on `windows-latest` too.
- **Budget baseline** (gate `C-phys-budget-baseline`): run the perf gate
  (`the_named_budgets_hold`) on the dev box and set `phys.step.avian3d` /
  `phys.step.rapier3d` in `tests/perf/budgets.ron` from the measured medians (they carry a
  provisional 0.85 from the 60 Hz requirement). Cloud sanity, 4 vCPU, calibration ~27 ms,
  2,000 bodies after 60 settling steps: **avian3d 10.3–11.0 ms (ratio 0.39–0.41), rapier3d
  2.9–3.4 ms (0.11–0.12)**. The positive control uses a 16x fault sized for the provisional
  allowance; once baselined a smaller one would do. `the_named_budgets_hold` itself was not
  run here (no Vulkan adapter in this container).
- **The editor's viewport during Play**: physics bodies move in the viewport (their entities'
  simulated transforms); check it visually, and the play controls with `physics.backend`
  switched between `avian3d` and `rapier3d` in Project Settings.
- **The debug drawing over the rendered scene**: with the "Colliders" switch on, each physics
  body's collider wireframe should sit on its rendered mesh and move with it (awake bodies in
  the accent colour, sleeping ones muted, static green, kinematic amber, triggers red,
  joints in the primary text colour); tested headless for placement and counts, not looked at.
- For the record, backend allocations per steady corpus step (printed by
  `backend_allocations_for_the_record`): avian3d ~1,717 (1.15 MB), rapier3d ~176 (173 KB).

## New dependencies

All permissive; `cargo deny check licenses bans sources` is green and `NOTICES` is
regenerated. Direct: `avian3d =0.7.0` (MIT OR Apache-2.0), `rapier3d-f64 =0.36.0`
(Apache-2.0), `bevy_app`, `bevy_math`, `bevy_tasks`, `bevy_time`, `bevy_transform` (all
`=0.19.1`, MIT OR Apache-2.0; `bevy_ecs` was already pinned). The 97 new crates in the graph
(every target), by licence: MIT OR Apache-2.0 (and spellings) 71, Apache-2.0 12, MIT 8,
Zlib 2, Zlib OR Apache-2.0 OR MIT 4 — among them bevy 0.19.1 and its minimal internal crates
(no bevy_render), avian_derive, bevy_heavy, bevy_transform_interpolation, glam_matrix_extras,
obvhs, parry3d-f64 0.27.0 and 0.31.1 (Apache-2.0), glamx, nalgebra, simba, rstar, spade.

## Decisions

- **ADR 0066** — one `PhysicsWorld` over two f64 backends: avian3d 0.7 (default; fits the
  `=0.19.1` pin; its `bevy` dependency contained in a private world — a scoped exception to
  Ch.5.1) and rapier3d-f64 0.36 (selectable per project); zones, interpolation, events,
  queries, the hash done once above them; shared semantics for kinematics and continuous
  collision; avian's `f32` corners named in one allow-listed file; avian's missing generic
  6DOF built as our own XPBD constraint (measured as rapier's generic joint); the character
  controller on shape casts; Play runs physics per frame; a backend-neutral debug drawing
  (only a hull's edges come from the backend) shown by the viewport during Play; provisional
  budget rows.
