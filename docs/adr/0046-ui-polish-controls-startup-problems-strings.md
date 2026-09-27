# ADR 0046 — UI polish: test-only fault switches, the startup gate, problems with a next step, every string a key

- Status: accepted (WP-U12)
- Governs: Ch.21 §21.3, §21.8, §21.10, §21.21 ("Requirements on every panel"), §21.22,
  §21.23; DoD M2-30, M2-31, M2-70 (and the M2-22 follow-up); gate rows `C-ui-startup`,
  `C-controls-dev-only`, `C-problem-next-step`, `C-panel-empty-states`,
  `C-pseudo-locale`, `C-editor-a11y`, `C-editor-contrast`, `C-keyboard-walkthrough`,
  `C-first-run-tips`, `C-ui-idle-every-panel`, `C-icon-set`, `C-ui-strings-source`; DoD M2-20 and M2-22
  settled on the way.
- Decision rules applied: (1) better for the user, (2) faster and more efficient.

## 1. W2 fault switches exist only in test builds

**Context.** Until now the switches were ordinary fields: every production struct carried
them and every hot path branched on them.

**Decision.** `forge_trace::control_switches!` declares a switch set. Under
`cfg(any(test, feature = "controls"))` it is a plain struct of `pub` fields; in every other
build it is a zero-sized struct with no public field whose accessors (`faults.x()`) return
the field type's `Default`, so each branch folds to a constant and no production struct
carries a switch. Single switches a widget or tool carries are the crate's own
`controls::Switch`. The constructors that turn a fault on (`with_fault_*`, `*_for_control`,
`set_faults`) are `#[cfg(any(test, feature = "controls"))]`.

Each crate with switches declares a `controls` feature (forwarding to the crates below it)
and turns it on for its own tests through a `[dev-dependencies]` self edge; downstream test
crates turn it on the same way. **`cargo xtask layering` rule L5** refuses a
`[dependencies]`/`[build-dependencies]` entry or a feature other than `controls` that enables
it (gate row `C-controls-dev-only`, positive control in `xtask/src/layering.rs`), so a
release build of the editor, the runtime or a game never links a control branch. The I16
scanner (`tests/plugin/test_no_privileged_plugin.rs`) reads a struct declared through the
macro as the struct it is, so the allow-list stays exact.

**Not converted, and why.** forge-render's `RenderOptions` controls (`hard_disc_sprites`,
`perf_fault_sky_repeats`, `isotropic_rayleigh_for_tests`) are uniform data read by WGSL:
removing the CPU field leaves the shader branch, so a sound conversion needs shader
specialisation (lane A's crate).

## 2. `ui_startup_budget` measures the real binary

**Decision.** The test spawns the real `forge-editor` (`--no-user-config --no-remote-host
--exit-after 0 --report-startup`) and measures from `spawn` to the line the editor prints
when the runner first settles (the new `UiApp::settled` hook: every window drawn, nothing
left to do — the first interactive frame). Process creation, plugin load, GPU device, window
and first frame are inside the figure. One untimed warm-up launch (the linker just wrote the
executable), then three timed launches; the gate reads the median. The launches run under
`forge_trace::timed` (machine-wide lock, High priority). The 1.5 s budget gates on the
reference machine (RTX 3080, 12 logical CPUs) only; elsewhere the figure prints under
`AWAITING(reference machine)`. The positive control injects a 2 s stall
(`FORGE_STARTUP_STALL_MS`, read only when the binary is built with `controls`).

Why the binary and not an in-process harness: a user's cold start includes process creation,
the adapter and the window; an in-process measurement would pass while the real launch
regressed (rule 1).

## 3. A problem says what to do next

**Decision.** A warning or an error reaches the user only as a `notify::Problem`: the stable
`ErrorCode`, what happened, why, and **what to do next**. `EditorError::next_step()` gives
each editor variant's next step, `next_step_for_rejection` each `CmdError`'s, and
`next_step_for_code` a crate-level step for any other code. `NotificationCentre::post`
carries information and successes only (a warning posted through it is demoted); an
incomplete problem is still shown — under `EDITOR-0015` (`EditorError::Unexplained`), so
nothing is dropped — and counted (`incomplete()`, the guard's measure). A panel's refusal of
an input before any command is `EDITOR-0014` (`EditorError::Refused`,
`SessionState::refuse`). Toasts and the Notifications panel show the next step after what
happened.

Why at the type: 40 call sites could each forget a next step; a required field cannot be
forgotten, and the counter makes the remaining runtime gap visible (rule 1).

## 4. Every panel has a declared empty state

**Decision.** A panel with nothing to show shows the catalogue's `EmptyState` (text and at
most one action). `EmptyState` now wraps a message wider than its panel and takes a bound
message (`impl Into<Bind<String>>`), so a panel whose reason for being empty changes (no
team server, no licence, nothing selected) keeps one widget. A panel that always has content
(a settings form, the viewport) declares it with `PanelCx::never_empty(reason)`.
`test_panel_empty_states` opens every panel on an empty project.

## 5. First-run tips

**Decision.** A panel declares one tip (`PanelCx::first_run_tip`, a localisation key); the
first-party tables carry one per panel. The tip is a bar at the top of the panel, never
focused, never modal. Dismissing it removes the bar and writes `tips_seen` in the editor
settings (user config); Settings has **First-run tips** (off: never) and **Reset tips**.
**A tip shows only when the editor has a user config directory**: without one
(`--no-user-config`, scripted runs, the test harness) a dismissal could not be remembered and
the tip would come back every start (rule 1). The audit dismisses every tip and measures
**0 frames and 0 wakeups** afterwards (after the one debounced settings save).

## 6. Every editor string is a localisation key

**Decision (gettext model).** A UI string's key is its source (English) text:
`forge_ui::tr!("Save")`; a formatted one is `forge_ui::trf!("{n} lights", n = count)` — the
template is the key, placeholders are named, so a translator can reorder them. A key known
only at run time (a reflected field's label) is `forge_ui::l10n::tr_str`. The lookup is
per UI thread (`set_ui_locale`): the source language returns the key itself with no
allocation; another locale's texts are looked up once and interned, so `tr!` returns
`&'static str` (bounded: keys × locales used in a run). The pseudo-locale (`qps-ploc`)
accents, lengthens and brackets every looked-up string. A language change applies to windows
built after it (the editor rebuilds its windows; a game UI binds labels through
`LocalisedTexts` and switches live).

Why not symbolic keys (`hierarchy.empty`): 1,500 invented keys would make the code unreadable
and every test that finds a label by its text would need a table; the gettext model keeps
the English text in the code, where the reviewer reads it (rule 1), and costs nothing in the
source language (rule 2).

`test_pseudo_locale_no_hardcoded_strings` opens every panel under the pseudo-locale and reads
every visible string from the AccessKit tree (label, value, description, placeholder); one
that is not pseudo-localised was not looked up. Data (entity names, numbers, codes, paths)
is exempt only when it names no words to translate, or is a data value the empty project
puts on screen.

**What "every editor string" covers, precisely (amended after the WP-U12 review).**

* *Keys.* Every string the editor composes for a person to read: labels, buttons, headings,
  tooltips, placeholders, empty states, first-run tips, status and readout lines, history
  labels (a transaction's label is looked up when it is emitted), a11y names and values,
  palette entries, menu titles, notification titles, the **next step of every problem**
  (`EditorError::next_step`, `notify::next_step_for_code`, `next_step_for_rejection`, the
  unexplained-problem fallbacks), the toast and notification-row templates, the level words,
  model enums' presentational words (`label()`/`title()`/`line()` of a licence tier, a
  role, a promotion's loss…) and the collaboration outcomes the team panel shows. A title a
  registry carries until a menu or tab shows it is written `tr_key!("…")` (like gettext's
  `N_()`) and looked up with `tr_str` where it is shown.
* *Data filled into a key.* Values in a template's placeholders are data: names, numbers,
  paths, codes — and an **error's own diagnostic message** (its `Display`, e.g. a
  `CmdError`, an importer's or a validator's reason), which is shown as the *detail* after
  the looked-up "what" and next step, next to its stable `ErrorCode`. Engine crates below the
  editor do not depend on `forge-ui` and keep their messages in the source language; the
  code identifies the error in any language. The audit record (`connect::audit` lines, a held
  security setting's line) stays in the source language too: it is a record, grepped and
  compared, not UI text. `Display` impls and error constructors carry an `// l10n-block:`
  note or are recognised by the source guard.
* *The guard is two-sided.* `tools/forge-editor-bin/tests/test_ui_strings_are_keys.rs`
  (gate `C-ui-strings-source`) reads the source: every worded string literal in the UI code
  (the eight first-party panel plugins, the shell's UI modules, forge-ui's widgets and dock
  views — 117 files) and in the presentational methods of forge-editor's model modules is a
  lookup key or carries an `// l10n:` note saying why it is data. The pseudo-locale audit
  reads what is shown, in **two passes**: an empty project, and a populated one in the
  editor as the binary wires it (`forge_editor_bin::wiring`: asset database, connect
  services, in-memory team server and licence) with a user config directory — two entities,
  the child carrying every built-in component and selected, an image imported into a
  folder, a team created, every first-run tip showing, and a refused rename whose toast and
  notification row are read. The populated pass found and fixed what the empty one could not
  see: the inspector's reflected labels, categories and enum variants, role labels, the
  licence's reasons, the team's outcome lines.
* `FORGE_UI_LOCALE=qps-ploc` runs the real editor under the pseudo-locale. No translation
  tables ship yet.
* Cost: `tr!` in the source language returns its key (no allocation); under another locale
  it is one hash lookup (the locale tag is an `Rc<str>`, cloned by pointer); `trf!` writes its
  arguments straight into the result (no per-argument `String`).

## 7. The editor-wide audits run on the real editor

`test_panel_empty_states`, `test_pseudo_locale_no_hardcoded_strings`, the first-run tips
check, the screen-reader tree audit, the three-theme contrast audit, the keyboard-only
walkthrough and idle-with-every-panel live in
`tools/forge-editor-bin/tests/test_editor_panels_audit.rs`, not `crates/forge-editor/tests`
as §21.23 first placed them: only the binary's crate assembles every first-party panel plugin
(`forge_editor_bin::first_party`), and an audit of a subset would pass while a panel a user
opens fails (rule 1). Each has a positive control built from a test panel plugin.

Findings fixed by the audits: two decorative fills recorded as UI boundaries below 3:1 (the
graph canvas's comment wash, the mixer's meter track and the sprite sheet backdrop) — now
`PaintCx::tint`, which is not a boundary; blank or label-only empty areas in the panels of §4.

## 8. The Forge icon set

**Decision.** 53 vector icons drawn for Forge Engine (`forge_ui::icons`, a 24 × 24 grid in a
tiny path language), offered under **MIT OR Apache-2.0** (`crates/forge-ui/icons/LICENSE`)
so games and plugins may ship them. Each is rasterised once per pixel size (zeno, the
rasteriser swash already brings; MIT OR Apache-2.0) into the glyph atlas's mask plane and drawn
as a glyph (`Primitive::Icon`, `PaintCx::icon`), themed by colour token and contrast-checked
as text. Stroking them as meshes was tried first: each icon split the text batch (a mesh draw
plus a new instance batch), and the gallery's icon sheet drew 65 draw calls against a budget of
6. `keyboard_focus_ring_does_not_break_batches` caught it; as glyphs the sheet adds none. `IconButton::icon`,
`IconView` and the glyph widgets draw from the set; the symbol glyphs the editor used (✕, ⚙,
ⓘ, ✎) map to their icons, so no system-font fallback is needed for them.

Why drawn, not Lucide (ISC, which the §21.16 note anticipated): no network download on the
metered link, no attribution file to keep in step, and the set is exactly the editor's
vocabulary. The owner may relicense or swap it; the `Icon` names are the interface.

## 9. AT-SPI observed in the Linux container

The verify image gains one layer: `at-spi2-core` and `dbus`.
`tools/docker/linux-verify/atspi-probe.sh` starts a private D-Bus session, the accessibility
bus launcher, switches accessibility on for the session, runs `ui_gallery`, and
`crates/forge-ui/examples/atspi_probe.rs` — a screen-reader-side client over zbus (already in
the graph through `accesskit_unix`) — walks the application's tree over the bus and checks
the widgets it expects by role and name.

Observed 2026-09-24 in the rebuilt image (at-spi2-core 2.46.0, dbus 1.14.10): the probe found
`ui_gallery` on the registry within 0.3 s and walked 110 nodes (105 named) — frame, panels,
push buttons, check box, slider, page tabs, images, paragraph and link — with the roles and
names a screen reader announces. The walk also found a defect, fixed: a `RadioGroup`
exposed no options (only `SegmentedControl` did); each option is now a virtual radio button
with its state, position in the set and the Click action (`test_ui_a11y_tree`
`a_radio_group_exposes_each_option_as_a_radio_button`). Evidence:
`docs/evidence/linux/M2-22.md`.

## 10. Text shown verbatim, and data in the pseudo-locale audit

Some text must not be translated: the credit, metadata, `forge.json` and `NOTICES` a build
writes (the licence fixes their wording) and file previews. `Label::verbatim()` marks such a
label (AccessKit class `forge-verbatim`); the audit skips it. Composed strings are checked
word by word outside the pseudo-localised segments: identifiers (`forge.tool.move`,
`human:tester`, `0x5eed…`), numbers and unit symbols pass, and a short reasoned `DATA` list
names the project values an empty project shows (the mixer's `Master` bus, the default
entity name, the seed path root, the product name); an entry nothing shows fails the audit.
Enum identifiers that are also stored values (`Interp::name`, `ParamKind::name`) are looked
up where they are displayed (`l10n::tr(x.name())`), never changed at the source, so a
project file never holds a translated identifier.

**Virtual rows (review fix).** The audit reads the virtual children a widget sends to
AccessKit too (`Ui::a11y_virtual_children`: the live rows of every tree, list and grid —
hierarchy, undo history, notifications, console, asset grid, keybindings), with its own
control (a panel whose list has one literal row, `C-pseudo-locale-rows`). The bus names a
one-command transaction in its canonical English form (`Set e1.light.kind`), which every
peer, the journal and the audit share; `command_labels::history_label` shows it translated
by recognising the canonical forms, and a test fails if forge-cmd adds or rewords one. Two
kinds of row text are data: a record's own line (an audit record's greppable form, marked
`RowItem::verbatim`, the row counterpart of `Label::verbatim`) and a log line's message
filled into the looked-up row template; and the per-panel `PANEL_DATA` list names asset
names one panel shows (the in-memory animation library's motion names), allowed in that
panel only so the same word hard-coded elsewhere is still found.
