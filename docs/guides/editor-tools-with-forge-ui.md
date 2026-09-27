# Building editor tools with forge-ui

A how-to for plugin authors and engine contributors who add a panel, a tool or an overlay
to the Forge editor (Ch.21; WP-U12). Everything here is the same API the first-party panels
use: a first-party panel has no path a third-party one lacks (I16). The code snippets are
taken from the shipped panels; follow the file references for complete versions.

## 1. The shape of a panel

A panel is registered through the `EditorPanel` extension point by a `SourcePlugin` (or a
WASM plugin's `ViewSpec`, Ch.32 §32.8). Its build function receives a `PanelCx` and **queues
build steps**; the dock runs them into the panel's frame whenever the panel is shown.

```rust
use std::sync::Arc;
use forge_editor::panels::PanelCx;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

pub struct MyTools {
    manifest: Manifest,
}

impl SourcePlugin for MyTools {
    fn manifest(&self) -> &Manifest {
        &self.manifest // declares `provides: [EditorPanel("studio.lights")]`
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<EditorPanel<PanelCx>>(
            "studio.lights",
            PanelDescriptor {
                title: forge_ui::l10n::tr("Lights").to_string(),
                icon: Some("light".into()), // a name from the Forge icon set
                default_dock: Dock::Right,
                build: Arc::new(build),
            },
            Order::Last,
        )
    }
}

fn build(cx: &mut PanelCx) {
    cx.first_run_tip(forge_ui::tr!("Pick a light to edit its colour and intensity."));
    cx.add_live(|pb| {
        // widgets, handlers and sync steps: see below
        Ok(())
    });
}
```

`cx.add` queues plain widgets; `cx.add_live` gets a `PanelBuilder` (`pb`) that can also
register **handlers** (`pb.on`) and **sync steps** (`pb.sync`).

## 2. Widgets

Widgets come from the catalogue in `forge_ui::widgets` (§21.16): `Button`, `IconButton`,
`TextField`, `NumericField`, `Slider`, `Checkbox`, `ComboBox`, `VirtualTree` (lists and
trees of any size), `VirtualTable`, `VirtualGrid`, `NodeCanvas`, `CurveEditor`,
`EmptyState`, `IconView`, … Add them under `pb.parent` with a **stable key** — ids derive
from keys (§21.4), so a key is how tests and the layout find a widget again:

```rust
let space = pb.b.theme_ref().space;
let bar = pb.b.add(
    pb.parent,
    "bar",
    NodeStyle::row(space[1]).padding(space[1]),
    Container::new(Role::Toolbar).labelled(forge_ui::tr!("Light actions")),
)?;
let add = pb.b.add(bar, "add", NodeStyle::leaf(), Button::new(forge_ui::tr!("Add light")))?;
let list = pb.b.add(
    pb.parent,
    "list",
    NodeStyle::leaf().grow(1.0),
    VirtualTree::list(forge_ui::tr!("Lights")).single_select(),
)?;
```

Style through **tokens**, never raw colours (`ColorRole::FgMuted`, `theme.space[2]`): a lint
rejects colour literals under `forge-ui/src/widgets/`, and `test_ui_contrast` plus the
editor-wide contrast audit check every `(fg, bg)` pair your panel paints in all three themes.
A translucent decorative surface is `PaintCx::tint`; only real boundaries are checked.

For a list of any length use `VirtualTree`/`VirtualTable`: 100,000 rows cost what a screenful
costs (`ui_virtual_list_100k`, ≤ 2 ms per frame). Never build one widget per row.

## 3. Changing the project: commands only (I7)

A panel reads the project through the **mirror** (`act.mirror`, `sy.mirror`) and changes it
**only** by emitting commands through the `CommandEmitter` (`act.cmd`). There is no other
path: `test_command_liveness` fails a handler that reaches a project mutator directly.

```rust
pb.on(add, |act, _: &Pressed| {
    act.cmd.emit(EditorCommand::Spawn {
        name: forge_ui::tr!("Light").to_string(),
        parent: None,
    });
});
```

- Several commands that form one user action: `act.cmd.emit_all(label, cmds)` — one
  transaction, one undo entry.
- Session state (selection, the focused panel, notifications) and user config (editor
  settings, keymap) are not project state: `act.session`.

## 4. Following changes without costing anything when idle

The editor draws nothing and never wakes when nothing changes (D-5, `ui_idle_zero_redraw`).
A panel keeps that promise by **reacting** instead of polling:

```rust
let mut seen = pb.mirror().revision();
pb.sync(list, move |sy| {
    let rev = sy.mirror.revision();
    if rev != seen {
        seen = rev;
        // re-read only what changed; `sy.mirror.changes_since(seq)` lists entity changes
    }
    Ok(())
});
```

- A sync step runs on a loop turn where the mirror, the session or the panel's own widgets
  changed — not every frame. Compare a revision and return early.
- Bounded work in slices (a filter over 100k rows): do a slice, call `sy.want_turn()`, and
  stop asking when done. Never `want_turn()` unconditionally — the idle guards catch it.
- Data from another thread or a device: a `LiveFeed` with a rate cap (`max_hz`), which the
  runner stops while the panel is hidden (`ui_live_panel_refresh_bounded`).

## 5. Strings: every one is a localisation key (M2-31)

Every string a user can see goes through `forge_ui::tr!` (a literal key; its English text is
the key) or `forge_ui::trf!` (a template with named placeholders):

```rust
Label::new(forge_ui::tr!("No lights yet."));
Label::new(forge_ui::trf!("{n} lights, {k} selected", n = lights.len(), k = sel.len()));
```

A runtime key (a reflected field's label, a plugin's title) is `forge_ui::l10n::tr_str`.
Under the pseudo-locale (`qps-ploc`) every looked-up string is accented, lengthened and
bracketed; `test_pseudo_locale_no_hardcoded_strings` opens every panel under it and fails on
any visible string that was not looked up. Data (an entity's name, a path, a number) is shown
as it is — never pass it through `tr!`. Text a build or a file carries word for word (a credit, a
`NOTICES` preview) is shown with `Label::verbatim()`, which the audit also leaves alone. An
identifier that is also a stored value (an enum's `name()` written into project settings)
is translated where it is shown, `forge_ui::l10n::tr(kind.name())`, never at its source.

A title a registry carries until a menu, a tab or the palette shows it (a panel's title, an
action's title, a tool's name, a table of labels in a `const`) is written
`forge_ui::tr_key!("Graph editor")`: the literal itself, marked as a key, looked up with
`tr_str` / `tr` where it is shown. Plural forms are separate keys (`"Move {n} entity"`,
`"Move {n} entities"`), never an `s` glued on. An error's own message (its `Display`) is
data: show it as the detail of a `Problem`, whose "what" and next step are keys. Code in
the headless core (`forge_editor::core`) never names `forge_ui`; its reports reach the panel
as data it fills into a looked-up line.

`test_ui_strings_are_keys` reads your plugin's source: a worded string literal that is not a
key fails it, naming the file and line. Data that reads as words (a manifest, a default
motion name the library carries) takes a note on its line or the line above,
`// l10n: <why it is data>`; a whole block of diagnostics (an error type's `Display`),
`// l10n-block: <why>` before it. `FORGE_UI_LOCALE=qps-ploc forge-editor` shows the real
editor under the pseudo-locale.

## 6. Empty states, problems and first-run tips (M2-70)

- **Empty state.** When the panel has nothing to show it shows an `EmptyState` from the
  catalogue — a short explanation and at most one action — never a blank area:

  ```rust
  let empty = pb.b.add(
      pb.parent,
      "empty",
      NodeStyle::leaf(),
      EmptyState::new(forge_ui::tr!("No lights yet.")).action(forge_ui::tr!("Add light")),
  )?;
  pb.on(empty, |act, _: &EmptyStateAction| { /* the same as the Add button */ });
  // in the sync step: sy.ui.set_hidden(empty, !lights.is_empty())
  ```

  A panel that always has content (a settings form) says so with
  `cx.never_empty(tr!("the light rig's settings"))`. `test_panel_empty_states` opens every
  panel on an empty project and fails one that shows neither.
- **Problems.** A warning or an error is a `notify::Problem`: its stable `ErrorCode`, what
  happened, why, and what to do next — `act.session.problem(Problem::coded(...))`, or
  `Problem::from_editor(...)` / `from_rejection(...)`. A refused input is
  `act.session.refuse(what, why)` (`EDITOR-0014`). Information and successes use
  `act.session.notify(Level::Info, …)`.
- **First-run tip.** `cx.first_run_tip(tr!(…))`: one sentence shown the first time the panel
  opens (when the editor has a user config directory). It never takes focus, and once
  dismissed it schedules nothing; the dismissal is user config.

## 7. Accessibility and the keyboard

- Every interactive widget has a role and a name: `Button::new(label)`,
  `IconButton::icon(Icon::Trash, tr!("Delete light"))` (an icon-only button *must* have a
  label), `Container::…labelled(…)` for groups. The editor-wide audit reads the AccessKit
  tree of every panel and fails a focusable widget without a name.
- Tab reaches every control of the panel in visual order; F6 moves between panels. Do not
  declare a `FocusScope` inside a panel unless the group is a composite control (a tab
  strip): an inner scope traps Tab (the keyboard walkthrough's control shows it).
- Give actions keyboard routes: `pb.on_op(widget, "lights.delete", …)` binds a palette/keymap
  action to the panel.

## 8. Icons

`forge_ui::Icon` is the Forge icon set (53 vector icons, MIT OR Apache-2.0): `IconButton::icon`,
`IconView::new(Icon::Warning, tr!("Warning"))`, or `PaintCx::icon` in a custom widget. They are
drawn by the mesh pipeline in a colour token, so they are crisp at every scale and themed.

## 9. Testing a panel

`forge_editor::testing::Rig` runs the real shell headlessly over injected time:

```rust
let mut rig = Rig::new(config).unwrap();       // the shell with your plugin loaded
rig.show_panels(&["studio.lights"]).unwrap();
let frame = rig.panel_frame("studio.lights").unwrap();
rig.h.click(frame.child(&Key::Str("bar".into())).child(&Key::Str("add".into())));
rig.settle();
let (frames, wakeups) = rig.advance(Duration::from_secs(10));
assert_eq!((frames, wakeups), (0, 0), "idle is free");
```

Every guard needs a positive control that fails (W2): build the broken variant (a panel that
polls, a handler that bypasses the bus) and assert the guard catches it. Fault switches that
only a control uses go in a `forge_trace::control_switches!` set behind your crate's
`controls` feature, turned on only from `[dev-dependencies]` (ADR 0046).

The editor-wide audits (`tools/forge-editor-bin/tests/test_editor_panels_audit.rs`) run over
every first-party panel; a third-party panel can run the same checks by loading its plugin
into the same harness.
