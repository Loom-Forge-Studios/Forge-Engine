# C-scene-* - Linux leg (WP-U20, scene composition, DoD M5-9)

- **Date:** 2026-09-27
- **Commit:** 55b5ac5 (lane-b; the tree the run used; this file is committed right after it)
- **Where:** `just verify-linux` on the dev machine: Docker Desktop (WSL2), image
  `forge-linux-verify:1.98.1` (rust 1.98.1-bookworm), ubuntu-x86_64 userland. Headless only:
  no GPU is used by these guards.
- **Command:** `just verify-linux cargo nextest run -p forge-cmd -p forge-scene -p forge-project -p forge-sim -p forge-editor -p forge-panels-scene --locked --no-fail-fast`
  (built first with the same packages and `--no-run`, 1 m 04 s).
- **Result:** 484 tests run, 484 passed (151 s), among them every scene-composition guard
  and its positive controls:
  - `forge-scene::test_scene_composition` (14: nested propagation except overrides, depth-3
    inheritance, cycles refused and named, placement, revert undoable, Make Local, structural
    refusals, pack, determinism; controls: overrides ignored, no propagation, no placement,
    cycle check skipped);
  - `forge-scene::test_scene_store` (4: lossless store form, unchanged mirrors not stored,
    missing scenes lose nothing; controls: a lost override record, the copying store);
  - `forge-project::test_scene_store_size` (2: 100 instances vs 100 copies through the real
    files, lossless decode, loaded instances still linked; control: the copying store);
  - `forge-cmd::test_deriver` (3: derived changes are one diff everywhere and one undo, a
    panicking deriver is a rejection, the reference index equals a scan);
  - `forge-panels-scene::test_scene_composition_ui` (4: hierarchy scene drop, inspector marks
    and reverts, Make Local's loss, Open base scene, viewport library and isolation; control).
- **Re-run after the deriver's fan-out memo (commit 5a3db49):** `just verify-linux cargo
  nextest run -p forge-scene -p forge-project -p forge-cmd --locked --no-fail-fast` — 112
  run, 112 passed.
- **Windows (same tree):** the full workspace nextest in twelve foreground partitions, all
  green (1994 run, 0 failed); `cargo test --workspace --doc` green; clippy `-D warnings`
  green. Store size measured on Windows: 59,164 B as instances vs 1,109,920 B as copies
  (18.8x).
