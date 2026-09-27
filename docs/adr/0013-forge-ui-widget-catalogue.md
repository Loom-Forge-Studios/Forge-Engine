# ADR 0013 — Build the forge-ui catalogue on bundled faces, a retained batch cache, painted virtual rows and composite builders

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.21 §21.2, §21.7, §21.9, §21.11, §21.12, §21.13, §21.16, §21.22;
  I7; D-4, D-5, D-7, D-8; DoD M2-24, M2-25; WP-U2. Follows ADR 0012 (forge-ui core) and
  ADR 0006 (BSL-1.0, `arboard`).

## Context

WP-U2 builds the §21.16 widget catalogue and the §21.12 virtualisation on the WP-U1 core,
plus the WP-U1 leftovers the orchestrator assigned (D-7 bundled fonts, D-8 OS clipboard,
the interaction-frame re-batching the WP-U1 verifier flagged, rounded panel
backgrounds). The plan fixes the widgets and the budgets; it leaves open how rows are
realised, how composite editors are assembled, how HDR colour is modelled, and what build
profile the millisecond budgets are measured in.

## Decision

1. **Bundled faces, lazy system fallback (D-7).** Roboto (400/400i/500/700) and a
   monospace face are compiled into `forge-ui` (`FontConfig::bundled`); the system font
   database is loaded once, lazily, on the first glyph the bundle lacks (CJK, Arabic,
   emoji). Goldens compare everywhere with `FontConfig::bundled_only`. The mono face is
   Droid Sans Mono (Apache-2.0) until an Apache-2.0 Roboto Mono file is vendored
   (`crates/forge-ui/fonts/README.md`). Gate rows: `C-ui-lazy-font-fallback` is Bound
   (bundled faces, lazy system scan); `C-ui-bundled-font` is Awaiting, because the
   googlefonts release files are not on disk and downloading them needs the owner's approval.
2. **Retained batch cache.** The batcher keeps each display-list slice's instances in
   place with slack; a frame re-batches and re-uploads only the dirty slices (ranged
   uploads), reusing its per-frame vectors. Hovering one button re-batches O(1) slices
   (`ui_interaction_rebatches_o1`, control `positive_control_rebatching_everything_fails`).
   Meshes (`lyon` fills/strokes, colour grids) are a second pipeline keyed by content hash,
   so an unchanged curve tessellates nothing.
3. **Virtual rows are painted, not widgets.** `VirtualTree` (tree and flat list) and
   `VirtualTable` paint the rows that meet the viewport from their model and expose the
   live range (visible ± 8, capped at ⌈viewport ÷ row⌉ + 16) as AccessKit virtual
   children. This replaces §21.12's `RowSource::build → WidgetId` with row recycling:
   100,000 rows are model entries plus ~40 painted records, never taffy nodes or widget
   objects. The tree's row index is §21.12's counted index unchanged (`TreeIndex` over
   `CountedSeq`), so expand/collapse of a 50k subtree touches ≤ (d+1)(⌈log₃₂ b⌉+1) index
   nodes. Rich per-row content (badges, toggles) is added to the row painter as data when
   a panel needs it (WP-U5), not as child widgets.
4. **I7 in collections.** Selection, expansion and scrolling are view state and change in
   the view. Rename in place, drag-drop/Alt+arrow reorder and delete raise
   `RowRenamed` / `RowsDropped` / `RowsDeleteRequested`; the owner (a panel emitting a
   command) applies them and updates the view (`VirtualTree::edit`). Every editor widget
   writes only the signal it was given and raises one `*Edited`/`*Committed` action per
   gesture, which the editor turns into one undoable command transaction.
5. **Composites are real widgets built through `widgets::Build`.** The colour picker,
   vector/quaternion/`FramePos` editors and the property grid are small trees of catalogue
   widgets (each part its own tab stop and AccessKit node), buildable from the `Ui` or
   from an `EventCx` (a popover building its picker when it opens). A composite's root
   reconciles its parts with the model signal on `BindingChanged`; the frame now flushes
   signals up to four rounds so a reconciling write settles in the same frame (equal
   writes are no-ops, so rounds converge).
6. **HDR colour.** `color::HdrColor` is linear-light RGB, unbounded above, straight alpha.
   The picker edits hue/saturation/value of the displayable base colour (sRGB, perceptual)
   times an exposure of 2^EV, keeping hue across greys in a shared `Hsve`. Data colours
   are constructed only in `color.rs`, so the "no colour literal under `widgets/`" lint
   stays meaningful; the lint now matches at word starts (`HdrColor {` is not `Color {`).
7. **The UI layer is optimised in dev builds** (`[profile.dev.package.forge-ui]
   opt-level = 2`, debug assertions on). Unoptimised, re-recording a screen of table cells
   costs ~17 ms instead of ~0.7 ms: `cargo run` would feel broken and the D-5 millisecond
   gates would measure the compiler instead of the design.
8. **Tab order uses unscrolled positions.** Visual reading order is computed from rects
   with ancestor scroll offsets removed, so Tab scrolling a widget into view cannot
   reshuffle the order (it trapped the walk on a long catalogue page).
9. **D-4 stand-ins.** The asset picker reads an `AssetIndex`; `MemoryAssetIndex` is the
   labelled in-memory stand-in (gate row `C-ui-asset-index` UNBUILT until Ch.8). The command
   palette is a view over the commands it is given; WP-U3 feeds it the registry.

## Why — the owner's two rules

1. **Better for the user:** every part of a composite is keyboard-reachable and named;
   destructive dialogs name the loss; a frame change never silently moves an object; HDR
   values are edited without clipping; the gallery walks every page as a user would.
2. **Faster / more efficient engine:** painted rows keep a 100k list/tree/table at
   0.2–0.7 ms per scrolled frame (p95, reference machine) with a bounded live set; an
   interaction frame re-batches one slice; meshes and text are cached by content; cold
   start scans no system fonts.

## Alternatives rejected

- **A widget per live row with a recycling pool** (§21.12 as written): more objects,
  taffy nodes and bookkeeping per scroll step for no user-visible gain at this stage;
  kept available as a later extension if a panel needs arbitrary widgets in rows.
- **Composites as single monolithic widgets:** one tab stop for a colour picker or a
  vector editor hides its parts from keyboard and screen-reader users.
- **Measuring budgets in the unoptimised dev profile:** fails a 2 ms budget by 8× for
  reasons users never see; widening the budget is forbidden (W5).
- **Sorting in the table view:** the plan puts sorting in the model; a 100k copy per sort
  in the view doubles memory.

## Consequences

- `ui_virtual_list_100k`, `ui_virtual_tree_100k`, `ui_virtual_table_100k` are bound
  (gate row `C-ui-virtual-budget`), each with a positive control that fails
  (virtualisation off; an O(k) row splice; cells realised for every row).
- `test_ui_widgets_catalogue` checks that `ui_gallery` shows every catalogue widget
  (control: a missing widget fails), that every page is named and Tab-reachable, and that
  every page idles at zero frames (gate row `C-ui-catalogue`).
- M2-24 stays blocked only on the node canvas (WP-U8) and the dock tab strip (WP-U3),
  which §21.16 lists and the plan assigns to those packages.
