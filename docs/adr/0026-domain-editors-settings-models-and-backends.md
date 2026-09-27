# ADR 0026 — Domain editors keep their models in project settings and reach unbuilt subsystems through plan traits

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.7 (frozen); Ch.20; Ch.21 §21.11, §21.18, §21.21, §21.22; Ch.28;
  Ch.35 §35.2; I7, I15, I16; DoD M2-66, M2-67, M2-68; D-4, D-5

(Numbered 0026: `dev` holds up to 0025; D-9.)

## Context

WP-U13 builds the 2D editors (tile palette with autotiling, sprite-sheet slicing, the
`Skeleton2D` cutout rig), the audio mixer (buses, sends, meters, spatial preview) and the
game input action map (§21.21). None of their subsystems exists yet: `forge-2d` is M4-11,
`forge-audio` is unscheduled, `forge-play` is M5-8. The plan fixes that every edit is a
command (I7) and names the backend traits (`Scene2d`, `AudioBuses`, `InputActions`) with
in-memory stand-ins (D-4). It leaves open where the edited data lives and what exactly the
traits answer.

## Decision

1. **The models are project settings in their own namespaces** — `d2.`, `audio.`, `input.` —
   one value per field (`audio.bus.music.volume_db = -6.0`), read back by total parsers
   (`forge_editor::domain::{scene2d, audio, input}`) that skip and report a malformed value
   instead of failing. Every edit is `SetSetting`; several fields are one transaction.
2. **A tile map stores terrains, not solved tiles**, in 16 × 16 chunks (one text value
   per chunk). The tile a terrain cell shows is solved when shown, from its 8 neighbours
   and the terrain's blob-47 rules. Painting one cell therefore re-tiles its neighbours
   without writing them; a stroke writes only the chunks it touched.
3. **A drag is one gesture.** A paint stroke, a fader, a send level and a bone-tip
   rotation each open a `Gesture`; a stroke's every step carries all chunks touched so far,
   because a gesture keeps only the newest step of a frame.
4. **The traits answer what only the subsystem can.** `Scene2d`: the autotile solve,
   sheet slicing, forward kinematics, image sizes. `AudioBuses`: the mix the engine
   follows (`set_mix`), test tones (auditions, session state), meters as a live feed
   (§21.11), spatialisation. `InputActions`: devices, their controls and shapes, key and
   mouse captures, and a poll for devices the window does not see. The in-memory
   implementations are real computations (the blob reduction, grid slicing, FK, a
   power-sum mix graph with solo-in-place and equal-power pan, rolloff curves), labelled in
   each panel's footer and listed `UNBUILT` for the real backend in `just gate`.
5. **Live data wakes the editor only when it changed.** The in-memory mixer bumps its
   meter feed only when a meter changes, so a steady tone costs 0 frames after it settles;
   a bind capture polls devices only while it captures.
6. **The canvas is virtualised.** The tile-map canvas visits only the cells in view.
7. **Panels may ask for a first sync turn** (`PanelBuilder::want_turn`, additive): the
   domain panels fill their lists in their sync step and show them at once.

## Why (the owner's two rules)

- **Faster:** a stroke writes a few chunk values, not a map; the canvas cost is a
  screenful whatever the map size (`test_tilemap_canvas_virtual`); meters and captures are
  zero-idle (`test_mixer_meters_idle`, the capture's poll stops with it).

## Consequences

- When `forge-2d`, `forge-audio` and `forge-play` land they implement the traits and read
  the same settings (or migrate them with a versioned command). The panels do not change.
- Very large tile maps are many chunk values; a binary blob backend for chunks is a later
  optimisation behind the same model.
- A stroke step brings the editor's model up to date from the mirror's setting log
  (`ProjectMirror::setting_changes_since`): a changed chunk key is applied in place
  (`Doc2d::apply_setting`), layers are `Arc`-shared with the canvas, so a step costs the
  chunks it wrote on any map size (`test_tile_stroke_bounded`). Other `d2.` changes
  re-read the namespace, which is small without the chunks.
- Gate rows: `C-tilemap-canvas-virtual`, `C-tile-stroke-single-undo`, `C-tile-stroke-bounded`,
  `C-mixer-meters-idle`, `C-domain-editors-a11y` (bound); `C-editors-2d-backend`,
  `C-audio-backend`, `C-input-backend` (UNBUILT, D-4).

**Amended by ADR 0032 (WP-U7):** point 3 — a stroke step now sends only the chunks that step dirtied (a step replacing a held one keeps the held chunks); and every other `d2.` key is applied in place too (maps and layers by exact lookup, a tile set, sheet or rig re-read alone), not by re-reading the namespace.
