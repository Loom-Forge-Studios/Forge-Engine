# ADR 0001 — Build the editor on the retained `forge-ui` layer from the first commit

- **Status:** accepted
- **Date:** 2026-09-20

This ADR changes a ratified resolution (O-6), so it is a **plan amendment**. The amendment
is recorded beside O-6 in `decisions.md` and carried out in Ch.21.

## Context

O-6 resolved "`egui` permanent or stopgap?" as **stopgap, M2–M4**. The editor would ship
on `egui` first, a retained layer (`taffy`, `cosmic-text`, `AccessKit`) would arrive at
M5–M7, and the editor would then move onto it. O-6 said the migration cost was "real and
budgeted, not discovered".

Three facts make that plan worse than building the retained layer first:

1. **The migration is a rewrite of every panel.** An immediate-mode panel is a function
   that runs every frame and reads state as it goes. A retained panel is a tree built once
   and then updated by bindings. They are different programs, not the same program on
   another backend. By M5 the editor has twenty-plus panels (Ch.21 §21.21), each with
   tests, so the budgeted migration comes to roughly twenty panels written twice.
2. **Immediate mode redraws every frame the window is live.** `egui` can sleep when
   nothing happens, but it finds out whether anything changed by running the whole UI
   again, and anything animating (a spinner, a caret, a hover fade) keeps the full UI
   running at the display rate. An editor is open for most of a working day and idle for
   most of that time. Using CPU and GPU while idle costs laptop battery and fan noise, and
   takes frame time from the viewport and the bake farm running on the same machine
   (Ch.26).
3. **Ch.21 needs one widget layer for the editor *and* for games.** Shipped game UI needs
   real text shaping (complex scripts, bidirectional text, IME), accessibility, and full
   control over styling. O-6 itself lists these as the reasons `egui` is wrong for games.
   Building them later means two widget layers exist from M2 to M7, and the editor in that
   period does not show what the engine's UI can do.

The orchestrator decided D-1 ("no egui stopgap") and gave WP-U0 the job of recording it
and expanding Ch.21.

## Decision

1. **There is one UI layer, `forge-ui`, and it is retained.** It is built on `taffy`
   (layout), `cosmic-text` (shaping and layout of text, with `swash` rasterisation),
   `AccessKit` (accessibility), `wgpu` (rendering) and `winit` (windowing and input). The
   editor (`forge-editor`) and shipped games (`forge-runtime`) both use it. `egui` is not
   a dependency of any crate, at any milestone.
2. **The editor's first commit is a `forge-ui` program.** No panel is written twice. The
   order of work is WP-U1 (core) → WP-U2 (widgets) → WP-U3 (docking) → WP-U4 (shell) →
   panels (WP-U5..U11), as listed in Ch.21 §21.24, which mirrors the orchestrator
   backlog. Panels no backlog WP names yet are listed there as `UNSCHEDULED` gaps with
   the backlog change each needs.
3. **Idle means zero redraws.** The event loop blocks (`ControlFlow::Wait`), and a frame is
   drawn only when there is damage, a running animation, or a timer that is due. The
   caret stops blinking 5 s after the last input. This is a named test with a positive
   control (D-5, Ch.21 §21.22).
4. **`forge-ui` has its own thin `UiRenderer` trait** (D-3). Only its `render_wgpu` module
   touches `wgpu`. When `forge-gpu` lands, that module moves onto `forge-gpu`'s device and
   pool, and nothing above it changes. `forge-ui` does not wait for the render spine.
5. **`forge-ui` knows nothing about commands.** Widgets raise typed UI actions. The
   editor shell turns the actions that change project state into `forge-cmd` commands,
   and project state reaches panels only through a read-only mirror fed by the bus's
   `Applied` stream (I7, Ch.21 §21.18). Games wire the same actions to their own systems.
6. **The whole layer stays `#![forbid(unsafe_code)]`.** `wgpu` surface creation from an
   `Arc<Window>` is safe API, so the renderer needs no exception to Ch.1.5.
7. **Dependencies are permissive only (I13).** The UI font ships as the Apache-2.0
   releases of Roboto and Roboto Mono. OFL-1.1 is not on the Appendix A.5 list, and
   widening the list is the owner's decision, not ours. System fonts cover other scripts
   and emoji at run time and are never redistributed. Fuzzy matching is written in-house,
   because the obvious crate (`nucleo`) is MPL-2.0.

## Why — the owner's two rules

1. **Better for the user:**
   - Real text shaping and IME from day one, so CJK, Arabic and Devanagari names work in
     the hierarchy and in shipped games.
   - Screen-reader support (UIA on Windows, AT-SPI on Linux) from day one, where the O-6
     path left it until M5–M7.
   - A high-contrast theme and a UI scale setting.
   - The editor does not drain a laptop battery while it sits idle.
   - Games get the same toolkit the editor uses, so a user who learns editor tooling can
     also build game UI. This is the property Ch.21 borrows from Godot.
   - Nothing panel users rely on changes at M5, because there is no M5 migration.
2. **Faster, more efficient engine:**
   - Zero idle redraws. Work is proportional to change: damage tracking re-records only
     dirty widgets, the shaping cache re-shapes only changed text, and one glyph atlas plus
     batched draws keep the GPU cost of a full frame small. The atlas stores coverage
     masks in an `R8` plane and only colour glyphs in an RGBA plane, so it costs at most
     32 MB of VRAM instead of the 256 MB a growing RGBA8 atlas would (Ch.21 §21.7).
   - Live-data panels such as the profiler refresh only on a visible change, only while
     shown, at a capped rate, and never because of the UI's own counters, so opening the
     profiler does not keep the editor awake (Ch.21 §21.11).
   - Virtualised 100k-row trees stay within a 2 ms layout+paint budget. Expanding or
     collapsing any subtree costs O(depth · log fanout) index updates, whatever its size
     (Ch.21 §21.12).
   - The budgeted M5–M7 migration is not paid at all, and that engineering time goes to
     the engine.

## Alternatives rejected

- **The O-6 plan (`egui` M2–M4, then migrate).** Rejected for the three reasons in
  *Context*. Its one advantage is that panels appear sooner, but the retained core is
  WP-U1 and WP-U2, which is small next to the editor it serves. The panels built on it
  are then final.
- **`egui` permanently, with a separate retained layer for games only.** This contradicts
  Ch.21's one-layer rule and the Godot property Ch.21 is built on. It means two layers to
  theme, test and make accessible, and it keeps the idle cost.
- **Adopt an existing retained Rust toolkit wholesale (Xilem/Masonry, Iced, Slint,
  Freya/Dioxus).** Each was weighed against three needs:
  - (a) a renderer we control, which Ch.9's adapter pool and D-3 require;
  - (b) exact command routing through `forge-cmd` (I7);
  - (c) being usable inside a shipped game without pulling in an application framework.
  - Slint's royalty-free licence is not one of the A.5 permissive licences.
  - Iced and the Dioxus family bring their own runtime and renderer assumptions.
  - Xilem/Masonry is the closest in spirit, but at the time of writing its APIs are still
    changing and its renderer (Vello) is a second GPU stack next to ours.
  - The pieces those toolkits are built from (`taffy`, `cosmic-text`, `AccessKit`,
    `winit`) are exactly the pieces adopted here. Ch.21 builds only the retained tree,
    bindings, damage tracking and renderer, which are the parts we must control.
- **Immediate-mode API over a retained backend (`egui`-style calls with diffing).** This
  keeps the "run all UI code every frame" cost, which D-5 forbids, and diffing a 100k-row
  tree every frame cannot meet the 2 ms budget without virtualisation, which requires
  retained state anyway.
- **Web technology (a webview editor).** This adds a browser engine to the editor. It
  uses more memory, gives weaker native IME and accessibility integration on Linux, and
  cannot be the game UI layer.

## Consequences

- **O-6 is amended** (recorded in `decisions.md`). The M2 editor is built on `forge-ui`.
  M7's "UI toolkit for shipping games" becomes polish (world-space UI, gamepad-first
  templates), because the toolkit itself exists from M2 (DoD M2-29).
- **The plan now requires, and these checks verify:**
  - `egui` is absent from the resolved graph. Checked by `test_ui_wgpu_confined`, which
    also checks that `wgpu` is named only inside `forge-ui::render_wgpu`.
  - Idle means zero redraws (`ui_idle_zero_redraw`).
  - Game UI links no editor crate (`test_game_ui_links_no_editor`).
  - Every panel is mapped to a chapter, a work package and a DoD id
    (`test_ui_panel_inventory`, bound now).
  - Every D-5 budget is a named test with a positive control (Ch.21 §21.22).
- **This is harder:**
  - WP-U1 must deliver a working retained core (tree, bindings, layout, text, input,
    renderer, accessibility) before the first panel.
  - The editor therefore appears later in calendar terms than an `egui` editor would have.
    The payback is that no panel is rewritten.
  - Retained UI code has to be written with bindings, which is less familiar to authors
    used to immediate mode. Ch.21 §21.5 gives the model, and the `ui_gallery` example
    (WP-U1) is the reference.
- **This is easier:**
  - The split editor (Ch.34) is a transport swap, because panels already read only the
    mirror and write only commands.
  - Accessibility, localisation and theming are properties of every widget from the
    start, not retrofits.
