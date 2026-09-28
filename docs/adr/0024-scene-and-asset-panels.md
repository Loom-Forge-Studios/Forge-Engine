# ADR 0024 — Scene and asset panels read through services and a change log; components are reflected structs under a property prefix

- **Status:** accepted
- **Date:** 2026-09-21

(Numbered 0024: `dev` holds 0022 and 0023; D-9.)

## Context

- what a *component* is in a project whose entities hold flat reflect-path properties;
- how a 100k-entity hierarchy follows the mirror without rebuilding on every change;
- how thumbnails arrive asynchronously without the UI polling (D-5 zero idle);
- how an OS file drop becomes an undoable import;

## Decision

1. **Services next to the mirror.** Panels read them through `PanelBuilder::services`,
   `PanelAct::services`, `PanelSync::services`. None is a way to change project state.
2. **Components are reflected structs under a key.** `ComponentCatalog::register::<T>(key)`
   walks `T`'s `#[forge_api]` metadata and `T::default()` through reflection: a field
   `f` is the property `key.f`; nested structs extend the path; an enum stores its variant
   name and its active data variant's fields beside it; a `FramePos`/`FrameVel` is
   `path.frame` + `path.local` (I1). Hidden fields are stored but get no row. Adding a
   component sets every default and removing it removes every property under the key, each
   one transaction. `EditorKind` maps every `PinKind` to an editor (`editor_for` is total).
3. **The mirror keeps a bounded entity change log** (`ProjectMirror::changes_since`,
   `CHANGE_LOG_CAP` = 8192, cleared on resync). The hierarchy applies it row by row and
   rebuilds only when it fell behind (with a filter too, decision 9); the inspector refreshes only
   the rows of entities the log touched, and rebuilds only when the *shape* (selection,
   components, active variants) changed.
4. **Thumbnails are pushed, never polled.** `VirtualGrid` (forge-ui) asks its
   `ThumbProvider` for a tile's thumbnail the first time the tile is painted; the browser's
   provider calls `AssetCatalog::thumbnail`, which puts a `std::task::Waker` on the asset
   server's `Handle<Thumbnail>`; when the worker finishes, the waker posts through the
   window's `Poster` (one wake), the grid uploads the RGBA into the image atlas and
   repaints. Resident thumbnails are capped at `THUMB_CACHE` (96) and evicted
   least-recently-painted, freeing their atlas slot (`ImageAtlas::remove`).
5. **A file drop is a copy, then a command.** `AssetCatalog::stage` copies the dropped bytes
   into the folder shown (the user's own file operation, like a copy in the OS file
   manager), then the browser emits `forge.asset.import` for each importable file, one
   transaction. Undo removes the registration and keeps the file .

<!-- -->

7. **Signal-bound editors act through `SignalRelay`.** A relay is an invisible widget that
   raises `SignalChanged` when its signal changes, so a rebuilt inspector leaves no
   runtime effects behind; its signals are disposed on rebuild.
8. **Console and reveal.** `ConsoleLog` groups consecutive repeats, is bounded, and takes
   entries from any thread through `LogSender` (one wake per drain). Refusals are logged
   with their command and code; the toast's "Details" is `NoticeAction::Reveal`, and
   `SessionState::reveal` lets the console, the navigator and the undo history select what
   another panel points at.
9. **Filtering is incremental and bounded** (amended after verification). Under a filter
   the hierarchy still applies the change log: a change re-tests only the entity it names
   and walks its ancestors, inserting or pruning dimmed context rows. The query is a
   `forge_ui::fuzzy::ContainsQuery`, folded once, so testing a name allocates nothing. A
   new query is applied as a diff (remove what it hides, insert what it adds, re-dim the
   rest) in `FILTER_SLICE` (4 ms) slices, one per loop turn, with a spinner in the bar;
   a narrowing query re-tests only the rows shown, a broadening one removes nothing. A
   panel with work left asks for another turn: `PanelAct::want_turn` /
   `PanelSync::want_turn` make the shell run that window's sync steps next turn even
   though no revision changed, and `Shell::next_deadline` wakes the loop for it.
   Idle panels never ask, so the idle loop still sleeps (`ui_idle_*` unchanged).
10. **Integers are exact.** Integer, id and time rows use `forge_ui::IntegerField` over
    `i128`, never `f64` (exact only to 2^53). `inspect::int_range` is the Rust type's range
    (`u8` 0..=255, ids the `u32` range, `Tick` the `i64` range) narrowed by
    `#[forge(range)]`; typed values outside it are refused with the reason, and
    `int_value` snaps and clamps whatever arrives. A `u64` above `i64::MAX` is stored in
    `Value::Int` as its two's-complement bit pattern and read back through the field's
    type (`int_of_value`), so a seed or hash round-trips all 64 bits; time is shown as
    fixed-point seconds with exactly six decimals.
11. **The console follows from a cursor.** It appends only entries past the last id it
    looked at (`ConsoleLog::entries_from`, O(log n) to the start), trims entries dropped at
    `CONSOLE_CAP` from the front of its rows, and relabels the newest row when a repeat
    bumps its count. `VirtualTree::remove` is O(subtree), so a line at the cap writes two
    rows. Only a filter change rebuilds.

## Alternatives rejected

- **Only debouncing the filter** (the search field already debounces 150 ms): the turn
  that applies it would still stall for the whole scan and diff (92 ms at 100k).
- **Caching lower-cased names**: 100k extra strings to keep in step with renames; the
  allocation-free matcher gives the same per-name cost with no state.
- **Keeping integers in `f64` and refusing values above 2^53**: a `u64` seed is routinely
  above it; an `i128` field costs nothing extra.
- **A `Value::UInt` variant**: a wire and schema change for every client; the bit pattern
  in `Value::Int` plus the field type carries the same information.
- **Reading `Project` directly or holding the `AssetServer` in a panel**: breaks the
  client/core split (Ch.34) and I7's import-graph guard.
- **Polling thumbnail handles each frame**: a busy loop while thumbnails render and a
  timer forever after (D-5).
- **Signal effects for editors**: effects live as long as the runtime; the inspector
  rebuilds on every selection change and would accumulate them.

## Consequences

- The project model has no sibling order: a drop between two rows reparents and does not
  reorder. Presence badges wait for WP-U10's stream (M2-35 BLOCKED); the shader-node
  click-through waits for the graph editor (WP-U8, M2-39 BLOCKED).
- Components are editor-side definitions until the ECS component registry exists; the
  `ComponentCatalog` API is the seam where engine components will register.
