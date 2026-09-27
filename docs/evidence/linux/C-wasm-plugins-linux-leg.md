# C-wasm-plugins-in-editor, C-project-trust, C-install-live-atomic — the Linux leg (WP-34)

Image `forge-linux-verify:1.98.1` (`sha256:c960b17d10838574eb866789250ce2e81c409649b57c216d36d900bbb058e222`) from `rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e`; environment in [README](README.md) (lavapipe, software Vulkan — not a hardware Linux GPU); 2026-09-23.

**Why a Linux leg.** WP-21's WASM plugin discovery lists plugin directories (`read_dir`, sorted)
and decides "unchanged since it was refused / since it was loaded" from **mtime ⊕ length
stamps** (`hosting::stamp_of`, `forge_wasm::PluginWatcher`). Both depend on the filesystem's
timestamp resolution and directory order, which differ between NTFS and ext4/overlayfs.

**Command:** `just verify-linux` (the whole `just verify`) on lane-a at commit `b3dba2d`,
snapshot tree `5354c15ded9aceca8c30c96019dc0cb886c7ce9d`, then
`sh tools/docker/linux-verify/verify-linux.sh bash -c "…"` for the steps after nextest on the
same tree.

**The plugin rows (all pass on Linux):**

```
PASS forge-editor::test_wasm_plugins_in_editor a_wasm_plugin_dropped_in_installs_in_the_running_editor_and_hot_reloads
PASS forge-editor::test_wasm_plugins_in_editor an_idle_editor_hosting_plugins_makes_no_wakeups
PASS forge-editor::test_wasm_plugins_in_editor positive_control_a_timer_poll_wakes_the_idle_editor
PASS forge-editor::test_wasm_plugins_in_editor positive_control_an_editor_that_never_polls_installs_nothing
PASS forge-editor::test_project_trust trusting_runs_the_projects_plugin_with_its_grants
PASS forge-editor::test_project_trust not_trusting_keeps_the_code_out_and_the_grants_held
PASS forge-editor::test_project_trust a_headless_run_trusts_only_with_the_launch_flag
PASS forge-editor::test_project_trust positive_control_a_hosting_that_ignores_trust_runs_the_projects_code
PASS forge-editor::test_project_trust positive_control_an_untrusted_load_takes_the_projects_grants
PASS forge-editor::test_project_trust positive_control_both_run_the_projects_code_with_its_grants
PASS forge-editor::test_install_live_atomic a_live_install_is_all_or_nothing_at_every_step
PASS forge-editor::test_install_live_atomic positive_control_committing_as_it_goes_leaves_commands_installed
PASS forge-editor::test_collab_sync_not_sendable no_client_can_send_the_team_sync
PASS forge-panels-connect::test_plugin_manager (9 tests, the two WP-34 trust tests included)
PASS forge-panels-project::test_plugin_presets (2 tests)
PASS forge-wasm::test_wasm_importer (4 tests)
```

The hot-reload and retry checks rewrite a plugin's files within the same second and rely on
the length changing the stamp (the test writes differ in length); a same-length rewrite within
the filesystem's mtime resolution would be missed until the next change — on ext4 (ns mtime)
and NTFS (100 ns) alike, so no Linux-specific gap was found.

**The rest of `just verify` on Linux:** fmt, both clippy runs, doc tests, gate-parity (13
commands), plan-coverage, fp-rules, layering, allocators, gate (271 rows: 225 BOUND, 4
AWAITING, 42 UNBUILT), dod, licence-audit — all green.

**Not green on Linux, and not this WP's:** the full nextest run was `1564 tests run: 1562
passed, 2 failed, 1 skipped` (1750.6 s). The two failures are wall-clock UI budget checks in
`forge-panels-scene::test_ui_hierarchy_100k` (its two filter controls failed their *expected*
message because the unfaulted incremental rename already missed the 2 ms budget: "an
incremental rename took 3.596 ms at p95"); a rerun alone failed `ui_hierarchy_100k` itself at
2.754 ms. The machine was loaded by the other lane's concurrent build and its own container.
The same control fails the same way on the tree **before** WP-34 (`b3dba2d~1`, 2.106 ms p95,
same load), and WP-34 touches no hierarchy code. Wall-clock budgets are gated on the reference
machine, not in the container (ADR 0044 decision 8); on Windows the same run was green
(`1564 tests run: 1564 passed`). Recorded as a follow-up for the UI perf gate's owner, not
widened here (W5).

## Green full run after the verifier's fixes (2026-09-24)

The first run above was not a clean observation: two lanes ran `just verify-linux` at once on
the one shared volume, rsync'ing their trees over each other's `/vol/src` (hence the
`test_no_privileged_plugin` "PanelFaults matches nothing" and 32 %-different UI goldens the
verifier saw, all green alone) and loading the VM under each other's timed tests. ADR 0044
Amendment 1 gives each worktree its own volume and serialises Linux runs with a lock.

**Command:** `just verify-linux` (the whole `just verify`) on lane-a at commit `4f9f0c6`,
snapshot tree `cd3a29b172959c4ee976b7a27831f7a601b9ec82`, volume
`forge-linux-target-a-c12b5d66` (seeded once from `forge-linux-target` by a local copy),
image `forge-linux-verify:1.98.1`
`sha256:0129932d13abccd08261a95d360084b6d36111e44e3b52e8bf401fb84baac7e7` (rebuilt from the
same Dockerfile since the first run), lavapipe `llvmpipe (LLVM 15.0.6, 256 bits)`, software
Vulkan — not a hardware Linux GPU.

**Result: green, one uninterrupted run.** Host crate cache 1240 -> 1240 files (nothing
downloaded). `Summary [1420.071s] 1570 tests run: 1570 passed (15 slow), 1 skipped`. Then
gate-parity (13 commands), plan-coverage (191 directories), fp-rules, layering (38 members),
allocators, gate (`271 rows: 225 BOUND, 4 AWAITING, 42 UNBUILT`), dod, licence-audit — all
green. Among them, every row the first run failed or that WP-34 touches:

```
PASS forge-panels-scene::test_ui_hierarchy_100k ui_hierarchy_100k
PASS forge-panels-scene::test_ui_hierarchy_100k positive_control_hierarchy_rebuilding_under_a_filter_fails
PASS forge-panels-scene::test_ui_hierarchy_100k positive_control_hierarchy_unbounded_filter_fails
PASS forge-panels-scene::test_ui_hierarchy_100k positive_control_hierarchy_without_virtualisation_fails
PASS forge-tests::test_no_privileged_plugin (all)
PASS forge-ui::test_ui_goldens (all)
PASS forge-editor::test_install_live_atomic a_live_install_is_all_or_nothing_at_every_step
PASS forge-editor::test_install_live_atomic a_plugin_whose_panel_id_is_taken_installs_nothing
PASS forge-editor::test_install_live_atomic positive_control_committing_as_it_goes_leaves_commands_installed
PASS forge-editor::test_install_live_atomic positive_control_unchecked_panels_install_partly
PASS forge-editor::test_collab_sync_not_sendable (2 tests)
PASS forge-editor::test_wasm_plugins_in_editor (4 tests)
PASS forge-panels-connect::test_plugin_manager (10 tests, the nothing-to-trust test included)
```

## WP-36 re-run (2026-09-26): read-once plugin files, the watcher's vet, idle vets, team pull, console trust

WP-36 moved the read that the trust gate vets onto `forge_wasm::PluginFiles` (stamps taken
before the bytes, one read compiled) and made the watcher vet the bytes it swaps in — both
depend on the same timestamp and directory-order behaviour as above. Same image; lane-a at
commit `947d935` (tree `8a263460e785e9eac834736621ecc39395fb8a99`), lavapipe not used (no GPU tests here).

**Commands:** `just verify-linux cargo nextest run -p forge-wasm -p forge-remote --locked --no-fail-fast`
(54/54 pass, including `console::tests::the_console_answers_project_trust_through_the_audited_call`,
its control, and `test_wasm_hot_reload`'s two WP-36 tests), and
`just verify-linux cargo nextest run -p forge-editor --locked --no-fail-fast -E '"binary(test_project_trust) | binary(test_install_live_atomic) | binary(test_wasm_plugins_in_editor) | test(hosting::)"'`
(31/31 pass):

```
PASS forge-editor hosting::tests::a_removed_plugin_folder_keeps_no_digest
PASS forge-editor hosting::tests::vet_answers_are_kept_against_the_version_and_the_project
PASS forge-editor::test_install_live_atomic a_live_install_is_all_or_nothing_at_every_step
PASS forge-editor::test_install_live_atomic a_plugin_whose_panel_id_is_taken_installs_nothing
PASS forge-editor::test_install_live_atomic positive_control_committing_as_it_goes_leaves_commands_installed
PASS forge-editor::test_install_live_atomic positive_control_unchecked_panels_install_partly
PASS forge-editor::test_project_trust a_headless_run_trusts_only_with_the_launch_flag
PASS forge-editor::test_project_trust a_persons_own_change_never_asks_again
PASS forge-editor::test_project_trust a_pull_into_a_trusted_project_asks_again_before_it_runs
PASS forge-editor::test_project_trust a_team_pull_into_a_trusted_project_asks_before_it_takes_effect
PASS forge-editor::test_project_trust an_untrusted_projects_cached_plugin_version_is_not_loaded
PASS forge-editor::test_project_trust code_swapped_in_after_the_trust_gate_looked_never_runs
PASS forge-editor::test_project_trust idle_polls_of_a_trusted_project_ask_the_trust_gate_nothing
PASS forge-editor::test_project_trust not_trusting_keeps_the_code_out_and_the_grants_held
PASS forge-editor::test_project_trust positive_control_a_hosting_that_ignores_trust_runs_the_cached_plugin
PASS forge-editor::test_project_trust positive_control_a_hosting_that_ignores_trust_runs_the_projects_code
PASS forge-editor::test_project_trust positive_control_an_untrusted_load_takes_the_projects_grants
PASS forge-editor::test_project_trust positive_control_both_run_the_projects_code_with_its_grants
PASS forge-editor::test_project_trust positive_control_reading_again_after_the_vet_runs_the_swapped_code
PASS forge-editor::test_project_trust positive_control_trust_by_folder_at_startup_loads_changed_code
PASS forge-editor::test_project_trust positive_control_trust_by_folder_runs_what_a_pull_brought
PASS forge-editor::test_project_trust positive_control_trust_by_folder_takes_what_a_team_pull_brought
PASS forge-editor::test_project_trust positive_control_without_the_cache_every_poll_asks
PASS forge-editor::test_project_trust the_startup_gate_loads_an_open_projects_plugins_only_as_trusted
PASS forge-editor::test_project_trust trusting_runs_the_projects_plugin_with_its_grants
PASS forge-editor::test_wasm_plugins_in_editor a_wasm_plugin_dropped_in_installs_in_the_running_editor_and_hot_reloads
PASS forge-editor::test_wasm_plugins_in_editor an_idle_editor_hosting_plugins_makes_no_wakeups
PASS forge-editor::test_wasm_plugins_in_editor positive_control_a_timer_poll_wakes_the_idle_editor
PASS forge-editor::test_wasm_plugins_in_editor positive_control_an_editor_that_never_polls_installs_nothing
```
