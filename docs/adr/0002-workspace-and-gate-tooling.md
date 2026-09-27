# ADR 0002 — Workspace layout and gate tooling choices (WP-00)

- **Status:** accepted
- **Date:** 2026-09-20
- **Plan references:** Ch.1.5, Ch.3.3, Ch.30, Ch.38.5, Appendix A.5; I12, I13, I18, I21;
  M0-1, M0-10, M0-11, M0-12, M0-16, M0-18; W2, W4, W8, W9; D-2, D-6

(ADR 0001 is reserved for the retained-UI decision, D-1, written by WP-U0.)

## Context

M0-1 asks for the workspace, `justfile`, `xtask` and CI; M0-10/11/12/18 ask for the gates
that keep them honest. The plan fixes the *what*; these are the choices it left open.

## Decision

1. **Globbed members** (`crates/*`, `tools/*`, `plugins/*`, `xtask`, plus `tests`) — D-6.
   Lanes add crates without touching the root manifest.
2. **Repo-level guard suites live in a `forge-tests` package rooted at `tests/`**, with
   `autotests = false` and one `[[test]]` target per guard file, at the paths the invariant
   table names (`tests/licence/test_no_runtime_phone_home.rs`, ...). A virtual workspace
   root cannot own `tests/`, and cargo's default discovery would demand `tests/tests/`,
   breaking every path the plan already cites. `tests/lib.rs` provides the shared harness:
   the workspace root and the *resolved* dependency graph (`cargo metadata`, host platform,
   normal edges only), which I21 (M0-19) and the liveness family need (W3: resolve, never
   name-match).
3. **Toolchain pinned exactly** (`1.98.1`, rustfmt + clippy, minimal profile). An exact
   pin means every machine and CI runner builds with the same compiler — the determinism
   contract (Ch.3) starts at the compiler.
4. **`just verify` equals CI's list exactly and in order** — stricter than I12's
   "superset". `xtask gate-parity` also rejects, inside CI's verify region, anything that
   makes a remote step behave differently from its local twin: `if:`, `continue-on-error`,
   multi-line `run: |`, and actions. The CI matrix must be windows + ubuntu with no macOS.
5. **Security advisories are not in the gate.** `cargo deny check advisories` needs a
   network fetch of the advisory database, so it would make `just verify` fail offline and
   turn red on an unrelated commit the day an advisory is published. It runs as
   `just audit`. The gate runs `licenses bans sources`, which are pure functions of the
   lockfile.
6. **The licence allow-list is itself audited** (`xtask licence-audit`): `deny.toml` and
   `about.toml` may list only licences from the permissive family in Appendix A.5. The
   positive control runs the real `cargo deny` with the committed `deny.toml` on a throwaway
   workspace depending on a GPL-3.0 path crate and asserts refusal (and that an MIT crate
   passes, so the control is not vacuous).
7. **NOTICES is generated for both co-primary targets** (`x86_64-pc-windows-msvc` and
   `x86_64-unknown-linux-gnu`) so it is byte-identical whichever OS generates it (I18), and
   compared line-ending-insensitively.
8. **`just` runs recipes under `cmd.exe` on Windows.** Windows PowerShell 5.1 mangles `--`
   and exit codes for native commands; cmd passes both through. CI steps run under bash on
   both runners.
9. **Dependencies build at `opt-level = 2` in the dev profile**; workspace crates stay at 0.
   Deps compile once and are cached, and debug runs and tests of a game engine are
   otherwise painfully slow. Rust never enables fast-math at any opt level, so Ch.3
   determinism is unaffected.
10. **Ch.3.3 is asserted, not trusted** (`xtask fp-rules`): `.cargo/config.toml` must pin
    `build.rustflags`, and no `target-cpu`, `target-feature`, `llvm-args` or fast-math flag
    may appear in it, in `Cargo.toml`, or in `RUSTFLAGS` / `CARGO_ENCODED_RUSTFLAGS` /
    `CARGO_BUILD_RUSTFLAGS` / `CARGO_TARGET_*_RUSTFLAGS` when the gate runs.
11. **Gate rows are data (`tests/gates.ron`) and a BOUND row must name its positive-control
    function**, which `xtask gate` checks exists in the named file (W2: a positive control
    that names something that never existed reports green). Pinned-contract rows use ids
    `C-<name>` alongside `I<n>` and `S<n>`.
12. **DoD status is data (`docs/plan/dod-status.ron`) cross-checked against the ids in
    `milestones.md`** in both directions. A settled item cites repo paths that must exist.
    Part of an item overridden by a decision is recorded as `superseded_wording` beside
    the item's status, so M0-1 / M0-3 / M0-16 keep their other obligations while their
    macOS wording is `superseded(by E-33)`.
13. **Error codes are `<PREFIX>-NNNN` with one prefix per crate**, registered in
    `docs/error-codes.md`, so lanes working in different crates cannot collide.
14. **The repo tree gained four claims** — `docs/plan/`, `docs/error-codes.md`, `.cargo/`,
    `.github/` — which existed or were required but were unclaimed, so `plan-coverage`
    could not otherwise be green honestly.

## Why — the owner's two rules

1. **Better for the user:** a green `just verify` predicts a green push with no hidden
   difference; every red gate names the file and the rule; nothing in the gate depends on
   the network or on which OS ran it.
2. **Faster engine:** optimised dependencies in dev, an exact toolchain and pinned FP rules
   (no accidental `target-cpu=native` artefacts that differ per machine), thin LTO in
   release.

## Alternatives rejected

- *CI runs a single `just verify` step.* Parity would be trivially true but the CI log
  would lose per-step status and timing; explicit steps plus a parity check keep both.
- *A YAML library for parsing ci.yml.* The maintained options are heavy or unmaintained;
  the marked-region parser is small, strict, and tested with positive controls.
- *Advisories in the gate.* Rejected above (network, time-dependent red).

## Consequences

- A new guard file under `tests/<suite>/` must be registered as a `[[test]]` in
  `tests/Cargo.toml` and bound in `tests/gates.ron`.
- A new DoD id appended to `milestones.md` turns `just dod` red until it gets a status row.
- A new crate directory turns `just plan-coverage` red until the repo tree claims it.
- Editing `just verify` without editing CI (or vice versa) turns `gate-parity` red.
