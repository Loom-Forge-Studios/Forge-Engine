# Cloud handoff — WP-60 (C-1): forge-phys, 3D physics for both editions

Status: **in progress** (updated at every push).

## Done

- `crates/forge-phys` (base): the backend-neutral model (`types`), the `forge.phys.backend`
  extension point and `PhysicsBackend` trait (`backend`), the `forge.phys` plugin, and
  `PhysicsWorld` (`world`): fixed step, render interpolation, gravity/damping zones,
  canonical events, ray/shape/overlap queries (batched across threads, async at the step
  boundary), state hash.
- Backends, both `f64` + `enhanced-determinism`: **avian3d 0.7.0** (default; fits the
  `bevy_ecs`/`bevy_reflect =0.19.1` pin) and **rapier3d-f64 0.36.0**.
- `tests/test_phys_behaviour.rs` (14 scenarios x 2 backends) and
  `tests/test_phys_features.rs` (15 x 2): every guard runs its positive control beside it.

## Remaining

- Determinism corpus + goldens, allocation guard, budget row, editor Play mode, records.

## Verify on the GPU machine

- (filled in at the end)

## New dependencies

- (filled in at the end)

## Decisions

- ADR 0064 (to be written).
