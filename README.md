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

**Forge Engine is commercial and source-available. It is not open source.**

The source is public and readable. Reading it is free; using it is not.

| Licence | Price | What it covers |
|---|---|---|
| **Individual** | **$5 once, forever** | One person, every version, ships commercial products. Does not include the team/collaboration features. |
| **Team** — project under $100K gross | **$25 / year** | **4 seats**: the purchaser plus 3 included. |
| **Team** — project at or above $100K | **$40 / year** | Whichever rate matches this year's revenue. |
| **Additional seat** | **$5 / year** each | Same terms as an included seat. |
| **Marketplace** | **5%** | Commission on sales through the Forge Index. **0% everywhere else.** |

Individual is a one-time purchase. Team is an annual subscription for the whole team — not
per seat, and not per month.

### There are no royalties. On anything. Ever.

Not on games, not on applications, not on plugins. Not at any revenue, not at any scale.
**You pay once and keep 100% of what you make, forever.**

A solo product grossing $2M pays Unreal $50,000 in royalties. It pays Forge $5, once.
A four-person team through three years of development pays about **$105 in total** — for
everyone.

Team revenue is **self-declared** — there is no audit, no reporting obligation and no
instrumentation of any kind. The included and additional team seats work on projects owned
by that team; someone who also wants their own unrelated project buys a $5 Individual
licence.

### What you may do

Build and ship games and applications, commercially or not. Modify the source for your own
use. Create and distribute plugins and assets through any channel you like. Ship
`forge-runtime` **in binary form**, embedded in your product.

### Crediting the engine

Products made with Forge Engine must credit it: a "Made with Forge Engine" line in the
product's credits or about screen, and on its store page or documentation. The build tools
also write the engine name and version into the executable's metadata automatically.

### What you may not do

Redistribute engine source, sublicense, distribute the editor itself, or build a competing
engine from the source.

Until the licence agreement is published, **no rights are granted** — the repository is
public for evaluation and nothing more.

### Commitments that come with paying for software

- **A purchased licence is perpetual and irrevocable for the versions it covers.** Pricing
  and terms changes apply only to versions released afterwards. **Nothing is ever changed
  retroactively on software you have already shipped on.**
- **If a team subscription lapses, the editor degrades — it never locks you out.** You keep
  the editor, you keep every project, you keep building and shipping. Only the
  collaboration features go quiet until you renew. Your work is never held hostage.
- **A lapsed or cancelled licence never affects a game you have already shipped.** There is
  no licensing code in the runtime, and a build gate makes sure there never will be.
- **Your projects are never held hostage, because we never hold them.** Forge operates no
  project hosting — the team server runs on your hardware, so there is nothing for us to
  withhold if you stop paying.
- **No DRM. No telemetry. No runtime licence check.** Activation happens once, against a
  signed file on your disk. The editor and build tools work offline indefinitely.
- **Nothing is added to your shipped product** beyond the runtime you licensed and a
  visible "Made with Forge Engine" credit. The credit is plain text and file metadata — it
  connects to nothing, reports nothing and runs nothing. The licensing code is not in the
  runtime's dependency graph and cannot be; a build gate enforces it.

### Contributing

**Pull requests are not being accepted.** Without a contributor licence agreement in
place there is no mechanism for a contributor to grant the rights this project needs, and
merging one now would create a problem that is expensive to unwind. A CLA will accompany
the licence agreement.

Issues and discussion are welcome.
