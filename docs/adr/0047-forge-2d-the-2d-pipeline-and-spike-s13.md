# ADR 0047 — Build forge-2d as its own render path and its own 2D solver; resolve Spike S13 by the §35.2 list and a shipped platformer

- **Status:** accepted
- **Date:** 2026-09-24
- **Plan references:** Ch.35 (§35.1-35.4, expanded to §35.5), Ch.31 §31.5 (`test_2d_tax_is_zero`), Ch.5 §5.1, Ch.17 (brief), Ch.2 (I1), Ch.3 (I2), Ch.1.5 (no `f32` below the renderer), Ch.29 (perf gate), Appendix B S13; DoD M4-11; decisions E-5, O-7; WP-U13 (the 2D editors), WP-16 (presets as data)

## Context

M4-11: `forge-2d` and the 2D preset — "the Ch.35 §35.2 list, complete, and nothing beyond it
(Spike S13)". S13's risk: 2D sprawls into a second engine; its mitigation: bound it by the list
and one shipped sample game (a small platformer with tilemaps, cutout animation, 2D lights and a
gamepad) as the acceptance test. The plan names `avian2d` for 2D physics (§35.2, §31.2) and
leaves the render path, the shadow technique, the data model and the solver's shape open.

## Decision

1. **`crates/forge-2d` holds the whole list**, one module per item (§35.5 maps them):
   `atlas` (MaxRects packing, padding + extrusion) and `sprite` (the batcher: sort by
   layer/order/kind/texture, one instanced draw per texture run); `tilemap` (16x16 chunks,
   layers, blob-47 autotiling with edge/isolated fallbacks, merged solid rectangles); `spline`
   (centripetal Catmull-Rom, ear-clipped fill, arc-length edge strip, collider segments);
   `light` (point/spot lights, **exact hard shadows as CPU visibility polygons**, normal maps);
   `physics` (the 2D solver, below); `skeleton` (`Skeleton2D`: FK, keyed clips, blending,
   two-bone IK, sprite parts); `anim` (grid slicing, per-frame durations in microseconds,
   loop / once / ping-pong / reverse); `aseprite` (a bounds-checked decoder of the file
   format, composited frames, tags as animations, a writer for fixtures) and `plugin`
   (`forge.2d`: `Importer("aseprite")`, `AssetType("sprite_sheet")`); `nav` (grid A*,
   8-connected without corner cutting, taut paths); `parallax`; `camera` (pixel-perfect:
   reference size, largest whole-number scale, letterbox, snapping); `particles` (counter-based
   randomness: particle `k` is a pure function of `(seed, k)`); `components` (the 2D inspector
   rows, `#[forge_api]`).
2. **Its own render path** (`render`, feature on by default): four retained render-graph
   passes — `2d.sprites` (albedo + normal G-buffer at the reference size), `2d.lights` (every
   light's visibility fan, additive, `N.L` against the normal buffer: **all lights one draw
   call**), `2d.composite`, `2d.upscale` (whole-number scale) — with `f32` made only in
   `render/last_mile.rs` (rule 4 of `test_no_f32_below_render`). Not the 3D renderer with an
   orthographic camera (O-7).
3. **Forge's own deterministic 2D solver instead of `avian2d`.** Rigid bodies (static,
   kinematic, dynamic), convex hulls with a rounding radius (circles, boxes, polygons,
   capsules, segments — one collision routine family), sort-and-sweep broad phase, persistent
   manifolds warm-started by feature id, speculative contacts widened by the pair's closing
   speed (no tunnelling at 2 units a step), friction, restitution, revolute joints, sensors,
   filters, ray casts and box queries, contact events; `f64` with `forge_num::det` only,
   dense id-ordered arrays, fixed iteration counts. Positions are `FramePos2` (new in
   `forge-frames`; `DVec2` new in `forge-num`); a 2D world is one frame deep and refuses any
   other (`TWOD-0002`).
4. **The editor binds to it**: `forge_editor::domain::scene2d::Forge2dScene` replaces the
   D-4 `MemoryScene2d` (autotile, slicing, rig poses from forge-2d; image sizes from PNG /
   Aseprite headers of the project's files, read through the asset catalogue that
   `EditorServices::set_assets` attaches, as the editor binary does; an Aseprite file's size
   is the sheet the importer lays its frames out on, so a slicing drawn in the editor matches
   the imported texture; the binary's catalogue is the D-4 in-memory store until a project
   folder backs it, so only files staged into it this session are read, and any other path
   falls back to 256x256); the editor's `canonical` / `blob47` are
   forge-2d's; `ComponentCatalog::editor()` registers the 2D components under every preset
   (I15) and the inspector offers them first in a project whose `render.path` is `"2d"`; the
   editor's default plugin set loads `forge.2d`, and the 2D preset names it (and
   `physics.backend = "forge.2d"`), with a template whose entities carry the 2D components.
5. **The dev profile optimises forge-2d** (`[profile.dev.package.forge-2d] opt-level = 2`),
   like forge-render and forge-sim: its last mile and solver run every frame.
6. **I1 covers `DVec2`**: a bare `DVec2` in a position-shaped public signature is flagged;
   `Camera2d::center` returns a `FramePos2`; eleven 2D geometry kernels (hull constructors,
   the shadow kernel, `Xform2`, animation keys, `FramePos2` itself) are allow-listed with
   reasons.
7. **S13 resolved**: the pipeline is exactly the §35.2 list, and the acceptance game ships
   (`tools/forge-2d-platformer`). Anything else is post-1.0 (§35.5 "Not in the list").

## Why — the owner's two rules

2. **Faster / more efficient**: the perf frame (1920x1080, ~5,000 sprites, 32 shadowed
   lights) is 7 draw calls and 0.176 ms of GPU time on the RTX 3080, its steady state
   reuses its compiled graph and makes no heap allocation, GPU or CPU (`test_2d_prepare_alloc`); shadows cost one fan per light, not a
   shadow map per light; the solver's dense body array and sorted contact list took a
   600-body step from 2.0 ms to 0.8 ms.

## Alternatives rejected

- **`avian2d`** (the plan's name, E-5 "PROPOSED"): it is a Bevy plugin — it depends on the
  `bevy` crate and runs as `bevy_app` systems — and Ch.5 §5.1 rules the framework out ("Do
  not depend on `bevy`"; ADR 0005, ADR 0038). It would also bring its own `f32`/`f64`
  feature split, a parry2d/nalgebra tree to download on a metered link, and a contact order
  we do not control (Ch.17: "a solver whose island order depends on insertion order is not
  deterministic"). §35.2's requirement — **a 2D solver, not a 3D solver with a frozen axis** —
  is what we keep. E-5 stands for 3D (`forge-phys`, lane A's M4-3).
- **3D renderer with an orthographic camera**: O-7 rejects it (§35.1).
- **Shadow maps per light (1D polar maps)**: a GPU pass per light and a resolution to tune;
  the CPU visibility polygon is exact, deterministic, one draw for all lights, and cheap at
  2D occluder counts (the whole last mile is ~1-2 ms for the perf frame).
- **A random-number state per particle system**: breaks I3 (a replay that spawns one more
  particle diverges forever); the counter-based stream does not.
- **Keeping `MemoryScene2d`**: two autotile rules (editor and game) can drift; the positive
  control shows the guard catches exactly that.

## Consequences

- **Measured (dev box, RTX 3080 / WARP on the same machine, 2026-09-24)**: perf rows
  `render2d.frame.gpu.{sprites, lights, composite, upscale, total}` = 0.071 / 0.066 / 0.022 /
  0.018 / 0.176 ms (RTX 3080), 244 / 13.8 / 6.6 / 6.6 / 271 ms (WARP); `render2d.frame.
  draw_calls` 7 (unbatched control: one per sprite); `phys2d.step` (2,000 bodies) ~2.2 ms;
  `2d.cold_start` ~0.25-0.45 ms; pyramid (10 rows) top drift 0.0044 over 10 s, 0.595 without
  warm starting. Each GPU row catches a 1.5x regression of its own pass on both classes.
- **Linux leg (lavapipe, Docker, ADR 0044)**: the three 2D goldens differ on 0.000 % of pixels
  on lavapipe and the RTX 3080 alike, and the 319-row I2 corpus has the same BLAKE3 on both
  (`docs/evidence/linux/C-2d-linux-leg.md`). The first lavapipe run differed on 4.8 % of the
  `lights` scene: at 2.5 px a texel, texel edges fell on pixel centres and the rasterisers
  rounded the nearest-neighbour lookup differently. The scene now draws at 64 px a unit (4 px
  a texel); the threshold was not widened (W5).
- **Golden changes (plan amendment, one line each)**: `tests/determinism/golden.txt` gains
  ten `2d/*` rows (appended; no existing row changes) — the 2D pipeline joins the I2 corpus.
- **New gate rows**: `C-2d-goldens`, `C-2d-goldens-linux-leg`, `C-2d-render-steady`,
  `C-2d-prepare-no-alloc`, `C-2d-physics`, `C-2d-tax-zero`, `C-2d-sample-game`, `C-2d-f32-render-only`
  (BOUND); `C-editors-2d-backend` is BOUND; `C-editors-2d-tile-pixels`,
  `C-editor-viewport-2d`, `C-2d-preset-binary-size`, `C-2d-gamepad-device` are UNBUILT with
  their reasons.
- **Not built here (follow-ups)**: the editor viewport drawing a 2D project with `Renderer2d`
  and `Camera2d`; the 2D editors' canvases drawing the project's own tile/sheet pixels (their
  asset store is still D-4 in-memory); a real gamepad backend (the HAL's, M5-8); the 2D
  preset's binary-size budget (needs the packager's release build).

## Note (WP-U15 x WP-22 integration, 2026-09-24): `test_2d_tax_is_zero`'s scope

`test_2d_tax_is_zero` checks the 2D preset's *resolved plugin manifest and dependency
closure* — what a 2D project loads and ships — never `tools/forge-editor-bin` itself.
