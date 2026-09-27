# ADR 0018 — The editor shell is a client of a headless core, through a ticketed BusClient

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.7.1, §7.3; Ch.21 §21.3, §21.17, §21.18, §21.21, §21.23; Ch.34 §34.2,
  §34.4; I7, I15, I16; DoD M2-1, M2-3, M2-4, M2-28, M2-41..M2-44; D-1, D-4, D-5

(Numbered 0018: `dev` holds 0017; D-9.)

## Context

§21.18 fixes the shape — panels get a read-only `ProjectMirror` and a `CommandEmitter`,
the mirror is fed only by the `Applied` stream, gestures are one transaction, the shell
stamps the issuer — and Ch.34 says the split editor must be a transport swap. It leaves
open: where the bus lives relative to the UI, how requests report results when the core
may be remote, how a `'static` panel registry wires widget actions and follows the mirror
across several OS windows (each with its own signal runtime), how the binary is packaged
next to first-party panel plugins, and what "every editor action is undoable" means for
actions that are session state.

## Decision

1. **Headless core + clients.** `core` is the only module that names `Bus` or `CommandSink`,
   and it names nothing in `forge-ui`; `test_command_liveness` enforces both through the
   import graph (gate row `C-command-liveness-split`).
2. **Ticketed requests, outcomes only from the stream.** `BusClient::apply/begin/commit/
   cancel/undo/redo` return a `Ticket`, never a result; state changes arrive through
   `pump()` (the `Applied` events of every issuer, plus a `Gap` marker) and refusals of
   this client's own requests arrive there too. The UI is written as if the core were
   remote; `LocalBus` happens to deliver in the same loop turn. Every refusal becomes a
   notification carrying its stable `ErrorCode`. A change by one client wakes the others'
   loops; a refusal wakes nobody.
3. **The mirror has no signals; panels have sync steps.** `ProjectMirror` keeps revision
   counters (whole, structure, settings, history, and per-path revisions only for watched
   paths). A panel built with `PanelCx::add_live` registers **handlers** for the actions its
   widgets raise (routed by the shell from the source widget up its ancestors, per window)
   and **sync steps** the shell runs in that panel's window only when the mirror's or the
   session's revision changed. Each window keeps its own `Signal`s; nothing crosses
   runtimes. `ProjectMirror::apply` is crate-private.
4. **Gestures** hold at most one command per loop turn (a second update in the same turn
   replaces the held one, which goes out next turn or at commit); dropping an uncommitted
   gesture cancels it. `forge_ui::widgets::Slider` raises `SliderEdit` with a phase
   (Begin/Update/End/Cancel/Step) so a panel can make a drag one transaction; Esc during
   a drag restores the start value and cancels.
5. **Session state and user config are not commands.** Opening panels, layouts, theme,
   UI scale, reduced motion, caret blink, the keymap, selection and notifications change
   directly; editor settings and the keymap are saved by the shell's debounced,
   crash-safe writer (the I7 allow-listed `user_config::write_atomic`). M2-3 is checked as:
   every *command* action is exactly one undo entry that Ctrl+Z restores and Ctrl+Y redoes,
   and every *session* action leaves the project state hash unchanged
   (`test_undo_every_editor_action`, gate row `C-undo-every-editor-action`).
6. **Settings from reflection.** A settings page is generated from a `#[forge_api]`
   struct's inspector metadata (`ForgeRegistry::inspector`): bool → checkbox, enum →
   choice, bounded float with `widget = "slider"` → slider (a drag is one gesture), other
   numbers → numeric field (one command per commit), text → text field (one command per
   Enter). Project pages store `prefix.field` keys through `SetSetting`; WP-U4 ships
   `ProjectEditorSettings` (`editor.*`: grid, snapping, up axis, new-entity name).
7. **The undo history travels on activation** (double click or Enter), not on selection:
   arrow keys through a long history must not fire a storm of undos. Activating an entry
   undoes the newer committed transactions newest first (or redoes up to an undone entry),
   one bus call each; "Initial state" undoes everything listed.
8. **Packaging.** The shell (`forge-editor`, a library, runner-agnostic and tested
   headless through `forge_editor::testing::Rig`) + the first-party panels
   (`plugins/forge-panels-core`, a source plugin through the ordinary loader) + the winit
   runner are assembled by the `forge-editor` binary in `tools/forge-editor-bin`: it cannot
   live inside `forge-editor`, because `forge-panels-core` depends on `forge-editor`.
   The panels later WPs build stay the labelled stand-in plugin (`StandInPanels::without`
   the real ids).
9. **Runner hooks, never polling.** `UiApp` gains `next_deadline` (the shell's debounced
   saves join the loop's `WaitUntil`), `user_scale` (a settings change re-lays out every
   window) and `exiting` (flush the autosaves). `Ui` gains a window key (actions are
   routed per window) and a window-wide caret-blink switch.

## Why — the owner's two rules

2. **Faster / more efficient:** idle is free — no frames, no wakeups, no sync work
   (`ui_editor_idle_zero_redraw`, gate row `C-editor-idle`); a panel does work only when the
   revisions it watches move; a 60-frame drag is 60 merged commands in one transaction, not
   60 undo entries; a history of 10,000 transactions updates one row per new entry.

## Alternatives rejected

- **Signals inside the mirror.** Each OS window has its own signal runtime, so mirror
  signals would be per-window anyway, and a torn-off panel would need its signals moved.
  Revisions plus per-window sync steps are simpler and cheaper.
- **`apply` returning `Result`.** Convenient locally, wrong for M2-16: every panel would
  grow a second code path when the core became remote.
- **Undo-history travel on selection change.** Selection moves with arrow keys; a
  keyboard user scrolling the list would undo their afternoon.
- **The binary inside `forge-editor`, panels as a module.** Would make the first-party
  panels privileged (not loaded through the plugin loader), against I16.
- **A timer widget for the autosave deadline.** Works, but hides application deadlines
  inside the widget tree; the `next_deadline` hook keeps the loop's wake reasons explicit.

## Consequences

- `RemoteBus` (M2-16) implements `BusClient`; nothing above it changes. Its optimistic
  overlay is `test_optimistic_reconciliation` (gate row `C-optimistic-reconciliation`,
  Unbuilt until then).
- A new panel crate `plugins/forge-panels-<x>` is checked by I7 and the split rule the
  moment it exists (the guard globs `forge_panels_*`).
- Not built here: the `EditorOverlay` extension point (§21.19; no backlog scope names
  it), a prompt for palette commands that need arguments (it posts a notification naming
  the fields), and writing the project to a store ("Save" says so; WP-U7).
