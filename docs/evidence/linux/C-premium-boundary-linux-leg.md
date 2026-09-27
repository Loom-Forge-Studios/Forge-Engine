# C-premium-boundary, C-export-leak-scan - Linux leg (WP-41)

- **Date:** 2026-09-27
- **Commit:** cef383c (lane-a; snapshot tree `5398a9c933085802c4aebef01afb5ed3578a8fbf`; this
  file is committed right after it)
- **Where:** `just verify-linux` on the dev machine: Docker Desktop (WSL2), image
  `forge-linux-verify:1.98.1` (rust 1.98.1-bookworm), ubuntu-x86_64 userland; Linux
  6.18.33.1-microsoft-standard-WSL2; cargo-nextest 0.9.145. No GPU work in this package
  (the container's Vulkan device is llvmpipe, software, not a hardware Linux GPU).
- **Command and result** (foreground):
  `just verify-linux cargo nextest run -p xtask --locked`: **114 of 114 passed**, including
  every `premium::` test: the boundary check's positive controls
  (`positive_control_a_base_crate_adding_a_premium_dependency_fails`, the identifier, lodged
  and feature controls, the shrink-only debt test, the lost-manifest control), `the_repository_holds_the_boundary`
  (the committed tree holds, and a premium dev-dependency added to a base crate fails alone),
  the leak scan's `positive_control_a_planted_identifier_is_caught_by_the_leak_scan` (a premium gate row, DoD id and range citation included), the scrub's whole-sentence, lead, numbering and listed-citation tests,
  `the_docs_only_export_of_this_repository_is_leak_free` (the real docs-only export, then an
  identifier planted in it is the one leak found), and
  `a_debt_free_code_export_builds_and_tests_offline` (a fixture exported, locked with
  `cargo generate-lockfile --offline`, built and tested with `cargo test --offline`).
- **Windows (same tree):** `cargo nextest run -p xtask` 114/114; the full workspace in three
  partitions 644 + 710 + 641 passed; clippy `-D warnings` (workspace and
  the tracy feature) clean; every `cargo xtask` gate of `just verify` green.
