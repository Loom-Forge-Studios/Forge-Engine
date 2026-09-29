# Cloud sessions: working on Forge from Claude Code on the web

This file is for **Claude Code cloud sessions** (claude.ai/code) that work on this public
repository. Each session takes one hard, CPU-only work package, builds it on its own branch
here and hands it back through a pull request. The maintainers then bring the change into
the development repository this edition is exported from, run the GPU and Windows tests
there, and publish it back to `main` with the next export.

This repository is an export: nothing is merged into `main` directly (see
`.forge-public-export`). A cloud pull request is a proposal the maintainers carry over by
hand, so write it to be easy to carry over: small, focused commits, and nothing outside the
task's scope.

---

## 1. For the maintainer: starting a cloud task

1. **One-time setup.**
   - Give the Claude GitHub app access to `Loom-Forge-Studios/Forge-Engine`.
   - Create a cloud environment for it:
     - Network access: **Trusted** (the default). It already allows crates.io,
       static.rust-lang.org, GitHub and the Ubuntu apt mirrors.
     - Environment variables:
       `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_INCREMENTAL=0`, `CARGO_TERM_COLOR=never`.
     - Setup script: the one in section 2.
2. **Each task.** Start a session on `Loom-Forge-Studios/Forge-Engine`, branch `main`,
   choose the Opus model, and paste:

   > Read docs/cloud-work.md and do task **C-1**. Follow it exactly.

   Replace C-1 with the next task in section 8 that has no open pull request and whose
   start condition holds. Start with C-1: it unblocks the most later work.
3. **One session, one task.** C-1…C-4 touch different crates, so two sessions may run at
   once if the budget allows.
4. **When a session finishes**, it opens a pull request into `main`. The maintainers pick it
   up from there.

## 2. Environment setup script

Paste this into the cloud environment's setup script. It installs what the project's Linux
verify image has (`tools/docker/linux-verify/Dockerfile`), plus ALSA and udev headers for
the audio and input crates:

```bash
#!/bin/bash
set -e
apt-get update
apt-get install -y --no-install-recommends \
  mesa-vulkan-drivers libvulkan1 vulkan-tools xvfb xauth \
  libxkbcommon0 libxkbcommon-x11-0 libx11-xcb1 libxcursor1 libxrandr2 libxi6 libwayland-client0 \
  libudev-dev libasound2-dev pkg-config \
  fonts-noto-color-emoji fonts-wqy-microhei
rustup toolchain install 1.98.1 --profile minimal -c rustfmt -c clippy
cargo +1.98.1 install --locked cargo-nextest@0.9.145
```

If a test needs a display or a GPU adapter, set up the display and the software renderer
(lavapipe) once per session:

```bash
Xvfb :99 -screen 0 1920x1080x24 & export DISPLAY=:99
export VK_ICD_FILENAMES=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json | head -n1)
```

## 3. What this machine can and cannot do

The cloud machine has 4 vCPU, 16 GB RAM, 30 GB of disk and no GPU. It runs Linux, and a
single command is cut off after 10 minutes. The workspace is far bigger than that: a full
debug build takes 100+ GB of target files, and the full suite (1,700+ tests) runs for well
over an hour even on a large desktop.

- **Never build or test the whole workspace.**
  - Do not run `cargo build --workspace`, `cargo nextest run --workspace` or
    `cargo clippy --workspace`: they fill the disk and overrun the time limit.
  - Work per package: `cargo check -p <crate>` while iterating, `cargo nextest run -p <crate>`
    at solid points, `cargo clippy -p <crate> --all-targets -- -D warnings` before pushing.
  - Also test the packages that directly use what you changed.
- **Watch the disk.** Check it with `df -h .` now and then. If it is tight, run
  `cargo clean -p <big-crate>` for crates you are done with.
- **No real GPU.**
  - lavapipe gives correct but slow Vulkan, so GPU tests can check correctness here.
  - A timing measured here means nothing. Never set, change or "fix" a perf budget or
    baseline from a cloud number, and never widen a tolerance (W5). Record the cloud number
    as a sanity check, and list the row as "measure on the GPU machine" in your handoff.
- **Linux only.** Write Windows code paths carefully; the maintainers run them on Windows.
  Determinism hashes you see here are the Linux side only: record them in the handoff, and
  the maintainers confirm Windows matches.
- **These repo checks are cheap (they build only `xtask`). Run them before you push:**
  - `cargo xtask fmt --check`
  - `cargo xtask premium-boundary`
  - `cargo xtask plan-coverage`
  - `cargo xtask layering`
  - `cargo xtask allocators`
  - `cargo xtask timed-gates`
  - `cargo xtask fp-rules`
  - `cargo xtask gate`
  - `cargo xtask dod`
- **Licence audit.** If you add a third-party crate, check its licence yourself (permissive
  only, see section 4) and list it in the handoff.

## 4. Project rules (binding: the maintainers' review rejects work that breaks them)

**The two decision rules.** Every choice must:
1. make things better for the user, not merely easier to code;
2. make the engine run faster and more efficiently.

When the plan leaves a choice open, decide by these two, record it in a short ADR, and keep
going. Do not stop to ask.

**Base edition.**
- This repository must build on its own.
- Add nothing that needs code outside it, and nothing that names another edition.
- `cargo xtask premium-boundary` must pass.

**Feature parity.** Work packages WP-50…78 bring every major Unity/Godot/Unreal feature into
the engine at the best engine's level.
- Read your M7 row in `docs/plan/milestones.md` and the plan chapter your card names first.
- Build in base crates, with a clean extension point wherever richer behaviour may layer on
  later.
- Measure it: every per-frame path gets a budget row (`tests/perf/budgets.ron` or the
  crate's own), and a counting or allocation guard where it is hot.
- The feature costs nothing when it is unused.

**Guards and tests.**
- **W1/W2:** every guard is a named test with a positive control. Break the property
  temporarily, see the test fail, then restore it.
- **W5:** never widen a tolerance and never add a retry to make a test pass.
- **W9:** something you cannot build is `Unbuilt(reason)` / `AWAITING(reason)`: never
  silence, never a fake green, never a stub presented as done.

**Code.**
- **I7:** every mutation of project state is a `forge-cmd` command on the bus; the UI has no
  privileged path.
- **I1:** no bare `DVec3` position in a public signature (use `FramePos`), and no `f32`
  below `forge-render`.
- `unsafe` is forbidden except in forge-gpu, forge-script and forge-jobs (with `// SAFETY:`).
- No `unwrap` outside tests and `main`, and no panic reachable from a command handler.
- Clock reads in `tests/` go through `forge_trace::timed::run_timed_alone` or carry
  `// timed-gates: exempt(<real reason>)`.
- Determinism: every generated quantity is a pure function of its seed path. No fast-math
  and no `target-cpu=native`.
- Re-blessing a golden needs the recorded reason (Ch.3.4).
- Edition 2024, toolchain 1.98.1. Dependencies are permissive only
  (MIT/Apache-2.0/BSD/Zlib/ISC/Unicode/BSL-1.0); copyleft is never allowed.
- Proprietary SDKs go only behind an extension point, as optional plugins, never in the tree.
- Workspace members are globbed: a new crate is just `crates/<name>/Cargo.toml`. Every new
  directory must be claimed in the repo tree in `docs/plan/master-plan.md`
  (`cargo xtask plan-coverage`).

**Editor UI.** Every UI string is a localisation key. Panels follow the §21 panel rules:
accessibility, empty states, first-run tips.

**Records.**
- Expand the governing plan chapter in `master-plan.md` from BRIEF to FULL for what you
  build; edit only your chapter's section.
- Add gate rows in `tests/gates.ron`.
- Set the milestone row in `docs/plan/dod-status.ron`: `Settled` with evidence only when the
  whole row is done; otherwise leave it `Unread` and say what remains.
- Defects go only in `docs/defects.md` and error codes only in `docs/error-codes.md`. Ids
  are identities: never renumber, only append.
- ADR: next free number in `docs/adr/`. The maintainers renumber on a collision (numbers
  used only by the other edition are skipped in this one).

**Cost discipline.** Guards protect engine properties: determinism, I7, perf, safety. Don't
build tests around planning prose or tables; a plain doc edit is enough there.

## 5. Git and delivery

1. **Start.** `git fetch origin main && git checkout -B cloud/<WP> origin/main`, for example
   `cloud/WP-60`.
2. **Commit and push at every solid point:** `git push -u origin cloud/<WP>`.
   - The session can end at any time; pushed work survives.
   - Commit messages look like `WP-60: forge-phys: rigid bodies, colliders and the fixed
     step`.
3. **Only ever push your own `cloud/<WP>` branch.**
   - Never push to `main`, never force-push, never merge a pull request.
   - Never touch another session's branch.
4. **Rebase before you open the pull request.** If `main` moved, run
   `git fetch origin main && git rebase origin/main`. On a `Cargo.lock` conflict, take
   main's version, then run `cargo metadata --format-version 1 > /dev/null` to add your
   dependencies back.
5. **Open ONE pull request into `main`** when the work package is done, or when you are
   running out of budget.
   - Title: `WP-60 (cloud): <title>`. Body: your handoff, see section 6.
   - CI runs on GitHub-hosted runners. Two perf-gate tests fail on hosted Windows today, a
     known and tracked machine-class issue: `the_named_budgets_hold` and
     `positive_control_a_1_5x_2d_pass_fails_its_own_row` in `tools/forge-perf-gate`. Ignore
     only those two.
   - Push fixes to an open PR in one batch, not one by one.
6. **The maintainers take it from there.** They carry the change into the development
   repository, run the full Windows and Linux suites and the GPU perf rows, fix what only the
   GPU shows, and export it back to `main`. The pull request is then closed; it is never
   merged here.

## 6. The handoff file

Keep `docs/local-notes/cloud-<WP>.md` on your branch and update it at every push. Paste it
into the PR body when you open the PR. It says:

- **Done:** crates, modules, tests, and each guard with its positive control.
- **Remaining:** precisely what in the scope is not done.
- **Verify on the GPU machine:** GPU/Windows-only tests, perf rows with the cloud's sanity
  numbers, determinism hashes to confirm on Windows, anything drawn in the viewport.
- **New dependencies:** each with its licence.
- **Decisions:** each with its ADR number.

## 7. Saving tokens

Tokens are the budget: the maintainers pay for every one.

**Scope.**
- **One session, one task.** Don't start a second work package in the same session: every
  turn re-reads the whole conversation.
- **Author directly.** No helper sessions, no workflows, no parallel fan-out: each helper
  re-reads the repository from nothing.

**Reading.**
- **Never read the big plan files whole.** `docs/plan/master-plan.md` is ~380 KB (~95k
  tokens). Find your chapter with `grep -n '^# Chapter\|^## ' docs/plan/master-plan.md`,
  then read only its line range. Treat `milestones.md` the same: grep for your row.
- **Search before reading.** Grep for the exact type or function name. Read the part of a
  file you need (offset/limit), not the whole file.
- **Don't re-read a file you just edited,** and don't re-run a green test without a change.

**Command output.** Keep it small; it all lands in the context.
- `cargo check -p <crate> --message-format short 2>&1 | tail -n 40`
- `cargo nextest run -p <crate> --status-level fail --final-status-level fail
  --failure-output final 2>&1 | tail -n 60`
- Grep long logs; never `cat` them whole. `CARGO_TERM_COLOR=never` keeps colour codes out.

**Working rhythm.**
- **Fix compile errors in batches:** read all the short-format errors once, fix them all,
  rebuild.
- **Iterate with `cargo check`, not full test runs.** Test at solid points; run clippy once
  per push.
- **Write docs once,** when a piece is finished: the plan chapter, ADR and gate rows at the
  end, not rewritten after every change.
- **Before a long context gets compacted,** update the handoff and push, so nothing lives
  only in memory.
- **Stop cleanly.** When the budget runs low: commit, push, finish the handoff, open the
  pull request. A clean, tested half is worth more than a broken whole.

## 8. Tasks

Each card is the work package's full scope from the maintainers' backlog, plus notes for
this machine. These packages are reserved for cloud sessions; nobody else takes them while
they are listed here.

### C-1 · WP-60 · Parity M4-3 + M7-9 — forge-phys: 3D physics for both editions

- **Branch:** `cloud/WP-60`.
- **Start condition:** none; it can start now.
- Why cloud: the physics core is CPU work, and it unblocks WP-75, WP-61, WP-28, WP-58 and WP-67.
- The workspace pins bevy_ecs and bevy_reflect at exactly =0.19.1 (ADR 0005). Pick an avian3d release that works with that pin; never bump the Bevy pin here (that is a deliberate, measured event). If no avian release fits, make rapier the first backend behind forge.phys.backend, keep the shared behaviour suite backend-generic, and record the avian finding in the ADR and the handoff.
- The editor Play mode running physics: wire it and test it headless. Viewport and debug drawing are checked on the GPU machine.
- Determinism: record the Linux state hash in the handoff. The maintainers confirm Windows is identical.
- Budget (2k bodies at 60 Hz): build the budget row and its test. The number is set on the GPU machine; give your cloud sanity number in the handoff.

**Scope**

Milestone M4-3 plus owner parity row M7-9 (first half). Create crates/forge-phys (base): avian 3D (generic over f32/f64 per E-5) behind the reserved forge.phys.backend extension point, with rapier as a selectable alternative backend (user-selectable per project); deterministic fixed step; rigid bodies; colliders — primitives, convex hulls (auto-generated from meshes), concave triangle meshes, heightfields; physics materials; collision layers/masks; triggers; CCD; render interpolation between fixed steps; raycast/shape/overlap queries including batched and async queries; the joint set fixed, hinge, spring, slider, cone-twist, 6DOF; gravity and damping zones; the editor Play mode runs it. A determinism check runs here: same inputs give the same state hash on Windows and Linux, with the tolerance policy of Ch.3.

**Acceptance**

forge-phys with both backends passing one shared behaviour suite; determinism hash identical Windows/Linux; query/joint/zone tests with controls; budgets (e.g. 2k bodies at 60 Hz); M4-3 settled; all green.

### C-2 · WP-68 · Parity M5-8 + M7-13 — forge-play: the gameplay framework

- **Branch:** `cloud/WP-68`.
- **Start only when `crates/forge-input` exists on main:** `git ls-tree origin/main crates/forge-input` must print a line. If it prints nothing, pick another card.
- Why cloud: the gameplay framework is CPU work, and it unblocks WP-69, WP-76 and WP-77.
- It builds on WP-65, the input runtime (crates/forge-input, wired into forge-runtime and the editor). Read it before you start; do not redo input.
- The Play-mode core lives in forge-runtime and forge-sim. crates/forge-play is new and builds on them.
- The cvar console in Play mode and the editor console binding: build them through the command bus and test them headless. Panel visuals are checked on the GPU machine.

**Scope**

Milestone M5-8 plus owner parity row M7-13 (all but the ability system). Create crates/forge-play (base): the game mode / game state / player controller / pawn pattern (ECS-native, documented), global services (subsystems with lifetimes: game, level, player), a save system (a SaveGame object via reflection, versioned, slots, async), hierarchical gameplay tags (registry, queries, containers), tweens and timers, coroutines and latent actions (async tasks on the scheduler, usable from blueprints), runtime console variables and console commands (typed, discoverable, bound to the editor console too), data tables and curve tables with CSV import/export, a runtime expression evaluator, game-feature plugins (content + code bundles enabled at runtime), and a gameplay-facing job API on the scheduler.

**Acceptance**

Each framework piece tested with controls; save round-trip across a version bump; cvar console usable in Play mode; data-table CSV round-trip; all green.

### C-3 · WP-66 · Parity M7-11a — forge-audio: the audio engine

- **Branch:** `cloud/WP-66`.
- **Start condition:** none; it can start now.
- Why cloud: DSP, mixing and scheduling are CPU work.
- There is no sound card here. Build the device layer on cpal (the setup script installs the ALSA headers), but test through an offline or null output.
- These tests run here: the offline-render reference test, the no-allocation audio-thread guard (a counting allocator in the test, with its control), and the concurrency and scheduling tests.
- Real device playback and the per-voice CPU budget number are verified on the GPU machine.
- Keep the existing mixer authoring (M2-67) working. MemoryAudio stays as the labelled in-memory backend for tests.

**Scope**

Owner parity row M7-11, first half. Today the mixer (M2-67) is authoring over MemoryAudio (gate C-audio-backend UNBUILT). Create crates/forge-audio (base): device output through a permissive crate (e.g. cpal), a real-time-safe mixer thread (no allocation or locks on the audio thread — guard), the authored bus graph, 2D/3D sources with the existing attenuation models, Doppler, the effect set (reverb, EQ, compressor, delay, chorus, distortion, filters, pitch, limiter), reverb zones, random/sequence containers, mixer snapshots with transitions, spectrum analysis, microphone input, voice concurrency rules (limits, stealing, priority), sample-accurate scheduling. Streaming for long files.

**Acceptance**

C-audio-backend bound; audio-thread no-alloc guard with control; offline render of a test scene matches a reference within a bound; concurrency and scheduling tests; CPU budget per voice; all green.

### C-4 · WP-72 · Parity M7-20 — the asset pipeline: FBX, OBJ, USD, Alembic, .blend, CAD, shared cache

- **Branch:** `cloud/WP-72`.
- **Start condition:** none; it can start now.
- Why cloud: importers are CPU and parsing work.
- FBX goes through ufbx (MIT; C code built by the cc crate, and gcc is present). Never the Autodesk SDK.
- CAD/STEP uses a permissive kernel only (for example truck, Apache-2.0). OpenCascade is LGPL: never.
- USD and Alembic use permissive readers only. Where none is adequate, implement the needed subset (with the reason in the ADR) or mark it Unbuilt(reason).
- .blend: do not install Blender here (it takes over 1 GB of the 30 GB disk). Build the headless-export call and the "Blender not found" message, and test both with a fake `blender` script on PATH. The real round-trip is checked on the GPU machine.
- Sample files under tests/assets are small: generated by the test, or permissively licensed with the licence recorded beside them.
- Import throughput is recorded on the GPU machine. Give a cloud sanity number in the handoff.

**Scope**

Owner parity row M7-20. Importers on the Importer extension point: FBX through ufbx (MIT, never the Autodesk SDK) with skeletons, skin weights, blend shapes and animation; OBJ/MTL; USD (a permissive reader; layers and variants as far as the scene model allows); Alembic caches; .blend through the user's installed Blender (headless export, the Godot approach; clear message when Blender is absent); CAD/STEP through a permissive kernel (tessellation settings); and a shared import cache server (self-hosted, content-addressed artefacts keyed by source hash + importer version, so a team imports once).

**Acceptance**

Round-trip/import tests per format with sample files under tests/assets; shared cache hit avoids re-import (control); import throughput recorded; all green.

### C-5 · WP-75 · Parity M4-6 + M7-17a — forge-nav: navigation

- **Branch:** `cloud/WP-75`.
- **Start only when `crates/forge-phys` exists on main:** `git ls-tree origin/main crates/forge-phys` must print a line. If it prints nothing, pick another card.
- The navmesh build, paths, tile rebake, avoidance and budgets run here. The navmesh debug overlay's drawing is checked on the GPU machine.
- Recast-class: a pure-Rust implementation, or a binding to Recast/Detour (Zlib, allowed). No copyleft ports.

**Scope**

Milestone M4-6 plus owner parity row M7-17 (navigation half). Create crates/forge-nav (base): navmesh generation from collision/scene geometry (Recast-class; a permissive implementation or binding), runtime rebaking by tiles, navigation profiles (radius/height/step/slope), off-mesh links, dynamic obstacles (carving), area types with costs, path smoothing, ORCA avoidance, 3D grid A* for grid games, and a navmesh debug overlay.

**Acceptance**

Path tests on reference levels with controls; tile rebake bounded time; avoidance never interpenetrates in a crowd test; budgets; all green.

### C-6 · WP-77 · Parity M4-7 + M7-15a — forge-net: replication

- **Branch:** `cloud/WP-77`.
- **Start only when `crates/forge-play` exists on main:** `git ls-tree origin/main crates/forge-play` must print a line. If it prints nothing, pick another card.
- QUIC and UDP work over 127.0.0.1 here. The network simulator makes latency, jitter and loss deterministic in tests.
- The dedicated-server export must build and run headless here.
- Multi-instance play-in-editor: build it and test it headless. The editor windows are checked on the GPU machine.

**Scope**

Milestone M4-7 plus owner parity row M7-15, first half. Create crates/forge-net (base): transports (QUIC via the existing quinn stack, raw UDP), connection management, replication of ECS components with authority (invariant I6 — the tests/net/test_authority_uniqueness.rs guard), reliable and unreliable RPCs, a dedicated-server export target (a stripped server build of the runtime), a network simulator (latency, jitter, loss) and real data in the profiler replication slot (closes C-profiler-replication), and multi-instance play-in-editor (N clients + a server from the editor).

**Acceptance**

I6 bound; replication/RPC tests over the simulator with controls; dedicated server exports and runs headless; multi-instance PIE works; bandwidth recorded; all green.

### C-7 · WP-69 · Parity M7-13b — the ability system

- **Branch:** `cloud/WP-69`.
- **Start only when `crates/forge-play` exists on main:** `git ls-tree origin/main crates/forge-play` must print a line. If it prints nothing, pick another card.
- All CPU work: fully testable here.

**Scope**

Owner parity row M7-13, ability-system part (UE GAS class). Build in forge-play: attributes and attribute sets, gameplay effects (instant, duration, periodic; stacking; modifiers), abilities with costs/cooldowns/tags, activation blocking by tags, and hooks for network prediction (used by WP-78).

**Acceptance**

Effect stacking/modifier math tested with controls; tag blocking; deterministic; all green.

### C-8 · WP-61 · Parity M7-9b + M7-10 — vehicles, cloth, character movement

- **Branch:** `cloud/WP-61`.
- **Start only when `crates/forge-phys` exists on main:** `git ls-tree origin/main crates/forge-phys` must print a line. If it prints nothing, pick another card.
- Vehicles, the character controller and movement modes are CPU physics: fully testable here.
- Cloth: build the simulation and its tests here. Its drawing and the painted-constraint editor view are checked on the GPU machine.
- Swimming uses the water query only when present (WP-55 builds it): leave the extension point, with a test double.

**Scope**

Owner parity rows M7-9 (second half) and M7-10. Build on forge-phys: vehicles with suspension, tyre model, engine torque curve, transmission and differential; cloth with authoring (painted constraints in the editor, collision with bodies); a kinematic character controller with platform-velocity inheritance and rich floor/wall/slide/step handling; movement modes walk, fall, swim (uses the WP-55 water query when present), fly, crouch, jump; projectile, rotating and interpolated movement components. Networked prediction is WP-78.

**Acceptance**

Vehicle, cloth and controller behaviour tests with controls; movement-mode transitions deterministic; budgets; all green.
