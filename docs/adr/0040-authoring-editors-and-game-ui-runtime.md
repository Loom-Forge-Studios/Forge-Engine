# ADR 0040 — Authoring editors on settings models and the play core; game UI in forge-runtime on forge-ui

- **Status:** accepted
- **Date:** 2026-09-23
- **Plan references:** Ch.6; Ch.19 (animate-any-property, state machine, blend spaces,
  masks); Ch.21 §21.9, §21.20, §21.21, §21.23; Ch.28 (localisation); Ch.38 §38.1, §38.7;
  E-64; I7, I15, I16, I21; DoD M2-29, M2-63, M2-64, M2-65; D-4, D-5

(Numbered 0040: `dev` holds up to 0039; D-9.)

## Context

WP-U11 builds the sequencer and timeline, the animation state machine / blend space /
bone mask editor, the localisation string-table editor with a pseudo-locale preview, and
game UI on `forge-ui` in `forge-runtime` (menus, HUD, a settings screen, gamepad
navigation, the E-64 credits entry). `forge-anim` and `forge-play` do not exist; WP-13's
play core (`forge-sim`) does. The plan fixes I7 (every edit a command), D-4 (named
backend traits with labelled in-memory stand-ins) and §21.20 (no editor crate in the
runtime). It leaves open where the authoring data lives, what "preview" runs on, how game
UI reaches pads, and where localisation lives so the editor and the game agree.

## Decision

1. **Authoring models are project settings**, as the domain editors' are (ADR 0026):
   `seq.` (clips, tracks, keys), `anim.` (machines, parameters, states, blend samples,
   transitions with textual conditions, bone masks), `loc.` (locales, tables, texts, notes,
   room). Every edit is `SetSetting`; a key drag, a state move, a blend-sample drag is one
   gesture; a CSV import is one transaction. Ch.7 stays frozen. Parsers are total.
2. **A property track keys a reflect path on an entity** (`transform.position.local`,
   `light.intensity`, anything the entity has) — animate-any-property. Floats and each
   vector component evaluate with the curve editor's own `Curve::eval` (constant, linear,
   Catmull-Rom Hermite), so the curve drawn is the curve played; integers round; bools,
   text and entity references step. `f64` throughout.
3. **The preview runs on the real play core.** Scrub/play samples the clip, writes the
   values over the *animated entities'* rows of the edit world only, and forks
   `forge_sim::SimWorld` from them; the viewport shows the forked transforms while no play
   session runs. Playing advances in the play core's fixed 60 Hz steps (`tick_of_step`),
   so the sampled times are reproducible whatever the frame rate. The preview is session
   state (`EditorServices::anim_preview`) and never writes the project
   (`C-timeline-preview-sandboxed`). A forge-sim change that plays clips inside a PIE
   session is left to `forge-anim`.
4. **The state machine is evaluated by the model itself** (`MachineSim`): any-state
   transitions first, then the current state's, in id order; triggers are consumed by the
   transition they fire; cross-fades weight the two states' blends. 2D blend weights use
   gradient-band interpolation (Johansen's "freeform cartesian"): exact at samples,
   continuous, no triangulation to keep valid while samples move. The editor's preview is
   the evaluation a game would run.
5. **Localisation lives in `forge-ui` (`forge_ui::l10n`)**: templates with `{name}`
   placeholders, pseudo-localisation (accented Latin-1/Ext-A look-alikes that every bundled
   font draws, ~40 % expansion, brackets, placeholders intact), the runtime `Localiser`
   (current locale → source → a visible `⟦key⟧`), `LocalisedTexts` (labels bound to keys;
   a language switch re-sets only changed signals) and CSV. The editor's tables compile
   to exactly this lookup, so the editor's "in the game" preview is the game's text.
6. **Game UI**: `forge_ui::game` adds resolution-independent scaling (fit / fill /
   integer), safe-area insets, `GamepadNav` and the E-64 `Credits`. Pads map onto the
   keyboard behaviour every catalogue widget already has (d-pad/stick → arrows with
   deadline-driven repeat, South → Enter, East → Escape or Back, Start → pause); a new
   `Ui::game_nav` makes Up/Down move between rows even on a slider, and spatial navigation
   now prefers candidates in the beam (overlapping on the cross axis). Pads come through
   the `GamepadSource` seam; the real HAL backends are `UNBUILT` (D-4, `ScriptedPad` is
   the labelled stand-in). Game events are `bevy_ecs` messages (`Messages<GameEvent>`);
   there is no `forge-cmd` in the game UI path.
7. **`test_game_ui_links_no_editor`** walks `forge-runtime`'s cargo-resolved closure and
   fails on an editor crate by name, by `forge-panels-` prefix and by location (a renamed
   crate under `plugins/forge-panels-*`), naming the dependency path; its controls are
   synthetic graphs and a real-cargo scratch workspace depending on the real `forge-editor`.
8. **I21's registry scan matches code tokens, not text.** Linking `forge-ui` brought
   `serde_json` into the runtime closure; its `from_reader` doc comment shows a
   `TcpStream`. The scan now tokenizes registry sources (`proc-macro2`) and flags an
   identifier, so documentation is not a finding while any code naming a socket type still
   is (unit cases for both; the telemetry-crate control still fails).

## Consequences

- The authoring editors are complete and testable today; `forge-anim`/`forge-play`
  replace `MemoryAnim`/`MemoryStrings` behind the same traits.
- Game UI ships from M2 with the editor's widgets, a11y and zero-idle behaviour; the
  pause menu over a paused game costs nothing.
- Reading a namespace is O(its keys) per change, as for the domain editors; a clip or a
  table of thousands of strings is fine, and the string list is virtualised.

## Alternatives rejected

- A preview that writes the sampled values into the project (undoable): pollutes history
  and the saved scene; a preview is session state like Play.
- Triangulated 2D blending: a Delaunay mesh changes topology as samples move, so weights
  jump; gradient bands do not.
- `gilrs` for pads now: pulls platform input into `forge-ui` ahead of the HAL (Ch.27) and
  `libudev` onto the Linux build; the seam costs nothing and the keyboard path is real.
