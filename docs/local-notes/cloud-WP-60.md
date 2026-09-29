# Cloud handoff — WP-60 (C-1): forge-phys, 3D physics for both editions

Milestone M4-3 plus owner parity row M7-9 (first half). Decision record: ADR 0064. Plan:
master-plan Chapter 17 (now FULL for §17.1–17.6).

> Branch note: this session could only push to its designated branch
> `claude/gallant-dirac-fmzhg6` (the cloud harness pins the branch), not `cloud/WP-60`; the
> work is the same, carry it over from there.

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
- `rapier` — rapier3d-f64 0.36.0; every joint a `GenericJoint`; query tree refitted after
  every step.
- `character` — the kinematic character controller (M4-3): collide and slide, walkable
  slopes, edge-aware step-up, ground snap, pushes free bodies, reports touches.
- `scenes` — the determinism corpus + script, the 2,000-body budget scene.
- Error codes `PHYS-0001..0005` (`docs/error-codes.md`).

**Play mode** — `forge-sim` (`src/phys.rs`): entities with `physics.*` properties become
bodies, one region-local physics world per frame, on the project's `physics.backend`; the edit
snapshot carries the `physics.*` settings (hashed only when present: existing recordings and
the golden replay unchanged); inputs drive physics bodies; `SIM-0010`. The editor loads
`forge.phys` in its default plugin set and hands Play the registry its load filled.

**Tests and guards** (each guard with its positive control; all green here):

| Gate row | Test | Positive control |
|---|---|---|
| C-phys-behaviour | `crates/forge-phys/tests/test_phys_behaviour.rs` (14 scenarios x 2 backends) | each scenario runs its broken configuration: continuous collision off tunnels, disjoint layers fall through, restitution 0 does not bounce, friction 0 keeps sliding, a solid instead of a trigger stops the body, unlocked axes turn, `can_sleep: false` never sleeps |
| C-phys-queries | `test_phys_features.rs` | an async ray aimed where the box will be misses before the step and hits after it; too-short and reversed rays miss |
| C-phys-joints | `test_phys_features.rs` | each joint scene without its joint (or with a free ball) breaks the constraint |
| C-phys-zones | `test_phys_features.rs` | `PhysFaults::ignore_zones`; no zone keeps speed; another layer's zone is ignored |
| C-phys-interpolation | `test_phys_features.rs` | `PhysFaults::no_interpolation` stutters |
| C-phys-determinism | `test_phys_determinism.rs` (goldens, two runs identical, queries never change the game) | `positive_control_a_one_ulp_nudge_changes_every_hash` |
| C-phys-step-no-alloc | `test_phys_step_alloc.rs` | `positive_control_a_rebuilt_moving_list_is_counted` |
| C-phys-budget | `tools/forge-perf-gate/tests/test_perf_gate.rs` | `positive_control_an_injected_3d_physics_regression_fails_the_gate` (16x steps) |
| C-phys-character | `test_phys_character.rs` (6 scenarios x 2 backends) | a 0.6 m step, a 60-degree ramp, `snap` 0, no wall |
| C-phys-play | `crates/forge-sim/tests/test_play_physics.rs` | `positive_control_without_physics_the_box_falls_through` |
| C-phys-play-in-editor | `crates/forge-editor/tests/test_play_physics_in_editor.rs` | `positive_control_a_play_core_without_the_registry_bypasses_plugins` |
| (C-extension-point-replaceable) | `tests/plugin/test_extension_point_replaceable.rs` gains the `forge.phys.backend` kit | the existing broken-registry control |

Also: `tests/perf/budgets.ron` rows, `tests/gates.ron` rows, `docs/plan/dod-status.ron`
comments on M4-3 / M7-9, `NOTICES` regenerated (`cargo xtask licence-audit --write`;
cargo-deny: bans, licences and sources ok), `[profile.dev.package.forge-phys] opt-level = 2`.

Checks run here: `cargo xtask fmt --check`, `premium-boundary`, `plan-coverage`, `layering`,
`allocators`, `timed-gates`, `fp-rules`, `gate`, `dod`; `cargo deny check licenses bans
sources`; nextest for forge-phys (78), forge-sim, forge-editor (all), forge-tests (the repo
guards), forge-perf-gate's CPU controls; clippy `-D warnings` on the changed crates.

## Remaining (in scope, not done)

- **M4-3 / M7-9 rows stay `Unread`**: M4-3 waits only on the Windows leg confirming the
  goldens; M7-9's second half (vehicles, cloth) is WP-61.
- **Collider debug drawing** in the viewport (wireframes of shapes, contacts): not built;
  bodies are drawn through their entities' simulated transforms.
- **avian3d generic 6DOF**: avian 0.7 has no generic joint; axis combinations none of its
  joints express are refused with `PHYS-0004` (rapier3d builds them all).
- The determinism corpus is crate-local (`crates/forge-phys/tests/goldens`), not rows of the
  I2 corpus in `tests/determinism/golden.txt`; moving it there is a small follow-up if wanted.

## Verify on the GPU machine / Windows

- **Determinism on Windows**: run `cargo nextest run -p forge-phys --test
  test_phys_determinism`; it must pass unchanged against
  `crates/forge-phys/tests/goldens/phys_state_hashes.txt` (gate
  `C-phys-determinism-windows-leg`). Linux (ubuntu-x86_64) hashes:
  - `avian3d/corpus/60` `8213c730c0c6a868687f4ae2889cda411ec83ccf2b27a6044266cf7472e0f998`
  - `avian3d/corpus/120` `d48baab023345263c93eb8b6bb2d1fefe6d044c30606ad820b2a6076f32b1e82`
  - `avian3d/corpus/240` `f83a5568c2409b35770436b7e47cb6dd42837a8515962cf8b9f2bb79e4e62367`
  - `rapier3d/corpus/60` `de1309b88f691b888afe622cbdd826adbd2e7af7de3ea6f0b124eb0b124c52ac`
  - `rapier3d/corpus/120` `85e546edb4387bdc3a7d34694e63ccb62e072945e77c76679629b269f7c12226`
  - `rapier3d/corpus/240` `6b8ef369c244e0b30071baefe800ab97c9b923cea3166da6dcbbf7750bfbd5a3`
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

- **ADR 0064** — one `PhysicsWorld` over two f64 backends: avian3d 0.7 (default; fits the
  `=0.19.1` pin; its `bevy` dependency contained in a private world — a scoped exception to
  Ch.5.1) and rapier3d-f64 0.36 (selectable per project); zones, interpolation, events,
  queries, the hash done once above them; shared semantics for kinematics and continuous
  collision; avian's `f32` corners named in one allow-listed file; avian's missing generic
  6DOF refused with a code; the character controller on shape casts; Play runs physics per
  frame; provisional budget rows.
