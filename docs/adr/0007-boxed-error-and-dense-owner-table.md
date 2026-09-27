# ADR 0007 — Box `forge_core::Error` to one pointer; make region ownership an O(1) dense table

- **Status:** accepted
- **Date:** 2026-09-20
- **Plan references:** Ch.1.2 (amended), Ch.5.3, Ch.5.5, orchestrator decision D-10,
  supersedes ADR 0005 item 7 and the "next thing to remove" note in its measurements

(Numbered 0007, not 0006: lane B already holds an unmerged 0006, so taking it here would force
a renumber at integration for no reason — D-9.)

## Context

Ch.1.2 pinned `Error` as a struct with three public fields: `code`, `ctx:
SmallVec<[Ctx; 4]>`, `source: Option<Box<dyn StdError>>`. Inline, that is ~200 bytes. Every
`Result<T, Error>` therefore returned through a caller-allocated out-slot of that size and
moved it on every `?`, even on the happy path; clippy's `result_large_err` flagged it and
WP-03 carried a crate-wide `allow`. D-10 decided: box it.

Separately, `RegionView` checked ownership on every component access with a per-region
`BTreeSet::contains` (`O(log n)`), and `Regions` kept a `HashMap<EntityId, RegionId>` beside
it. The integrate benchmark spent most of its per-entity cost there.

## Decision

1. `pub struct Error(Box<ErrorInner>)`. `ErrorInner` is `#[non_exhaustive]` and carries the
   three Ch.1.2 fields unchanged; `Error: Deref<Target = ErrorInner> + DerefMut`, so `e.code`,
   `e.ctx`, `e.source` read and write exactly as before. Constructors (`new`, `with_source`,
   `From<impl CodedError>`), `ctx`/`push_ctx`, `code()`, `source_as()` are unchanged.
   `size_of::<Result<(), Error>>() == size_of::<usize>()` is a `const` assert (build error if
   broken) and the test `error_is_pointer_sized`, whose control shows the unboxed layout is
   > 128 bytes. The `result_large_err` allow is gone and clippy `-D warnings` is green.
2. `Regions` owns an `OwnerTable`: `Vec<Option<(Entity, RegionId)>>` indexed by entity slot
   index, storing the full entity so a stale generation never matches. It replaces the
   `HashMap`, and `RegionView::owns`/every access check read it in `O(1)` (one index, one
   compare). Region entity sets stay `BTreeSet`s — they are the deterministic iteration order.

## Why — the owner's two rules

1. **Better for the user**: nothing in the API a game or tool author sees changed shape —
   the fields are still the pinned fields. The editor stays responsive when a system does
   many fallible accesses per entity.
2. **Faster engine**: an error is built once (one allocation) on the failure path only;
   the success path returns a register. The ownership check dominated `RegionView`:
   integrate through `RegionView` (one checked `get` + one checked `get_mut` per entity),
   `cargo run --release -p forge-core --example tick_cost`, three runs each:

   | regions × entities | before (BTreeSet + HashMap) | after (dense table) |
   |---|---|---|
   | 1 × 100 000   | 81.3–82.1 ns/entity (8.1 ms/tick) | 25.7–26.3 ns/entity (2.6 ms/tick) |
   | 16 × 6 250    | 74.4–78.8 ns/entity | 24.0–25.3 ns/entity |
   | 256 × 390     | 41.1–41.7 ns/entity | 24.2–24.8 ns/entity |

   ~3.2× at 100k entities, and the cost no longer grows with region size.

## Alternatives rejected

- **Keep the inline layout, allow the lint** — pays ~200-byte moves on every fallible call
  forever to save one allocation on a path that is already slow (formatting, logging).
- **`thin-vec`/custom thin error** — more code for the same pointer width `Box` gives.
- **Per-region bitset** — `O(1)` too, but one bitset per region is `regions × max_index`
  bits and cannot check the generation; one shared table is smaller and exact.
- **`HashMap` with a faster hasher** — still hashing on every access; the dense index is
  cheaper and has no hash order to leak.

## Consequences

- `Error` values cannot be built with a struct literal (they never could: `#[non_exhaustive]`).
- The owner table grows to the highest entity index ever owned; `bevy_ecs` reuses slot
  indices, so it tracks live entities (≈1.6 MB at 100k).
- `test_region_disjointness` (and its `mutate-disjointness` control) still pass unchanged;
  `owner_table_matches_exact_generation_only` covers stale handles.
