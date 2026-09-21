# Forge Engine

A game engine and editor for large-scale worlds, written in Rust.

Forge is built around a single idea: **the unit of content is a celestial body, not a
level.** A universe is a seed. Galaxies, systems, planets, terrain, climate and asset
placement all derive from it deterministically, with nothing stored. Bodies are true
scale — an Earth is 6,371 km and stays 6,371 km, with no "scaled down for gameplay" mode
and no global coordinate type to lose precision in.

> **Status: planning.** There is no code in this repository yet. The engineering plan is
> complete and public — see [`docs/plan/`](docs/plan/). Implementation begins at M0.

## What makes it different

- **True-scale planetary bodies.** Every position is `(frame_id, local offset)`; there is
  no global vector type to overflow. A four-shell frustum stack spans 1 cm to 10 trillion
  metres without z-fighting.
- **Universes from a seed.** Closed-form astrophysics with real statistics — stellar mass
  functions, mass–luminosity relations, planet-occurrence rates, Hill-radius stability,
  atmospheric retention, Roche limits. Orbits are a Kepler ephemeris `f(t)`, never an
  integrator, so any moment in time is O(1) and identical on every machine.
- **Physically-grounded climate.** A real energy-balance model, not a lookup table.
  Circulation-cell count falls out of rotation rate; rain shadows land on the correct side
  of mountains; seasons and Milankovitch cycles come free from obliquity and eccentricity.
  Terrain, weather, hydrology, atmosphere rendering and vegetation all read the same
  fields, so the world agrees with itself without an art pass.
- **Three workspace presets** — 2D, 3D and Planetary. Presets set defaults and never gate
  capability; any project can be promoted to any preset. 2D is a true 2D pipeline, not a
  3D scene with an orthographic camera.
- **A per-world volumetric toggle.** Flip it and the world becomes fully mineable and
  buildable, with caves and overhangs, hybrid with ordinary static meshes.
- **Blueprints that compile.** Visual graphs lower to a typed IR and then to Rust — no
  virtual machine, no interpreted-graph performance cliff, no "port it before you ship."
  The same IR serves materials, world generation and procedural placement.
- **Everything is a plugin, including the engine.** No first-party subsystem uses a
  capability a plugin cannot use. Extension points support replace and wrap, not only add.
- **Agent-native.** The editor is a headless core driven by a command bus; the UI is one
  client of it and holds no privileged path. An AI agent is a peer of the UI, not a
  bolted-on plugin, and can run the game headlessly, observe it and assert on the result.
- **Multi-user with sandboxes.** Host the editor on a server, add your team, and each
  developer works in an isolated sandbox layered over the shared baseline. Per-user Live
  or Pull/Push update modes, switchable at any time.
- **Split editor.** Run the core on a workstation and the interface on a laptop. Panels,
  the inspector and text entry stay local at native latency; only the viewport is streamed.
- **Multi-GPU and multi-machine.** Batch work — generation, erosion, bakes, transcodes —
  spreads across every adapter in the machine, including an old second card, and across
  other machines on the LAN.
- **Self-hosted, always.** No account, no relay service, no telemetry, no cloud dependency.

## Targets

**Windows and Linux are co-primary**, and a behavioural difference between them is treated
as a defect rather than a caveat. Web, iOS and Android are staged and best-effort. Console
backends sit behind a platform abstraction layer so licensed porters can implement them
privately. macOS is out of scope.

## Documentation

| Document | What it covers |
|---|---|
| [Master plan](docs/plan/master-plan.md) | 20 binding invariants, system contracts, 37 chapters |
| [Decisions](docs/plan/decisions.md) | What is ratified, what is rejected and why, what is still open |
| [Milestones](docs/plan/milestones.md) | M0–M8, definitions of done, gate rows |

## Licensing

**This project is not yet licensed. All rights reserved.**

The repository is public for visibility while the design is settled. No licence is granted
to use, copy, modify or redistribute this code. A permissive licence (`Apache-2.0 OR MIT`)
will be applied once the project is ready to accept outside use and contribution.

Until then, **pull requests are not being accepted** — without a licence in place there is
no mechanism for a contributor to grant rights in their contribution, and merging one now
would create a problem that is expensive to unwind later. Issues and discussion are
welcome.

The engine will be free and open source. It will never carry royalties, seat fees or a
revenue share.
