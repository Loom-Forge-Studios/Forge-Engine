# ADR 0005 — Build forge-core on a pinned `bevy_ecs`, prove region disjointness incrementally

- **Status:** accepted
- **Date:** 2026-09-20
- **Plan references:** Ch.1.2, Ch.1.3, Ch.1.4, Ch.5 (§5.5 records the implementation), I5,
  W1/W2, W8, Risk S6, DoD M0-9

## Context

M0-9 asks for `forge-core`: the Ch.1.2 error contract, the Ch.1.4 `Profile`, `bevy_ecs` as a
library with the S6 baseline recorded, `EntityId`, a `World`, the region types of §5.3 and a
single-threaded scheduler skeleton that already enforces disjointness. The plan pins the
shapes (`Error{code,ctx,source}`, `Profile`, `RegionSystem`) and leaves the rest open.

## Decision

1. **`bevy_ecs = "=0.19.1"`, `default-features = false, features = ["std"]`**, declared once
   in `[workspace.dependencies]` and re-exported as `forge_core::bevy_ecs`. The latest
   published release is `0.20.0-rc.1`; a release candidate is not a baseline. No
   `bevy_reflect` feature until Ch.6 (WP-04) decides how reflection is wired; no async
   executor or `multi_threaded` until M4-1 needs them.
2. **Error contract.** `Error` has exactly the Ch.1.2 fields. `ErrorCode` wraps a
   `&'static str`; `ErrorCode::new` is a `const fn` that makes a malformed code a compile
   error in a `const` item, `ErrorCode::parse` is the panic-free run-time form. Crates above
   forge-core implement `CodedError` and get `From<E> for Error` from one blanket impl. The
   three crates below cannot see the trait, so forge-core implements it for their enums by
   **parsing their existing `code()` text** — no per-variant mirror to drift, and their
   `#[non_exhaustive]` enums cost nothing. A malformed lower code degrades to `CORE-0013`.
3. **Region ownership** is two structures kept in step inside `Regions` — each region's
   sorted `BTreeSet<EntityId>` (deterministic iteration) and an `entity → owner` `HashMap`
   (lookup only, never iterated, so hash order cannot leak into behaviour).
4. **The scheduler's disjointness proof is incremental.** Every ownership change is
   journalled as `(entity, previous owner)`. The per-tick proof replays the journal: the
   entity is in its current owner's set and *not* in its previous owner's. After a successful
   proof the owner map is a function consistent with every set, so the sets are disjoint;
   the replay re-establishes that at `O(moved · log n)`. A journal longer than the table (or
   than 64) collapses into one full re-derivation (`Regions::check_invariants`, `O(n)`),
   which is also what the read-only `Scheduler::verify` and the property test use.
5. **Waves and barrier.** A stage is a run of consecutive systems whose declared accesses do
   not conflict; a wave is a stage over every region. Declarations are snapshotted once per
   tick and the same snapshot drives both the proof and `RegionView`'s run-time enforcement.
   The barrier runs once per tick, applying outboxes ordered by (sending region, system,
   send order) — independent of wave packing and, later, of thread timing.
6. **Profiles**: a stub classifier over the four Foundations measurements **plus
   `fluid_fraction`** (nothing among the four signals "this region is fluid"), fixed priority
   Fluid > Cave > Crowd > Sparse > Surface, with two hysteresis layers — enter/exit value
   bands and a dwell of 4 consecutive indications. Thresholds are placeholders until M4;
   tuning them cannot change an outcome (I5).
7. *(Superseded by ADR 0007: `Error` is boxed and the allow is removed.)*
   **`#![allow(clippy::result_large_err)]` in forge-core.** The pinned `Error` layout (code +
   `SmallVec<[Ctx; 4]>` + boxed source) is ~200 bytes, over clippy's 128. Forge-core's own
   operations return the small `CoreError` and convert at a subsystem boundary (the tick),
   so the size is paid on error paths.

## Why — the owner's two rules

1. **Better for the user**: every misuse — touching an entity another region owns, an
   undeclared component, a stale barrier message, merging across frames — is a coded,
   greppable error in a report, never a panic that takes down the editor with it. A player
   in a cave mouth does not see the engine flip cost models every frame.
2. **Faster engine**: the first version of the proof hashed every entity every tick —
   measured **2.2 ms per tick at 100k entities (22 ns/entity)**, a 13% tax on a 60 Hz
   frame before any system ran. The journal replay makes it **< 0.01 ms** when membership is
   stable, and proportional to what moved otherwise. Minimal `bevy_ecs` features keep the
   dependency tree and cold build small.

## Alternatives rejected

- **Track `bevy_ecs` latest / `^0.19`** — S6 is exactly the risk of silent churn; an `=` pin
  makes every upgrade a deliberate, measured diff.
- **Full `bevy` or `bevy_app` schedules** — `bevy_app`'s scheduler proves disjointness of
  component access, not of *entity ownership*; the region model needs the latter, and Ch.5.1
  forbids the framework.
- **Re-derive disjointness from scratch every tick** — correct, and what shipped first; lost
  on the measurement above.
- **Mirror `FRAMES-*`/`SEED-*` codes as constants in forge-core** — drifts, and forces a
  wildcard arm on non-exhaustive enums that would silently mis-code new variants.
- **Box the `Error` to 8 bytes** — better for clippy and hot paths, but changes a pinned
  Ch.1.2 signature; that is a plan amendment for the owner, not a WP choice (follow-up).

## Consequences

- **S6 baseline (Risk S6, recorded here):** `bevy_ecs 0.19.1` with the features above pulls
  **53 unique crates** (normal edges, host target; `cargo tree -p bevy_ecs@0.19.1 -e normal`),
  direct: arrayvec, bevy_ecs_macros, bevy_platform, bevy_ptr, bevy_tasks, bevy_utils,
  bitflags, bumpalo, concurrent-queue, derive_more, fixedbitset, indexmap, log, nonmax,
  slotmap, smallvec, thiserror, variadics_please — all permissive (`cargo deny` green).
  Cold debug build of `forge-core` and all its dependencies: **43 s** on the dev machine
  (12 threads, deps at opt-level 2). API surface used: `World` (spawn, get_entity(_mut),
  get, get_mut, despawn, entities), `Entity` (to/from bits, index, generation), `Component`
  + `Mutable`, `Bundle`, `Mut`. The M2-10 spike measures two upgrade cycles against this.
- **Scheduler skeleton cost** (`cargo run --release -p forge-core --example tick_cost`):
  plan + proof + barrier with a no-op system: 0.000 / 0.001 / 0.007 ms per tick at 1 / 16 /
  256 regions (100k entities total); an integrate system through `RegionView` (one `get`
  and one `get_mut` per entity, both ownership- and declaration-checked): 83 / 75 / 42 ns
  per entity. The per-access `BTreeSet` ownership lookup is the next thing to remove when
  M4-1 hands out views over proven-disjoint slices. *(Removed in WP-04: dense owner table,
  26 / 24 / 24 ns per entity — ADR 0007.)*
- **Trust boundary of the incremental proof:** it is sound as long as every membership
  change goes through `Regions`' own methods (all fields private, one module). A bug that
  edits a set *without* journalling would evade the replay until the next full derivation;
  the property test re-derives from scratch after every fuzzed step, so such a bug fails it.
- **Guards:** `crates/forge-core/tests/test_region_disjointness.rs`, control
  `positive_control_mutate_disjointness_build_fails` (the `mutate-disjointness` feature makes
  `split` leave moved entities in the source region; the property test fails and the
  scheduler's incremental proof refuses the tick with `CORE-0007`);
  `profile.rs::positive_control_no_hysteresis_thrashes`;
  `tests/test_error_codes_registered.rs` (every emitted code is allocated to its crate).
- **Follow-ups:** region derivation from the interaction graph and the merge/split
  *policies* (Ch.5.3); parallel waves (M4-1) with `test_region_determinism` and
  `test_thread_boundary`; a possible plan amendment to box `Error`.
