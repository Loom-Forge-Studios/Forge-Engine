# ADR 0032 — The project lifecycle is commands the core performs through one store host

- **Status:** accepted
- **Date:** 2026-09-22

(Numbered 0032: `dev` holds up to 0031; D-9.)

## Decision

1. **A new crate, `forge-project`, holds the lifecycle below the UI**: the project's files
   (`forge-project.ron` manifest, `project/settings.ron`, `project/scene.ron` — RON,
   diffable), the load command, semantic history (`summarize`), fast-forward push and pull
   through the `ProjectStore` trait alone, the `Packager` trait with the E-64 attribution
   and a labelled in-memory packager, in-process `memory:<name>` stores, and
   **`ProjectHost`** — the one object that holds the open project's store. The editor's
   core owns exactly one. The I7 guard lists `forge_project::host::ProjectHost` as a
   project-state writer: the core's three items that name it are allow-listed with their
   reason, and a panel naming it fails (`positive_control_a_panel_holding_the_project_host_fails`).
2. **Create, open, clone, save, push, pull and build are session commands**
   (`forge.project.*`, `forge.export.build`): `Invoke`s whose planners validate the
   arguments and change nothing, so the bus audits them with their issuer; then the core
   performs the operation through the host (E-36) and records an **outcome** in a
   `ProjectStatus` every client receives through `BusClient::pump` (a remote client carries
   it the same way). The shell toasts its own outcomes; the panels read the status from the
   mirror.

<!-- -->

4. **Opening replaces the project with one `forge.project.load` command** planned on a
   `DiffBuilder` (clear, then settings and entities parents-first, entity references
   remapped to fresh keys): atomic, streamed to every mirror like any edit; then the undo
   history is cleared and the status's `epoch` bumps so clients re-read it.
5. **The first save is when a remote is offered (O-11)**, never at creation; the remote is
   a project setting (`project.remote`) so a teammate who clones gets it. Any store is a
   remote: a Forge store in a folder (`file:`, a NAS share) works today; Git, S3 and SQL
   URLs are refused at link time with what works instead (their backends are `UNBUILT`).
   Push and pull are fast-forwards with revision ids checked; divergence is refused, not
   merged (Ch.37 §37.4 owns merging). Clone is how a second editor starts from a remote.

<!-- -->

7. **Presets stay the compiled-in manifests** (`presets/`) read through
   `forge_editor::presets`; WP-16 replaces the source (project-folder presets) without
   changing the rules or the panels.
8. **WP-U13 follow-ups (amends ADR 0026 point 3).** A tile paint stroke's step sends only
   the chunks that step dirtied (`Stroke::unsent`; a step replacing a held one keeps its
   chunks), not every chunk touched so far; every other `d2.` key is applied in place
   (maps and layers by exact lookup, a tile set / sheet / rig re-read alone).

## Why (the owner's two rules)

- **Faster:** the lifecycle status crosses the client boundary only when its generation
  changed (an idle editor with all five panels open: 0 frames, 0 wakeups); a load is one
  diff; saves rewrite only changed files and commits are content-addressed; a tile stroke
  step is O(its cells), not O(stroke) or O(map).

## Consequences

- WP-16's Git backend plugs in as another `StoreBackend` and another remote scheme; the
  host, the commands and the panels do not change. `check_remote` then accepts Git URLs.
- The HAL exporters (M8) implement `Packager`; the attribution and the panel stay.
- The asset browser still reads its in-memory catalogue; pointing it at the opened
  project's folder is follow-up work (it needs the asset database over the project store).
- Gate rows: `C-lossy-warnings-present`, `C-tile-stroke-deltas` (bound);
  `C-store-git-remote`, `C-packager-exporters`, `C-project-presets-from-folder` (UNBUILT).
