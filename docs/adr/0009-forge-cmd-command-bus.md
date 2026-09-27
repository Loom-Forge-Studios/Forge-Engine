# ADR 0009 — Plan every command into an invertible diff; the bus owns the project

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.7 (§7.4 records the implementation), Ch.1.2, Ch.6 (output 4),
  Ch.21.18, Ch.33.4, Ch.34.2, I7, I8, O-13, W2, W3, DoD M0-8

## Context

Ch.7 pins `CommandEnvelope`, `CommandSink { dry_run, apply, undo }` and three guards, and
Ch.21.18 adds what the UI needs: gestures that are one undo step, Esc to cancel, redo,
`Applied` as the only way state flows back, non-redoable security commands, a panicking
command caught at the bus. It leaves open what "project state" is before `forge-store`
(WP-06) exists, how undo is produced per command, how transactions merge, how
`#[forge_api]` command outputs reach the bus, and how the I7 guard can exist before
`forge-editor` (WP-U4).

## Decision

1. **The bus owns a `Project` document** (entities keyed by a stable `EntityKey`, a
   hierarchy, reflect-path properties of a small `Value` enum, project settings). Its
   mutators are crate-private; `Bus::project()` is read-only; there is no `project_mut`.
   `EntityKey` is not `forge_core::EntityId` (a per-session `bevy_ecs` handle): keys are
   allocated by the core, never reused, and identical on every client, which the command log
   (Ch.33.4) and a remote mirror (Ch.34.2) need.
2. **Every command is planned into a `Diff` of elementary `Change`s, each carrying `before`
   and `after`**, by a `DiffBuilder` over `&Project`. Undo applies the inverse; redo applies
   it again. No command has undo code of its own, so I8's "undoable" holds by construction.
   `dry_run` returns the planned diff — exactly what `apply` applies (the O-13 optimistic
   preview), checked per variant by the contract guard.
3. **`apply` is atomic**: each change checks its `before` against the project, and a failure
   rolls back the applied prefix (`CMD-0008`, nothing changed). A panic while planning is a
   `Rejection` (`CMD-0009`) and cannot corrupt state (planning holds `&Project`); a panic
   while applying (unreachable: no indexing, all preconditions checked) poisons the bus
   (`CMD-0015`) instead of building on unknown state.
4. **Transactions.** A fresh `TxnId` on an envelope = one command, one undo step.
   `begin`/`commit`/`cancel` bracket a gesture. Inside a transaction, value changes
   (`Property`, `Setting`, `Renamed`) **merge** by key — earliest `before`, latest `after`,
   dropped entirely when they cancel out — so a drag of any length is one change per
   property; structural changes never merge (moving a reparent earlier could name a parent
   created later). `undo(open txn)` = cancel. Undo is selective: any committed transaction
   whose changes still apply cleanly can be undone; Ctrl+Z/Ctrl+Y use `undo_target` /
   `redo_target` (a new commit clears Ctrl+Y). History is bounded (default 10 000; older
   transactions expire and their records are dropped — see 10).
5. **`CommandPolicy { human_only, undoable, redoable }`** per command (an `Invoke` takes its
   handler's). This is the primitive WP-U9 needs for "undo and redo never grant".
6. **`EditorCommand::Invoke { target, args }`** reaches registered `CommandHandler`s: a
   mutating `#[forge_api]` fn's `CommandDesc` (`Bus::register_api`, arguments checked against
   the descriptor's fields) or a plugin command. Handlers receive a `DiffBuilder`, never
   `&mut Project`. `args` is JSON text so the command stays `Reflect`.
7. **Provenance** — every envelope carries its `Issuer`; the transaction record, every
   `Applied` event and every `AuditEntry` (including every refusal) records it. `undo_as` /
   `redo_as` attribute an undo to whoever asked. Audit timestamps come from an injected
   `Clock` (provenance, not content: determinism is unaffected).
8. **The `Applied` stream** is `std::sync::mpsc` of `Arc<Applied>` — shared, never deep-copied
   per subscriber; dropped subscribers are pruned on the next send.
9. **Guards.** I8 is `forge_cmd::contract::check_contract`, generic over `ContractSubject` so
   the future `RemoteBus` runs the same check; its variant list comes from the reflected
   `TypeInfo`. The undo fuzz is a fixed-seed proptest with a `mutate-undo` child build. I7 is
   an AST + import-graph resolver over every workspace crate (aliases, globs, re-export chains
   within and across crates, `crate`/`self`/`super`, fn-local `use`, macro arguments), with a
   reasoned allow-list; until forge-editor exists it runs on a scaffold fixture and the real
   check reports UNBUILT.
10. **Bounded memory in a long session** (amended after verification found per-frame
    growth). Nothing the bus holds grows per gesture frame or per command ever sent:
    - only *live* transactions (open, or in the history) keep a `TxnRecord`; expired,
      cancelled and empty-commit transactions are retired to a tombstone of their final
      state (at most `RETIRED_WINDOW` = 4096), and older retired ids report `Expired` from a
      watermark — so late envelopes and undos still get `CMD-0006`, never `CMD-0005`;
    - a record stores its commands as a `CommandSpan { first, last, count }`, not a list;
    - duplicate refusal (`CMD-0013`) remembers the last `REPLAY_WINDOW` = 4096 ids exactly
      and refuses anything older as a stale resend (ids are bus-allocated and increasing);
    - inside an open gesture, the owner's frames on one target (property, setting, name,
      `Invoke` handler) with one outcome fold into one `AuditEntry` (`merged` counts them,
      `first_command..command` spans them); other issuers' attempts keep their own lines;
    - the undrained audit is capped (`DEFAULT_MAX_AUDIT` = 65 536; the store drains it);
      overflow drops the oldest quarter and counts it in `Bus::audit_dropped` — never silent;
    - the accumulator reuses a no-op slot, so a drag crossing its start value does not grow.
    `Bus::footprint()` reports all of it; `tests/test_bus_memory.rs` pins it to history cap x
    touched properties and to the same footprint for a 3 000- and a 10 000-frame drag.

## Why — the owner's two rules

2. **Faster / more efficient**: merging keeps undo memory at one change per property per
   gesture (not 60 per second); the builder's overlay keeps a 10k-entity despawn linear; the
   stream shares one `Arc` per event; undo/redo cost is proportional to the transaction's net
   changes, never a snapshot of the project.

## Alternatives rejected

- **Per-command `undo()` implementations** — every new command is a new chance to get undo
  wrong, and a dry run would need a third code path. Inverting a diff is one code path for all.
- **Snapshot-based undo** (clone the project per transaction) — memory and time scale with
  project size instead of edit size.
- **Merging every change kind** — reordering structural changes can break redo (a reparent
  onto a parent that does not exist yet); the fuzz found no need for it.
- **Name-based I7 check** (grep for `World`) — W3: a local `struct World` false-positives and
  `use forge_core::World as Scene` false-negatives; the resolver handles both (negative and
  positive controls).
- **Waiting for forge-editor to write the I7 guard** — the resolver and its controls are the
  expensive part and are editor-independent; the gate row stays honest (`Unbuilt` for I7,
  `Bound` for the resolver's controls).

## Consequences

- New project-state kinds (assets, graphs, plugins, team membership) become new `Change`
  variants with a `before`/`after`, or `Invoke` handlers over the builder — the contract
  guard fails until each new `EditorCommand` variant has a sample.
- `forge-store` (WP-06) persists the `Project` and the command log; `Project` is the in-memory
  document it loads into, through commands.
- Gate rows: I8 `Bound`, `C-undo-fuzz` `Bound`, `C-command-liveness-resolver` `Bound`, I7
  `Unbuilt` until WP-U4.
- Not built here (follow-ups): `RemoteBus` (M2-16), `ProjectMirror` / `CommandEmitter` /
  `Gesture` in the editor shell (WP-U4), persistence of the audit log (drained by the store),
  sibling ordering in the hierarchy (children are in key order), security-class commands
  themselves (WP-U9).
