# ADR 0011 — Replaceable registries with manifest-level conflict detection; content-addressed project store

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.32 (§32.2, §32.4, §32.6), Ch.33 (§33.1–§33.4, §33.6), Ch.21.19,
  I16, I17, E-27, E-28, D-4, DoD M0-14, M0-15

## Context

M0-14 asks for `ExtensionPoint`, a `Registry` with add/replace/remove/chain, `PluginId` and
the capability enum; M0-15 for the `ProjectStore` trait, `LocalFs` and BLAKE3 blob
addressing. The plan's sketches leave open: who is acting on a replace/remove/chain, how a
manifest names items of points that do not exist yet, when conflicts are found, what a
revision id hashes, and how the store keeps Windows and Linux projects identical.

## Decision

**Plugins (forge-plugin).**

1. Every registry operation names the acting plugin (`add(owner, …)`, `replace(by, …)`,
   `remove(by, …)`, `chain(by, …)`), and each item records owner, replacer and chain. A
   second plugin replacing (or removing) another plugin's replacement is `PLUGIN-0006`
   naming both; the registry itself never lets last-wins happen.
2. Plugins declare every operation in the manifest (`provides`, `replaces`, and the added
   `removes`, `chains`). The loader finds every conflict **from manifests alone**, WASM
   manifests included, before any plugin code runs; an install doing something undeclared
   is refused. Operations apply by phase (add → replace → remove → chain), then plugin id:
   the result is independent of discovery order.
3. `ExtensionPoint` gains `const NAME` (the manifest spelling). Stable ids for the whole seed
   list are fixed now in `points::SEED_POINTS`, so manifests written today resolve when the
   defining crate lands. The editor points are generic over the panel context
   (`EditorPanel<Cx>`), keeping forge-plugin below forge-editor with full typing.

<!-- -->

5. Plugin commands are `Invoke` handlers on the bus (`install_commands`): a plugin command is
   undoable, dry-runnable and provenance-tagged by construction (I7, I8).

**Store (forge-store).**

6. A `RevId` is BLAKE3 over (parent, tree address, command-log address, message) — never the
   time or anything backend-specific — so a project has the same revision ids on every
   backend, and `copy_project` can prove a round trip through the trait alone.
7. Trees and command logs are blobs; everything a revision names is content-addressed and
   verified on read (`STORE-0004` instead of silently wrong bytes).
8. `StorePath` accepts only paths that mean the same file on Windows and Linux, and a path
   differing from an existing file or directory only by case is refused on every backend.
9. `lock(path)` locks for the store's identity (the plan's signature); while held, writes and
   deletes by any other identity are refused. On `LocalFs` the lock file is created
   exclusively.
10. The two M0 backends — `LocalFs` and `MemoryStore` (labelled in-memory, D-4) — are
    registered on the `StoreBackend` point by the first-party `forge.store` source plugin
    through the ordinary loader.

## Why — the owner's two rules

2. **Faster engine**: registries are plain ordered vectors (a lookup over a handful of items
   per point beats hashing); unchanged files cost nothing at commit (content addressing plus
   a size/mtime cache that distrusts same-tick rewrites); `Bytes` makes reads zero-copy to
   share; the memory backend keeps a case index so collision checks are `O(depth)`.

## Alternatives rejected

- **Last-wins registries** — the silent failure Ch.32.4 forbids.
- **Conflict detection at apply time only** — plugin code would already have run, and the
  report would depend on load order.
- **Timestamps in the revision id** (as git does) — the same project would get different ids
  per backend and per copy, and I17's round trip could not be checked by id.
- **Case-sensitive paths as on Linux** — the file silently aliases on Windows (the bug class
  M0-17 exists to kill).
- **A mock memory store** — D-4 asks for a real in-memory implementation; the parity guard
  holds it to the same hashes as `LocalFs`.

## Consequences

- Gate rows: I17 `Bound`; `C-extension-point-replaceable`, `C-plugin-conflicts` `Bound`;
  `C-store-git-s3-sql` `Unbuilt` (later backends join the parity test); I16 `Unbuilt` until
  the first `plugins/forge-panels-*` crate gives the kernel boundary something to scan.
- A load error leaves the `Extensions` host partly applied; callers load into a fresh host
  and report the error (documented in `loader`).
- `LocalFs` skips working files whose names are not portable project paths (not project
  files by definition); they are never committed.
- Not built here: the WASM host (M2-13), `test_no_privileged_plugin` (I16), the signed plugin
  index (M5-14), Git/S3/SQL backends, semantic three-way merge (Ch.33.3), the grant commands
  themselves (WP-U9).
