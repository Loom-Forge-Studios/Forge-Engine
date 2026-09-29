# ADR 0066 — Build 3D physics as one world over two f64 backends: avian3d by default, rapier3d selectable

- **Status:** accepted
- **Date:** 2026-09-29
- **Plan references:** Ch.17, Ch.3 (determinism), Ch.5.1, Ch.32.2 (`forge.phys.backend`),
  I1, I2, I7, I16; M4-3, M7-9 (first half); E-5; WP-60 (cloud card C-1)

## Context

M4-3 asks for `forge-phys`: "`avian` f64 in region-local frames, character controller,
deterministic fixed step". M7-9 (first half, WP-60) adds every collider kind, materials,
layers, triggers, CCD, interpolation, batched and async queries, the full joint set, gravity
and damping zones and a user-selectable backend, and the editor's Play mode running it. E-5
names avian (generic over `f32`/`f64`) with rapier as fallback. The workspace pins `bevy_ecs`
and `bevy_reflect` at exactly `=0.19.1` (ADR 0005), and Ch.5.1 keeps the Bevy framework (the
`bevy` crate, `bevy_render`) out of the engine; ADR 0047 used that rule to keep avian2d out of
the 2D pipeline, leaving E-5 standing for 3D.

## Decision

1. **`crates/forge-phys`** holds a backend-neutral model (`types`), the `forge.phys.backend`
   extension point with its `PhysicsBackend` trait (`backend`), and `PhysicsWorld` (`world`):
   one region-local world in one frame, which does once, above every backend, everything that
   must behave the same whichever backend runs — the fixed step and `advance` (bounded
   catch-up), render interpolation (last two poses, `nlerp`), gravity/point-gravity/damping
   zones (applied kick-drift-kick as velocity changes: second order, identical per backend),
   events in canonical order, queries (checked, answered by the backend, batches across scoped
   threads, async queries answered at the next step boundary), and the BLAKE3 state hash.
2. **Two first-party backends, both `f64` and `enhanced-determinism`** (E-5's "generic over
   `f32`/`f64`" is avian's own property; M4-3 asks for its `f64` build and Ch.1.5 keeps `f32`
   out of simulation, so only that build is used): **avian3d 0.7.0**
   (the default; the 3D preset already names it) and **rapier3d-f64 0.36.0**, registered by the
   `forge.phys` plugin. The project's `physics.backend` setting picks one; a plugin can add,
   replace or chain one (I16).
3. **avian runs in a private Bevy `World`, stepped by hand.** avian3d 0.7 depends on `bevy`
   (`default-features = false`, `std` + `bevy_log`: bevy_app, bevy_ecs, bevy_math,
   bevy_reflect, bevy_tasks, bevy_time, bevy_transform, bevy_input, bevy_diagnostic — no
   `bevy_render`, no windowing, no multi-threaded executor) and its `bevy ^0.19.0` resolves to
   0.19.1: the pin holds and is not bumped. This is a scoped exception to Ch.5.1, contained in
   one backend behind a cargo feature: the engine's ECS never sees avian's world, no Bevy
   `App` loop or clock runs (one schedule is run per fixed step with the step's `dt`),
   `Transform` sync and interpolation plugins are off (avian's `Position`/`Rotation` are the
   `f64` truth), and avian's tree optimiser runs inline, not as an async task.
4. **Both backends mean the same thing:** kinematic bodies move by velocity (a target is the
   one-step velocity to it, then the exact pose); `PhysicsSettings::continuous` (default on)
   sweeps fast bodies against static geometry (both engines do this by default today), off it
   is the cheapest step and may tunnel; a body's `ccd` sweeps it against moving bodies too.
   After every step each backend's query structures are brought up to the step's result
   (rapier refits its tree with `set_aabb`), and structural changes and teleports are taken in
   eagerly, so **a query never changes the simulation** (guarded).
5. **avian's `f32` corners are named, not hidden:** avian 0.7 keeps `ColliderDensity` and its
   query trees' ray/sweep direction in `f32` in its `f64` build. The narrowing happens in one
   allow-listed file (`crates/forge-phys/src/avian/narrow.rs`, `tests/liveness/f32_allow.txt`),
   and every reported hit is recomputed in `f64` against the collider's exact pose.
6. **avian has no generic 6DOF joint, so we add one.** A 6DOF joint whose axes one of its
   stock joints expresses (all locked, one free/limited angular axis, one free/limited linear
   axis, a free or twist-limited ball, all free) is built as that joint; every other
   combination is our `SixDofJoint` (`crates/forge-phys/src/avian/six_dof.rs`), an XPBD
   constraint on avian's own custom-constraint API (prepared per step, solved per substep in
   `XpbdSolverSystems::SolveUserConstraints`, in avian's joint graph for sleeping and
   `JointCollisionDisabled`). It measures what rapier's `GenericJoint` measures — linear axes
   as the anchor offset along A's frame axes, angular axes as `2 atan2(q_i, q_w)` of the
   relative rotation (with `forge_num::det::atan2`) — so a 6DOF joint means the same on both
   backends and no axis combination is refused.
7. **The kinematic character controller** (`character`, M4-3) is shape casts only —
   collide-and-slide, walkable slopes, step-up (edge-aware), ground snap, a kinematic body
   that pushes — so it is the same on both backends; movement modes are WP-61.
8. **Play runs physics** in `forge-sim`: entities with `physics.*` properties become bodies in
   one physics world per frame (the regions' frames), stepped after the scheduler's systems
   and written back to `Placement`/`Orientation`; the edit snapshot carries the project's
   `physics.*` settings (and hashes them only when present, so existing recordings and
   hashes are unchanged). The editor loads `forge.phys` in its default plugin set and hands
   Play the registry its load filled.
9. **Budgets:** `phys.step.avian3d` / `phys.step.rapier3d` (2,000 bodies in a pile, one
   step) in `tests/perf/budgets.ron`, measured by the perf gate, with a positive control.
   Their ratio is provisional (0.85: the requirement's one 60 Hz frame over the dev box's
   calibration) until the dev box records the baseline (gate row `C-phys-budget-baseline`).
10. **Debug drawing** (`forge_phys::debug`): each world draws its colliders and joints as
    line segments in its frame from what it recorded at add time (shapes, joint frames) and
    the poses the last step left, so it is the same for every backend; the one thing a
    backend adds is a convex hull's edges (`PhysicsBackend::hull_edges`, defaulted: a
    third-party backend without it gets the vertices' bounds). The play core hands the lines
    up per region frame and the viewport draws them while playing, under a "Colliders"
    switch. The simulation never reads them.
11. **Determinism corpus:** `tests/test_phys_determinism.rs` pins each backend's hash at steps
    60/120/240 of `scenes::corpus` in `tests/goldens/phys_state_hashes.txt`, recorded on
    ubuntu-x86_64; the windows-x86_64 leg must pass the same file (`C-phys-determinism-windows-leg`).

## Why — the owner's two rules

1. **Better for the user:** one API and one behaviour whichever backend a project picks; the
   plan's default (avian, E-5) and the more complete solver (rapier: a native generic joint, an
   exact cone limit) both available per project; zones, interpolation, async and batched
   queries and events behave identically because they are done once; a query (the editor
   picking during Play) never changes the game; clear coded errors (`PHYS-0001..0005`,
   `SIM-0010`), never a panic.
2. **Faster / more efficient:** nothing runs for a world without physics bodies (the play core
   builds no physics world then) or without zones; a steady `PhysicsWorld::step` allocates
   nothing of its own (`test_phys_step_alloc`); query batches spread over threads; the backend
   is behind an `RwLock` that `&mut` methods reach without locking. Cloud sanity (4 vCPU,
   calibration ~27 ms): 2,000 bodies step in ~10.3-11.0 ms on avian3d, ~2.9-3.4 ms on
   rapier3d; avian allocates ~1,700 times per corpus step, rapier ~176 (printed, not gated).

## Alternatives rejected

- **rapier only** (the card's fallback if no avian release fit): avian 0.7.0 fits the pin, and
  E-5 is avian; keeping both costs one backend module and gives users the choice M7-9 asks for.
- **Our own 3D solver** (as ADR 0047 did for 2D): M7-9 needs the full collider and joint set
  now; two maintained upstream solvers behind one trait are the faster path to parity.
- **avian inside the engine's ECS**: its systems would join the engine's world and schedule
  (Ch.5.1's framework), and its `Transform` sync is `f32`.
- **Child colliders through Bevy `Transform`s**: `f32` offsets. Offsets are exact `f64`
  `ColliderTransform`s, written after avian's hooks (which read identity `GlobalTransform`s).
- **Zone gravity as a single velocity change before the step**: first order; avian landed
  5 cm off after one second. The half-before/half-after kick is second order.
- **Refusing the 6DOF combinations avian's stock joints cannot express** (with `PHYS-0004`
  naming rapier3d; this ADR's first draft): the default backend would lack part of the joint
  set M7-9 asks for. avian's custom-constraint API takes a generic joint for one small file.
- **Building every avian 6DOF joint as `SixDofJoint`**: the stock joints are avian's tuned
  paths (the revolute's hinge alignment, the spherical's swing cone) for the combinations
  they express; ours covers the rest.
- **Refreshing rapier's query tree lazily at the first query**: would make the next step
  depend on whether anything queried; refreshed eagerly after every step instead.

## Consequences

- **Guards (tests/gates.ron):** `C-phys-behaviour`, `C-phys-queries`, `C-phys-joints`,
  `C-phys-zones`, `C-phys-interpolation`, `C-phys-determinism`, `C-phys-step-no-alloc`,
  `C-phys-budget`, `C-phys-character`, `C-phys-play`, `C-phys-play-in-editor`,
  `C-phys-debug-draw`, `C-phys-debug-draw-viewport` bound;
  `C-phys-determinism-windows-leg`, `C-phys-budget-baseline` awaiting the dev box; the
  `forge.phys.backend` point joins `test_extension_point_replaceable`.
- **Measured differences between the backends** the suite bounds rather than hides: avian's
  XPBD limits overshoot a violent slam into a cone by up to ~0.08 rad at its default 6
  substeps (rapier holds it exactly); both integrate gravity per substep (4 on rapier, 6 on
  avian), within 2.5 cm of `g t²/2` after one second.
- **New dependencies** (all permissive): avian3d, avian_derive, bevy and its minimal crates
  (MIT OR Apache-2.0), parry3d-f64 (Apache-2.0), rapier3d-f64 (Apache-2.0), nalgebra, simba,
  glamx, obvhs and their trees — listed with licences in the WP-60 handoff; `NOTICES`
  regenerates with them.
- **Workspace feature unification:** a build including forge-phys enables `bevy_ecs`'s
  `bevy_reflect` feature (through bevy_internal); additive, and every forge crate's tests and
  goldens that were run (forge-core users, forge-sim's golden replay, the editor) are
  unchanged.
- **Debug drawing** of colliders in the viewport is not built here; bodies are drawn by their
  entities' simulated transforms.
