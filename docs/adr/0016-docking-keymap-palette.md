# ADR 0016 — Dock with flat keyed frames, key chords by context with named overrides, one action registry

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.21 §21.9, §21.14, §21.17, §21.19, §21.23; Ch.31 §31.3; I7, I15, I16;
  DoD M2-26, M2-27, M2-45; D-1, D-4, D-5, D-7 (amended)

(Numbered 0016: `dev` holds 0014 and lane A's worktree already uses 0015; D-9.)

## Context

§21.17 fixes the docking model (`DockNode`, `Layout` as RON), the interactions, the keymap
contexts and conflict rule, and the palette's reach. It leaves open how the dock maps a
layout onto the retained widget tree, where docking code lives, how a key reaches the
keymap when nothing is focused, how an inner context may legitimately reuse an outer
chord, how a `'static` panel registry builds widgets, and how floating windows share one
layout across per-window `Ui`s.

## Decision

1. **Docking lives in `forge-ui` (`forge_ui::dock`)**; the editor supplies panels through a
   `PanelHost` (in `forge-editor`: the `EditorPanel` registry). The layout model is pure data
   with normalising operations; hand-edited or older files always load into a valid tree
   (empty groups, one-child splits and nested same-axis splits collapse; ratios repaired;
   newer versions refused with UI-0008). Normalising is idempotent to the bit, so a saved
   layout reads back exactly.
2. **Flat, keyed frames.** Every panel frame is a direct child of the dock area keyed
   `panel:<id>` and absolutely positioned by percent insets plus pixel margins from the
   tree's geometry. Re-docking, splitting, resizing and maximising restyle and show/hide
   children; **no panel is ever rebuilt**, and a window resize reflows in `taffy` alone.
   Only children whose placement changed are restyled. The same geometry function drives
   hit testing (drop zones) and styles, so the preview and the result always agree.
3. **Interactions are requests.** Widgets raise `DockRequest`; the dock area's handler
   applies local ones to its `Signal<DockTree>`; a tear-off bubbles to the application, whose
   `DockController` owns the multi-window `Layout`, opens the OS window (runner `OpenWindow`
   with a desktop position and a monitor hint), docks cross-window drops, and puts a closed
   floating window's panels back into the main window. Desktop coordinates are physical
   pixels divided by the primary monitor's scale (one linear space across mixed DPI);
   `place_window` reopens a window whose monitor is gone on one that exists.
4. **Keys: a window key sink.** `Ui::set_key_sink` gives the keymap host every key the
   focus chain did not handle, including with nothing focused. Resolution walks role →
   panel (a focused tab strip counts as its active panel) → window → global.
5. **Conflicts are errors naming both actions**; the same chord in overlapping contexts, or
   a chord that is a prefix of another, is refused. An inner context may override an outer
   binding only by naming the action it shadows (`bind_shadowing`, `shadows:` in data).
   Layers (defaults → preset → user) rebind per action; a colliding layer entry is
   reported and skipped, never last-wins.
6. **One action registry** — the `Action` extension point (`forge.editor.action`, defined
   in `forge-editor`) — feeds keymap display, menus and the palette. Invoking returns an
   `Invocation` (commands for the bus, undo/redo, or a session op); the registry never
   touches project state (I7). The palette covers actions, panels, every `EditorCommand`
   variant (constructible, from the selection, or a prompt for arguments; coverage is
   checked by reflection) and every registered `Invoke` handler.
7. **Panels as build steps.** `PanelCx` is `'static` data: a panel queues build steps that
   run against its frame, so `Fn(&mut PanelCx)` registries replace and chain naturally.
8. **User config is written directly, crash-safely** (write-then-rename) — the three
   `write_atomic` references are the reasoned I7 allow-list entries. Autosave: 2 s after
   the last change, at most 10 s after the first, and on exit; a deadline, not polling.
9. **Stand-in panels (D-4).** Until `plugins/forge-panels-*` exist, the labelled
   `forge_editor::stand_in` plugin registers every §21.21 panel through the ordinary loader
   (gate row `C-first-party-panels` UNBUILT).
10. **Shift+Space is reserved for maximise**: buttons, toggles and switches no longer take
    Space with modifiers (text entry still types).

## Why — the owner's two rules

1. **Better for the user:** a re-dock never loses a panel's scroll, selection or focus;
   disabling a plugin never destroys a layout; a torn-off window never opens off-screen;
   closing a floating window never loses panels; a chord conflict is explained with both
   names instead of silently stealing a shortcut; every command is one fuzzy search away
   and recently used ones come first; a crash never tears the layout file.
2. **Faster engine:** a dock change touches only moved children (a tab switch re-records
   9 slices, a splitter-drag frame costs ~0.17 ms including recording render, measured in
   `dock_interaction_frames_are_cheap`); window resizes need no widget code; idle stays at
   zero frames; batching reuses retained scratch lists and uploads only dirty mesh ranges.

## Alternatives rejected

- **Nested widget containers mirroring the tree** — every re-dock would rebuild panels and
  lose their state; the positive control `rebuild_panels_on_change` shows the difference.
- **Last-wins keymaps** — silent shortcut theft; the conflict guard's control is exactly this.
- **Docking in `forge-editor`** — game tools UIs could not use it, and the widgets need
  `forge-ui` internals (drag sessions, key sink).
- **Per-window independent layouts** — cross-window drops and persistence would need a
  second source of truth; one `Layout` in the controller keeps save/restore trivial.

## Consequences

- WP-U4 hosts `DockController`, `keymap_host`, the palette popup and autosave in the shell,
  and adds the keybindings editor on `KeyMap` (rebind/unbind/audit already exist).
- New points `EditorOverlay`, `ViewportTool` and `Theme` still need defining (M2-27 blocked).
- A dock area's children are absolutely positioned: panel content must fill its frame
  (the frame is a flex column).
