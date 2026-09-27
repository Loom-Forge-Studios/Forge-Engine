# ADR 0010 — Harden forge-cmd: owner-only gesture close, bounded stream, order-preserving audit, shared paths

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.7, Ch.21.18, I7, I8, W1/W2, WP-05 verifier follow-ups (WP-06)

## Decision

1. **Only a transaction's own issuer closes it while it is open.** `Bus::cancel_as(txn, by)`
   and `Bus::undo_as(txn, by)` on an open transaction refuse any other issuer with
   `CMD-0012` — the rule `apply` already enforces for sending into it — audit the refusal
   under `by`, and change nothing. Undoing a *committed* transaction stays open to every
   issuer (provenance-tagged).
2. **Each subscription is a bounded, coalescing queue** (`DEFAULT_SUBSCRIPTION_CAPACITY` =
   4096, `Bus::subscribe_with_capacity`). On overflow the queued events collapse into one
   `Gap { first_missed, last_missed, missed }` that keeps absorbing events in O(1) until the
   subscriber drains. The subscriber resyncs from `Bus::project()` and `Bus::next_seq()` and
   skips delivered events below that seq. `Subscription::drain()` returns `Drained { gap,
   events }` (`#[must_use]`), and `try_next()` yields `StreamItem::Gap` before events, so a
   gap cannot be ignored by accident. The bus keeps only a `Weak` per subscriber; dropping
   the subscription unsubscribes.
3. **Audit folding never reorders the trail.** A gesture folds only while its own lines are
   the newest entries: any other entry (another transaction, someone else's refused
   attempt, begin/commit) ends the fold, and the next frame starts a new line after it. The
   lines of one gesture that are the tail together (a gizmo moving x, y, z each frame) keep
   folding, so a multi-property drag is still one line per property per uninterrupted run.
   `AuditEntry::last_at_ms` records the latest folded frame; no foreign entry's time falls
   strictly inside any line's `at_ms..last_at_ms` (checked by
   `audit_folding_never_moves_a_frame_across_another_entry`, with a checker control).
4. **`drain_audit` keeps the bus's buffer capacity; `drain_audit_into(&mut Vec)`** reuses the
   caller's too.
5. **Property paths are shared, not copied.** `Change::Property::path` is an `Arc<str>`; the
   project's property map is keyed by the same `Arc`, and the builder reuses it when the
   property exists, so after the first frame a drag allocates no path. Transaction merging
   updates the slot in place (only the value is cloned). `apply`, `commit`, `cancel`, `undo`
   and `redo` return `Arc<Applied>`: one allocation shared by the caller and every
   subscriber instead of a deep copy. Audit fold matching compares borrowed strings.
6. **I8 gains a totality clause** (`contract::check_totality`, `Rule::NotTotal`): the
   fixture's `fixture.panic` handler panics mid-plan, and both `dry_run` and `apply` must
   return `CMD-0009` without unwinding or changing state. Removing the bus's `catch_unwind`
   was run and fails I8 with "dry_run unwound into the caller" / "apply unwound".

## Why — the owner's two rules

2. **Faster engine**: gesture frame (3 × `SetProperty` + one subscriber pumped each frame,
   `cargo run -p forge-cmd --release --example gesture_bench`, best of 5 × 200 000 frames,
   RTX 3080 box): **3 590 ns → 2 270 ns per frame (−37 %)**. Subscriber memory is capped at
   `capacity` events, and draining the audit no longer reallocates.

## Alternatives rejected

- **Fold only into the single newest entry** — literal "newest only" breaks folding for any
  multi-property gesture (x, y, z alternate), so a gizmo drag would write one audit line per
  frame per axis again. Treating the gesture's own tail lines as one run keeps the memory
  bound and still never crosses a foreign entry.
- **Keep the newest N events on overflow (drop oldest)** — the subscriber would hold events
  that do not apply to any state it has; a gap plus a snapshot is the only consistent resync.
- **Block the bus on a full subscriber (backpressure)** — one frozen panel would freeze the
  editor.
- **Intern paths in a bus-level interner** — `dry_run` is `&self`; the project's own map keys
  are already a perfect interner with no interior mutability.
- **Adding the issuer to the trait's `undo`** — a larger API change for every sink; the
  in-process owner path and the `_as` forms cover both cases without it.

## Consequences

- Stream consumers must handle `Drained::gap` (the type makes it hard to miss).
- `EditorCommand::SetProperty::path` stays a `String` (it is the reflected, serialised wire
  form); the client allocates it once per frame. Everything after the bus boundary shares.
