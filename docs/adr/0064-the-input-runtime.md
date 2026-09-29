# ADR 0064 — The input runtime (`forge-input`)

- **Status:** accepted (WP-65)
- **Date:** 2026-09-28
- **Plan references:** Ch.28 §28.9–§28.18 (this ADR's chapter), Ch.27 (HAL), Ch.21 §21.20 (game
  UI), §21.21 (input map, input debugger); DoD M2-68, M7-12; E-49 (permissive only), E-67
  (parity in both editions); D-4, D-5; owner rules 1 and 2.

## Context

Until WP-65 the game's input was authoring only: the input-map panel (M2-68) wrote contexts,
actions and default bindings as project settings against a labelled in-memory `MemoryInput`
(gate `C-input-backend` UNBUILT), and nothing played them. Parity row M7-12 asks for the input
runtime at the level of the best engine (Unreal's Enhanced Input, Unity's Input System, Godot's
InputMap): real devices, deadzones, response curves, hold/tap/multi-tap/combo/chord triggers,
runtime rebinding with a game-facing UI and persistence, local-multiplayer device assignment,
touch gestures and gyro, haptics, on-screen controls, platform glyphs, and an input debugger —
in both editions, measured.

## Decision

1. **Its own crate, `crates/forge-input`, not `forge-play`.** The shipping runtime, the editor
   (the input map's capture, the debugger) and headless tests all need it; it depends only on
   `forge-trace` (fault switches, the `input.update` zone), `serde` and `ron`, with the
   winit and gilrs adapters as features. `forge-play` (abilities, save, localisation) would
   drag gameplay into the editor's input capture. Owner rule 2: nothing links what it does
   not use.
2. **One vocabulary.** The editor's `Device`/`Control`/`Binding` types move into
   `forge_input::control` and the editor re-exports them; the runtime reads the very settings
   the panel writes (`InputMapDef::from_settings` over `input.map.*`). New keys:
   `.priority`, `.consume`, `.action.<a>.modifiers`, `.triggers`, `.bindmod.<n>`; the panel
   edits them through the runtime's own parsers, so what is authored is exactly what plays.
3. **Compiled, index-addressed evaluation.** The map compiles into flat tables (controls by
   index into per-class tables, triggers resolved to action indices, dependencies ordered so
   a chord or combo reads this frame's result). A steady frame allocates nothing and
   compares no string; an unused runtime returns after one branch.
4. **Semantics follow Enhanced Input where it is the best of the three**: explicit triggers
   combine as *any*, implicit (chords) as *all*; phase events Started / Triggered / Completed /
   Canceled; per-context priority with consumption by higher-priority contexts only. Where the
   three disagree we pick for the player: a **multi-tap fires on the completing press**, not a
   later release; a **press and release inside one frame still register** (per-control edge
   bits), so a fast tap is never lost; an event pushed before an update is visible in that
   update.
5. **Devices.** Gamepads through **gilrs** (MIT OR Apache-2.0; Windows.Gaming.Input, evdev +
   udev on Linux) with its default filters off (our deadzones apply to raw values); pads keep
   their player across a reconnect by UUID. Keys bind by **physical** position, labelled with the
   layout's own character (learnt from winit's key text); mouse aim uses raw device motion.
   Linux needs `libudev-dev` at build time (added to the Linux verify image and CI).
6. **Rebinding** is interactive in the runtime (listen for the next control that fits the
   action; Escape/Select cancel; conflicts swap by default and are named; composites rebind per
   direction; the completing press does not also fire). Overrides persist **per profile, only
   what the player changed**, written atomically; a game update that changes an untouched
   default reaches the player. `forge-runtime`'s `ControlsScreen` is the game-facing screen: it
   saves on every change.
7. **Local multiplayer**: `Single` (one player owns every device), `AutoJoin` (a button on a
   free device seats the next player; keyboard and mice are one seat; optional join button;
   player cap) and `Manual`; contexts are enabled per player.
8. **Touch** gestures become Touch-class controls (bindable like any button or axis), and
   **on-screen controls drive a virtual gamepad**, so pad bindings work by touch with nothing
   extra to author. **Gyro** processing (continuous calibration, player space, tightening)
   produces a `Gamepad/Gyro` delta; samples enter through the `MotionSource` seam. A platform
   HID backend for DualShock 4 / DualSense / Switch Pro motion reports is not built
   (`C-input-gyro-device` UNBUILT): gilrs exposes no motion.
9. **Haptics**: two motors per pad, effects with an envelope mixed per motor, and only changed
   levels sent; every rumble ends with a zero command.
10. **The input debugger** (`forge.input_debugger`) reads a snapshot of the live runtime.
    Opening it reads no device; Refresh reads once; Live refreshes 20 times a second only while
    on and visible — with Live off the editor sleeps (D-5).
11. **No premium seam is needed.** Input is identical in both editions and names nothing
    premium; a premium system that wants input binds actions like any game code.

## Consequences

- `C-input-backend` is bound; fifteen new gate rows guard the runtime, one (`C-input-gyro-device`)
  is UNBUILT with its reason. DoD M7-12 stays open until motion reaches the runtime from a real
  pad and a game loop in `forge-runtime` drives the adapters every frame (the runtime binary
  has no game loop yet; M7-13).
- The editor's default services use the real backend (`DeviceInput`); `MemoryInput` stays as
  the labelled test double.
- Adding a device class or a control is a table edit in `forge_input::control`; the editor's
  lists follow.
