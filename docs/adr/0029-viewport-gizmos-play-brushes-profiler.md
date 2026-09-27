# ADR 0029 — Build the viewport as GPU-free editor logic plus a `forge-render` host, and reach play, trace and volume through plan traits

- **Status:** accepted
- **Date:** 2026-09-22

(Numbered 0029: renumbered from 0028 at integration, lane A took 0028; D-9).

## Decision

1. **The viewport's logic is GPU-free, in `forge-editor::viewport`**: `EditorCamera` (an
   `f64` `FramePos` + orientation; orbit / fly / pan; dolly and fly speed on a log scale;
   `offset_of` resolves a point into the camera's frame and subtracts in `f64`), the
   gizmos (`gizmo`: translate / rotate / scale handles in local, world or frame space with
   snapping, producing `Transform` `SetProperty` commands), picking and line overlays
   (`scene`: drawables kept incrementally from the mirror's change log), the tool point
   (`tool`) and the input controller (`controller`). Everything projected is
   camera-relative; only pixel coordinates are narrowed.
2. **The real renderer is hosted, not linked into the editor library.** A viewport cell
   requests a frame in `ViewportSurfaces` (camera, size, drawables, selection) when what it
   shows changed and paints a `Primitive::Viewport`. `forge-ui`'s runner gets one additive
   hook, `UiApp::render_external`, called for a window frame that has damage just before
   it is drawn, with `ExternalCx` (the UI's own pool device and texture registration). The
   editor binary's `ViewportRenderHost` renders each dirty request with
   `forge_render::Renderer` through its **public API only** and registers the texture.
   Without a host (headless, tests, no adapter) the same lines — grid, frame axes,
   bounding boxes — are the placeholder view, and the stats overlay says which it is.
3. **Transform scale is uniform.** The built-in `Transform` has one `scale`; every scale
   handle scales uniformly (an axis handle measures the factor along its axis). A per-axis
   scale arrives with a component that has one.
4. **Play controls are session commands on the bus.** The core hands each applied session
   command (`SessionCommand`: issuer, target, arguments and the `seq` of its own `Applied`
   event) to the clients that follow session commands; the shell follows them and runs each,
   in stream order after the events before it, as a `PlayCommand` on its `PlayBackend` (the
   trait WP-13's core implements), which records it with its tick in the session log — the
   stream replay consumes. A headless run accepts them and reports `UNBUILT(no play core
   runs headless until WP-13)`. `MemoryPlay` is the labelled in-memory backend (D-4): it
   forks the mirror's transforms and integrates `motion.velocity` / `motion.spin_dps` in
   closed form at a fixed 60 Hz tick. The viewport's play view carries the play revision in
   its version, so every simulated step reaches the GPU host. The shell wakes the loop at
   the display rate only while a simulation plays.

<!-- -->

6. **`Theme` is defined in `forge-editor`.** Its item is `forge_ui::Theme`, but `forge-ui`
   must not depend on the plugin kernel (`forge-plugin` pulls `forge-cmd`; games link
   `forge-ui` without it, §21.20), so the point's marker lives beside `EditorOverlay` and
   `ViewportTool`, and the seed catalogue's `defined_in` for `forge.ui.theme` says
   `forge-editor`. The id is unchanged. The built-in themes register through the point;
   a plugin theme gets an action and is remembered as `EditorSettings::theme_override`.
7. **`EditorOverlay` items are built like panels** (generic over the build context, the
   editor's `PanelCx`), once, into a pointer-transparent layer over the main window.
   Toasts and the play banner are first-party overlays, so a plugin can replace, chain or
   remove them.

## Why — the owner's two rules

2. **Faster / more efficient**: the editor library stays GPU-free (its tests and panels
   link no renderer); an idle viewport requests no frame, so the host renders nothing and
   the loop sleeps (0 frames, 0 wakeups over 10 s); a stroke frame applies only its new dabs
   and sends only them; the scene follows the mirror's change log (one entity re-read per change);
   bounding boxes are capped nearest-first; the profiler refreshes ≤ 10 Hz and only while
   visible.

## Alternatives rejected

- **`forge-render` as a dependency of `forge-editor`**: every editor test and panel crate
  would link wgpu and the renderer; the library's no-GPU contract (ADR 0018) would go.
- **Play as `SetSetting` / project state**: an undoable "Play" is meaningless, and
  project state is shared with teammates while a play session is one user's sandbox.
- **A brush command per dab**: a gesture holds back and replaces a second command in a
  frame, which would drop dabs; and a stroke of hundreds of dabs would be hundreds of
  history-relevant changes.
- **Re-sending the whole stroke every frame** (this ADR's first version): O(n²) bytes over
  the bus per stroke and a whole-field copy per frame in the field; the guard
  `each_dab_of_a_stroke_crosses_the_bus_once` has that protocol as its positive control.
- **`Theme` under a `forge-ui` feature that pulls `forge-plugin`**: a game build could
  enable it by accident; the id is the contract, not the crate.
- **Egui-style immediate redraw of the viewport** (a render loop): violates D-5; the
  positive control in `test_viewport` is exactly that loop.
