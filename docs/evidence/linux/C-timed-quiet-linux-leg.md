# C-timed-quiet, C-timed-gates-routed - Linux leg (WP-40)

- **Date:** 2026-09-26
- **Commit:** 3459823 (lane-a; the tree the runs used; this file is committed right after it)
- **Where:** `just verify-linux` on the dev machine: Docker Desktop (WSL2), image
  `forge-linux-verify:1.98.1` (rust 1.98.1-bookworm), ubuntu-x86_64 userland, 6 CPUs. No GPU
  test is in these packages' timed bodies; the load sampler reads the WSL2 VM's `/proc/stat`
  (all of the VM's cores, not the container's 6-CPU quota) and `/proc/self/stat`.
- **Commands and results** (each in the foreground):
  - `just verify-linux cargo nextest run -p forge-trace -p xtask --locked --no-fail-fast`:
    115 of 115 passed, including `forge-trace::test_timed_quiet` (the real `/proc/stat`
    sampler returns sane shares and sees two spinning threads as this process's load; the
    faked-busy positive control waits out its bound and reports MEASURED UNDER LOAD; a failure
    under load says so), `forge-trace::test_timed_alone`, and `xtask timed_gates::tests`
    (`the_committed_repository_holds_the_guard` and the three positive controls).
  - `just verify-linux cargo nextest run -p forge-panels-domain --locked --no-fail-fast`: 27 of
    27, `test_tile_stroke_bounded` now timed alone.
