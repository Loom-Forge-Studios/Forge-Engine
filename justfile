# Forge command ladder (Ch.30.1). Run `just` to list recipes.
#
# The recipes run under cmd.exe on Windows (it passes `--` and exit codes through
# faithfully) and sh elsewhere; every command below is valid in both.
set windows-shell := ["cmd.exe", "/d", "/c"]

# List the recipes.
default:
    @just --list --unsorted

# Inner-loop command. A green `check` does not predict a green push: run `just verify`
# (exactly CI's list) before pushing.
[doc('Fast SUBSET of verify (not CI): skips the tracy-feature clippy, doc tests, gate-parity, plan-coverage, fp-rules, layering, premium-boundary, allocators, dod, licence-audit and tests/e2e.')]
check:
    cargo xtask fmt --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo nextest run --workspace --locked
    cargo xtask gate

# `cargo xtask gate-parity` asserts this recipe equals .github/workflows/ci.yml (I12):
# edit the two together or the gate goes red.
[doc("EXACTLY CI's command list, in CI's order (W4, I12): a green verify predicts a green push.")]
verify:
    cargo xtask fmt --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo clippy -p forge-trace --all-targets --features tracy --locked -- -D warnings
    cargo nextest run --workspace --locked --no-fail-fast
    cargo test --workspace --doc --locked
    cargo xtask gate-parity
    cargo xtask plan-coverage
    cargo xtask fp-rules
    cargo xtask layering
    cargo xtask premium-boundary
    cargo xtask allocators
    cargo xtask gpu-turn
    cargo xtask timed-gates
    cargo xtask gate
    cargo xtask dod
    cargo xtask licence-audit

# The ubuntu-x86_64 leg, locally, in Docker (ADR 0044): `just verify` (or the given command)
# in the pinned forge-linux-verify container on this worktree. GPU work runs on lavapipe
# (software Vulkan), not a hardware Linux GPU. Needs Docker Desktop running.
[windows]
[doc('Run `just verify` (or ARGS) on Linux in Docker: just verify-linux [command...]')]
verify-linux *ARGS:
    powershell -NoProfile -ExecutionPolicy Bypass -File tools/docker/linux-verify/verify-linux.ps1 {{ARGS}}

[unix]
[doc('Run `just verify` (or ARGS) on Linux in Docker: just verify-linux [command...]')]
verify-linux *ARGS:
    sh tools/docker/linux-verify/verify-linux.sh {{ARGS}}

# Invariant rows: BOUND / AWAITING(reason) / UNBUILT(reason).
gate:
    cargo xtask gate

# Every DoD id and its status. `just dod --milestone M0`, `just dod --ship M0`.
dod *ARGS:
    cargo xtask dod {{ARGS}}

# Gate row C-ui-panel-backlog (AWAITING in `just gate`): checks Ch.21 §21.21/§21.24 against
# the orchestrator's backlog.json, which is outside the repo, so verify and CI cannot run it.
[doc('Ch.21 panel inventory vs the orchestrator backlog: just backlog-check <path-to-backlog.json>')]
backlog-check $FORGE_BACKLOG:
    cargo test -p forge-tests --test test_ui_panel_inventory --locked -- --nocapture

# Every directory in the repo is claimed by the master-plan repo tree.
plan-coverage:
    cargo xtask plan-coverage

# `just verify` == CI's command list, same order (I12).
gate-parity:
    cargo xtask gate-parity

# Permissive allow-lists, cargo-deny licences/bans/sources, NOTICES fresh (I13).
licence-audit:
    cargo xtask licence-audit

# Regenerate NOTICES from the resolved dependency graph (commit the result).
notices:
    cargo xtask licence-audit --write

# Security advisories (needs network for the advisory database; not part of the gate).
audit:
    cargo deny --locked check advisories

# Format the workspace in place.
fmt:
    cargo xtask fmt
