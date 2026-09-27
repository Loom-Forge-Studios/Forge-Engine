# ADR 0038 — Spikes S4 (the regionized scheduler under load) and S6 (bevy churn): both resolved, no fallback taken

- **Status:** accepted
- **Date:** 2026-09-23
- **Plan references:** Appendix B (S4, S6), Ch.5 §5.1, §5.3, §5.5, ADR 0005 (the S6 baseline and
  the scheduler skeleton), ADR 0007 (the dense owner table), I5, W1/W2; DoD M2-9, M2-10

## Context

Appendix B gives both spikes a written fallback. **S4**: "fuzz merge/split under load; measure
contention at 4/16/64 regions" — fallback: coarse per-region locks, then process-level
projection. **S6**: "track two upgrade cycles, measure the diff" — fallback: vendor and pin the
five crates, fork if churn exceeds budget twice. ADR 0005 recorded the S6 baseline (`bevy_ecs` /
`bevy_reflect` `=0.19.1`, the API surface used) and the scheduler skeleton (single-threaded;
a wave is exactly what M4-1 hands to threads).

## S4 — what was measured

**Correctness under load** (`crates/forge-core/tests/test_scheduler_s4.rs`, gate
`C-scheduler-s4`): at 4, 16 and 64 regions, two seeds each, 120 ticks of 2,048 entities under
three systems — integrate; a per-entity write plus a barrier effect into the next region from
every region; ~2 % of entities handed to another region per tick — with 1–3 merges or splits
between every pair of ticks, a refused cross-frame merge, spawns and despawns. After every tick
the plan and the ownership table verify from scratch, every live entity is owned exactly once,
and no barrier message is rejected; across the three region counts the integrated state is
identical. Per run: 268–351 merges and splits, 4,440–12,528 barrier messages; 4.8–5.3 ms/tick
(debug). The positive control rebuilds it with `mutate-disjointness` and it fails on an S4
violation.

**Contention** (`cargo run --release -p forge-core --example s4_contention`: 100,000 entities,
~60 ns of work per entity-step, 8 worker threads spawned once, a wave started and ended on a
barrier):

| Regions | forge-core serial tick | owned + barrier, 1 → 8 threads | barrier | lock wait in a wave | one shared queue: lock wait | per-region locks: lock wait |
|---|---|---|---|---|---|---|
| 4 | 6.59 ms | 2.25 → 0.78 ms (×2.9) | 0.086 ms | 0 | 0.43 ms | 0.22 ms |
| 16 | 6.78 ms | 2.27 → 0.46 ms (×4.9) | 0.114 ms | 0 | 0.83 ms | 0.43 ms |
| 64 | 7.21 ms | 2.31 → 0.40 ms (×5.7) | 0.140 ms | 0 | 0.42 ms | 0.73 ms |

Every parallel result equals the one-thread result.

## S6 — what was measured

Two upgrade cycles, each on a clean export of this tree with only the workspace pin changed,
checking and testing every crate that names `bevy_ecs` or `bevy_reflect` (`forge-core`,
`forge-frames`, `forge-reflect`, `forge-cmd`, `forge-sim`, `forge-editor`, `forge-tests`):

| Cycle | Source changes | What broke | Tests | Unique deps (`bevy_ecs` / `bevy_reflect`) | `Cargo.lock` |
|---|---|---|---|---|---|
| 0.18.1 → 0.19.1 (the cycle we took, measured backwards) | 3 files, 4 lines | `bevy_reflect::enums::*` and `::structs::*` became public modules in 0.19; our imports of them (`VariantInfo`, `Enum`, `DynamicEnum`, `DynamicVariant`, `Struct`) move to the crate root in 0.18 | all green (5 core crates; the four-outputs and command-contract guards: see below) | 53 / 41 → 56 / 42 | 132 lines, 5 crates in/out |
| 0.19.1 → 0.20.0-rc.1 (the next cycle, the newest published) | **none** | nothing | all green, the determinism goldens and the replay golden included | 56 / 42 → 59 / 45 | 99 lines, 3 crates in/out |

Cold `cargo check` of the five core crates: 45 s (0.18.1), 49 s (0.20.0-rc.1).

## Decision

1. **S4 resolved; keep the design as built.** A wave's jobs share nothing mutable — each owns its
   region and its own `Outbox`, and everything cross-region waits for the serial barrier — so no
   lock is contended during a wave, and the barrier costs ~0.1 ms per ~2,000 messages. Neither
   fallback is taken: a shared message queue spends 0.4–0.8 ms per tick waiting on its lock, and
   per-region locks (effects applied at once) wait 0.2–0.7 ms per tick — growing with region
   count, because an effect waits for its neighbour's whole job — and they give up the barrier's
   fixed order, which determinism needs. M4-1's guidance: split on load to **at least two regions
   per worker** (4 regions keep 4 of 8 threads busy; 16 and 64 balance), and remove the per-access
   `RegionView` checks inside a proven wave (the serial scheduler's ~4.5 ms over the prototype's
   work is those checks and the bevy lookups, ADR 0007).
2. **S6 resolved; stay on the pinned crates, no vendoring.** Two cycles cost 4 lines and 0 lines:
   far inside any budget, and the `=` pin already makes each upgrade a deliberate, measured
   event. The next upgrade waits for 0.20.0 final (a release candidate is not a baseline, ADR
   0005); this ADR is its dry run. The fallback (vendor the five crates, fork on a second
   over-budget cycle) stays written down; the trigger is a cycle costing more than one working day
   or breaking a determinism golden.

## Why — the owner's two rules

1. **Better for the user**: the region layout can never change a simulation's outcome (the fuzz
   proves it at three region counts under churn), and staying on upstream `bevy_*` keeps its fixes
   and speed-ups a four-line upgrade away.
2. **Faster / more efficient**: no lock on the hot path of a wave (×4.9–5.7 on 8 threads with ≥ 16
   regions, before M4-1 removes the view checks); no vendored fork to maintain or rebuild.

## Alternatives rejected

- **Per-region locks now** (Appendix B's first fallback) — measured slower under load and
  non-deterministic in effect order.
- **One shared barrier queue** — simpler code, but its lock is the wave's only contention point.
- **Vendoring `bevy_ecs` / `bevy_reflect` pre-emptively** — the measured churn does not pay for a
  fork's upkeep.

## Consequences

- Gate row `C-scheduler-s4` (Bound). DoD M2-9 and M2-10 settled.
- The 0.20 upgrade (when final) is: change the two pins, run the same checks.
