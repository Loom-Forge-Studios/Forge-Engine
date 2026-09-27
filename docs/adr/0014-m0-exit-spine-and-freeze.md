# ADR 0014 — Close M0 with `forge spine`, two platform guards, and the contract freeze

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** M0 exit criterion, M0-13, M0-17, M0-19, M0-20; I1, I2, I17, I18, I21;
  W2, W3, W10, Ch.3.2, Ch.4.3, Ch.30.5 (new), Ch.32.2, Ch.33.4, Ch.38.1; Risk S14; D-0001

## Decision

1. **`forge spine`** is a subcommand of the `forge` binary in `tools/forge-cli` (the Ch.30
   tool, and the same binary WP-13's `--headless` extends). Each step is the real crate; each
   step checks its own result (`CLI-0003`) before its line enters the report. **The hash is
   BLAKE3 over the report text**, with every float in Rust's shortest round-trip form, so the
   printed report and the hash say the same thing and a golden mismatch shows *which* step
   moved. Nothing machine-specific (paths, wall time, pids) is reported.

<!-- -->

3. **The replaced built-in** is the `memory` store backend: `forge.cli.spine` replaces it with
   the same `MemoryStore` on a fixed clock. The hash covers the memory leg's commit time, so
   the golden is stable *only because* the replacement took effect — the replacement is
   load-bearing, not decorative — and a unit control shows the built-in is refused.
4. **I21 reads the runtime's own build:** edges and package sources from `cargo metadata`,
   features from `cargo tree -p forge-runtime`. The workspace-unified features include what
   the editor enables (they flagged `windows-sys/Win32_Networking` that the runtime never
   links), which would force allow-list entries that excuse nothing. Rules: licensing crates
   (never allow-listable), network/telemetry crates by name, networking features, socket
   types in registry sources, `std::net` sockets in local sources resolved through `use`
   trees and aliases. `mutate-det` is refused twice: in the graph, and by a constant
   assertion that makes `forge-runtime` fail to compile with it.
5. **M0-17's reference set** is every string literal in Rust source — through
   `proc-macro2` tokens, so literals inside `include_str!`, `concat!(env!(…))`, `#[path]`
   are seen — plus quoted strings in TOML/RON/JSON/YAML. A string is a reference iff it
   resolves (exactly or by case only) against its file's directory, crate root or the
   workspace root; resolution lists directories and compares names, never asks the
   filesystem. Backslash separators and case-colliding siblings are the same bug class and
   fail too.
6. **Frozen (M0-20)** means: the chapter's signatures, as written and as the crate's public
   API implements them, change only by plan amendment (an `Amended in WP-NN (ADR NNNN)` note
   in the chapter plus an ADR). Additive, non-breaking API is ordinary work. Frozen: Ch.1–7,
   31, 32, and Ch.33's `ProjectStore` trait with its M0 backends (collaboration freezes with
   Ch.37).
7. **D-0001:** `serde_json` is used workspace-wide with `float_roundtrip`. The spine's
   reload check found that the default parser returned a neighbour of the printed float
   (`262682861718.86282` → `…8628`), so a command log re-read from a store replayed 1 ulp off.

## Why — the owner's two rules

1. **Better for the user:** one command a person can run on any machine and compare one
   line; when it differs, the report names the step. The case lint turns "loads on my
   Windows box, missing on the Linux server" into a red test on the author's own machine.
   D-0001 would have made a project's history replay to a different project — the kind of
   bug users experience as "my level changed by itself".
2. **Faster / more efficient engine:** the spine runs in well under a second in debug; the
   guards are static (graph and source reads) except two cargo invocations in controls.
   Correctly rounded float parsing costs a few percent on log parsing, which is not a hot
   path; divergence is not an optimisation.

## Alternatives rejected

- *Hash raw bits in a side channel, print prose separately:* two things to keep in sync; a
  mismatch would not say where.
- *Replace an `Invoke` command handler instead of a store backend:* no built-in handler
  exists at M0, and the store leg then proves nothing about the replacement.
- *Workspace-unified features for I21:* false alarms from editor-only crates, answered by
  allow-list lines that would later hide a real one.
- *Lint only `include_*!` and manifest paths:* misses `Path::new("Assets/…")`, data-file
  references and gate/DoD paths (the lint's first manual check was a wrong-case gate-row path
  that `xtask gate` accepted on Windows).
- *An API-snapshot guard for the freeze:* the cost-discipline rule (guards protect engine
  properties, not planning prose); the freeze is enforced by review and the ADR rule. A
  public-API snapshot test is a candidate follow-up if drift appears.

## Consequences

- A spine golden change is a determinism event: plan amendment plus a line here or in a new
  ADR, like `tests/determinism/golden.txt`.
- `forge-net`'s transport (M4-7) must add itself to `tests/licence/net_allow.txt` with its
  reason; anything else networked in the runtime closure fails I21.
- `forge-asset` (WP-12) routes asset references through the same exact-name resolution.
