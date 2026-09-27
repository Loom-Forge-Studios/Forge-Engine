# ADR 0021 — Allow additive, defaulted methods on the frozen `ProjectStore` trait

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.33 §33.1 (frozen at M0-20), Ch.8 §8.4 (hot reload), W10, ADR 0014 point 6, ADR 0015, ADR 0017

## Context

Chapter 33's `ProjectStore` trait was frozen at M0-20 (ADR 0014). WP-12 (forge-asset hot
reload) needed three things the frozen surface could only give expensively — a full read of
every working file per poll:

- `stamps()` — every working file with a cheap change stamp;
- `size(path)` — a working file's byte count without reading it;
- `stamps_of(paths)` — the stamps of just the files hot reload wrote itself.

They landed in WP-12 as new trait methods. This ADR records that decision against the
freeze, which the WP-12 review asked for.

## Decision

The three methods stay on `ProjectStore`. Each has a **default body built only from the
frozen methods** (`list` + `read` + hash, `read().len()`, a filter of `stamps()`), so no
existing implementation, caller or signature changed, and every backend is correct without
touching it. `LocalFs` and `MemoryStore` override them with metadata / stored-hash versions.

This is inside the FROZEN definition (master-plan "Document map", ADR 0014 point 6):
"additive, non-breaking API is ordinary work." A method is additive only when it has a
default that preserves behaviour for implementors that do not know it exists; a new
required method, or any change to an existing signature, would still be a plan amendment.

## Why — the owner's two rules

1. **Better for the user:** a saved texture shows up in the editor within one poll, and an
   idle editor does not grind the disk re-reading a project.
2. **Faster / more efficient engine:** a quiet poll on `LocalFs` is one directory walk of
   metadata (ADR 0017 measures it) instead of hashing every file.

## Alternatives rejected

- *A separate `StampedStore` extension trait:* every consumer would need a second bound or
  a downcast, and backends lacking it would silently fall back at the call site — the
  default method gives the same fallback in one place.
- *Keep polling with `list` + `read`:* correct but O(project bytes) per poll; fails rule 2.

## Consequences

- Future Ch.33 additions follow the same rule: defaulted, built on frozen methods, marked
  "(additive, WPn)" in the doc comment.
- forge-asset's poll relies on `stamps()` being a full listing of working files (step 3 of
  `AssetServer::poll` checks sidecar presence with a map lookup into it).
