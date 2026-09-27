# ADR 0050 — The 2D sample game, its export, and the exported-build guard

Status: accepted (WP-U16, 2026-09-24). Governs DoD M4-12 and M4-13; Ch.35 §35.5, Ch.31 §31.5.

## Context

M4-12: the 2D sample game ships — tilemaps, cutout animation, 2D lights, a gamepad — and is
2D's acceptance test (Spike S13). The work package adds menus on the game-UI runtime
(WP-U11), the E-64 credits entry, play-in-editor and an exported build. WP-U15 had built the
game itself as `tools/forge-2d-platformer`.

## Decisions

1. **`samples/2d-game` (crate `forge-2d-game`) is the sample; the WP-U15 platformer moved
   there** (git history kept). A sample is a product, not a tool: `samples/*` is a workspace
   glob (D-6), claimed by the repo tree, treated as a host by `xtask layering`, and scanned
   by the I1 and I7 guards like `tools/*`. Why (rule 1): a user looking for "the 2D sample"
   finds a game, not a test tool.
2. **The shipped game is the runtime's `GameMenu` over a game view** (`session.rs`): the view
   composites the `Renderer2d` texture through `UiApp::render_external` (the path the editor's
   viewport uses), so the menus, HUD, pause menu, credits (E-64 entry first, static text) and a
   level-clear banner are the same retained `forge-ui` widgets as everything else. Every string
   is a localisation key (en, fr, pseudo). The level runs at a fixed 60 Hz on the UI clock, at
   most 15 steps a turn (a stall drops the backlog); a menu or a pause schedules nothing (D-5).
3. **`UiApp::key_first`** (forge-ui runner): a game needs held keys — presses *and* releases —
   which the UI's routing drops. The hook sees every key before the UI; the sample consumes
   only its movement keys while the level plays. The editor does not override it.
4. **Play-in-editor is a `PlayBackend`** (`pie::SamplePlay`): the editor's own play controls
   (the `forge.play.control` bus command, from the real panel) run the sample's level; it is
   handed the mirror read-only and never a command sink (I7), and the level is the sample's
   generated level, not project entities. `forge-editor --play-2d-sample` installs it; while it
   plays, the viewports draw the level with `Renderer2d` and the movement keys go to it.
5. **`forge_project::packager::CargoPackager`** is the real exporter for a game built from
   source: `cargo rustc --release --locked --offline` for the **host** target (cross-target
   export stays the HAL exporters', M8), stripped (`profile.release.strip = "symbols"`: a
   shipped binary carries no debug symbols — smaller download, rule 1), the linker's map
   (`/MAP` on MSVC, `-Wl,-Map` on Linux) kept beside the package, and the package folder:
   the executable named after the product, `forge.json`, `NOTICES`, `credits.txt`, the project's
   data. The executable metadata (VERSIONINFO / `.note.forge`) is still M5-27's; the report lists
   it as unbuilt.

<!-- -->

7. **Corpus row `2d/sample-game/run`** appended to `tests/determinism/golden.txt`: the shipped
   game's scripted trajectory (state folded every 30 steps). Reason line for the golden change:
   *a new row for the M4-12 sample game; no existing row changed.*

## Measured (2026-09-24)

- Exported binary: **13.3 MiB** on windows-x86_64 (stripped, no LTO; 13.98 MB); **18.2 MiB** on
  ubuntu-x86_64 (23.7 MiB before stripping; `docs/evidence/linux/C-2d-sample-linux-leg.md`).
- Headless startup to the first rendered 1280x720 frame (`--probe`, exported binary):
  0.74–2.1 s on the RTX 3080, of which the GPU device is 0.73–2.1 s (it varies with the other
  lane's GPU load), the level and the menus 1.6–2.9 ms, the first frame 11–17 ms; on lavapipe
  0.81 s (device 0.47 s, first frame 0.34 s). Windowed, first interactive frame 1.43 s (debug build).
- The scripted run: fingerprint `0xb051597b3a7423b3` over 900 steps; the level clears at step 695
  with `0xc366f9b68e66f6ac` — identical on windows-x86_64 and ubuntu-x86_64.
- The export guard costs one release build the first time (~3 min here), then relinks only.

## Not done here

- A budget number for the 2D binary size (`C-2d-preset-binary-size` stays UNBUILT: measured and
  recorded, but the plan names no number and the perf gate makes no release builds).
- The executable metadata (M5-27), a device gamepad (M5-8), the editor viewport drawing a 2D
  project outside play (`C-editor-viewport-2d`), GPU device creation time (0.5–2 s, dominated by
  the adapter pool — outside this WP).
